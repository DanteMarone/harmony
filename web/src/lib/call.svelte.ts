// A voice call in one chat.
//
// Audio goes through LiveKit, a media relay, and every frame is encrypted in
// the browser first (LiveKit's end-to-end encryption, keyed from the chat's
// MLS group). LiveKit and the Harmony server only ever see ciphertext.

import {
  BaseKeyProvider,
  createKeyMaterialFromBuffer,
  isE2EESupported,
  type Participant,
  type RemoteTrack,
  Room,
  RoomEvent,
  Track,
} from 'livekit-client'
import E2EEWorker from 'livekit-client/e2ee-worker?worker'
import { api, ApiError } from './api'
import type { Engine } from './engine.svelte'

/** Must match LiveKit's keyring size: keys for this many recent epochs are kept. */
const KEYRING = 16

/**
 * Hands LiveKit one shared key per chat epoch. Frames carry the index of
 * the key they were encrypted with, so people who have not caught up on a
 * membership change yet can still be heard for the moment it takes them.
 */
class EpochKeyProvider extends BaseKeyProvider {
  constructor() {
    // A wrong key is never "fixed" by ratcheting: it means a device is not
    // in the chat, or is a moment behind.
    super({ sharedKey: true, ratchetWindowSize: 0, failureTolerance: -1, keyringSize: KEYRING })
  }

  async setEpochKey(secret: Uint8Array, epoch: number) {
    const material = await createKeyMaterialFromBuffer(secret.slice().buffer as ArrayBuffer)
    this.onSetEncryptionKey(material, undefined, epoch % KEYRING)
  }
}

export interface CallMember {
  identity: string
  userId: number
  speaking: boolean
  muted: boolean
  /** Not one of the devices in the chat's encryption group: cannot hear or be heard. */
  outsider: boolean
}

export class Call {
  status = $state<'connecting' | 'connected' | 'reconnecting' | 'ended'>('connecting')
  error = $state('')
  muted = $state(false)
  members = $state<CallMember[]>([])
  /** Someone's audio could not be decrypted: they may be a moment behind on a key change. */
  decryptProblems = $state(0)
  /** Set when this device is in the call listening only, because its microphone could not be used. */
  micProblem = $state('')
  /** Microphones this browser can see, for the picker. */
  microphones = $state<{ id: string; label: string }[]>([])
  microphone = $state('')
  /** The browser blocked sound until the person clicks something on the page. */
  audioBlocked = $state(false)

  private room: Room | null = null
  private keys = new EpochKeyProvider()
  private epoch = -1
  private timers: ReturnType<typeof setInterval>[] = []
  private audio = new Map<string, HTMLMediaElement>()
  private onExit = () => api.leaveCallOnExit(this.chatId)

  constructor(
    private engine: Engine,
    readonly chatId: string,
  ) {}

  static supported(): boolean {
    return isE2EESupported()
  }

  async start() {
    try {
      if (!isE2EESupported()) throw new Error('This browser cannot encrypt calls. Try a current Chrome, Edge, Firefox or Safari.')
      await this.updateKey()
      const joined = await api.joinCall(this.chatId)
      const room = new Room({
        adaptiveStream: true,
        dynacast: true,
        e2ee: { keyProvider: this.keys, worker: new E2EEWorker() },
        audioCaptureDefaults: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      })
      this.room = room
      room
        .on(RoomEvent.ParticipantConnected, () => this.refreshMembers())
        .on(RoomEvent.ParticipantDisconnected, () => this.refreshMembers())
        .on(RoomEvent.ActiveSpeakersChanged, () => this.refreshMembers())
        .on(RoomEvent.TrackMuted, () => this.refreshMembers())
        .on(RoomEvent.TrackUnmuted, () => this.refreshMembers())
        .on(RoomEvent.TrackSubscribed, (track, _publication, participant) => this.attach(track, participant))
        .on(RoomEvent.TrackUnsubscribed, (track) => track.detach().forEach((el) => el.remove()))
        .on(RoomEvent.EncryptionError, () => (this.decryptProblems += 1))
        .on(RoomEvent.AudioPlaybackStatusChanged, () => (this.audioBlocked = !room.canPlaybackAudio))
        .on(RoomEvent.MediaDevicesChanged, () => void this.listMicrophones())
        .on(RoomEvent.Reconnecting, () => (this.status = 'reconnecting'))
        .on(RoomEvent.Reconnected, () => (this.status = 'connected'))
        .on(RoomEvent.Disconnected, () => {
          if (this.status === 'connecting') void this.end('Could not reach the call server.')
          else if (this.status !== 'ended') void this.end('The call was disconnected.')
        })
      await room.setE2EEEnabled(true)
      await room.connect(joined.url, joined.token)
      this.status = 'connected'
      addEventListener('pagehide', this.onExit)
      // Keep the call key in step with the chat's membership.
      this.timers.push(setInterval(() => void this.updateKey(), 1000))
      this.timers.push(setInterval(() => void this.heartbeat(), joined.heartbeat_secs * 1000))
      this.refreshMembers()
      this.audioBlocked = !room.canPlaybackAudio
      // No microphone is not a reason to miss the call: stay in and listen.
      await this.enableMicrophone()
    } catch (err) {
      console.error('call failed', err)
      const message =
        err instanceof ApiError
          ? err.message
          : err instanceof DOMException && err.name === 'NotAllowedError'
            ? 'Harmony needs permission to use your microphone.'
            : err instanceof Error
              ? err.message
              : 'Could not join the call.'
      await this.end(message)
    }
  }

  /** Turns the microphone on, or leaves this device listening only with a note saying why. */
  async enableMicrophone(deviceId = this.microphone) {
    const room = this.room
    if (!room) return
    try {
      if (deviceId) await room.switchActiveDevice('audioinput', deviceId, true)
      await room.localParticipant.setMicrophoneEnabled(true, deviceId ? { deviceId: { exact: deviceId } } : undefined)
      this.micProblem = ''
      this.muted = false
    } catch (err) {
      console.warn('microphone unavailable', err)
      await room.localParticipant.setMicrophoneEnabled(false).catch(() => {})
      this.micProblem = microphoneMessage(err)
      this.muted = true
    }
    await this.listMicrophones()
    this.refreshMembers()
  }

  async chooseMicrophone(deviceId: string) {
    this.microphone = deviceId
    await this.enableMicrophone(deviceId)
  }

  private async listMicrophones() {
    try {
      const devices = await Room.getLocalDevices('audioinput', false)
      this.microphones = devices
        .filter((d) => d.deviceId)
        .map((d, i) => ({ id: d.deviceId, label: d.label || `Microphone ${i + 1}` }))
      const active = this.room?.getActiveDevice('audioinput')
      if (active && this.microphones.some((m) => m.id === active)) this.microphone = active
    } catch {
      this.microphones = []
    }
  }

  /** Browsers only play sound after a click on the page. */
  async startAudio() {
    await this.room?.startAudio().catch(() => {})
    this.audioBlocked = !(this.room?.canPlaybackAudio ?? true)
  }

  private async updateKey() {
    const epoch = this.engine.chatEpoch(this.chatId)
    if (epoch === null) {
      // This device lost access to the chat (removed, or signed out).
      if (this.status !== 'connecting') await this.end('You are no longer in this chat.')
      else throw new Error('This device cannot read this chat yet.')
      return
    }
    if (epoch === this.epoch) return
    this.epoch = epoch
    await this.keys.setEpochKey(this.engine.callKey(this.chatId), epoch)
    this.refreshMembers()
  }

  private async heartbeat() {
    try {
      await api.callHeartbeat(this.chatId)
    } catch (err) {
      // Dropped from the list (say, after the computer slept): sign back in to it.
      if (err instanceof ApiError && err.status === 409) await api.joinCall(this.chatId).catch(() => {})
    }
  }

  private attach(track: RemoteTrack, participant: Participant) {
    if (track.kind !== Track.Kind.Audio) return
    const element = track.attach()
    element.dataset.identity = participant.identity
    element.hidden = true
    document.body.append(element)
    this.audio.set(participant.identity, element)
    this.refreshMembers()
  }

  private refreshMembers() {
    const room = this.room
    if (!room) return
    const inChat = new Set(this.engine.chatDevices(this.chatId))
    const people: Participant[] = [room.localParticipant, ...room.remoteParticipants.values()]
    this.members = people.map((p) => ({
      identity: p.identity,
      userId: Number(/^u(\d+):d\d+$/.exec(p.identity)?.[1] ?? 0),
      speaking: p.isSpeaking,
      muted: !p.isMicrophoneEnabled,
      outsider: !inChat.has(p.identity),
    }))
  }

  async toggleMute() {
    if (!this.room) return
    if (this.micProblem) return this.enableMicrophone()
    this.muted = !this.muted
    await this.room.localParticipant.setMicrophoneEnabled(!this.muted)
    this.refreshMembers()
  }

  /** Leaves the call. `reason` is shown if it ended on its own. */
  async end(reason = '') {
    if (this.status === 'ended') return
    this.status = 'ended'
    this.error = reason
    this.timers.forEach(clearInterval)
    this.timers = []
    removeEventListener('pagehide', this.onExit)
    this.audio.forEach((el) => el.remove())
    this.audio.clear()
    const room = this.room
    this.room = null
    await room?.disconnect().catch(() => {})
    await api.leaveCall(this.chatId).catch(() => {})
  }
}

/** A short, plain explanation of why the microphone could not be used. */
function microphoneMessage(err: unknown): string {
  const name = err instanceof Error ? err.name : ''
  switch (name) {
    case 'NotFoundError':
    case 'OverconstrainedError':
      return 'No microphone found. You can listen, but others cannot hear you. Plug one in, or pick one below, then try again.'
    case 'NotAllowedError':
    case 'SecurityError':
      return 'Microphone permission is blocked. You can listen, but others cannot hear you. Allow the microphone for this site (the icon left of the address bar), then try again.'
    case 'NotReadableError':
    case 'AbortError':
      return 'Your microphone is busy or failed to start (another app may be using it). You can listen meanwhile. Try again, or pick another one below.'
    default:
      return 'Your microphone could not be started. You can listen, but others cannot hear you. Try again, or pick another one below.'
  }
}
