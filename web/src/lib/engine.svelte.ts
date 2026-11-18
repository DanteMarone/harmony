// The messaging engine: this device's keys, its chats, and keeping both in
// step with the server.
//
// Everything that leaves this file for the server is either public
// (key packages) or encrypted (messages, key changes). Decrypted messages
// and private keys stay in the browser's own storage.

import init, { Device, openBackup, Recovery, safetyNumber, sealBackup, verifyDevice } from '../../wasm/harmony_crypto'
import wasmUrl from '../../wasm/harmony_crypto_bg.wasm?url'
import {
  api,
  ApiError,
  fromBase64,
  toBase64,
  useDevice,
  type ChatEvent,
  type ChatInfo,
  type User,
  type UserInfo,
} from './api'
import { Store, type StoredChat, type StoredMessage, type TrustRecord } from './store'

const ONE_TIME_KEY_PACKAGES = 20
const BACKUP_BATCH = 100

/**
 * How far this device can vouch for another person:
 * - `verified`: safety numbers were compared and nothing has changed since.
 * - `unverified`: nothing looks wrong, but nobody has compared safety numbers.
 * - `changed`: their identity key is not the one this device remembers.
 * - `unconfirmed`: a device of theirs in the chat is not signed by their identity key.
 * - `none`: they have not set up a recovery code, so nothing can be checked.
 */
export type TrustStatus = 'verified' | 'unverified' | 'changed' | 'unconfirmed' | 'none'
const encoder = new TextEncoder()
const decoder = new TextDecoder()

/** A default name for this device, such as "Firefox on Windows". Only the owner sees it. */
function describeThisDevice(): string {
  const agent = navigator.userAgent
  const browser = /Edg\//.test(agent)
    ? 'Edge'
    : /Firefox\//.test(agent)
      ? 'Firefox'
      : /Chrome\//.test(agent)
        ? 'Chrome'
        : /Safari\//.test(agent)
          ? 'Safari'
          : 'Browser'
  const system = /Windows/.test(agent)
    ? 'Windows'
    : /Android/.test(agent)
      ? 'Android'
      : /iPhone|iPad/.test(agent)
        ? 'iOS'
        : /Mac OS X/.test(agent)
          ? 'macOS'
          : /Linux/.test(agent)
            ? 'Linux'
            : ''
  return system ? `${browser} on ${system}` : browser
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
const isStatus = (err: unknown, status: number) => err instanceof ApiError && err.status === status

/** Messages in the order they appear: by position, with unsent ones last. */
function inOrder(a: StoredMessage, b: StoredMessage): number {
  if (a.seq !== null && b.seq !== null) return a.seq - b.seq
  if (a.seq === null && b.seq === null) return a.sentAt - b.sentAt
  return a.seq === null ? 1 : -1
}

export class Engine {
  /**
   * `locked`: Harmony is already open in another tab. `revoked`: this device
   * was signed out from another one and its data erased. `signedOut`: the
   * login session ended.
   */
  status = $state<'starting' | 'ready' | 'locked' | 'error' | 'revoked' | 'signedOut'>('starting')
  error = $state('')
  connected = $state(false)
  users = $state<UserInfo[]>([])
  chats = $state<ChatInfo[]>([])
  /** Local bookkeeping per chat: read position, unread count, problems. */
  meta = $state<Record<string, StoredChat>>({})
  /** Messages of the chats opened so far. */
  messages = $state<Record<string, StoredMessage[]>>({})
  activeChat = $state<string | null>(null)
  /**
   * Where this device stands with the person's recovery code: `setup` (they
   * have none yet), `code` (they have one; it has not been entered here),
   * `skipped` (they chose to enter it later) or `ready`.
   */
  recovery = $state<'unknown' | 'setup' | 'code' | 'skipped' | 'ready'>('unknown')
  /** Bringing back history from the backup. */
  restoring = $state(false)
  /** Identity keys this device remembers for other people, by user id. */
  trust = $state<Record<number, TrustRecord>>({})
  /** Per chat: people with a device in it that their identity key does not vouch for. */
  unconfirmed = $state<Record<string, number[]>>({})

  private device!: Device
  private deviceId = 0
  private identity = ''
  private identityKey: Uint8Array | undefined
  private backupKey: Uint8Array | undefined
  private recoverySkipped = false
  private backupTimer: ReturnType<typeof setTimeout> | null = null
  private backingUp = false
  private signatureChecks = new Map<string, boolean>()
  private store!: Store
  private socket: WebSocket | null = null
  private stopped = false
  private releaseLock: (() => void) | null = null
  /** One queue per chat: steps on the same chat never overlap. */
  private queues = new Map<string, Promise<unknown>>()
  private refreshing: Promise<void> | null = null
  private refreshAgain: Set<string> | 'all' | null = null

  constructor(readonly me: User) {}

  // -- starting and stopping ------------------------------------------------

  async start() {
    try {
      if (!(await this.takeLock())) {
        this.status = 'locked'
        return
      }
      await init({ module_or_path: wasmUrl })
      this.store = await Store.open(this.me.id)
      await this.loadOrCreateDevice()
      for (const chat of await this.store.chats()) this.meta[chat.chatId] = chat
      for (const record of await this.store.trust()) this.trust[record.userId] = record
      await this.topUpKeyPackages()
      this.connect()
      await this.refresh('all')
      this.status = 'ready'
    } catch (err) {
      console.error(err)
      this.error = err instanceof Error ? err.message : String(err)
      this.status = 'error'
    }
  }

  stop() {
    this.stopped = true
    if (this.backupTimer) clearTimeout(this.backupTimer)
    this.socket?.close()
    this.store?.close()
    this.releaseLock?.()
    useDevice(null)
  }

  /**
   * Two tabs sharing one set of keys would corrupt them, so only one tab at
   * a time may run the engine for an account.
   */
  private takeLock(): Promise<boolean> {
    if (!navigator.locks) return Promise.resolve(true)
    return new Promise((resolve) => {
      navigator.locks.request(`harmony-user-${this.me.id}`, { ifAvailable: true }, (lock) => {
        if (!lock) {
          resolve(false)
          return
        }
        resolve(true)
        return new Promise<void>((release) => (this.releaseLock = release))
      })
    })
  }

  private async loadOrCreateDevice() {
    const saved = await this.store.device()
    if (saved) {
      useDevice(saved.deviceId)
      try {
        await api.keyPackageCount(saved.deviceId)
        this.device = Device.restore(saved.state)
        this.deviceId = saved.deviceId
        this.identity = saved.identity
        this.identityKey = saved.identityKey
        this.backupKey = saved.backupKey
        this.recoverySkipped = saved.recoverySkipped ?? false
        return
      } catch (err) {
        // 403: the server no longer knows this device (it was signed out
        // elsewhere, or the server was reset). Start over as a new device.
        if (!isStatus(err, 403)) throw err
        useDevice(null)
        this.store.close()
        await Store.erase(this.me.id)
        this.store = await Store.open(this.me.id)
      }
    }
    const created = await api.createDevice(describeThisDevice())
    this.device = Device.create(created.identity)
    this.deviceId = created.id
    this.identity = created.identity
    useDevice(created.id)
    await this.persist()
  }

  /** Keeps enough key packages on the server for others to add this device while it is offline. */
  private async topUpKeyPackages() {
    const count = await api.keyPackageCount(this.deviceId)
    const missing = ONE_TIME_KEY_PACKAGES - count.one_time
    if (missing < ONE_TIME_KEY_PACKAGES / 2 && count.last_resort) return
    const oneTime = (this.device.keyPackages(Math.max(missing, 0), false) as Uint8Array[]).map(toBase64)
    const lastResort = count.last_resort
      ? undefined
      : toBase64((this.device.keyPackages(1, true) as Uint8Array[])[0])
    // Save first: the private halves of these packages live in the device state.
    await this.persist()
    await api.uploadKeyPackages(this.deviceId, oneTime, lastResort)
  }

  // -- saving ------------------------------------------------------------------

  private persist(changes: { chats?: StoredChat[]; messages?: StoredMessage[]; deleteChats?: string[] } = {}) {
    return this.store.save({
      device: {
        deviceId: this.deviceId,
        identity: this.identity,
        state: this.device.exportState(),
        identityKey: this.identityKey,
        backupKey: this.backupKey,
        recoverySkipped: this.recoverySkipped,
      },
      ...changes,
    })
  }

  private metaFor(chatId: string): StoredChat {
    return (this.meta[chatId] ??= { chatId, cursor: 0, unread: 0 })
  }

  /** A plain copy for storage (the live one is wrapped for the UI). */
  private metaCopy(chatId: string): StoredChat {
    return { ...this.metaFor(chatId) }
  }

  private showMessage(message: StoredMessage) {
    const list = this.messages[message.chatId]
    if (!list) return // chat not opened yet; it will be read from storage when it is
    const at = list.findIndex((m) => m.key === message.key)
    if (at >= 0) list[at] = message
    else list.push(message)
    list.sort(inOrder)
  }

  // -- live connection ---------------------------------------------------------

  private connect() {
    if (this.stopped) return
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws'
    const socket = new WebSocket(`${scheme}://${location.host}/api/ws?device=${this.deviceId}`)
    this.socket = socket
    socket.onopen = () => {
      this.connected = true
      // Catch up on anything that happened while disconnected.
      void this.refresh('all')
    }
    socket.onmessage = (event) => {
      try {
        const notice = JSON.parse(event.data)
        if (notice.type === 'revoked' && notice.device_id === this.deviceId) {
          void this.wipe()
          return
        }
        if (notice.type === 'backup') {
          const chatId = notice.chat_id as string
          const after = Math.max(0, Number(notice.from_seq) - 1)
          if (this.backupKey && this.chats.some((c) => c.id === chatId)) {
            void this.enqueue(chatId, () => this.restoreChat(chatId, after)).catch((err) =>
              console.error('could not restore chat', chatId, err),
            )
          }
          return
        }
        void this.refresh(notice.type === 'chat' ? new Set([notice.chat_id as string]) : 'all')
      } catch {
        void this.refresh('all')
      }
    }
    socket.onclose = () => {
      this.connected = false
      if (!this.stopped) setTimeout(() => this.connect(), 2000 + Math.random() * 3000)
    }
  }

  // -- syncing -----------------------------------------------------------------

  /**
   * Brings this device up to date. `dirty` names chats known to have
   * changed; other chats are only synced if the server shows new events.
   * Calls made while one is running are merged into a single follow-up run.
   */
  refresh(dirty: Set<string> | 'all'): Promise<void> {
    if (this.refreshing) {
      if (dirty === 'all' || this.refreshAgain === 'all') this.refreshAgain = 'all'
      else this.refreshAgain = new Set([...(this.refreshAgain ?? []), ...dirty])
      return this.refreshing
    }
    this.refreshing = (async () => {
      try {
        let next: Set<string> | 'all' | null = dirty
        while (next && !this.stopped) {
          this.refreshAgain = null
          await this.refreshOnce(next)
          next = this.refreshAgain
        }
      } catch (err) {
        console.error('sync failed', err)
      } finally {
        this.refreshing = null
      }
    })()
    return this.refreshing
  }

  private async refreshOnce(dirty: Set<string> | 'all') {
    let users: UserInfo[], chats: ChatInfo[]
    try {
      ;[users, chats] = await Promise.all([api.users(), api.chats()])
    } catch (err) {
      // 401: the login session ended (possibly because this device was
      // signed out elsewhere). 403: the server no longer accepts this device.
      if (isStatus(err, 403)) await this.wipe()
      else if (isStatus(err, 401)) this.endSession()
      throw err
    }
    this.users = users
    this.chats = chats
    await this.checkRecovery()
    await this.rememberIdentities()

    // Chats we are no longer in: forget their keys and messages.
    const current = new Set(chats.map((c) => c.id))
    for (const chatId of Object.keys(this.meta)) {
      if (!current.has(chatId)) await this.enqueue(chatId, () => this.forgetChat(chatId))
    }

    await this.acceptWelcomes()

    // Re-read the list: accepting a welcome changes `joined`.
    const fresh = await api.chats()
    this.chats = fresh
    await Promise.all(
      fresh
        .filter((c) => dirty === 'all' || dirty.has(c.id) || c.last_seq > this.metaFor(c.id).cursor)
        .map((c) => this.enqueue(c.id, () => this.syncChat(c)).catch((err) => console.error('chat sync failed', c.id, err))),
    )
    this.checkDevices()
    this.backupSoon()
  }

  private enqueue<T>(chatId: string, step: () => Promise<T>): Promise<T> {
    const previous = this.queues.get(chatId) ?? Promise.resolve()
    const next = previous.catch(() => {}).then(step)
    this.queues.set(chatId, next)
    return next
  }

  private async forgetChat(chatId: string) {
    if (this.device.hasChat(chatId)) this.device.deleteChat(chatId)
    delete this.meta[chatId]
    delete this.messages[chatId]
    if (this.activeChat === chatId) this.activeChat = null
    await this.persist({ deleteChats: [chatId] })
  }

  /** Joins the chats this device has been added to. */
  private async acceptWelcomes() {
    const { welcomes } = await api.welcomes()
    for (const welcome of welcomes) {
      await this.enqueue(welcome.chat_id, async () => {
        const meta = this.metaFor(welcome.chat_id)
        try {
          // Left over from before a repair: the fresh invitation replaces it.
          if (this.device.hasChat(welcome.chat_id)) this.device.deleteChat(welcome.chat_id)
          const joined = this.device.joinChat(fromBase64(welcome.data))
          if (joined !== welcome.chat_id) throw new Error('invitation is for a different chat')
          meta.cursor = welcome.after_seq
          meta.broken = false
        } catch (err) {
          console.error('could not join chat', welcome.chat_id, err)
          meta.broken = true
        }
        await this.persist({ chats: [this.metaCopy(welcome.chat_id)] })
        await api.ackWelcome(welcome.id)
      })
    }
  }

  private async syncChat(chat: ChatInfo) {
    const meta = this.metaFor(chat.id)
    const hasKeys = this.device.hasChat(chat.id)

    if (!chat.joined) {
      // Waiting to be added (or added back). Old keys, if any, are useless now.
      if (hasKeys) {
        this.device.deleteChat(chat.id)
        await this.persist()
      }
      return
    }
    if (!hasKeys) {
      if (meta.broken) return
      if (chat.created_by === this.me.id && chat.epoch === 0 && chat.last_seq === 0) {
        // We created this chat but were interrupted before starting its keys.
        this.device.createChat(chat.id)
        await this.persist({ chats: [this.metaCopy(chat.id)] })
      } else {
        // The server thinks this device has the keys, but it does not. Ask to be added back.
        await api.rejoin(chat.id)
        return
      }
    }

    for (let round = 0; round < 8; round++) {
      if (!(await this.pullEvents(chat))) return
      if (!(await this.reconcile(chat, round))) return
    }
  }

  /** Fetches and applies new events. Returns false if the chat cannot be used right now. */
  private async pullEvents(chat: ChatInfo): Promise<boolean> {
    const meta = this.metaFor(chat.id)
    for (;;) {
      let page: { events: ChatEvent[]; more: boolean }
      try {
        page = await api.events(chat.id, meta.cursor)
      } catch (err) {
        if (isStatus(err, 409) || isStatus(err, 404)) return false
        throw err
      }
      const changed: StoredMessage[] = []
      for (const event of page.events) {
        const mine = event.sender_device === this.deviceId
        if (mine && event.kind === 'commit') {
          // Our own key change came back in order: now it takes effect.
          if (meta.pendingCommit) {
            this.device.confirmCommit(chat.id)
            meta.pendingCommit = false
          }
        } else if (mine) {
          const own = await this.ownMessage(chat.id, event.client_id)
          if (own) changed.push({ ...own, seq: event.seq, sentAt: event.created_at, problem: undefined })
        } else {
          try {
            const processed = this.device.process(chat.id, fromBase64(event.data))
            const message = this.readMessage(chat.id, event, processed.sender, processed.content)
            if (message) {
              changed.push(message)
              if (this.activeChat !== chat.id || document.hidden) meta.unread += 1
            }
          } catch (err) {
            console.error('could not process event', chat.id, event.seq, err)
            if (event.kind === 'commit') {
              // We missed a key change and can no longer follow the chat.
              // Drop our keys and ask another device to add us back.
              this.device.deleteChat(chat.id)
              meta.cursor = event.seq
              await this.persist({ chats: [this.metaCopy(chat.id)] })
              await api.rejoin(chat.id)
              return false
            }
            changed.push({
              key: `${chat.id}/#${event.seq}`,
              chatId: chat.id,
              seq: event.seq,
              senderUser: event.sender_user ?? 0,
              body: '',
              sentAt: event.created_at,
              problem: 'undecryptable',
            })
          }
        }
        meta.cursor = event.seq
      }
      if (page.events.length) {
        await this.persist({ chats: [this.metaCopy(chat.id)], messages: changed })
        changed.forEach((m) => this.showMessage(m))
      }
      if (!page.more) break
    }
    if (meta.pendingCommit) {
      // A key change of ours never reached the server (or lost a race): drop it.
      this.device.discardCommit(chat.id)
      meta.pendingCommit = false
      await this.persist({ chats: [this.metaCopy(chat.id)] })
    }
    return true
  }

  private async ownMessage(chatId: string, clientId: string | null): Promise<StoredMessage | undefined> {
    const key = `${chatId}/${clientId}`
    const cached = this.messages[chatId]?.find((m) => m.key === key)
    if (cached) return { ...cached }
    return (await this.store.messages(chatId)).find((m) => m.key === key)
  }

  private readMessage(
    chatId: string,
    event: ChatEvent,
    sender: string | undefined,
    content: Uint8Array | undefined,
  ): StoredMessage | null {
    if (!sender || !content) return null // a key change, not a message
    // The sender comes from the encryption layer, which proves it; the
    // server's own claim about who sent an event is not trusted for display.
    const match = /^u(\d+):d\d+$/.exec(sender)
    const parsed = JSON.parse(decoder.decode(content))
    if (!match || parsed?.t !== 'text' || typeof parsed.body !== 'string') return null
    return {
      key: `${chatId}/#${event.seq}`,
      chatId,
      seq: event.seq,
      senderUser: Number(match[1]),
      body: parsed.body,
      sentAt: event.created_at,
    }
  }

  /**
   * Makes the chat's key group match its member list: adds devices that
   * should have the keys and removes ones that should not. Returns true if
   * a change was attempted and events should be pulled again.
   */
  private async reconcile(chat: ChatInfo, round: number): Promise<boolean> {
    let pending = await api.pending(chat.id)
    if (!pending.add.length && !pending.remove.length) return false

    // Every online device of every member sees the same to-do list. A short
    // random wait keeps them from all acting at once; the chat's creator goes first.
    if (round === 0 && chat.created_by !== this.me.id) {
      await sleep(200 + Math.random() * 1500)
      if (!(await this.pullEvents(chat))) return false
      pending = await api.pending(chat.id)
      if (!pending.add.length && !pending.remove.length) return false
    }

    const claimed = pending.add.length
      ? (await api.claim(chat.id, pending.add.map((d) => d.device_id))).key_packages
      : []
    if (!claimed.length && !pending.remove.length) return false

    const meta = this.metaFor(chat.id)
    const epoch = this.device.epoch(chat.id)
    let commit
    try {
      commit = this.device.changeMembers(
        chat.id,
        claimed.map((k) => k.identity),
        claimed.map((k) => fromBase64(k.key_package)),
        pending.remove.map((d) => d.identity),
      )
    } catch (err) {
      // For example a key package that does not match the device it was presented for.
      console.error('could not build key change', chat.id, err)
      return false
    }
    meta.pendingCommit = true
    await this.persist({ chats: [this.metaCopy(chat.id)] })

    try {
      await api.commit(chat.id, {
        epoch,
        commit: toBase64(commit.commit),
        welcome: commit.welcome ? toBase64(commit.welcome) : undefined,
        added: claimed.map((k) => k.device_id),
        removed: pending.remove.map((d) => d.device_id),
      })
    } catch (err) {
      if (!isStatus(err, 409)) console.error('key change refused', chat.id, err)
      // Refused (someone else got there first) or unknown outcome. Pulling
      // events settles it: our change is applied if it comes back, dropped if not.
    }
    return true
  }

  // -- recovery code, verification and backup ----------------------------------

  /** Works out where this device stands with the person's recovery code. */
  private async checkRecovery() {
    const mine = this.users.find((u) => u.id === this.me.id)
    const serverKey = mine?.identity_key ?? null
    const signed = !!mine?.devices.find((d) => d.id === this.deviceId)?.signature
    if (serverKey && signed && this.identityKey && toBase64(this.identityKey) === serverKey) {
      this.recovery = 'ready'
      return
    }
    if (this.identityKey || this.backupKey) {
      // The identity was replaced from another device: the keys held here are stale.
      this.identityKey = undefined
      this.backupKey = undefined
      this.recoverySkipped = false
      await this.persist()
    }
    this.recovery = !serverKey ? 'setup' : this.recoverySkipped ? 'skipped' : 'code'
  }

  /** Remembers each person's identity key the first time it is seen. */
  private async rememberIdentities() {
    const fresh: TrustRecord[] = []
    for (const user of this.users) {
      if (user.id === this.me.id || !user.identity_key || this.trust[user.id]) continue
      const record = { userId: user.id, identityKey: user.identity_key, verified: false }
      this.trust[user.id] = record
      fresh.push(record)
    }
    if (fresh.length) await this.store.save({ trust: fresh })
  }

  /** The identity key this device is prepared to check a person's devices against. */
  private trustedKey(userId: number): string | null {
    if (userId === this.me.id) return this.identityKey ? toBase64(this.identityKey) : null
    const current = this.users.find((u) => u.id === userId)?.identity_key ?? null
    const remembered = this.trust[userId]?.identityKey ?? null
    return current && current === remembered ? current : null
  }

  /**
   * Checks every device in every chat against its owner's identity key.
   * The device list comes from the chat's own key group, not from the
   * server, so a device slipped in by the server shows up here.
   */
  private checkDevices() {
    const result: Record<string, number[]> = {}
    for (const chat of this.chats) {
      if (!chat.joined || !this.device.hasChat(chat.id)) continue
      const flagged = new Set<number>()
      for (const [identity, deviceKey] of this.device.memberKeys(chat.id) as [string, Uint8Array][]) {
        if (identity === this.identity) continue
        const match = /^u(\d+):d\d+$/.exec(identity)
        if (!match) continue
        const userId = Number(match[1])
        // Our own other devices can only be judged once this one has the code.
        if (userId === this.me.id && !this.identityKey) continue
        const key = this.trustedKey(userId)
        const signature = this.users
          .find((u) => u.id === userId)
          ?.devices.find((d) => d.identity === identity)?.signature
        if (!key || !signature || !this.signatureHolds(key, identity, deviceKey, signature)) flagged.add(userId)
      }
      if (flagged.size) result[chat.id] = [...flagged]
    }
    this.unconfirmed = result
  }

  private signatureHolds(key: string, identity: string, deviceKey: Uint8Array, signature: string): boolean {
    const id = `${key}|${identity}|${toBase64(deviceKey)}|${signature}`
    let holds = this.signatureChecks.get(id)
    if (holds === undefined) {
      try {
        holds = verifyDevice(fromBase64(key), identity, deviceKey, fromBase64(signature))
      } catch {
        holds = false
      }
      this.signatureChecks.set(id, holds)
    }
    return holds
  }

  /** How far this device can vouch for a person, in a given chat if one is named. */
  trustStatus(userId: number, chatId?: string): TrustStatus {
    const current = this.users.find((u) => u.id === userId)?.identity_key ?? null
    const remembered = this.trust[userId]
    if (remembered && remembered.identityKey !== current) return 'changed'
    if (!current) return 'none'
    if (chatId && this.unconfirmed[chatId]?.includes(userId)) return 'unconfirmed'
    return remembered?.verified ? 'verified' : 'unverified'
  }

  /** People in a chat whose status calls for a warning, with ourselves last. */
  chatWarnings(chat: ChatInfo): { userId: number; status: 'changed' | 'unconfirmed' }[] {
    const warnings: { userId: number; status: 'changed' | 'unconfirmed' }[] = []
    for (const member of chat.members) {
      if (member.user_id === this.me.id) continue
      const status = this.trustStatus(member.user_id, chat.id)
      if (status === 'changed' || status === 'unconfirmed') warnings.push({ userId: member.user_id, status })
    }
    if (this.unconfirmed[chat.id]?.includes(this.me.id)) warnings.push({ userId: this.me.id, status: 'unconfirmed' })
    return warnings
  }

  /** The number to compare with a friend, or null if either side has no identity key yet. */
  safetyNumber(userId: number): string | null {
    const theirs = this.users.find((u) => u.id === userId)?.identity_key
    if (!this.identityKey || !theirs) return null
    return safetyNumber(this.me.id, this.identityKey, userId, fromBase64(theirs))
  }

  /** Records that safety numbers were compared with this person (or takes that back). */
  async setVerified(userId: number, verified: boolean) {
    const current = this.users.find((u) => u.id === userId)?.identity_key
    if (!current) return
    const record = { userId, identityKey: current, verified }
    this.trust[userId] = record
    await this.store.save({ trust: [record] })
    this.checkDevices()
  }

  /** Accepts a person's changed identity key as the one to remember, unverified. */
  async acceptIdentity(userId: number) {
    const current = this.users.find((u) => u.id === userId)?.identity_key
    if (current) {
      await this.setVerified(userId, false)
    } else {
      delete this.trust[userId]
      await this.store.save({ deleteTrust: [userId] })
    }
  }

  /** A fresh recovery code, to show the person before it is put to use. */
  newRecoveryCode(): string {
    return Recovery.generate().code
  }

  /**
   * Puts a new recovery code to use: the first one, or a replacement after
   * the old one was lost. A replacement is a new identity, so friends are
   * asked to verify again and the backup starts over from this device.
   */
  async useNewRecoveryCode(code: string) {
    const recovery = Recovery.fromCode(code)
    const identityKey = recovery.identityKey()
    const signature = recovery.signDevice(this.identity, this.device.signaturePublicKey())
    await api.setIdentity(toBase64(identityKey), toBase64(signature))
    this.identityKey = identityKey
    this.backupKey = recovery.backupKey()
    this.recoverySkipped = false
    await this.persist()
    // The server dropped any backup made under the old code.
    await this.store.setBackedUp('all', false)
    this.recovery = 'ready'
    await this.refresh('all')
  }

  /** Confirms this device with the person's existing recovery code and brings back their history. */
  async enterRecoveryCode(code: string) {
    const recovery = Recovery.fromCode(code)
    const identityKey = recovery.identityKey()
    const mine = (await api.users()).find((u) => u.id === this.me.id)
    if (mine?.identity_key !== toBase64(identityKey)) {
      throw new Error('That is not your recovery code. Check it and try again.')
    }
    const signature = recovery.signDevice(this.identity, this.device.signaturePublicKey())
    await api.signDevice(this.deviceId, toBase64(signature))
    this.identityKey = identityKey
    this.backupKey = recovery.backupKey()
    this.recoverySkipped = false
    await this.persist()
    this.recovery = 'ready'
    await this.refresh('all')
    await this.restoreHistory()
  }

  /** Carries on without the recovery code: no history, and friends see this device as unconfirmed. */
  async skipRecoveryCode() {
    this.recoverySkipped = true
    await this.persist()
    this.recovery = 'skipped'
  }

  private backupContext(chatId: string, seq: number): string {
    return `u${this.me.id}/${chatId}/${seq}`
  }

  private backupSoon() {
    if (!this.backupKey || this.backupTimer || this.stopped) return
    this.backupTimer = setTimeout(() => {
      this.backupTimer = null
      void this.backupNow()
    }, 1500)
  }

  /** Uploads encrypted copies of messages that are not in the backup yet. */
  private async backupNow() {
    const key = this.backupKey
    if (!key || this.backingUp || this.stopped) return
    this.backingUp = true
    try {
      const waiting = new Map<string, StoredMessage[]>()
      for (const message of await this.store.allMessages()) {
        if (message.backedUp || message.seq === null || message.problem) continue
        if (!this.chats.some((c) => c.id === message.chatId)) continue
        waiting.set(message.chatId, [...(waiting.get(message.chatId) ?? []), message])
      }
      for (const [chatId, messages] of waiting) {
        for (let i = 0; i < messages.length; i += BACKUP_BATCH) {
          const batch = messages.slice(i, i + BACKUP_BATCH)
          const items = batch.map((m) => ({
            seq: m.seq!,
            data: toBase64(
              sealBackup(
                key,
                this.backupContext(chatId, m.seq!),
                encoder.encode(JSON.stringify({ u: m.senderUser, b: m.body, t: m.sentAt })),
              ),
            ),
          }))
          await api.writeBackup(chatId, items)
          await this.store.setBackedUp(batch.map((m) => m.key), true)
        }
      }
    } catch (err) {
      console.error('backup failed', err)
    } finally {
      this.backingUp = false
    }
  }

  /** Brings back, from the backup, messages this device does not have. */
  private async restoreHistory() {
    this.restoring = true
    try {
      for (const chat of this.chats) {
        await this.enqueue(chat.id, () => this.restoreChat(chat.id)).catch((err) =>
          console.error('could not restore chat', chat.id, err),
        )
      }
    } finally {
      this.restoring = false
    }
  }

  private async restoreChat(chatId: string, from = 0) {
    const key = this.backupKey
    if (!key) return
    const local = new Map((await this.store.messages(chatId)).map((m) => [m.seq, m]))
    const restored: StoredMessage[] = []
    for (let after = from; ; ) {
      const page = await api.readBackup(chatId, after)
      for (const item of page.items) {
        after = item.seq
        const existing = local.get(item.seq)
        if (existing && !existing.problem) continue
        try {
          const plain = JSON.parse(
            decoder.decode(openBackup(key, this.backupContext(chatId, item.seq), fromBase64(item.data))),
          )
          if (typeof plain?.u !== 'number' || typeof plain.b !== 'string' || typeof plain.t !== 'number') continue
          restored.push({
            key: existing?.key ?? `${chatId}/#${item.seq}`,
            chatId,
            seq: item.seq,
            senderUser: plain.u,
            body: plain.b,
            sentAt: plain.t,
            backedUp: true,
          })
        } catch (err) {
          console.error('could not read backup item', chatId, item.seq, err)
        }
      }
      if (!page.more || !page.items.length) break
    }
    if (restored.length) {
      await this.store.save({ messages: restored })
      restored.forEach((m) => this.showMessage(m))
    }
  }

  // -- this person's devices -------------------------------------------------

  get thisDeviceId(): number {
    return this.deviceId
  }

  /** Whether one of the person's devices is confirmed with their recovery code. */
  deviceConfirmed(deviceId: number): boolean {
    const me = this.users.find((u) => u.id === this.me.id)
    const device = me?.devices.find((d) => d.id === deviceId)
    if (!me?.identity_key || !device?.signature) return false
    const key = this.identityKey ? toBase64(this.identityKey) : me.identity_key
    if (key !== me.identity_key) return false
    const publicKey = this.devicePublicKey(deviceId)
    // Not in any chat with this device: its key cannot be checked here, so
    // the signature's presence is all there is to show. Chats still check it.
    if (!publicKey.length) return true
    try {
      return verifyDevice(fromBase64(key), device.identity, publicKey, fromBase64(device.signature))
    } catch {
      return false
    }
  }

  /**
   * A device's public signature key, as recorded in the key group of a chat
   * this device shares with it. Empty when no shared chat holds it.
   */
  private devicePublicKey(deviceId: number): Uint8Array {
    if (deviceId === this.deviceId) return this.device.signaturePublicKey()
    const identity = `u${this.me.id}:d${deviceId}`
    for (const chat of this.chats) {
      if (!chat.joined || !this.device.hasChat(chat.id)) continue
      for (const [member, key] of this.device.memberKeys(chat.id) as [string, Uint8Array][]) {
        if (member === identity) return key
      }
    }
    return new Uint8Array()
  }

  /** Signs out another of this person's devices for good. */
  async revokeDevice(deviceId: number) {
    await api.revokeDevice(deviceId)
    await this.refresh('all')
  }

  /** Signs this device out for good and erases everything it stored. */
  async removeThisDevice() {
    await api.revokeDevice(this.deviceId)
    await this.wipe()
  }

  /** The server no longer accepts this device: forget its keys and messages. */
  private async wipe() {
    if (this.status === 'revoked') return
    this.status = 'revoked'
    this.stop()
    await Store.erase(this.me.id).catch((err) => console.error('could not erase local data', err))
  }

  private endSession() {
    if (this.status === 'signedOut' || this.status === 'revoked') return
    this.status = 'signedOut'
    this.stop()
  }

  // -- things the person does ------------------------------------------------

  async openChat(chatId: string) {
    this.activeChat = chatId
    if (!this.messages[chatId]) {
      this.messages[chatId] = (await this.store.messages(chatId)).sort(inOrder)
    }
    await this.markRead(chatId)
  }

  async markRead(chatId: string) {
    const meta = this.metaFor(chatId)
    if (meta.unread === 0) return
    meta.unread = 0
    await this.store.save({ chats: [this.metaCopy(chatId)] })
  }

  /** Starts a DM (one other person) or a group chat, and returns its id. */
  async createChat(kind: 'dm' | 'group', memberIds: number[], name?: string): Promise<string> {
    const { created, chat } = await api.createChat(kind, memberIds, name)
    if (created) {
      await this.enqueue(chat.id, async () => {
        this.device.createChat(chat.id)
        await this.persist({ chats: [this.metaCopy(chat.id)] })
      })
    }
    await this.refresh(new Set([chat.id]))
    return chat.id
  }

  async addMembers(chatId: string, userIds: number[]) {
    await api.addMembers(chatId, userIds)
    await this.refresh(new Set([chatId]))
  }

  async removeMember(chatId: string, userId: number) {
    await api.removeMember(chatId, userId)
    await this.refresh(new Set([chatId]))
  }

  /** Asks another member's device to add this one back to a chat it cannot read. */
  async repair(chatId: string) {
    await this.enqueue(chatId, async () => {
      if (this.device.hasChat(chatId)) this.device.deleteChat(chatId)
      this.metaFor(chatId).broken = false
      await this.persist({ chats: [this.metaCopy(chatId)] })
      await api.rejoin(chatId)
    })
    await this.refresh(new Set([chatId]))
  }

  async send(chatId: string, body: string, retryKey?: string) {
    const clientId = retryKey?.split('/')[1] ?? crypto.randomUUID()
    const message: StoredMessage = {
      key: `${chatId}/${clientId}`,
      chatId,
      seq: null,
      senderUser: this.me.id,
      body,
      sentAt: Math.floor(Date.now() / 1000),
    }
    // Shown and saved straight away; it gets its place in the chat once the server has it.
    await this.store.save({ messages: [message] })
    this.showMessage(message)

    await this.enqueue(chatId, async () => {
      const chat = this.chats.find((c) => c.id === chatId)
      try {
        if (!chat) throw new Error('chat not found')
        for (let attempt = 0; attempt < 4; attempt++) {
          const data = this.device.encrypt(chatId, encoder.encode(JSON.stringify({ t: 'text', body })))
          // Save before sending: each encryption moves the keys forward,
          // and a position must never be used twice.
          await this.persist()
          try {
            const sent = await api.sendMessage(chatId, this.device.epoch(chatId), toBase64(data), clientId)
            const final = { ...message, seq: sent.seq, sentAt: sent.created_at }
            await this.store.save({ messages: [final] })
            this.showMessage(final)
            this.backupSoon()
            return
          } catch (err) {
            if (!isStatus(err, 409)) throw err
            // The chat's keys changed under us: catch up, then encrypt again.
            if (!(await this.pullEvents(chat))) throw err
          }
        }
        throw new Error('could not send after several attempts')
      } catch (err) {
        console.error('send failed', err)
        const failed: StoredMessage = { ...message, problem: 'failed' }
        await this.store.save({ messages: [failed] })
        this.showMessage(failed)
      }
    })
  }

  // -- for display -------------------------------------------------------------

  userName(userId: number): string {
    return this.users.find((u) => u.id === userId)?.display_name ?? 'Someone who left'
  }

  chatTitle(chat: ChatInfo): string {
    if (chat.kind === 'group') return chat.name ?? 'Group'
    const other = chat.members.find((m) => m.user_id !== this.me.id)
    return other?.display_name ?? 'Just you'
  }
}
