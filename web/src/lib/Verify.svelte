<script lang="ts">
  import type { Engine } from './engine.svelte'

  let {
    engine,
    userId,
    chatId,
    onClose,
    onRecovery,
  }: { engine: Engine; userId: number; chatId?: string; onClose: () => void; onRecovery: () => void } = $props()

  const name = $derived(engine.userName(userId))
  const status = $derived(engine.trustStatus(userId, chatId))
  const number = $derived(engine.safetyNumber(userId))
  const isMe = $derived(userId === engine.me.id)
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="verify-title">
    {#if isMe}
      <h2 id="verify-title">One of your devices is unconfirmed</h2>
      <p class="warning">
        Another device signed in to your account is in this chat but has not been confirmed with your recovery code.
        If it is yours, enter the code on it. If you do not recognise it, change your password.
      </p>
    {:else}
      <h2 id="verify-title">Verify {name}</h2>
      {#if status === 'changed'}
        <p class="warning" role="alert">
          {name}'s security identity has changed since this device last saw it. That happens when someone replaces
          their recovery code, but it could also mean someone is pretending to be them. Compare the safety number
          below with {name} before trusting it.
        </p>
      {:else if status === 'unconfirmed'}
        <p class="warning" role="alert">
          {name} has a device in this chat that they have not confirmed with their recovery code. It may be a new
          device they have not finished setting up, or it may not be theirs. Ask them before sharing anything
          sensitive.
        </p>
      {:else if status === 'none'}
        <p>{name} has not set up a recovery code yet, so there is nothing to compare. This fixes itself the next time they open Harmony.</p>
      {:else if status === 'verified'}
        <p class="ok">You have verified {name}. Harmony will warn you if their identity ever changes.</p>
      {/if}

      {#if number}
        <p>
          Compare this number with the one {name} sees for you, in person or on a call you trust. If they match,
          nobody is sitting between you.
        </p>
        <output class="safety-number" aria-label="Safety number">
          {#each number.split(' ') as group, i (i)}<span>{group}</span>{/each}
        </output>
      {:else if status !== 'none'}
        <p>
          To compare safety numbers, first confirm this device with your recovery code.
          <button class="link" onclick={onRecovery}>Enter recovery code</button>
        </p>
      {/if}
    {/if}

    <div class="actions">
      {#if !isMe && status === 'changed'}
        <button class="secondary" onclick={() => engine.acceptIdentity(userId)}>Accept without verifying</button>
      {/if}
      {#if !isMe && number && status !== 'verified'}
        <button onclick={() => engine.setVerified(userId, true)}>The numbers match</button>
      {:else if !isMe && status === 'verified'}
        <button class="secondary" onclick={() => engine.setVerified(userId, false)}>Remove verification</button>
      {/if}
      <button class="secondary" onclick={onClose}>Close</button>
    </div>
  </div>
</div>
