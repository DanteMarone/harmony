// What this device keeps on disk (IndexedDB): its encryption keys and the
// messages it has decrypted.
//
// Messages can only be decrypted once (the keys are destroyed after use, which
// is what makes past messages safe if a key later leaks), so the decrypted
// copy kept here is the only readable one. It never leaves this device.

export interface StoredMessage {
  /** `${chatId}/${clientId}` for our own messages, `${chatId}/#${seq}` for others'. */
  key: string
  chatId: string
  /** Position in the chat; null while one of our own messages is still being sent. */
  seq: number | null
  /** Who sent it, as proven by the encryption layer (not just claimed by the server). */
  senderUser: number
  body: string
  /** Seconds since 1970. */
  sentAt: number
  /** Set when the message could not be sent or could not be decrypted. */
  problem?: 'failed' | 'undecryptable'
}

export interface StoredChat {
  chatId: string
  /** Last event of this chat that has been processed. */
  cursor: number
  /** Messages from others that arrived while the chat was not on screen. */
  unread: number
  /** This device could not join or follow the chat and needs to be added back. */
  broken?: boolean
  /** A key change of ours has been sent but has not come back from the server yet. */
  pendingCommit?: boolean
}

export interface DeviceRecord {
  deviceId: number
  identity: string
  state: Uint8Array
}

function wrap<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
    tx.onabort = () => reject(tx.error ?? new Error('storage transaction aborted'))
  })
}

export class Store {
  private constructor(private db: IDBDatabase) {}

  /** One database per account, so two people sharing a browser never mix data. */
  static async open(userId: number): Promise<Store> {
    const request = indexedDB.open(`harmony-user-${userId}`, 1)
    request.onupgradeneeded = () => {
      const db = request.result
      db.createObjectStore('device')
      db.createObjectStore('chats', { keyPath: 'chatId' })
      const messages = db.createObjectStore('messages', { keyPath: 'key' })
      messages.createIndex('byChat', 'chatId')
    }
    return new Store(await wrap(request))
  }

  static async erase(userId: number): Promise<void> {
    await wrap(indexedDB.deleteDatabase(`harmony-user-${userId}`))
  }

  close() {
    this.db.close()
  }

  async device(): Promise<DeviceRecord | undefined> {
    return wrap(this.db.transaction('device').objectStore('device').get('self'))
  }

  /**
   * Saves keys, chat positions and messages together. Either all of it is
   * written or none, so the keys and the messages they decrypted never get
   * out of step, even if the tab is closed mid-write.
   */
  async save(changes: {
    device?: DeviceRecord
    chats?: StoredChat[]
    messages?: StoredMessage[]
    deleteMessages?: string[]
    deleteChats?: string[]
  }): Promise<void> {
    const tx = this.db.transaction(['device', 'chats', 'messages'], 'readwrite')
    if (changes.device) tx.objectStore('device').put(changes.device, 'self')
    for (const chat of changes.chats ?? []) tx.objectStore('chats').put(chat)
    for (const message of changes.messages ?? []) tx.objectStore('messages').put(message)
    for (const key of changes.deleteMessages ?? []) tx.objectStore('messages').delete(key)
    for (const chatId of changes.deleteChats ?? []) {
      tx.objectStore('chats').delete(chatId)
      const index = tx.objectStore('messages').index('byChat')
      index.openKeyCursor(IDBKeyRange.only(chatId)).onsuccess = (event) => {
        const cursor = (event.target as IDBRequest<IDBCursor | null>).result
        if (cursor) {
          tx.objectStore('messages').delete(cursor.primaryKey)
          cursor.continue()
        }
      }
    }
    await done(tx)
  }

  async chats(): Promise<StoredChat[]> {
    return wrap(this.db.transaction('chats').objectStore('chats').getAll())
  }

  async messages(chatId: string): Promise<StoredMessage[]> {
    const index = this.db.transaction('messages').objectStore('messages').index('byChat')
    return wrap(index.getAll(IDBKeyRange.only(chatId)))
  }
}
