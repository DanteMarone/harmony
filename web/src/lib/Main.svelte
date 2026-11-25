<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import type { User } from './api'
  import Avatar from './Avatar.svelte'
  import type { Call } from './call.svelte'
  import CallBar from './CallBar.svelte'
  import Chat from './Chat.svelte'
  import Devices from './Devices.svelte'
  import { Engine } from './engine.svelte'
  import Icon from './Icon.svelte'
  import IncomingCall from './IncomingCall.svelte'
  import Invites from './Invites.svelte'
  import Members from './Members.svelte'
  import NewChat from './NewChat.svelte'
  import NewRoom from './NewRoom.svelte'
  import Recovery from './Recovery.svelte'
  import RoomPreview from './RoomPreview.svelte'
  import Settings from './Settings.svelte'
  import { playJoin, playLeave, startRinging } from './sounds'
  import Verify from './Verify.svelte'

  let { user, onSignOut }: { user: User; onSignOut: () => void } = $props()

  // A new account gets a new Main (see App.svelte), so the first value is the only one.
  // svelte-ignore state_referenced_locally
  const engine = new Engine(user)
  void engine.start()
  onDestroy(() => engine.stop())

  /** The call this device is in, if any. One at a time. */
  let call = $state<Call | null>(null)
  let callError = $state('')
  const callChat = $derived(call ? (engine.chats.find((c) => c.id === call!.chatId) ?? null) : null)

  async function joinCall(chatId: string) {
    callError = ''
    ringing = null
    // Asking from a click is the only time browsers allow it; later calls can then alert in the background.
    if (typeof Notification !== 'undefined' && Notification.permission === 'default') void Notification.requestPermission()
    if (call?.chatId === chatId) return
    await call?.end()
    // The call code (LiveKit) is only downloaded the first time someone joins a call.
    const { Call } = await import('./call.svelte')
    const next = new Call(engine, chatId)
    call = next
    await next.start()
  }

  // A call that ended (left, dropped, or failed) is cleared away.
  $effect(() => {
    if (call?.status === 'ended') {
      callError = call.error
      call = null
      playLeave()
    }
  })

  // Join and leave sounds while in a call: one when this device connects, then
  // one each time someone else comes or goes.
  let heard: Set<string> | null = null
  $effect(() => {
    if (!call || call.status === 'connecting') {
      heard = null
      return
    }
    const now = new Set(call.members.map((m) => m.identity))
    if (heard === null) playJoin()
    else if ([...now].some((id) => !heard!.has(id))) playJoin()
    else if ([...heard].some((id) => !now.has(id))) playLeave()
    heard = now
  })

  // -- incoming calls --
  // A call "rings" when a chat goes from no call to a call that this person is
  // not in. Calls already running when Harmony opens don't ring; the chat list
  // shows them instead.
  let ringing = $state<{ chatId: string; callerId: number } | null>(null)
  let seenCalls: Record<string, number[]> | null = null
  $effect(() => {
    const calls = engine.calls
    if (!engine.callsEnabled) return
    untrack(() => {
      const before = seenCalls
      seenCalls = calls
      if (before) {
        for (const [chatId, people] of Object.entries(calls)) {
          if (!people.length || before[chatId]?.length) continue
          if (people.includes(engine.me.id) || call?.chatId === chatId) continue
          // A room's voice channel is somewhere people drop in, not a call to answer.
          const chat = engine.chats.find((c) => c.id === chatId)
          if (!chat || chat.kind === 'room') continue
          ringing = { chatId, callerId: people[0] }
        }
      }
      // Stop ringing once the call is over, or this person joined from another device.
      if (ringing && (!calls[ringing.chatId]?.length || calls[ringing.chatId].includes(engine.me.id))) ringing = null
    })
  })
  const ringChat = $derived(ringing ? (engine.chats.find((c) => c.id === ringing!.chatId) ?? null) : null)

  // While ringing: play the ringtone, flash the tab title, and show a desktop
  // alert if Harmony is in the background. Give up after 45 seconds.
  $effect(() => {
    if (!ringing || !ringChat) return
    const caller = engine.userName(ringing.callerId)
    const stop = startRinging()
    const title = document.title
    let flip = false
    const flash = setInterval(() => (document.title = (flip = !flip) ? `📞 ${caller} is calling` : title), 1000)
    let alert: Notification | null = null
    if (document.hidden && typeof Notification !== 'undefined' && Notification.permission === 'granted') {
      const chatId = ringing.chatId
      alert = new Notification(`${caller} is calling`, {
        body: ringChat.kind === 'group' ? `Voice call in # ${engine.chatTitle(ringChat)}` : 'Encrypted voice call on Harmony',
        tag: `call-${chatId}`,
        requireInteraction: true,
      })
      alert.onclick = () => {
        window.focus()
        open(chatId)
      }
    }
    const giveUp = setTimeout(() => (ringing = null), 45_000)
    return () => {
      stop()
      clearInterval(flash)
      clearTimeout(giveUp)
      document.title = title
      alert?.close()
    }
  })

  function answer() {
    if (!ringing) return
    const chatId = ringing.chatId
    open(chatId)
    void joinCall(chatId)
  }
  // Leaving a chat, or this device being signed out, ends its call.
  $effect(() => {
    if (call && (engine.status !== 'ready' || (engine.chats.length && !callChat))) void call.end()
  })
  onDestroy(() => void call?.end())

  // -- rooms --
  /** A room this person is looking at but is not in yet. */
  let previewRoom = $state<string | null>(null)
  const previewInfo = $derived(previewRoom ? (engine.rooms.find((r) => r.id === previewRoom) ?? null) : null)
  /** The room whose voice channel this device is on its way into. */
  let joiningVoice = $state<string | null>(null)
  let joiningRoom = $state(false)

  function openRoom(roomId: string) {
    if (engine.chats.some((c) => c.id === roomId)) {
      open(roomId)
      return
    }
    engine.activeChat = null
    previewRoom = roomId
    showList = false
  }

  async function joinRoom(roomId: string) {
    joiningRoom = true
    callError = ''
    try {
      await engine.joinRoom(roomId)
      open(roomId)
    } catch (err) {
      callError = err instanceof Error ? err.message : 'Could not join the room.'
    } finally {
      joiningRoom = false
    }
  }

  /** Into the room (if need be), then into its voice channel. */
  async function joinVoice(roomId: string) {
    if (joiningVoice) return
    callError = ''
    if (call?.chatId === roomId) {
      open(roomId)
      return
    }
    joiningVoice = roomId
    try {
      await engine.joinRoom(roomId)
      open(roomId)
      // Someone already in the room has to hand this device its keys first.
      if (!(await engine.waitUntilReadable(roomId, 30_000))) {
        if (engine.chats.some((c) => c.id === roomId)) {
          callError =
            "Nobody in this room has been online to let this device in yet. You're in the room, and can join its voice once someone is."
        }
        return
      }
      await joinCall(roomId)
    } catch (err) {
      callError = err instanceof Error ? err.message : 'Could not join the voice channel.'
    } finally {
      joiningVoice = null
    }
  }

  let dialog = $state<'new' | 'room' | 'members' | 'invites' | 'security' | 'devices' | 'settings' | null>(null)
  /** The person whose verification screen is open, on top of any other dialog. */
  let verifying = $state<number | null>(null)
  // A recovery code must be set up before anything else; entering one on a
  // new device is asked for once and can be skipped.
  const mustRecover = $derived(engine.recovery === 'setup' || engine.recovery === 'code')

  function openRecovery() {
    verifying = null
    dialog = 'security'
  }

  const chats = $derived(
    engine.chats.filter((c) => c.kind !== 'room').sort((a, b) => Math.max(b.last_seq, 0) - Math.max(a.last_seq, 0) || b.created_at - a.created_at),
  )
  const active = $derived(engine.chats.find((c) => c.id === engine.activeChat) ?? null)

  function onVisible() {
    if (!document.hidden && engine.activeChat) void engine.markRead(engine.activeChat)
  }
  document.addEventListener('visibilitychange', onVisible)
  onDestroy(() => document.removeEventListener('visibilitychange', onVisible))

  /** On a narrow screen only one pane fits: the chat list, or the open chat. */
  let showList = $state(true)

  function open(chatId: string) {
    showList = false
    previewRoom = null
    void engine.openChat(chatId)
  }

  function newRoomDone(roomId: string | null) {
    dialog = null
    if (roomId) open(roomId)
  }

  function newChatDone(chatId: string | null) {
    dialog = null
    if (chatId) open(chatId)
  }

  // Esc closes the topmost dialog. The recovery code set-up can't be skipped this way.
  function onKeydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || mustRecover) return
    if (ringing) ringing = null
    else if (verifying !== null) verifying = null
    else if (dialog) dialog = null
  }
</script>

<svelte:window onkeydown={onKeydown} />

{#if engine.status === 'starting'}
  <main class="card"><p class="lead">Setting up encryption on this device…</p></main>
{:else if engine.status === 'locked'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">Harmony is already open in another tab or window. Close it there, then reload this page.</p>
    <button onclick={() => location.reload()}>Reload</button>
  </main>
{:else if engine.status === 'revoked'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">This device was signed out, and its keys and messages were erased from this browser.</p>
    <p class="hint">Sign in again to use it as a new device. Your recovery code brings your history back.</p>
    <button onclick={onSignOut}>Sign in again</button>
  </main>
{:else if engine.status === 'signedOut'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">You have been signed out.</p>
    <button onclick={onSignOut}>Sign in</button>
  </main>
{:else if engine.status === 'error'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="error" role="alert">Could not start: {engine.error}</p>
    <button onclick={() => location.reload()}>Try again</button>
  </main>
{:else}
  <div class="layout" class:chat-open={(active || previewInfo) && !showList}>
    <aside class="sidebar">
      <header>
        <span class="brand">Harmony</span>
        <button class="icon-button" title="New chat" aria-label="New chat" onclick={() => (dialog = 'new')}>
          <Icon name="plus" />
        </button>
      </header>
      <nav aria-label="Rooms and chats">
        <div class="section-head">
          <p class="section-label">Rooms</p>
          <button class="icon-button small" title="New room" aria-label="New room" onclick={() => (dialog = 'room')}>
            <Icon name="plus" size={16} />
          </button>
        </div>
        {#each engine.rooms as room (room.id)}
          {@const unread = engine.meta[room.id]?.unread ?? 0}
          {@const current = room.id === engine.activeChat || room.id === previewRoom}
          <div class="room-row">
            <button
              class="chat-item room-item"
              class:active={current}
              class:unread={unread > 0 && room.member}
              aria-current={current ? 'page' : undefined}
              onclick={() => openRoom(room.id)}
            >
              <Icon name="hash" size={18} />
              <span class="title">{room.name}</span>
              {#if unread && room.member}
                <span class="badge" aria-label="{unread} unread">{unread > 99 ? '99+' : unread}</span>
              {/if}
            </button>
            {#if engine.callsEnabled}
              <button
                class="icon-button small voice-button"
                class:live={call?.chatId === room.id}
                title={call?.chatId === room.id ? `In voice in ${room.name}` : `Join voice in ${room.name}`}
                aria-label="Join voice in {room.name}"
                disabled={joiningVoice !== null}
                onclick={() => joinVoice(room.id)}
              >
                <Icon name="volume" size={16} />
              </button>
            {/if}
          </div>
          {#if room.voice.length || joiningVoice === room.id}
            <ul class="voice-people" aria-label="In voice in {room.name}">
              {#each room.voice as userId (userId)}
                <li>
                  <Avatar name={engine.userName(userId)} id={userId} size={20} />
                  <span>{engine.userName(userId)}</span>
                </li>
              {/each}
              {#if joiningVoice === room.id && !room.voice.includes(engine.me.id)}
                <li class="hint">Joining…</li>
              {/if}
            </ul>
          {/if}
        {:else}
          <p class="hint room-empty">No rooms yet. A room is a chat with its own voice channel that anyone can join.</p>
        {/each}

        {#if chats.length}<p class="section-label">Chats</p>{/if}
        {#each chats as chat (chat.id)}
          {@const unread = engine.meta[chat.id]?.unread ?? 0}
          {@const inCall = engine.calls[chat.id]?.length ?? 0}
          {@const other = chat.members.find((m) => m.user_id !== engine.me.id)}
          <button
            class="chat-item"
            class:active={chat.id === engine.activeChat}
            class:unread={unread > 0}
            aria-current={chat.id === engine.activeChat ? 'page' : undefined}
            onclick={() => open(chat.id)}
          >
            <Avatar
              name={engine.chatTitle(chat)}
              id={chat.kind === 'dm' ? (other?.user_id ?? engine.me.id) : chat.id}
              group={chat.kind !== 'dm'}
              size={32}
            />
            <span class="title">{engine.chatTitle(chat)}</span>
            {#if inCall}
              <span class="in-call" title="Call in progress" aria-label="Call in progress with {inCall}">
                <Icon name="phone" size={14} />{inCall}
              </span>
            {/if}
            {#if unread}
              <span class="badge" aria-label="{unread} unread">{unread > 99 ? '99+' : unread}</span>
            {/if}
          </button>
        {:else}
          <div class="empty">
            <p class="hint">No chats yet.</p>
            <button onclick={() => (dialog = 'new')}>Start a chat</button>
          </div>
        {/each}
      </nav>
      {#if call && callChat}
        <CallBar {engine} {call} title={engine.chatTitle(callChat)} />
      {/if}
      {#if engine.recovery === 'skipped'}
        <button class="secondary attention confirm-device" onclick={openRecovery}>
          <Icon name="alert" size={16} /> Confirm this device
        </button>
      {/if}
      <footer class="me">
        <span class="presence" class:offline={!engine.connected}>
          <Avatar name={user.display_name} id={user.id} size={34} />
        </span>
        <div class="who">
          <span class="name">{user.display_name}</span>
          <span class="hint">{engine.connected ? 'Connected' : 'Reconnecting…'}</span>
        </div>
        <button class="icon-button" title="Settings" aria-label="Settings" onclick={() => (dialog = 'settings')}>
          <Icon name="settings" />
        </button>
      </footer>
    </aside>

    {#if active}
      <Chat
        {engine}
        chat={active}
        inCall={call?.chatId === active.id}
        onCall={() => joinCall(active.id)}
        onMembers={() => (dialog = 'members')}
        onVerify={(userId) => (verifying = userId)}
        onBack={() => (showList = true)}
      />
    {:else if previewInfo}
      <RoomPreview
        {engine}
        room={previewInfo}
        busy={joiningRoom || joiningVoice !== null}
        onJoin={() => joinRoom(previewInfo.id)}
        onJoinVoice={() => joinVoice(previewInfo.id)}
        onBack={() => (showList = true)}
      />
    {:else}
      <section class="chat placeholder">
        <div class="welcome">
          <Avatar name={user.display_name} id={user.id} size={56} />
          <h2>Hi, {user.display_name}</h2>
          <p class="lead">
            Pick a room or chat on the left, or start a new one. Everything here is end-to-end encrypted.
          </p>
          <button onclick={() => (dialog = 'new')}>New chat</button>
        </div>
      </section>
    {/if}
  </div>

  {#if ringing && ringChat}
    <IncomingCall
      caller={engine.userName(ringing.callerId)}
      callerId={ringing.callerId}
      chatTitle={engine.chatTitle(ringChat)}
      group={ringChat.kind === 'group'}
      onJoin={answer}
      onDecline={() => (ringing = null)}
    />
  {/if}

  {#if callError}
    <div class="toast" role="alert">
      <Icon name="alert" size={18} />
      <span>{callError}</span>
      <button class="icon-button" aria-label="Dismiss" title="Dismiss" onclick={() => (callError = '')}>
        <Icon name="close" size={16} />
      </button>
    </div>
  {/if}

  {#if dialog === 'settings'}
    <Settings
      {user}
      needsConfirm={engine.recovery === 'skipped'}
      onOpen={(page) => (page === 'security' ? openRecovery() : (dialog = page))}
      {onSignOut}
      onClose={() => (dialog = null)}
    />
  {:else if dialog === 'new'}
    <NewChat {engine} onDone={newChatDone} />
  {:else if dialog === 'room'}
    <NewRoom {engine} onDone={newRoomDone} />
  {:else if dialog === 'members' && active}
    <Members {engine} chat={active} onClose={() => (dialog = null)} onVerify={(userId) => (verifying = userId)} />
  {:else if dialog === 'invites'}
    <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (dialog = null)}>
      <div class="dialog">
        <Invites />
        <div class="actions"><button class="secondary" onclick={() => (dialog = null)}>Close</button></div>
      </div>
    </div>
  {/if}

  {#if mustRecover}
    <Recovery {engine} />
  {:else if verifying !== null}
    <Verify
      {engine}
      userId={verifying}
      chatId={active?.id}
      onClose={() => (verifying = null)}
      onRecovery={openRecovery}
    />
  {:else if dialog === 'devices'}
    <Devices {engine} onClose={() => (dialog = null)} />
  {:else if dialog === 'security'}
    <Recovery {engine} onClose={() => (dialog = null)} />
  {/if}
{/if}
