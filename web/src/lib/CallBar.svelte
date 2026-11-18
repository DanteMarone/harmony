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
  {#if call.micProblem}
    <p class="call-note" role="status">{call.micProblem}</p>
  {/if}
  {#if call.audioBlocked}
    <button class="secondary" onclick={() => call.startAudio()}>Click to hear the call</button>
  {/if}
  {#if call.microphones.length > 1 || (call.micProblem && call.microphones.length > 0)}
    <label class="call-mic">
      <span class="hint">Microphone</span>
      <select value={call.microphone} onchange={(e) => call.chooseMicrophone(e.currentTarget.value)}>
        {#if !call.microphone}<option value="" disabled>Choose a microphone</option>{/if}
        {#each call.microphones as mic (mic.id)}
          <option value={mic.id}>{mic.label}</option>
        {/each}
      </select>
    </label>
  {/if}
  {#if call.decryptProblems > 0}
    <p class="hint">Some audio could not be decrypted. It usually clears in a moment after someone joins or leaves the chat.</p>
  {/if}
  <div class="call-actions">
    <button class="secondary" aria-pressed={call.micProblem ? undefined : call.muted} onclick={() => call.toggleMute()}>
      {call.micProblem ? 'Try microphone again' : call.muted ? 'Unmute' : 'Mute'}
    </button>
    <button class="danger-solid" onclick={() => call.end()}>Leave call</button>
  </div>
</section>
