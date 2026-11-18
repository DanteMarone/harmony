<script lang="ts">
  import Avatar from './Avatar.svelte'
  import Icon from './Icon.svelte'

  let {
    caller,
    callerId,
    chatTitle,
    group,
    onJoin,
    onDecline,
  }: {
    caller: string
    callerId: number
    chatTitle: string
    group: boolean
    onJoin: () => void
    onDecline: () => void
  } = $props()
</script>

<div class="incoming" role="alertdialog" aria-labelledby="incoming-title" aria-describedby="incoming-detail">
  <span class="ring-pulse"><Avatar name={caller} id={callerId} size={48} /></span>
  <div class="incoming-text">
    <strong id="incoming-title">{caller} is calling</strong>
    <span id="incoming-detail" class="hint">{group ? `Voice call in # ${chatTitle}` : 'Encrypted voice call'}</span>
  </div>
  <div class="incoming-actions">
    <button class="decline" title="Decline" aria-label="Decline" onclick={onDecline}>
      <Icon name="phone" size={20} />
    </button>
    <button class="accept" title="Join call" aria-label="Join call" onclick={onJoin}>
      <Icon name="phone" size={20} />
    </button>
  </div>
</div>
