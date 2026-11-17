// Thin wrapper around the Harmony server API.

export interface User {
  id: number
  username: string
  display_name: string
  is_admin: boolean
}

export interface NewInvite {
  id: number
  code: string
  expires_at: number
}

export interface InviteInfo {
  id: number
  created_at: number
  expires_at: number
  used_at: number | null
  used_by: string | null
}

export interface DeviceInfo {
  id: number
  identity: string
  /** The owner's signature over this device's key (base64), once they confirmed it. */
  signature: string | null
}

export interface UserInfo {
  id: number
  username: string
  display_name: string
  /** Public identity key (base64); null until the person sets up a recovery code. */
  identity_key: string | null
  devices: DeviceInfo[]
}

export interface BackupItem {
  seq: number
  data: string
}

export interface ChatMember {
  user_id: number
  username: string
  display_name: string
}

export interface ChatInfo {
  id: string
  kind: 'dm' | 'group'
  name: string | null
  epoch: number
  created_by: number | null
  created_at: number
  last_seq: number
  members: ChatMember[]
  /** Whether this device holds the chat's keys yet. */
  joined: boolean
}

export interface PendingDevice {
  user_id: number
  device_id: number
  identity: string
}

export interface ClaimedKeyPackage extends PendingDevice {
  key_package: string
}

export interface ChatEvent {
  seq: number
  kind: 'message' | 'commit'
  sender_user: number | null
  sender_device: number | null
  epoch: number
  client_id: string | null
  data: string
  created_at: number
}

export interface Welcome {
  id: number
  chat_id: string
  after_seq: number
  data: string
}

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

let deviceId: number | null = null

/** Sets the device that later calls are made from. */
export function useDevice(id: number | null) {
  deviceId = id
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {}
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  if (deviceId !== null) headers['X-Harmony-Device'] = String(deviceId)
  const response = await fetch(`/api${path}`, {
    method,
    credentials: 'same-origin',
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (response.status === 204) return undefined as T
  const data = await response.json().catch(() => null)
  if (!response.ok) {
    throw new ApiError(response.status, data?.error ?? `request failed (${response.status})`)
  }
  return data as T
}

export const api = {
  /** The signed-in user, or null when nobody is signed in. */
  async me(): Promise<User | null> {
    try {
      return await request<User>('GET', '/me')
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) return null
      throw err
    }
  },
  login: (username: string, password: string) =>
    request<User>('POST', '/auth/login', { username, password }),
  register: (invite: string, username: string, password: string, display_name: string) =>
    request<User>('POST', '/auth/register', { invite, username, password, display_name }),
  logout: () => request<void>('POST', '/auth/logout'),
  listInvites: () => request<InviteInfo[]>('GET', '/invites'),
  createInvite: () => request<NewInvite>('POST', '/invites'),
  revokeInvite: (id: number) => request<void>('DELETE', `/invites/${id}`),

  users: () => request<UserInfo[]>('GET', '/users'),
  createDevice: () => request<{ id: number; identity: string }>('POST', '/devices'),
  keyPackageCount: (device: number) =>
    request<{ one_time: number; last_resort: boolean }>('GET', `/devices/${device}/key-packages`),
  uploadKeyPackages: (device: number, one_time: string[], last_resort?: string) =>
    request<void>('POST', `/devices/${device}/key-packages`, { one_time, last_resort }),

  chats: () => request<ChatInfo[]>('GET', '/chats'),
  createChat: (kind: 'dm' | 'group', member_ids: number[], name?: string) =>
    request<{ created: boolean; chat: ChatInfo }>('POST', '/chats', { kind, member_ids, name }),
  addMembers: (chat: string, user_ids: number[]) =>
    request<void>('POST', `/chats/${chat}/members`, { user_ids }),
  removeMember: (chat: string, user: number) => request<void>('DELETE', `/chats/${chat}/members/${user}`),
  pending: (chat: string) =>
    request<{ add: PendingDevice[]; remove: PendingDevice[] }>('GET', `/chats/${chat}/pending`),
  claim: (chat: string, device_ids: number[]) =>
    request<{ key_packages: ClaimedKeyPackage[] }>('POST', `/chats/${chat}/claim`, { device_ids }),
  commit: (
    chat: string,
    body: { epoch: number; commit: string; welcome?: string; added: number[]; removed: number[] },
  ) => request<{ seq: number; epoch: number }>('POST', `/chats/${chat}/commit`, body),
  rejoin: (chat: string) => request<void>('POST', `/chats/${chat}/rejoin`),
  sendMessage: (chat: string, epoch: number, data: string, client_id: string) =>
    request<{ seq: number; created_at: number }>('POST', `/chats/${chat}/messages`, { epoch, data, client_id }),
  events: (chat: string, after: number) =>
    request<{ events: ChatEvent[]; more: boolean }>('GET', `/chats/${chat}/events?after=${after}`),
  setIdentity: (identity_key: string, signature: string) =>
    request<void>('PUT', '/identity', { identity_key, signature }),
  signDevice: (device: number, signature: string) =>
    request<void>('PUT', `/devices/${device}/signature`, { signature }),
  writeBackup: (chat: string, items: BackupItem[]) => request<void>('POST', `/backup/${chat}`, { items }),
  readBackup: (chat: string, after: number) =>
    request<{ items: BackupItem[]; more: boolean }>('GET', `/backup/${chat}?after=${after}`),
  welcomes: () => request<{ welcomes: Welcome[] }>('GET', '/welcomes'),
  ackWelcome: (id: number) => request<void>('DELETE', `/welcomes/${id}`),
}

export function toBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  }
  return btoa(binary)
}

export function fromBase64(text: string): Uint8Array {
  const binary = atob(text)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes
}
