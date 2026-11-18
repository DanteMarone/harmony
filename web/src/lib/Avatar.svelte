<script lang="ts" module>
  // Each person gets a steady colour from their id, so they are easy to spot
  // in a busy chat. These all keep white initials readable.
  const colours = ['#5865d9', '#2f8f6b', '#c0573d', '#8a54c9', '#b8812a', '#2b83b5', '#c24577', '#5b7a2e']

  export function initials(name: string): string {
    const words = name.trim().split(/\s+/).filter(Boolean)
    if (words.length === 0) return '?'
    const first = [...words[0]][0] ?? ''
    const last = words.length > 1 ? ([...words[words.length - 1]][0] ?? '') : ''
    return (first + last).toUpperCase()
  }
</script>

<script lang="ts">
  let { name, id, size = 40, group = false }: { name: string; id: number | string; size?: number; group?: boolean } =
    $props()

  const colour = $derived.by(() => {
    const key = String(id)
    let hash = 0
    for (let i = 0; i < key.length; i++) hash = (hash * 31 + key.charCodeAt(i)) >>> 0
    return colours[hash % colours.length]
  })
</script>

<span
  class="avatar"
  class:group
  style:width="{size}px"
  style:height="{size}px"
  style:font-size="{Math.round(size * 0.4)}px"
  style:background={colour}
  aria-hidden="true">{group ? '#' : initials(name)}</span
>
