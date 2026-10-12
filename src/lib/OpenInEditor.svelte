<script lang="ts">
  import { onMount } from "svelte";
  import { chooseEditor, editorName, editors, loadEditors, openInEditor } from "./editor.svelte";

  let { repo, path, line }: { repo: string; path: string; line?: number } = $props();

  onMount(() => {
    loadEditors();
  });
</script>

<span class="split">
  <button onclick={() => openInEditor(repo, path, line)} title={`Open ${path} in ${editorName()}`}>Open in {editorName()}</button>
  <span class="pick" title="Choose the editor">
    <select value={editors.chosen} onchange={(e) => chooseEditor(e.currentTarget.value)} aria-label="Editor">
      {#each editors.list as e (e.id)}
        <option value={e.id}>{e.name}</option>
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
