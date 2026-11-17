<script lang="ts">
  import type { Engine } from './engine.svelte'

  let { engine, onDone }: { engine: Engine; onDone: (chatId: string | null) => void } = $props()

  let picked = $state<number[]>([])
  let name = $state('')
  let error = $state('')
  let busy = $state(false)

  const others = $derived(engine.users.filter((u) => u.id !== engine.me.id))
  const isGroup = $derived(picked.length > 1)

  async function create(event: SubmitEvent) {
    event.preventDefault()
    error = ''
    busy = true
    try {
      onDone(await engine.createChat(isGroup ? 'group' : 'dm', picked, isGroup ? name : undefined))
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not start the chat.'
    } finally {
      busy = false
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onDone(null)}>
  <form class="dialog card" onsubmit={create}>
    <h2>New chat</h2>
    {#if others.length === 0}
      <p class="lead">Nobody else has joined yet. Invite a friend first.</p>
    {:else}
      <p class="hint">Pick one person for a direct message, or several for a group.</p>
      <ul class="people">
        {#each others as user (user.id)}
          <li>
            <label class="check">
              <input type="checkbox" bind:group={picked} value={user.id} />
              <span>{user.display_name} <span class="hint">@{user.username}</span></span>
            </label>
          </li>
        {/each}
      </ul>
      {#if isGroup}
        <label>
          Group name
          <input bind:value={name} required maxlength="64" />
        </label>
        <p class="hint">Group names and member lists are visible to the server. Messages are not.</p>
      {/if}
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button type="button" class="secondary" onclick={() => onDone(null)}>Cancel</button>
      <button type="submit" disabled={busy || picked.length === 0}>{isGroup ? 'Create group' : 'Open chat'}</button>
    </div>
  </form>
</div>
