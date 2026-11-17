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
  const candidates = $derived(engine.users.filter((u) => !chat.members.some((m) => m.user_id === u.id)))

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
      await engine.removeMember(chat.id, engine.me.id)
      onClose()
    })
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog card">
    <h2>{chat.name}: members</h2>
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
              {#if isCreator}
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

    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button class="secondary danger" onclick={leave}>Leave group</button>
      <button class="secondary" onclick={onClose}>Close</button>
    </div>
  </div>
</div>
