<script lang="ts">
  import { tick } from 'svelte'
  import type { ChatInfo } from './api'
  import type { Engine } from './engine.svelte'
  import type { StoredMessage } from './store'
  import Avatar from './Avatar.svelte'
  import Icon from './Icon.svelte'

  let {
    engine,
    chat,
    inCall,
    onCall,
    onMembers,
    onVerify,
    onBack,
  }: {
    engine: Engine
    chat: ChatInfo
    inCall: boolean
    onCall: () => void
    onMembers: () => void
    onVerify: (userId: number) => void
    /** Back to the chat list, on narrow screens. */
    onBack: () => void
  } = $props()

  const inProgress = $derived(engine.calls[chat.id] ?? [])
  const room = $derived(chat.kind === 'room')

  const warnings = $derived(engine.chatWarnings(chat))
  const other = $derived(chat.kind === 'dm' ? chat.members.find((m) => m.user_id !== engine.me.id) : undefined)
  const otherVerified = $derived(other ? engine.trustStatus(other.user_id, chat.id) === 'verified' : false)

  let draft = $state('')
  let scroller: HTMLElement | undefined = $state()

  const messages = $derived(engine.messages[chat.id] ?? [])
  const broken = $derived(engine.meta[chat.id]?.broken ?? false)
  const waiting = $derived(!chat.joined && !broken)

  // Keep the newest message in view when messages arrive or the chat changes.
  $effect(() => {
    void messages.length
    void chat.id
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }))
  })

  function send(event: SubmitEvent) {
    event.preventDefault()
    const body = draft.trim()
    if (!body) return
    draft = ''
    void engine.send(chat.id, body)
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault()
      ;(event.currentTarget as HTMLTextAreaElement).form?.requestSubmit()
    }
  }

  const time = (m: StoredMessage) =>
    new Date(m.sentAt * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  const day = (m: StoredMessage) =>
    new Date(m.sentAt * 1000).toLocaleDateString([], { weekday: 'long', month: 'long', day: 'numeric' })
  const newDay = (i: number) => i === 0 || day(messages[i]) !== day(messages[i - 1])
  const newSender = (i: number) =>
    newDay(i) || messages[i].senderUser !== messages[i - 1].senderUser || messages[i].sentAt - messages[i - 1].sentAt > 300
</script>

<section class="chat">
  <header class="chat-head">
    <button class="icon-button back" aria-label="Back to chats" title="Back to chats" onclick={onBack}>
      <Icon name="back" />
    </button>
    <Avatar
      name={engine.chatTitle(chat)}
      id={chat.kind === 'dm' ? (other?.user_id ?? engine.me.id) : chat.id}
      group={chat.kind !== 'dm'}
      size={36}
    />
    <div class="chat-title">
      <h2>{engine.chatTitle(chat)}</h2>
      <p class="hint secure" class:ok={otherVerified}>
        <Icon name="lock" size={13} />
        {chat.kind === 'room'
          ? `Room · ${chat.members.length} in it · end-to-end encrypted`
          : chat.kind === 'group'
            ? `${chat.members.length} members · end-to-end encrypted`
            : 'End-to-end encrypted'}{otherVerified
          ? ' · verified'
          : ''}
      </p>
    </div>
    {#if engine.callsEnabled && chat.joined && !broken}
      {#if inCall}
        <span class="pill live"><Icon name={room ? 'volume' : 'phone'} size={14} /> {room ? 'In voice' : 'In call'}</span>
      {:else if room}
        <button
          class={inProgress.length ? 'join-voice live' : 'join-voice secondary'}
          title="Join this room's voice channel"
          onclick={onCall}
        >
          <Icon name="volume" size={18} /> <span class="label">Join voice</span>{inProgress.length ? ` · ${inProgress.length}` : ''}
        </button>
      {:else if inProgress.length}
        <button class="join-call" onclick={onCall}>
          <Icon name="phone" size={18} /> <span class="label">Join call ·</span> {inProgress.length}
        </button>
      {:else}
        <button class="icon-button" title="Start a voice call" aria-label="Start a voice call" onclick={onCall}>
          <Icon name="phone" />
        </button>
      {/if}
    {/if}
    {#if chat.kind !== 'dm'}
      <button class="icon-button" title={room ? 'Room' : 'Members'} aria-label={room ? 'Room' : 'Members'} onclick={onMembers}
        ><Icon name="users" /></button
      >
    {:else if other}
      <button
        class="icon-button"
        class:ok={otherVerified}
        title={otherVerified ? 'Verified. Show safety number' : `Verify ${other.display_name}`}
        aria-label="Verify"
        onclick={() => onVerify(other.user_id)}><Icon name="shield" /></button
      >
    {/if}
  </header>

  {#each warnings as warning (warning.userId)}
    <div class="warning strip" role="alert">
      <span>
        {#if warning.userId === engine.me.id}
          One of your own devices in this chat has not been confirmed with your recovery code.
        {:else if warning.status === 'changed'}
          {engine.userName(warning.userId)}'s security identity has changed.
        {:else}
          {engine.userName(warning.userId)} has a device in this chat that they have not confirmed.
        {/if}
      </span>
      <button class="secondary" onclick={() => onVerify(warning.userId)}>Review</button>
    </div>
  {/each}

  <div class="messages" bind:this={scroller}>
    {#if broken}
      <div class="notice">
        <p>This device can't read this chat right now. Another member's device needs to let it back in.</p>
        <button onclick={() => engine.repair(chat.id)}>Ask to be let back in</button>
      </div>
    {:else if waiting}
      <p class="notice">
        {#if room}
          Waiting for someone already in this room to share its keys with this device. That happens
          automatically as soon as one of their devices is online.
        {:else}
          Waiting for another member's device to share this chat's keys with this device. That happens
          automatically the next time one of them is online.
        {/if}
      </p>
    {:else if messages.length === 0}
      <p class="notice">
        No messages yet.
        {#if engine.recovery === 'ready'}
          Messages sent before you joined this chat can't be shown.
        {:else}
          Messages sent before this device joined appear once you confirm it with your recovery code.
        {/if}
      </p>
    {/if}

    {#each messages as message, i (message.key)}
      {#if newDay(i)}<div class="day"><span>{day(message)}</span></div>{/if}
      <article class="message" class:first={newSender(i)} class:pending={message.seq === null && !message.problem}>
        {#if newSender(i)}
          <span class="gutter"><Avatar name={engine.userName(message.senderUser)} id={message.senderUser} size={38} /></span>
          <header>
            <strong>{engine.userName(message.senderUser)}</strong>
            <time>{time(message)}</time>
          </header>
        {:else}
          <time class="gutter side-time">{time(message)}</time>
        {/if}
        {#if message.problem === 'undecryptable'}
          <p class="hint">This message could not be decrypted on this device.</p>
        {:else}
          <p class="body">{message.body}</p>
        {/if}
        {#if message.problem === 'failed'}
          <p class="error">
            Not sent.
            <button class="link" onclick={() => engine.send(chat.id, message.body, message.key)}>Try again</button>
          </p>
        {/if}
      </article>
    {/each}
  </div>

  <form class="composer" onsubmit={send}>
    <textarea
      bind:value={draft}
      onkeydown={onKey}
      rows="1"
      maxlength="8000"
      placeholder={chat.joined && !broken ? `Message ${engine.chatTitle(chat)}` : 'You can send messages once this device has joined'}
      disabled={!chat.joined || broken}
      aria-label="Message"
    ></textarea>
    <button type="submit" class="send" aria-label="Send" title="Send" disabled={!chat.joined || broken || !draft.trim()}>
      <Icon name="send" size={18} />
    </button>
  </form>
</section>
