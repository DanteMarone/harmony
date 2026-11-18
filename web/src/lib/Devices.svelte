<script lang="ts">
  import { api, type OwnDevice } from './api'
  import type { Engine } from './engine.svelte'

  let { engine, onClose }: { engine: Engine; onClose: () => void } = $props()

  let devices = $state<OwnDevice[]>([])
  let loaded = $state(false)
  let error = $state('')
  let busy = $state(false)
  /** The device whose sign-out is waiting for a second click. */
  let confirming = $state<number | null>(null)
  let renaming = $state<number | null>(null)
  let newName = $state('')

  async function load() {
    try {
      devices = await api.listDevices()
      loaded = true
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not load your devices.'
    }
  }
  void load()

  async function run(action: () => Promise<void>) {
    busy = true
    error = ''
    try {
      await action()
    } catch (err) {
      error = err instanceof Error ? err.message : 'That did not work.'
    } finally {
      busy = false
    }
  }

  function startRename(device: OwnDevice) {
    renaming = device.id
    newName = device.name ?? ''
  }

  function rename(event: SubmitEvent) {
    event.preventDefault()
    const id = renaming
    if (id === null) return
    void run(async () => {
      await api.renameDevice(id, newName)
      renaming = null
      await load()
    })
  }

  const signOut = (id: number) =>
    run(async () => {
      confirming = null
      if (id === engine.thisDeviceId) {
        await engine.removeThisDevice()
        return
      }
      await engine.revokeDevice(id)
      await load()
    })

  function when(seconds: number | null): string {
    if (seconds === null) return 'never'
    const minutes = Math.round((Date.now() / 1000 - seconds) / 60)
    if (minutes < 2) return 'just now'
    if (minutes < 60) return `${minutes} minutes ago`
    const date = new Date(seconds * 1000)
    if (minutes < 24 * 60) return `today at ${date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`
    return date.toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' })
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="devices-title">
    <h2 id="devices-title">Your devices</h2>
    <p class="hint">
      Every browser or app you are signed in to. If you don't recognise one, sign it out and change your password.
    </p>

    {#if !loaded && !error}
      <p class="hint">Loading…</p>
    {/if}
    <ul class="rows devices">
      {#each devices as device (device.id)}
        {@const current = device.id === engine.thisDeviceId}
        {@const confirmed = engine.deviceConfirmed(device.id)}
        <li>
          {#if renaming === device.id}
            <form class="rename" onsubmit={rename}>
              <input bind:value={newName} maxlength="48" aria-label="Device name" required />
              <button type="submit" disabled={busy || !newName.trim()}>Save</button>
              <button type="button" class="secondary" onclick={() => (renaming = null)}>Cancel</button>
            </form>
          {:else}
            <div class="device">
              <strong>{device.name ?? 'Unnamed device'}</strong>
              {#if current}<span class="tag">This device</span>{/if}
              <span class="trust {confirmed ? 'verified' : 'unconfirmed'}">
                {confirmed ? 'Confirmed' : 'Not confirmed'}
              </span>
              <span class="hint">
                Signed in {when(device.created_at)}{current ? '' : `, last active ${when(device.last_seen_at)}`}
              </span>
            </div>
            <span class="row-actions">
              <button class="secondary" disabled={busy} onclick={() => startRename(device)}>Rename</button>
              {#if confirming === device.id}
                <button class="danger-solid" disabled={busy} onclick={() => signOut(device.id)}>
                  {current ? 'Sign out and erase' : 'Sign out'}
                </button>
                <button class="secondary" disabled={busy} onclick={() => (confirming = null)}>Keep</button>
              {:else}
                <button class="secondary danger" disabled={busy} onclick={() => (confirming = device.id)}>Sign out</button>
              {/if}
            </span>
          {/if}
          {#if confirming === device.id}
            <p class="warning confirm">
              {#if current}
                This erases this device's keys and messages and signs it out everywhere. Your history stays in your
                encrypted backup.
              {:else}
                That device is signed out straight away, its keys are removed from every chat, and it can't read new
                messages.
              {/if}
            </p>
          {/if}
        </li>
      {/each}
    </ul>

    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions"><button class="secondary" onclick={onClose}>Close</button></div>
  </div>
</div>
