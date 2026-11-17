<script lang="ts">
  import { api, type User } from './lib/api'
  import Auth from './lib/Auth.svelte'
  import Main from './lib/Main.svelte'

  let user = $state<User | null>(null)
  let loading = $state(true)
  let loadError = $state('')
  let invite = $state(inviteFromHash())

  function inviteFromHash(): string | null {
    const match = location.hash.match(/^#\/invite\/([0-9a-f]{64})$/)
    return match ? match[1] : null
  }

  function clearInvite() {
    invite = null
    history.replaceState(null, '', location.pathname)
  }

  function signedIn(next: User) {
    user = next
    clearInvite()
  }

  async function signOut() {
    await api.logout().catch(() => {})
    user = null
  }

  window.addEventListener('hashchange', () => (invite = inviteFromHash()))

  api
    .me()
    .then((me) => {
      user = me
      if (me) clearInvite()
    })
    .catch(() => (loadError = 'Cannot reach the Harmony server.'))
    .finally(() => (loading = false))
</script>

{#if loading}
  <main class="card"><p class="lead">Loading…</p></main>
{:else if loadError}
  <main class="card"><h1>Harmony</h1><p class="error" role="alert">{loadError}</p></main>
{:else if !user}
  <Auth {invite} onSignedIn={signedIn} />
{:else}
  {#key user.id}<Main {user} onSignOut={signOut} />{/key}
{/if}
