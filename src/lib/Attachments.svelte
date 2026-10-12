<script lang="ts">
  // Thumbnails of the pending images plus an attach button. The parent hands its textarea's paste
  // events to `paste`; files dropped anywhere inside `zone` are added too.
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import Icon from "./Icon.svelte";
  import { IMAGE_TYPES, fromFile, fromPath, pastedFiles, preview, type Pending } from "./attach";

  let {
    items = $bindable([]),
    zone,
    disabled = false,
    compact = false,
  }: { items: Pending[]; zone?: HTMLElement; disabled?: boolean; compact?: boolean } = $props();

  let input: HTMLInputElement;
  let error = $state("");
  let over = $state(false);

  async function add(files: (File | string)[]) {
    error = "";
    for (const f of files) {
      try {
        items.push(typeof f === "string" ? await fromPath(f) : await fromFile(f));
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      }
    }
  }

  /** For the parent's `onpaste`: takes the images, leaves text to the textarea. */
  export function paste(e: ClipboardEvent) {
    const files = pastedFiles(e);
    if (files.length && !disabled) {
      e.preventDefault();
      add(files);
    }
  }

  export function pick() {
    input?.click();
  }

  const inside = (x: number, y: number) => {
    const r = zone?.getBoundingClientRect();
    return !!r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  };

  onMount(() => {
    // Tauri handles file drops itself (the webview never sees a drop event) and reports
    // physical pixels.
    let unlisten: (() => void) | undefined;
    Promise.resolve()
      .then(() => getCurrentWebview().onDragDropEvent(({ payload: p }) => {
        if (p.type === "leave") return void (over = false);
        const x = p.position.x / devicePixelRatio;
        const y = p.position.y / devicePixelRatio;
        over = !disabled && inside(x, y);
        if (p.type === "drop") {
          if (over) add(p.paths);
          over = false;
        }
      }))
      .then((f) => (unlisten = f))
      .catch(() => {});
    return () => unlisten?.();
  });

  $effect(() => {
    zone?.classList.toggle("drop-over", over);
  });
</script>

<input
  bind:this={input}
  type="file"
  accept={IMAGE_TYPES.join(",")}
  multiple
  hidden
  onchange={() => {
    add([...(input.files ?? [])]);
    input.value = "";
  }}
/>
{#if items.length || error || !compact}
  <div class="att" class:compact>
    {#each items as p (p.id)}
      <div class="thumb" title={p.name}>
        <img src={preview(p)} alt={p.name} />
        <button type="button" class="rm" title="Remove" onclick={() => (items = items.filter((x) => x.id !== p.id))} {disabled}>
          <Icon name="x" size={11} stroke={2.5} />
        </button>
      </div>
    {/each}
    {#if !compact}
      <button type="button" class="ghost small add" onclick={pick} {disabled} title="Attach images (or paste / drop them)">
        <Icon name="paperclip" size={13} /> Attach image
      </button>
      {#if !items.length}<span class="hint">or paste a screenshot (⌘V) / drop a file</span>{/if}
    {/if}
    {#if error}<span class="error">{error}</span>{/if}
  </div>
{/if}

<style>
  .att {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .thumb {
    position: relative;
    width: 64px;
    height: 64px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: var(--code-bg);
    overflow: hidden;
  }
  .compact .thumb {
    width: 52px;
    height: 52px;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .rm {
    position: absolute;
    top: 3px;
    right: 3px;
    width: 18px;
    height: 18px;
    padding: 0;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.6);
    border: none;
    color: #fff;
    box-shadow: none;
    opacity: 0;
  }
  .thumb:hover .rm {
    opacity: 1;
  }
  .hint {
    color: var(--faint);
    font-size: 12px;
  }
  .error {
    font-size: 12px;
  }
  :global(.drop-over) {
    outline: 2px dashed var(--accent) !important;
    outline-offset: 2px;
  }
</style>
