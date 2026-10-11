<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import { STATUS_LABELS, type StatusKind, type TaskSummary } from "./types";

  let {
    repo,
    task,
    version,
    busy,
    onrefine,
    onclose,
  }: {
    repo: string;
    task: TaskSummary;
    /** Bumped whenever task docs change on disk. */
    version: number;
    busy: boolean;
    onrefine: () => void;
    onclose: () => void;
  } = $props();

  let markdown = $state("");
  let draft = $state("");
  let editing = $state(false);
  let changedOnDisk = $state(false);
  let error = $state("");

  // Agent-written content: sanitize before it reaches the webview, which can call Tauri commands.
  let html = $derived(DOMPurify.sanitize(marked.parse(markdown, { async: false }) as string));

  $effect(() => {
    // Re-read when the slug changes or the docs change on disk.
    const slug = task.slug;
    void version;
    invoke<string>("read_task", { repo, slug })
      .then((md) => {
        if (editing && md !== markdown) changedOnDisk = true;
        markdown = md;
        error = "";
      })
      .catch((e) => (error = String(e)));
  });

  $effect(() => {
    // Leaving the task ends editing.
    void task.slug;
    editing = false;
    changedOnDisk = false;
  });

  function edit() {
    draft = markdown;
    changedOnDisk = false;
    editing = true;
  }

  async function save() {
    try {
      await invoke("write_task", { repo, slug: task.slug, content: draft });
      markdown = draft;
      editing = false;
      changedOnDisk = false;
    } catch (e) {
      error = String(e);
    }
  }

  async function setStatus(ev: Event) {
    const status = (ev.target as HTMLSelectElement).value as StatusKind;
    try {
      await invoke("set_task_status", { repo, slug: task.slug, status });
    } catch (e) {
      error = String(e);
    }
  }

  // Links must open in the browser; navigating the app window would replace the UI.
  function onDocClick(ev: MouseEvent) {
    const a = (ev.target as HTMLElement).closest("a");
    if (!a) return;
    ev.preventDefault();
    const href = a.getAttribute("href") ?? "";
    if (/^https?:\/\//.test(href)) openUrl(href).catch(() => {});
  }
</script>

<div class="detail">
  <div class="bar">
    <div class="head">
      <span class="slug">{task.slug}</span>
      <select value={task.status_kind} onchange={setStatus} disabled={editing} title="Status line in the doc">
        {#if task.status_kind === "unknown"}<option value="unknown" disabled>{task.status || "No status"}</option>{/if}
        {#each Object.entries(STATUS_LABELS) as [kind, label] (kind)}
          <option value={kind}>{label}</option>
        {/each}
      </select>
    </div>
    <div class="actions">
      {#if editing}
        <button onclick={() => (editing = false)}>Cancel</button>
        <button class="primary" onclick={save}>Save</button>
      {:else}
        <button
          onclick={onrefine}
          disabled={busy || task.status_kind !== "analysis"}
          title="Continue the analysis after answering Open questions in the doc">Refine</button
        >
        <button onclick={edit}>Edit</button>
        <button onclick={onclose} title="Close">✕</button>
      {/if}
    </div>
  </div>
  {#if error}<p class="error pad">{error}</p>{/if}
  {#if changedOnDisk}<p class="warn">The doc changed on disk while you were editing. Saving overwrites that change.</p>{/if}
  {#if editing}
    <textarea bind:value={draft} spellcheck="false"></textarea>
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized above -->
    <article onclick={onDocClick} role="presentation">{@html html}</article>
  {/if}
</div>

<style>
  .detail {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
  }
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--line);
    flex-wrap: wrap;
  }
  .head {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .slug {
    font-family: ui-monospace, Menlo, monospace;
    font-weight: 600;
  }
  select {
    padding: 4px 6px;
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .pad {
    padding: 8px 14px;
  }
  .warn {
    margin: 0;
    padding: 8px 14px;
    background: var(--warn-bg);
    font-size: 12px;
  }
  textarea {
    flex: 1;
    margin: 12px;
    resize: none;
    font-family: ui-monospace, Menlo, monospace;
    font-size: 12.5px;
    line-height: 1.5;
  }
  article {
    flex: 1;
    overflow: auto;
    padding: 4px 20px 24px;
    line-height: 1.55;
  }
  article :global(h1) {
    font-size: 18px;
  }
  article :global(h2) {
    font-size: 15px;
    margin-top: 22px;
    border-bottom: 1px solid var(--line);
    padding-bottom: 4px;
  }
  article :global(h3) {
    font-size: 14px;
  }
  article :global(pre) {
    background: var(--code-bg);
    padding: 10px;
    border-radius: 6px;
    overflow: auto;
  }
  article :global(pre code) {
    padding: 0;
  }
  article :global(table) {
    border-collapse: collapse;
  }
  article :global(td),
  article :global(th) {
    border: 1px solid var(--line);
    padding: 4px 8px;
  }
  article :global(li:has(> input[type="checkbox"])) {
    list-style: none;
    margin-left: -20px;
  }
  article :global(a) {
    color: var(--accent);
  }
</style>
