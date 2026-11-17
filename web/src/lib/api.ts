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

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    credentials: 'same-origin',
    headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
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
}
