<script lang="ts">
  import type { Engine } from './engine.svelte'

  // `onClose` is absent when the dialog must be dealt with before using Harmony.
  let { engine, onClose }: { engine: Engine; onClose?: () => void } = $props()

  /** Set while replacing a lost code. */
  let replacing = $state(false)
  let newCode = $state('')
  let typed = $state('')
  let saved = $state(false)
  let copied = $state(false)
  let busy = $state(false)
  let error = $state('')

  const creating = $derived(engine.recovery === 'setup' || replacing)

  // A code is made as soon as one is needed, and kept while the dialog is open.
  $effect(() => {
    if (creating && !newCode) newCode = engine.newRecoveryCode()
  })

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

  async function copy() {
    await navigator.clipboard.writeText(newCode).catch(() => {})
    copied = true
  }

  const useNew = () =>
    run(async () => {
      await engine.useNewRecoveryCode(newCode)
      replacing = false
      newCode = ''
      saved = false
      onClose?.()
    })

  function confirmDevice(event: SubmitEvent) {
    event.preventDefault()
    void run(async () => {
      await engine.enterRecoveryCode(typed)
      typed = ''
      onClose?.()
    })
  }

  const skip = () =>
    run(async () => {
      await engine.skipRecoveryCode()
      onClose?.()
    })
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose?.()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="recovery-title">
    {#if creating}
      <h2 id="recovery-title">{replacing ? 'Your new recovery code' : 'Save your recovery code'}</h2>
      {#if replacing}
        <p class="warning">
          Replacing your code gives you a new security identity. Your friends will be asked to verify you again, your
          other devices will need the new code, and your history backup starts over from what this device has.
        </p>
      {:else}
        <p>
          This code proves a device is yours and unlocks your message history on a new one. Harmony cannot show it
          again or reset it, because the server never sees it.
        </p>
      {/if}
      <output class="recovery-code" aria-label="Recovery code">{newCode}</output>
      <button class="secondary" onclick={copy}>{copied ? 'Copied' : 'Copy code'}</button>
      <p class="hint">Keep it in a password manager or on paper. Anyone who has it can read your backed-up messages.</p>
      <label class="check">
        <input type="checkbox" bind:checked={saved} />
        <span>I have saved this code somewhere safe</span>
      </label>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="actions">
        {#if replacing}
          <button class="secondary" disabled={busy} onclick={() => ((replacing = false), (newCode = ''), (saved = false))}>
            Back
          </button>
        {/if}
        <button disabled={!saved || busy} onclick={useNew}>{replacing ? 'Replace my code' : 'Continue'}</button>
      </div>
    {:else if engine.recovery === 'ready'}
      <h2 id="recovery-title">Security</h2>
      <p>This device is confirmed with your recovery code, and your message history is being backed up, encrypted.</p>
      <p class="hint">
        Lost the code? You can replace it. Friends will need to verify you again and your other devices will need the
        new code.
      </p>
      <div class="actions">
        <button class="secondary danger" onclick={() => (replacing = true)}>Replace recovery code</button>
        <button class="secondary" onclick={onClose}>Close</button>
      </div>
    {:else}
      <h2 id="recovery-title">Confirm this device</h2>
      <p>
        Enter the recovery code you saved when you first set up Harmony. It tells your friends this device is really
        yours and brings back your message history.
      </p>
      <form onsubmit={confirmDevice}>
        <label>
          Recovery code
          <input
            class="code-input"
            bind:value={typed}
            autocomplete="off"
            autocapitalize="characters"
            spellcheck="false"
            placeholder="XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX"
            required
          />
        </label>
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        {#if engine.restoring}<p class="hint" role="status">Bringing back your message history…</p>{/if}
        <button type="submit" disabled={busy || !typed.trim()}>{busy ? 'Checking…' : 'Confirm device'}</button>
      </form>
      <div class="actions">
        <button class="link" disabled={busy} onclick={() => (replacing = true)}>I lost my code</button>
        <span class="spacer"></span>
        <button class="secondary" disabled={busy} onclick={skip}>Skip for now</button>
      </div>
      <p class="hint">
        If you skip, this device starts with no history and your friends see a warning that it is unconfirmed.
      </p>
    {/if}
  </div>
</div>
