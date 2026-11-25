<script lang="ts">
  import type { ChatInfo } from './api'
  import type { Engine } from './engine.svelte'

  let {
    engine,
    chat,
    onClose,
    onVerify,
  }: { engine: Engine; chat: ChatInfo; onClose: () => void; onVerify: (userId: number) => void } = $props()

  const statusText = {
    verified: 'Verified',
    unverified: 'Not verified',
    changed: 'Identity changed',
    unconfirmed: 'Unconfirmed device',
    none: 'No recovery code yet',
  }

  let toAdd = $state<number[]>([])
  let error = $state('')

  const isCreator = $derived(chat.created_by === engine.me.id)
  const room = $derived(chat.kind === 'room')
  /** Whoever made a room, or an admin, can rename or delete it. */
  const ownsRoom = $derived(room && (isCreator || engine.me.is_admin))
  // Anyone can join a room on their own, so nobody is added to one.
  const candidates = $derived(room ? [] : engine.users.filter((u) => !chat.members.some((m) => m.user_id === u.id)))
  let newName = $state('')
  let confirmDelete = $state(false)

  async function run(action: () => Promise<void>) {
    error = ''
    try {
      await action()
    } catch (err) {
      error = err instanceof Error ? err.message : 'That did not work.'
    }
  }

  const add = () =>
    run(async () => {
      await engine.addMembers(chat.id, toAdd)
      toAdd = []
    })
  const remove = (userId: number) => run(() => engine.removeMember(chat.id, userId))
  const leave = () =>
    run(async () => {
      if (room) await engine.leaveRoom(chat.id)
      else await engine.removeMember(chat.id, engine.me.id)
      onClose()
    })
  const rename = (event: SubmitEvent) => {
    event.preventDefault()
    return run(async () => {
      await engine.renameRoom(chat.id, newName.trim())
      newName = ''
    })
  }
  const deleteRoom = () =>
    run(async () => {
      await engine.deleteRoom(chat.id)
      onClose()
    })
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog card">
    <h2>{chat.name}: {room ? 'people in the room' : 'members'}</h2>
    <ul class="rows">
      {#each chat.members as member (member.user_id)}
        <li>
          <span>
            {member.display_name}
            {#if member.user_id === chat.created_by}<span class="hint">(creator)</span>{/if}
          </span>
          {#if member.user_id !== engine.me.id}
            {@const status = engine.trustStatus(member.user_id, chat.id)}
            <span class="row-actions">
              <span class="trust {status}">{statusText[status]}</span>
              <button class="secondary" onclick={() => onVerify(member.user_id)}>Verify</button>
              {#if isCreator && !room}
                <button class="secondary" onclick={() => remove(member.user_id)}>Remove</button>
              {/if}
            </span>
          {/if}
        </li>
      {/each}
    </ul>

    {#if candidates.length}
      <h3>Add people</h3>
      <ul class="people">
        {#each candidates as user (user.id)}
          <li>
            <label class="check">
              <input type="checkbox" bind:group={toAdd} value={user.id} />
              <span>{user.display_name} <span class="hint">@{user.username}</span></span>
            </label>
          </li>
        {/each}
      </ul>
      <p class="hint">New members can only read messages sent after they join.</p>
      <button disabled={toAdd.length === 0} onclick={add}>Add to group</button>
    {/if}

    {#if ownsRoom}
      <h3>Room settings</h3>
      <form class="inline-form" onsubmit={rename}>
        <input bind:value={newName} maxlength="64" placeholder={chat.name ?? 'Room name'} aria-label="New room name" />
        <button type="submit" class="secondary" disabled={!newName.trim()}>Rename</button>
      </form>
      {#if confirmDelete}
        <p class="warning">This deletes the room and everything said in it, for everyone. It can't be undone.</p>
        <div class="actions">
          <button class="secondary" onclick={() => (confirmDelete = false)}>Keep the room</button>
          <button class="danger-solid" onclick={deleteRoom}>Delete for everyone</button>
        </div>
      {:else}
        <button class="secondary danger" onclick={() => (confirmDelete = true)}>Delete room</button>
      {/if}
    {/if}

    {#if room}
      <p class="hint">Anyone on this server can join this room. People who leave can come back, but can't read what was said while they were away.</p>
    {/if}

    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button class="secondary danger" onclick={leave}>{room ? 'Leave room' : 'Leave group'}</button>
      <button class="secondary" onclick={onClose}>Close</button>
    </div>
  </div>
</div>
