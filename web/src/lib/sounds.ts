// Short sound cues for calls, made on the fly with the Web Audio API so no
// audio files are shipped or fetched.
//
// Browsers only let a page make sound after the person has interacted with it,
// so the audio context is created (or resumed) on the first click or key press.

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

export function soundsOn(): boolean {
  try {
    return localStorage.getItem(SETTING) !== 'off'
  } catch {
    return true
  }
}

export function setSoundsOn(on: boolean) {
  try {
    localStorage.setItem(SETTING, on ? 'on' : 'off')
  } catch {
    // Private windows can refuse storage; the setting then lasts until reload.
  }
}

/** Plays notes as [frequency in Hz, start in seconds, length in seconds]. */
function play(notes: [number, number, number][], volume = 0.12) {
  if (!soundsOn()) return
  const ctx = audio()
  if (!ctx || ctx.state !== 'running') return
  const now = ctx.currentTime
  for (const [frequency, start, length] of notes) {
    const osc = ctx.createOscillator()
    const gain = ctx.createGain()
    osc.type = 'sine'
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

/** Someone joined the call you are in (or you joined). */
export const playJoin = () =>
  play([
    [523.25, 0, 0.12],
    [783.99, 0.1, 0.18],
  ])

/** Someone left the call you are in (or you left). */
export const playLeave = () =>
  play([
    [783.99, 0, 0.12],
    [523.25, 0.1, 0.18],
  ])

/**
 * Starts a ringtone that repeats until the returned function is called.
 * Each ring is a pair of two-note chimes, then a pause.
 */
export function startRinging(): () => void {
  const ring = () =>
    play(
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
