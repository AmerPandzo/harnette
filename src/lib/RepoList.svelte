<script lang="ts">
  // Related repositories of a task: chips, plus a path field (recent repos suggested) and a folder picker.
  import { open } from "@tauri-apps/plugin-dialog";
  import Icon from "./Icon.svelte";
  import { remembered } from "./types";

  let {
    items = $bindable([]),
    repo,
    onchange,
  }: { items?: string[]; repo: string; onchange?: (items: string[]) => void } = $props();

  let input = $state("");
  const home = (p: string) => p.replace(/^\/Users\/[^/]+/, "~");
  const recent = $derived((JSON.parse(remembered("harnette.recentRepos", "[]")) as string[]).filter((r) => r !== repo).map(home));
  const id = `repos-${Math.random().toString(36).slice(2)}`;

  function set(next: string[]) {
    items = next;
    onchange?.(next);
  }
  function add(path: string) {
    const p = home(path.trim());
    if (p && p !== home(repo) && !items.includes(p)) set([...items, p]);
    input = "";
  }
  async function pick() {
    const picked = await open({ directory: true, multiple: false, title: "Add a related repository" });
    if (typeof picked === "string") add(picked);
  }
</script>

<div class="repos">
  {#each items as r (r)}
    <span class="chip" title={r}><Icon name="folder" size={12} /><code>{r.split("/").filter(Boolean).at(-1)}</code>
      <button type="button" class="x" onclick={() => set(items.filter((x) => x !== r))} title="Remove">×</button></span>
  {/each}
  <input
    list={id}
    bind:value={input}
    placeholder="~/path/to/other-repo"
    onkeydown={(e) => e.key === "Enter" && (e.preventDefault(), add(input))}
    onchange={() => recent.includes(input) && add(input)}
  />
  <datalist {id}>{#each recent as r (r)}<option value={r}></option>{/each}</datalist>
  <button type="button" class="ghost" onclick={pick} title="Choose folder"><Icon name="plus" size={13} /></button>
</div>

<style>
  .repos {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px 2px 8px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--panel);
    font-size: 12px;
  }
  .x {
    border: 0;
    background: none;
    padding: 0 4px;
    cursor: pointer;
    color: var(--quiet);
  }
  input {
    flex: 1;
    min-width: 160px;
  }
</style>
