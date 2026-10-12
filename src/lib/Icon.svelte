<script lang="ts" module>
  // Line icons (24×24, 2px stroke, lucide-style), drawn inline so the app needs no icon font.
  const PATHS: Record<string, string> = {
    board: "M4 4h4v16H4zM10 4h4v10h-4zM16 4h4v13h-4z",
    run: "M5 4l14 8-14 8z",
    shield: "M12 3l8 3v6c0 4.5-3.4 8.3-8 9-4.6-.7-8-4.5-8-9V6z",
    gauge: "M12 14l4-4M3.5 15a9 9 0 1 1 17 0",
    folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
    chevron: "M6 9l6 6 6-6",
    "chevron-right": "M9 6l6 6-6 6",
    file: "M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8zM14 3v5h5",
    edit: "M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4",
    terminal: "M4 6l5 6-5 6M12 18h8",
    search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
    globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z",
    check: "M5 12.5l4.5 4.5L19 7",
    x: "M6 6l12 12M18 6L6 18",
    alert: "M12 4l9 16H3zM12 10v4M12 17.5v.5",
    branch: "M6 4v12M18 8a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM6 20a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM18 8c0 5-6 4-12 8",
    hammer: "M14 6l4 4M4 20l9-9M12 4l8 8-3 3-8-8z",
    eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
    user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21c0-4 3.6-6 8-6s8 2 8 6",
    sparkles: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z",
    clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3 2",
    coins: "M9 14a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM15.5 9.6A6 6 0 1 1 9.6 15.5",
    pause: "M8 5v14M16 5v14",
    stop: "M6 6h12v12H6z",
    send: "M4 12l16-8-6 16-2.5-6.5z",
    plus: "M12 5v14M5 12h14",
    flag: "M5 21V4M5 4h11l-2 4 2 4H5",
    book: "M4 5a2 2 0 0 1 2-2h13v16H6a2 2 0 0 0-2 2zM4 19V5",
    question: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6M12 17v.5",
    package: "M12 3l8 4.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9",
    zap: "M13 3L5 13h6l-1 8 8-10h-6z",
    dot: "M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2z",
  };
</script>

<script lang="ts">
  let { name, size = 16, stroke = 2 }: { name: string; size?: number; stroke?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  class="icon"><path d={PATHS[name] ?? PATHS.dot} /></svg
>

<style>
  .icon {
    flex: none;
    display: inline-block;
    vertical-align: -0.2em;
  }
</style>
