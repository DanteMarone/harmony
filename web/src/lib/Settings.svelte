<script lang="ts">
  import type { User } from './api'
  import Avatar from './Avatar.svelte'
  import Icon, { type IconName } from './Icon.svelte'
  import { preview, setSoundOn, soundOn, SOUNDS, type SoundKind } from './sounds'

  let {
    user,
    needsConfirm,
    onOpen,
    onSignOut,
    onClose,
  }: {
    user: User
    /** This device skipped entering the recovery code. */
    needsConfirm: boolean
    onOpen: (page: 'security' | 'devices' | 'invites') => void
    onSignOut: () => void
    onClose: () => void
  } = $props()

  let sounds = $state(Object.fromEntries(SOUNDS.map((s) => [s.kind, soundOn(s.kind)])) as Record<SoundKind, boolean>)
  let notify = $state(typeof Notification === 'undefined' ? 'unsupported' : Notification.permission)

  function toggleSound(kind: SoundKind) {
    sounds[kind] = !sounds[kind]
    setSoundOn(kind, sounds[kind])
    if (sounds[kind]) preview(kind)
  }

  async function allowNotifications() {
    notify = await Notification.requestPermission()
  }

  const pages: { page: 'security' | 'devices' | 'invites'; icon: IconName; title: string; detail: string }[] = $derived([
    {
      page: 'security',
      icon: 'key',
      title: needsConfirm ? 'Confirm this device' : 'Security',
      detail: needsConfirm ? 'Enter your recovery code to bring back your history' : 'Recovery code and history backup',
    },
    { page: 'devices', icon: 'devices', title: 'Devices', detail: 'See and sign out the devices on your account' },
    ...(user.is_admin
      ? [{ page: 'invites' as const, icon: 'ticket' as IconName, title: 'Invites', detail: 'Make invite links for friends' }]
      : []),
  ])
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onClose()}>
  <div class="dialog card settings" role="dialog" aria-labelledby="settings-title">
    <div class="settings-head">
      <Avatar name={user.display_name} id={user.id} size={44} />
      <div>
        <h2 id="settings-title">{user.display_name}</h2>
        <p class="hint">@{user.username}{user.is_admin ? ' · admin' : ''}</p>
      </div>
      <button class="icon-button" aria-label="Close" title="Close" onclick={onClose}><Icon name="close" /></button>
    </div>

    <ul class="menu">
      {#each pages as item (item.page)}
        <li>
          <button class="menu-item" class:attention={item.page === 'security' && needsConfirm} onclick={() => onOpen(item.page)}>
            <Icon name={item.icon} />
            <span class="menu-text"><strong>{item.title}</strong><span class="hint">{item.detail}</span></span>
            <Icon name="chevron" size={16} />
          </button>
        </li>
      {/each}
    </ul>

    <h3 class="section-label">Sounds</h3>
    <ul class="menu sounds">
      {#each SOUNDS as sound (sound.kind)}
        <li>
          <div class="menu-item static">
            <Icon name="volume" />
            <label class="menu-text" for="sound-{sound.kind}">
              <strong>{sound.title}</strong><span class="hint">{sound.detail}</span>
            </label>
            <button
              class="icon-button"
              aria-label="Play the {sound.title.toLowerCase()} sound"
              title="Play"
              onclick={() => preview(sound.kind)}><Icon name="play" size={16} /></button
            >
            <input
              id="sound-{sound.kind}"
              type="checkbox"
              class="switch"
              checked={sounds[sound.kind]}
              onchange={() => toggleSound(sound.kind)}
            />
          </div>
        </li>
      {/each}
    </ul>

    <ul class="menu">
      {#if notify !== 'unsupported'}
        <li>
          <div class="menu-item static">
            <Icon name="bell" />
            <span class="menu-text">
              <strong>Desktop alerts</strong>
              <span class="hint">
                {notify === 'granted'
                  ? 'On: you get an alert for calls when Harmony is in the background'
                  : notify === 'denied'
                    ? 'Blocked in your browser. Allow notifications for this site to turn them on.'
                    : 'Get an alert for calls when Harmony is in the background'}
              </span>
            </span>
            {#if notify === 'default'}<button class="secondary" onclick={allowNotifications}>Turn on</button>{/if}
          </div>
        </li>
      {/if}
    </ul>

    <div class="actions">
      <button class="secondary danger" onclick={onSignOut}><Icon name="logout" size={18} /> Sign out</button>
    </div>
  </div>
</div>
