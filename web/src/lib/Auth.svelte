<script lang="ts">
  import { api, type User } from './api'

  let { invite, onSignedIn }: { invite: string | null; onSignedIn: (user: User) => void } = $props()

  let username = $state('')
  let displayName = $state('')
  let password = $state('')
  let confirm = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    error = ''
    if (invite && password !== confirm) {
      error = 'The two passwords do not match.'
      return
    }
    busy = true
    try {
      const user = invite
        ? await api.register(invite, username, password, displayName)
        : await api.login(username, password)
      onSignedIn(user)
    } catch (err) {
      error = err instanceof Error ? err.message : 'Something went wrong.'
    } finally {
      busy = false
    }
  }
</script>

<main class="card">
  <h1>Harmony</h1>
  {#if invite}
    <p class="lead">You've been invited. Pick a username and password to join.</p>
  {:else}
    <p class="lead">Sign in to continue. New here? Ask for an invite link.</p>
  {/if}

  <form onsubmit={submit}>
    <label>
      Username
      <input bind:value={username} autocomplete="username" required minlength="3" maxlength="24" />
    </label>
    {#if invite}
      <label>
        Display name <span class="hint">(optional)</span>
        <input bind:value={displayName} maxlength="48" />
      </label>
    {/if}
    <label>
      Password
      <input
        type="password"
        bind:value={password}
        autocomplete={invite ? 'new-password' : 'current-password'}
        required
        minlength={invite ? 10 : 1}
      />
    </label>
    {#if invite}
      <label>
        Confirm password
        <input type="password" bind:value={confirm} autocomplete="new-password" required />
      </label>
      <p class="hint">At least 10 characters. No email or phone number needed, so there is no password reset: keep it somewhere safe.</p>
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <button type="submit" disabled={busy}>{invite ? 'Create account' : 'Sign in'}</button>
  </form>
</main>
