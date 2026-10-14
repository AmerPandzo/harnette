<script lang="ts">
  // A button with a small picker beside it: "Open in <chosen>" plus the list to choose from.
  import type { Snippet } from "svelte";

  let {
    options,
    chosen,
    onpick,
    onclick,
    title,
    label,
    aria,
  }: {
    options: { id: string; name: string }[];
    chosen: string;
    onpick: (id: string) => void;
    onclick: () => void;
    title: string;
    label: Snippet;
    aria: string;
  } = $props();
</script>

<span class="split">
  <button {onclick} {title}>{@render label()}</button>
  <span class="pick" title={`Choose the ${aria.toLowerCase()}`}>
    <select value={chosen} onchange={(e) => onpick(e.currentTarget.value)} aria-label={aria}>
      {#each options as o (o.id)}
        <option value={o.id}>{o.name}</option>
      {/each}
    </select>
    <span class="caret" aria-hidden="true">▾</span>
  </span>
</span>

<style>
  .split {
    display: inline-flex;
  }
  .split > button {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    white-space: nowrap;
  }
  .pick {
    position: relative;
    display: inline-flex;
  }
  .pick select {
    width: 22px;
    padding: 0;
    margin-left: -1px;
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    color: transparent;
    appearance: none;
    cursor: pointer;
  }
  .pick select option {
    color: var(--ink);
  }
  .caret {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 13px;
    pointer-events: none;
    color: var(--ink);
  }
</style>
