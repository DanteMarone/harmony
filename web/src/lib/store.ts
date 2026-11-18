// What this device keeps on disk (IndexedDB): its encryption keys and the
// messages it has decrypted.
//
// Messages can only be decrypted once (the keys are destroyed after use, which
// is what makes past messages safe if a key later leaks), so the decrypted
// copy kept here is the only readable one. It never leaves this device.

/** A photo or file sent in a message. */
export interface FileRef {
  /** Where the encrypted copy waits on the server; empty until it is uploaded. */
  id: string
  name: string
  /** The sender's stated type, such as `image/png`. Only shown, never trusted. */
  type: string
  size: number
  /** The file's own key and nonce (base64), carried inside the encrypted message. */
  key: string
  iv: string
  /** Pixel size of a photo, so the chat does not jump while it loads. */
  width?: number
  height?: number
}

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
  /** An encrypted copy is in this person's history backup on the server. */
  backedUp?: boolean
  /** A photo or file, with `body` as its (possibly empty) caption. */
  file?: FileRef
  /**
   * `ready`: the file is saved on this device. `fetching`: it is still to be
   * picked up from the server. `gone`: this device does not have it and the
   * server no longer does either.
   */
  fileState?: 'ready' | 'fetching' | 'gone'
  /** Why a message could not be sent, when the server said. */
  note?: string
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
  /** The owner's public identity key this device was confirmed under. */
  identityKey?: Uint8Array
  /** Encrypts the history backup. Derived from the recovery code; never sent anywhere. */
  backupKey?: Uint8Array
  /** The person chose not to enter their recovery code on this device for now. */
  recoverySkipped?: boolean
}

/** What this device remembers about another person's identity key. */
export interface TrustRecord {
  userId: number
  /** The identity key (base64) first seen, or last accepted, for this person. */
  identityKey: string
  /** The person using this device compared safety numbers with them. */
  verified: boolean
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
    const request = indexedDB.open(`harmony-user-${userId}`, 3)
    request.onupgradeneeded = (event) => {
      const db = request.result
      if (event.oldVersion < 1) {
        db.createObjectStore('device')
        db.createObjectStore('chats', { keyPath: 'chatId' })
        const messages = db.createObjectStore('messages', { keyPath: 'key' })
        messages.createIndex('byChat', 'chatId')
      }
      if (event.oldVersion < 2) db.createObjectStore('trust', { keyPath: 'userId' })
      // Decrypted photos and files, by the key of the message they came in.
      if (event.oldVersion < 3) db.createObjectStore('files')
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
    trust?: TrustRecord[]
    deleteTrust?: number[]
    /** Decrypted files, by message key. */
    files?: { key: string; blob: Blob }[]
  }): Promise<void> {
    const tx = this.db.transaction(['device', 'chats', 'messages', 'trust', 'files'], 'readwrite')
    for (const record of changes.trust ?? []) tx.objectStore('trust').put(record)
    for (const userId of changes.deleteTrust ?? []) tx.objectStore('trust').delete(userId)
    if (changes.device) tx.objectStore('device').put(changes.device, 'self')
    for (const chat of changes.chats ?? []) tx.objectStore('chats').put(chat)
    for (const message of changes.messages ?? []) tx.objectStore('messages').put(message)
    for (const file of changes.files ?? []) tx.objectStore('files').put(file.blob, file.key)
    for (const key of changes.deleteMessages ?? []) {
      tx.objectStore('messages').delete(key)
      tx.objectStore('files').delete(key)
    }
    for (const chatId of changes.deleteChats ?? []) {
      tx.objectStore('chats').delete(chatId)
      // Message keys start with the chat id and a slash.
      tx.objectStore('files').delete(IDBKeyRange.bound(`${chatId}/`, `${chatId}/\uffff`))
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

  async allMessages(): Promise<StoredMessage[]> {
    return wrap(this.db.transaction('messages').objectStore('messages').getAll())
  }

  /** A decrypted file saved on this device, by the key of its message. */
  async file(key: string): Promise<Blob | undefined> {
    return wrap(this.db.transaction('files').objectStore('files').get(key))
  }

  async trust(): Promise<TrustRecord[]> {
    return wrap(this.db.transaction('trust').objectStore('trust').getAll())
  }

  /**
   * Records which messages now have (or, with `false`, no longer have) a
   * copy in the backup. Reads and writes in one step so a message changed
   * in the meantime is not overwritten with an old version.
   */
  async setBackedUp(keys: string[] | 'all', backedUp: boolean): Promise<void> {
    const tx = this.db.transaction('messages', 'readwrite')
    const messages = tx.objectStore('messages')
    const update = (message: StoredMessage | undefined) => {
      if (message && !!message.backedUp !== backedUp) messages.put({ ...message, backedUp })
    }
    if (keys === 'all') {
      messages.getAll().onsuccess = (event) =>
        (event.target as IDBRequest<StoredMessage[]>).result.forEach(update)
    } else {
      for (const key of keys) {
        messages.get(key).onsuccess = (event) => update((event.target as IDBRequest<StoredMessage>).result)
      }
    }
    await done(tx)
  }
}
