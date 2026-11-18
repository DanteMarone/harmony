<script lang="ts">
  import type { Call } from './call.svelte'
  import type { Engine } from './engine.svelte'

  let { engine, call, title }: { engine: Engine; call: Call; title: string } = $props()

  const statusText = $derived(
    call.status === 'connecting' ? 'Connecting…' : call.status === 'reconnecting' ? 'Reconnecting…' : 'Encrypted call',
  )
</script>

<section class="callbar" aria-label="Call in {title}">
  <div class="call-head">
    <strong>{title}</strong>
    <span class="hint" role="status">{statusText}</span>
  </div>
  <ul class="call-members">
    {#each call.members as member (member.identity)}
      <li class:speaking={member.speaking && !member.muted} class:outsider={member.outsider}>
        <span>{member.identity === engine.deviceIdentity ? 'You' : engine.userName(member.userId)}</span>
        {#if member.muted}<span class="hint">(muted)</span>{/if}
        {#if member.outsider}<span class="hint">(not in this chat's encryption, can't hear)</span>{/if}
      </li>
    {/each}
  </ul>
  {#if call.decryptProblems > 0}
    <p class="hint">Some audio could not be decrypted. It usually clears in a moment after someone joins or leaves the chat.</p>
  {/if}
  <div class="call-actions">
    <button class="secondary" aria-pressed={call.muted} onclick={() => call.toggleMute()}>
      {call.muted ? 'Unmute' : 'Mute'}
    </button>
    <button class="danger-solid" onclick={() => call.end()}>Leave call</button>
  </div>
</section>
