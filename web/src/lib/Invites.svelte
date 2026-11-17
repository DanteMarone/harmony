<script lang="ts">
  import { api, type InviteInfo } from './api'

  let invites = $state<InviteInfo[]>([])
  let newLink = $state('')
  let copied = $state(false)
  let error = $state('')

  async function refresh() {
    try {
      invites = await api.listInvites()
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not load invites.'
    }
  }

  async function create() {
    error = ''
    copied = false
    try {
      const invite = await api.createInvite()
      // The code sits after the # so it is never sent to the server in a URL
      // and never shows up in access logs.
      newLink = `${location.origin}/#/invite/${invite.code}`
      await refresh()
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not create an invite.'
    }
  }

  async function revoke(id: number) {
    await api.revokeInvite(id).catch(() => {})
    newLink = ''
    await refresh()
  }

  async function copy() {
    await navigator.clipboard.writeText(newLink)
    copied = true
  }

  const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString()
  const status = (invite: InviteInfo) =>
    invite.used_at
      ? `used by ${invite.used_by ?? 'a deleted account'}`
      : invite.expires_at * 1000 < Date.now()
        ? 'expired'
        : `unused, expires ${day(invite.expires_at)}`

  refresh()
</script>

<section class="panel">
  <h2>Invites</h2>
  <p class="hint">Each link works once and expires after 7 days.</p>
  <button onclick={create}>Create invite link</button>

  {#if newLink}
    <div class="new-link">
      <p>Send this link to your friend. It is shown only once.</p>
      <code>{newLink}</code>
      <button class="secondary" onclick={copy}>{copied ? 'Copied' : 'Copy link'}</button>
    </div>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if invites.length}
    <ul class="rows">
      {#each invites as invite (invite.id)}
        <li>
          <span>Created {day(invite.created_at)}: {status(invite)}</span>
          {#if !invite.used_at}
            <button class="secondary" onclick={() => revoke(invite.id)}>Revoke</button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>
