<script lang="ts">
  import { onMount } from "svelte";
  import SplitPick from "./SplitPick.svelte";
  import { chooseEditor, editorName, editors, loadEditors, openInEditor } from "./editor.svelte";

  let { repo, path, line }: { repo: string; path: string; line?: number } = $props();

  onMount(() => {
    loadEditors();
  });
</script>

<SplitPick
  options={editors.list}
  chosen={editors.chosen}
  onpick={chooseEditor}
  onclick={() => openInEditor(repo, path, line)}
  title={`Open ${path} in ${editorName()}`}
  aria="Editor"
>
  {#snippet label()}Open in {editorName()}{/snippet}
</SplitPick>
