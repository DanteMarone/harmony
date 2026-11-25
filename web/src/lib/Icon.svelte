<script lang="ts" module>
  // Simple line icons, drawn on a 24×24 grid. Kept inline so the app has no
  // icon dependency and nothing is fetched from elsewhere.
  const paths = {
    phone:
      'M22 16.9v3a2 2 0 0 1-2.2 2 19.8 19.8 0 0 1-8.6-3.1 19.5 19.5 0 0 1-6-6A19.8 19.8 0 0 1 2.1 4.2 2 2 0 0 1 4.1 2h3a2 2 0 0 1 2 1.7c.1 1 .4 1.9.7 2.8a2 2 0 0 1-.5 2.1L8 9.9a16 16 0 0 0 6 6l1.3-1.3a2 2 0 0 1 2.1-.4c.9.3 1.8.6 2.8.7a2 2 0 0 1 1.7 2z',
    shield: 'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z|m9 12 2 2 4-4',
    users:
      'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2|M9 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8z|M22 21v-2a4 4 0 0 0-3-3.9|M16 3.1a4 4 0 0 1 0 7.8',
    lock: 'M5 11h14v10H5z|M8 11V7a4 4 0 0 1 8 0v4',
    send: 'm22 2-7 20-4-9-9-4z|M22 2 11 13',
    plus: 'M12 5v14|M5 12h14',
    key: 'M15.5 7.5 19 4|m21 2-9.6 9.6|M7.5 13a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11z|m15.5 7.5 3 3L22 7l-3-3',
    devices: 'M4 4h16v11H4z|M2 20h20|M10 15v5|M14 15v5',
    ticket: 'M3 7a2 2 0 0 0 2-2h14a2 2 0 0 0 2 2v3a2 2 0 0 0 0 4v3a2 2 0 0 0-2 2H5a2 2 0 0 0-2-2v-3a2 2 0 0 0 0-4z|M13 5v2|M13 17v2|M13 11v2',
    logout: 'M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4|m16 17 5-5-5-5|M21 12H9',
    back: 'm15 18-6-6 6-6',
    close: 'M18 6 6 18|M6 6l12 12',
    hash: 'M4 9h16|M4 15h16|M10 3 8 21|M16 3l-2 18',
    settings: 'M4 21v-7|M4 10V3|M12 21v-9|M12 8V3|M20 21v-5|M20 12V3|M1 14h6|M9 8h6|M17 16h6',
    volume: 'M11 5 6 9H2v6h4l5 4z|M15.5 8.5a5 5 0 0 1 0 7|M19 5a10 10 0 0 1 0 14',
    play: 'M7 4v16l13-8z',
    bell: 'M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9|M10.3 21a1.9 1.9 0 0 0 3.4 0',
    chevron: 'm9 18 6-6-6-6',
    alert: 'M12 9v4|M12 17h.01|M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z',
  } as const
  export type IconName = keyof typeof paths
</script>

<script lang="ts">
  let { name, size = 20 }: { name: IconName; size?: number } = $props()
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each paths[name].split('|') as d (d)}<path {d} />{/each}
</svg>
