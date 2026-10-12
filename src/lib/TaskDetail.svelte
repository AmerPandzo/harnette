<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import OpenInEditor from "./OpenInEditor.svelte";
  import ReviewPane from "./ReviewPane.svelte";
  import { editors } from "./editor.svelte";
  import { STATUS_LABELS, type Checkpoint, type StatusKind, type TaskSummary } from "./types";

  let {
    repo,
    task,
    version,
    busy,
    onrefine,
    onrun,
    ondeliver,
    onclose,
  }: {
    repo: string;
    task: TaskSummary;
    /** Bumped whenever task docs change on disk. */
    version: number;
    busy: boolean;
    onrefine: () => void;
    /** base: review against this branch/ref instead of the loop's first checkpoint. */
    onrun: (base: string) => void;
    ondeliver: () => void;
    onclose: () => void;
  } = $props();

  let checkpoints = $state<Checkpoint[]>([]);
  let view = $state<"doc" | "review">("doc");
  let base = $state("");
  const runnable = $derived(["ready", "changes_requested", "in_review"].includes(task.status_kind));

  $effect(() => {
    const slug = task.slug;
    void version;
    void busy;
    invoke<Checkpoint[]>("list_checkpoints", { repo, slug })
      .then((c) => (checkpoints = c))
      .catch(() => (checkpoints = []));
  });

  async function restore(c: Checkpoint) {
    const yes = await ask(
      `Put the working tree back to checkpoint ${c.round}? Changes made after it are overwritten and files added after it are deleted.`,
      { title: "Restore checkpoint", kind: "warning", okLabel: "Restore", cancelLabel: "Cancel" },
    );
    if (!yes) return;
    try {
      await invoke("restore_checkpoint", { repo, slug: task.slug, round: c.round });
      error = "";
    } catch (e) {
      error = String(e);
    }
  }

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
      <span class="seg">
        <button class:on={view === "doc"} onclick={() => (view = "doc")}>Doc</button>
        <button class:on={view === "review"} onclick={() => (view = "review")} disabled={editing}>Review</button>
      </span>
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
        {#if task.status_kind === "in_review"}
          <input
            class="base"
            bind:value={base}
            placeholder="vs. checkpoint"
            title="Review against a branch or ref (e.g. develop) when the work is already committed. Empty = since the loop's first checkpoint."
          />
        {/if}
        <button
          class:primary={runnable}
          onclick={() => onrun(task.status_kind === "in_review" ? base.trim() : "")}
          disabled={busy || !runnable}
          title="Implement → build → review until Done">Run</button
        >
        {#if task.status_kind === "done"}
          <button onclick={ondeliver} disabled={busy} title="Commit, push or open a PR, per [delivery] in the policy">Deliver…</button>
        {/if}
        <OpenInEditor {repo} path={`.ai-tasks/${task.slug}.md`} />
        <button onclick={edit}>Edit</button>
        <button onclick={onclose} title="Close">✕</button>
      {/if}
    </div>
  </div>
  {#if error}<p class="error pad">{error}</p>{/if}
  {#if editors.error}<p class="error pad">{editors.error}</p>{/if}
  {#if changedOnDisk}<p class="warn">The doc changed on disk while you were editing. Saving overwrites that change.</p>{/if}
  {#if checkpoints.length > 0 && !editing}
    <div class="cps">
      <span class="meta">Checkpoints</span>
      {#each checkpoints as c (c.round)}
        <span class="cp" title={`${c.git_ref} · ${new Date(c.created_unix * 1000).toLocaleString()}`}>
          {c.round}
          <button class="link" onclick={() => restore(c)} disabled={busy}>Restore</button>
        </span>
      {/each}
    </div>
  {/if}
  {#if editing}
    <textarea bind:value={draft} spellcheck="false"></textarea>
  {:else if view === "review"}
    <ReviewPane {repo} slug={task.slug} {version} />
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
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg button {
    border: none;
    border-radius: 0;
    padding: 3px 10px;
    font-size: 12px;
  }
  .seg button.on {
    background: var(--accent-soft);
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .cps {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    padding: 6px 14px;
    border-bottom: 1px solid var(--line);
    font-size: 12px;
  }
  .meta {
    color: var(--quiet);
  }
  .cp {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    font-family: ui-monospace, Menlo, monospace;
  }
  .link {
    border: none;
    background: none;
    padding: 0 2px;
    color: var(--accent);
    font-size: 12px;
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
  .base {
    width: 9em;
    font-size: 12px;
  }
</style>
