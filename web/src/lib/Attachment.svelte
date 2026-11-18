<script lang="ts">
  import type { Engine } from './engine.svelte'
  import { formatSize, isImage } from './files'
  import type { StoredMessage } from './store'

  let { engine, message }: { engine: Engine; message: StoredMessage } = $props()

  const file = $derived(message.file!)
  const photo = $derived(isImage(file.type))
  let url = $state<string | null>(null)

  // Load this device's copy whenever it becomes available, and let go of it
  // when the message leaves the screen.
  $effect(() => {
    if (message.fileState !== 'ready') return
    let objectUrl: string | null = null
    let cancelled = false
    void engine.fileBlob(message).then((blob) => {
      if (cancelled || !blob) return
      objectUrl = URL.createObjectURL(blob)
      url = objectUrl
    })
    return () => {
      cancelled = true
      if (objectUrl) URL.revokeObjectURL(objectUrl)
      url = null
    }
  })
</script>

{#if photo && url}
  <a class="photo" href={url} download={file.name} title="Save {file.name}">
    <img src={url} alt={file.name} />
  </a>
{:else}
  <div class="file-card">
    <span class="file-icon" aria-hidden="true">{photo ? '🖼' : '📄'}</span>
    <span class="file-info">
      <span class="file-name">{file.name}</span>
      <span class="hint">
        {formatSize(file.size)}
        {#if message.fileState === 'fetching'}
          · receiving…
        {:else if message.fileState === 'gone'}
          · not on this device
        {:else if message.seq === null && !message.problem}
          · sending…
        {/if}
      </span>
    </span>
    {#if url}
      <a class="button secondary" href={url} download={file.name}>Save</a>
    {/if}
  </div>
  {#if message.fileState === 'gone'}
    <p class="hint">
      Files are kept only on the devices that received them, and the server lets go of its copy after a few days.
    </p>
  {/if}
{/if}

<style>
  .photo {
    display: inline-block;
    margin-top: 0.25rem;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--border);
    line-height: 0;
  }
  .photo img {
    display: block;
    /* No percentages: they would make the frame size itself to the full photo. */
    max-width: min(22rem, 70vw);
    max-height: 20rem;
  }
  .file-card {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    max-width: 24rem;
    margin-top: 0.25rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
  }
  .file-icon { font-size: 1.5rem; }
  .file-info { display: grid; flex: 1; min-width: 0; }
  .file-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 600; }
  .file-info .hint { font-size: 0.8rem; }
  .button {
    padding: 0.4rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    text-decoration: none;
    font-weight: 500;
  }
  .button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
