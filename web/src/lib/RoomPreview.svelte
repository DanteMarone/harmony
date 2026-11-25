<script lang="ts">
  import type { RoomInfo } from './api'
  import Avatar from './Avatar.svelte'
  import type { Engine } from './engine.svelte'
  import Icon from './Icon.svelte'

  // A room this person is not in yet: who is there, and a way in.
  let {
    engine,
    room,
    busy,
    onJoin,
    onJoinVoice,
    onBack,
  }: {
    engine: Engine
    room: RoomInfo
    busy: boolean
    onJoin: () => void
    onJoinVoice: () => void
    onBack: () => void
  } = $props()
</script>

<section class="chat">
  <header class="chat-head">
    <button class="icon-button back" aria-label="Back to chats" title="Back to chats" onclick={onBack}>
      <Icon name="back" />
    </button>
    <Avatar name={room.name} id={room.id} group size={36} />
    <div class="chat-title">
      <h2>{room.name}</h2>
      <p class="hint secure"><Icon name="lock" size={13} /> Room · end-to-end encrypted</p>
    </div>
  </header>
  <div class="messages">
    <div class="notice room-preview">
      <p>
        {#if room.member_ids.length}
          In this room: {room.member_ids.map((id) => engine.userName(id)).join(', ')}.
        {:else}
          Nobody is in this room right now.
        {/if}
      </p>
      {#if room.voice.length}
        <p class="in-voice">
          <Icon name="volume" size={16} /> Talking now: {room.voice.map((id) => engine.userName(id)).join(', ')}
        </p>
      {/if}
      <p class="hint">Messages sent before you join can't be read, since they were encrypted before you were in the room.</p>
      <div class="actions">
        <button class="secondary" disabled={busy} onclick={onJoin}>Join room</button>
        {#if engine.callsEnabled}
          <button disabled={busy} onclick={onJoinVoice}><Icon name="volume" size={18} /> Join voice</button>
        {/if}
      </div>
    </div>
  </div>
</section>
