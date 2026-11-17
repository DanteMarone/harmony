<script lang="ts">
  import { tick } from 'svelte'
  import type { ChatInfo } from './api'
  import type { Engine } from './engine.svelte'
  import type { StoredMessage } from './store'

  let {
    engine,
    chat,
    onMembers,
    onVerify,
  }: { engine: Engine; chat: ChatInfo; onMembers: () => void; onVerify: (userId: number) => void } = $props()

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
    <div>
      <h2>{engine.chatTitle(chat)}</h2>
      <p class="hint">
        {chat.kind === 'group' ? `${chat.members.length} members, end-to-end encrypted` : 'End-to-end encrypted'}{otherVerified ? ', verified' : ''}
      </p>
    </div>
    {#if chat.kind === 'group'}
      <button class="secondary" onclick={onMembers}>Members</button>
    {:else if other}
      <button class="secondary" onclick={() => onVerify(other.user_id)}>Verify</button>
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
        Waiting for another member's device to share this chat's keys with this device. That happens
        automatically the next time one of them is online.
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
          <header>
            <strong>{engine.userName(message.senderUser)}</strong>
            <time>{time(message)}</time>
          </header>
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
    <button type="submit" disabled={!chat.joined || broken || !draft.trim()}>Send</button>
  </form>
</section>
