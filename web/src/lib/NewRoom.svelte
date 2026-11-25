<script lang="ts">
  import type { Engine } from './engine.svelte'

  let { engine, onDone }: { engine: Engine; onDone: (roomId: string | null) => void } = $props()

  let name = $state('')
  let error = $state('')
  let busy = $state(false)

  async function create(event: SubmitEvent) {
    event.preventDefault()
    error = ''
    busy = true
    try {
      onDone(await engine.createRoom(name.trim()))
    } catch (err) {
      error = err instanceof Error ? err.message : 'Could not make the room.'
    } finally {
      busy = false
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onDone(null)}>
  <form class="dialog card" onsubmit={create}>
    <h2>New room</h2>
    <p class="hint">
      A room is a chat with its own voice channel. Everyone on this Harmony server can see it and join, and
      people can drop in and out of its voice channel whenever they like.
    </p>
    <label>
      Room name
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={name} required maxlength="64" placeholder="Raid night" autofocus />
    </label>
    <p class="hint">
      Room names and who is in each room are visible to the server. Messages and voice are end-to-end encrypted.
    </p>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button type="button" class="secondary" onclick={() => onDone(null)}>Cancel</button>
      <button type="submit" disabled={busy || !name.trim()}>Create room</button>
    </div>
  </form>
</div>
