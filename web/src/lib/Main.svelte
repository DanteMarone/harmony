<script lang="ts">
  import { onDestroy } from 'svelte'
  import type { User } from './api'
  import Chat from './Chat.svelte'
  import Devices from './Devices.svelte'
  import { Engine } from './engine.svelte'
  import Invites from './Invites.svelte'
  import Members from './Members.svelte'
  import NewChat from './NewChat.svelte'
  import Recovery from './Recovery.svelte'
  import Verify from './Verify.svelte'

  let { user, onSignOut }: { user: User; onSignOut: () => void } = $props()

  // A new account gets a new Main (see App.svelte), so the first value is the only one.
  // svelte-ignore state_referenced_locally
  const engine = new Engine(user)
  void engine.start()
  onDestroy(() => engine.stop())

  let dialog = $state<'new' | 'members' | 'invites' | 'security' | 'devices' | null>(null)
  /** The person whose verification screen is open, on top of any other dialog. */
  let verifying = $state<number | null>(null)
  // A recovery code must be set up before anything else; entering one on a
  // new device is asked for once and can be skipped.
  const mustRecover = $derived(engine.recovery === 'setup' || engine.recovery === 'code')

  function openRecovery() {
    verifying = null
    dialog = 'security'
  }

  const chats = $derived(
    [...engine.chats].sort((a, b) => Math.max(b.last_seq, 0) - Math.max(a.last_seq, 0) || b.created_at - a.created_at),
  )
  const active = $derived(engine.chats.find((c) => c.id === engine.activeChat) ?? null)

  function onVisible() {
    if (!document.hidden && engine.activeChat) void engine.markRead(engine.activeChat)
  }
  document.addEventListener('visibilitychange', onVisible)
  onDestroy(() => document.removeEventListener('visibilitychange', onVisible))

  function newChatDone(chatId: string | null) {
    dialog = null
    if (chatId) void engine.openChat(chatId)
  }
</script>

{#if engine.status === 'starting'}
  <main class="card"><p class="lead">Setting up encryption on this device…</p></main>
{:else if engine.status === 'locked'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">Harmony is already open in another tab or window. Close it there, then reload this page.</p>
    <button onclick={() => location.reload()}>Reload</button>
  </main>
{:else if engine.status === 'revoked'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">This device was signed out, and its keys and messages were erased from this browser.</p>
    <p class="hint">Sign in again to use it as a new device. Your recovery code brings your history back.</p>
    <button onclick={onSignOut}>Sign in again</button>
  </main>
{:else if engine.status === 'signedOut'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="lead">You have been signed out.</p>
    <button onclick={onSignOut}>Sign in</button>
  </main>
{:else if engine.status === 'error'}
  <main class="card">
    <h1>Harmony</h1>
    <p class="error" role="alert">Could not start: {engine.error}</p>
    <button onclick={() => location.reload()}>Try again</button>
  </main>
{:else}
  <div class="layout">
    <aside class="sidebar">
      <header>
        <strong>Harmony</strong>
        <button onclick={() => (dialog = 'new')}>New chat</button>
      </header>
      <nav aria-label="Chats">
        {#each chats as chat (chat.id)}
          <button class="chat-item" class:active={chat.id === engine.activeChat} onclick={() => engine.openChat(chat.id)}>
            <span class="title">{chat.kind === 'group' ? '# ' : ''}{engine.chatTitle(chat)}</span>
            {#if engine.meta[chat.id]?.unread}
              <span class="badge" aria-label="{engine.meta[chat.id].unread} unread">{engine.meta[chat.id].unread}</span>
            {/if}
          </button>
        {:else}
          <p class="hint empty">No chats yet. Start one with “New chat”.</p>
        {/each}
      </nav>
      <footer>
        <div class="who">
          <span>{user.display_name}</span>
          <span class="hint">{engine.connected ? 'Connected' : 'Reconnecting…'}</span>
        </div>
        {#if engine.recovery === 'skipped'}
          <button class="secondary attention" onclick={openRecovery}>Confirm this device</button>
        {:else}
          <button class="secondary" onclick={openRecovery}>Security</button>
        {/if}
        <button class="secondary" onclick={() => (dialog = 'devices')}>Devices</button>
        {#if user.is_admin}<button class="secondary" onclick={() => (dialog = 'invites')}>Invites</button>{/if}
        <button class="secondary" onclick={onSignOut}>Sign out</button>
      </footer>
    </aside>

    {#if active}
      <Chat {engine} chat={active} onMembers={() => (dialog = 'members')} onVerify={(userId) => (verifying = userId)} />
    {:else}
      <section class="chat placeholder">
        <p class="lead">Pick a chat, or start a new one.</p>
      </section>
    {/if}
  </div>

  {#if dialog === 'new'}
    <NewChat {engine} onDone={newChatDone} />
  {:else if dialog === 'members' && active}
    <Members {engine} chat={active} onClose={() => (dialog = null)} onVerify={(userId) => (verifying = userId)} />
  {:else if dialog === 'invites'}
    <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && (dialog = null)}>
      <div class="dialog">
        <Invites />
        <div class="actions"><button class="secondary" onclick={() => (dialog = null)}>Close</button></div>
      </div>
    </div>
  {/if}

  {#if mustRecover}
    <Recovery {engine} />
  {:else if verifying !== null}
    <Verify
      {engine}
      userId={verifying}
      chatId={active?.id}
      onClose={() => (verifying = null)}
      onRecovery={openRecovery}
    />
  {:else if dialog === 'devices'}
    <Devices {engine} onClose={() => (dialog = null)} />
  {:else if dialog === 'security'}
    <Recovery {engine} onClose={() => (dialog = null)} />
  {/if}
{/if}
