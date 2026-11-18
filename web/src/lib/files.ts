// Photos and files: encrypted on the sender's device with a fresh key for
// each file (AES-256-GCM, from the browser's own Web Crypto). The key travels
// inside the chat's end-to-end encrypted message; the server only ever relays
// the ciphertext, and deletes it once every device has its own copy.

import { fromBase64, toBase64 } from './api'

/** The server refuses larger files by default. The tag adds 16 bytes. */
export const MAX_FILE_BYTES = 25 * 1024 * 1024 - 16

/**
 * Photo types shown inline. Anything else (SVG and HTML included) is only
 * offered as a download, so a file can never run code in this page.
 */
const IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif', 'image/bmp'])

export const isImage = (type: string) => IMAGE_TYPES.has(type)

/** The type a decrypted copy is stored under: a known photo type, or plain bytes. */
export const safeType = (type: string) => (isImage(type) ? type : 'application/octet-stream')

export async function encryptFile(plain: Uint8Array): Promise<{ data: Uint8Array; key: string; iv: string }> {
  const raw = crypto.getRandomValues(new Uint8Array(32))
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const key = await crypto.subtle.importKey('raw', raw, 'AES-GCM', false, ['encrypt'])
  const data = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, plain as BufferSource))
  return { data, key: toBase64(raw), iv: toBase64(iv) }
}

export async function decryptFile(data: Uint8Array, key: string, iv: string): Promise<Uint8Array> {
  const cryptoKey = await crypto.subtle.importKey('raw', fromBase64(key) as BufferSource, 'AES-GCM', false, ['decrypt'])
  return new Uint8Array(
    await crypto.subtle.decrypt({ name: 'AES-GCM', iv: fromBase64(iv) as BufferSource }, cryptoKey, data as BufferSource),
  )
}

/** Pixel size of a photo, if the browser can read it. */
export async function imageSize(file: Blob): Promise<{ width: number; height: number } | undefined> {
  if (!isImage(file.type)) return undefined
  try {
    const bitmap = await createImageBitmap(file)
    const size = { width: bitmap.width, height: bitmap.height }
    bitmap.close()
    return size
  } catch {
    return undefined
  }
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/** Checks a file description that came from someone else's device. */
export function validFileRef(f: unknown): f is {
  id: string
  name: string
  type: string
  size: number
  key: string
  iv: string
  width?: number
  height?: number
} {
  if (!f || typeof f !== 'object') return false
  const r = f as Record<string, unknown>
  const dimension = (v: unknown) => v === undefined || (typeof v === 'number' && Number.isInteger(v) && v > 0 && v < 100_000)
  return (
    typeof r.id === 'string' &&
    /^[0-9a-f]{64}$/.test(r.id) &&
    typeof r.name === 'string' &&
    r.name.length > 0 &&
    r.name.length <= 255 &&
    typeof r.type === 'string' &&
    r.type.length <= 255 &&
    typeof r.size === 'number' &&
    Number.isInteger(r.size) &&
    r.size >= 0 &&
    typeof r.key === 'string' &&
    typeof r.iv === 'string' &&
    dimension(r.width) &&
    dimension(r.height)
  )
}
