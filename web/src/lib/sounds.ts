// Short sound cues for calls and messages, made on the fly with the Web Audio
// API so no audio files are shipped or fetched.
//
// Browsers only let a page make sound after the person has interacted with it,
// so the audio context is created (or resumed) on the first click or key press.

export type SoundKind = 'ring' | 'join' | 'leave' | 'hangup' | 'message'

/** Every sound, in the order Settings lists them. */
export const SOUNDS: { kind: SoundKind; title: string; detail: string }[] = [
  { kind: 'ring', title: 'Incoming call', detail: 'Rings when someone calls you' },
  { kind: 'join', title: 'Someone joins', detail: 'When you or someone else connects to your call' },
  { kind: 'leave', title: 'Someone leaves', detail: 'When someone disconnects from your call' },
  { kind: 'hangup', title: 'Hang up', detail: 'When you leave a call' },
  { kind: 'message', title: 'New message', detail: 'When a message arrives in a chat you are not looking at' },
]

const SETTING = 'harmony.sounds'

let context: AudioContext | null = null

function audio(): AudioContext | null {
  if (!context) {
    try {
      context = new AudioContext()
    } catch {
      return null
    }
  }
  if (context.state === 'suspended') void context.resume().catch(() => {})
  return context
}

// Unlock audio on the first interaction, so a ring that arrives later can play.
for (const type of ['pointerdown', 'keydown'] as const) {
  window.addEventListener(type, () => audio(), { once: true, capture: true })
}

/** Which sounds are turned off. Stored per browser, since it is a device preference. */
function readOff(): Set<SoundKind> {
  try {
    const saved = localStorage.getItem(SETTING)
    // Older versions had one switch for every sound, saved as "on" or "off".
    if (saved === 'off') return new Set(SOUNDS.map((s) => s.kind))
    if (!saved || saved === 'on') return new Set()
    const parsed = JSON.parse(saved)
    return new Set(Array.isArray(parsed?.off) ? parsed.off : [])
  } catch {
    return new Set()
  }
}

let off = readOff()

export function soundOn(kind: SoundKind): boolean {
  return !off.has(kind)
}

export function setSoundOn(kind: SoundKind, on: boolean) {
  off = new Set(off)
  if (on) off.delete(kind)
  else off.add(kind)
  try {
    localStorage.setItem(SETTING, JSON.stringify({ off: [...off] }))
  } catch {
    // Private windows can refuse storage; the setting then lasts until reload.
  }
}

/** Plays notes as [frequency in Hz, start in seconds, length in seconds]. */
function play(kind: SoundKind, notes: [number, number, number][], volume = 0.12, wave: OscillatorType = 'sine') {
  if (!soundOn(kind)) return
  const ctx = audio()
  if (!ctx || ctx.state !== 'running') return
  const now = ctx.currentTime
  for (const [frequency, start, length] of notes) {
    const osc = ctx.createOscillator()
    const gain = ctx.createGain()
    osc.type = wave
    osc.frequency.value = frequency
    // Short fades in and out stop the clicks a hard start or stop makes.
    gain.gain.setValueAtTime(0, now + start)
    gain.gain.linearRampToValueAtTime(volume, now + start + 0.02)
    gain.gain.setValueAtTime(volume, now + start + length - 0.04)
    gain.gain.linearRampToValueAtTime(0, now + start + length)
    osc.connect(gain).connect(ctx.destination)
    osc.start(now + start)
    osc.stop(now + start + length + 0.02)
  }
}

/** Someone joined the call you are in (or you joined). A rising pair of notes. */
export const playJoin = () =>
  play('join', [
    [523.25, 0, 0.12],
    [783.99, 0.1, 0.18],
  ])

/** Someone else left the call you are in, or it dropped. A falling pair of notes. */
export const playLeave = () =>
  play('leave', [
    [783.99, 0, 0.12],
    [523.25, 0.1, 0.18],
  ])

/** You hung up. Three quick falling notes, lower than the leave sound. */
export const playHangup = () =>
  play(
    'hangup',
    [
      [587.33, 0, 0.1],
      [440, 0.09, 0.1],
      [293.66, 0.18, 0.22],
    ],
    0.13,
    'triangle',
  )

let lastMessage = 0

/** A message arrived. A soft two-tone blip, played at most once a second. */
export function playMessage() {
  const now = Date.now()
  if (!soundOn('message') || now - lastMessage < 1000) return
  lastMessage = now
  play(
    'message',
    [
      [880, 0, 0.08],
      [1318.51, 0.07, 0.14],
    ],
    0.08,
  )
}

/** Plays one sound by name, for the preview in Settings. */
export function preview(kind: SoundKind) {
  if (kind === 'ring') {
    const stop = startRinging()
    setTimeout(stop, 100)
  } else ({ join: playJoin, leave: playLeave, hangup: playHangup, message: playMessage })[kind]()
}

/**
 * Starts a ringtone that repeats until the returned function is called.
 * Each ring is a pair of two-note chimes, then a pause.
 */
export function startRinging(): () => void {
  const ring = () =>
    play(
      'ring',
      [
        [659.25, 0, 0.16],
        [880, 0.16, 0.22],
        [659.25, 0.5, 0.16],
        [880, 0.66, 0.22],
      ],
      0.14,
    )
  ring()
  const timer = setInterval(ring, 2500)
  return () => clearInterval(timer)
}
