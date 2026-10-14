<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { ask } from "@tauri-apps/plugin-dialog";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import Icon from "./Icon.svelte";
  import OpenInEditor from "./OpenInEditor.svelte";
  import QuestionsPane from "./QuestionsPane.svelte";
  import RepoList from "./RepoList.svelte";
  import { loadTerminals, openTerminal, terminalName, terminals } from "./terminal.svelte";
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
    /** Run the analyst on the answered questions, with an optional message. */
    onrefine: (message: string, images: string[]) => void;
    /** base: review against this branch/ref instead of the loop's first checkpoint. */
    onrun: (base: string) => void;
    ondeliver: () => void;
    onclose: () => void;
  } = $props();

  loadTerminals();
  let checkpoints = $state<Checkpoint[]>([]);
  let view = $state<"doc" | "questions" | "review">("doc");
  const analysis = $derived(task.status_kind === "analysis");
  const showQuestions = $derived(analysis || task.open_questions > 0);

  $effect(() => {
    // Opening a task in Analysis lands on its questions; other tasks on the doc.
    void task.slug;
    view = task.status_kind === "analysis" ? "questions" : "doc";
  });
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

  async function setRepos(repos: string[]) {
    try {
      await invoke("set_task_repos", { repo, slug: task.slug, repos });
      error = "";
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
    <div class="titlerow">
      <div class="titles">
        <span class="slug">{task.slug}</span>
        <span class="title">{task.title.replace(new RegExp(`^${task.slug.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*[—:-]\\s*`), "")}</span>
      </div>
      <select class="status s-{task.status_kind}" value={task.status_kind} onchange={setStatus} disabled={editing} title="Status line in the doc">
        {#if task.status_kind === "unknown"}<option value="unknown" disabled>{task.status || "No status"}</option>{/if}
        {#each Object.entries(STATUS_LABELS) as [kind, label] (kind)}
          <option value={kind}>{label}</option>
        {/each}
      </select>
      <button class="ghost icon" onclick={onclose} title="Close"><Icon name="x" size={15} /></button>
    </div>
    <div class="toolrow">
      <span class="seg">
        <button class:on={view === "doc"} onclick={() => (view = "doc")}><Icon name="book" size={12} /> Doc</button>
        {#if showQuestions}
          <button class:on={view === "questions"} onclick={() => (view = "questions")} disabled={editing}
            ><Icon name="question" size={12} /> Questions{#if task.open_questions}<span class="count">{task.open_questions}</span>{/if}</button
          >
        {/if}
        <button class:on={view === "review"} onclick={() => (view = "review")} disabled={editing}><Icon name="eye" size={12} /> Review</button>
      </span>
      <div class="actions">
        {#if editing}
          <button onclick={() => (editing = false)}>Cancel</button>
          <button class="primary" onclick={save}>Save</button>
        {:else}
          <OpenInEditor {repo} path={`.ai-tasks/${task.slug}.md`} />
          {#if terminals.list.length}
            <button onclick={() => openTerminal(repo)} title={`Open ${terminalName()} in the repository (pick the terminal in Chat)`}
              ><Icon name="terminal" size={13} /></button
            >
          {/if}
          <button onclick={edit} title="Edit the doc here"><Icon name="edit" size={13} /></button>
          {#if task.status_kind === "done"}
            <button onclick={ondeliver} disabled={busy} title="Commit, push or open a PR, per [delivery] in the policy"
              ><Icon name="package" size={13} /> Deliver…</button
            >
          {/if}
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
            title="Implement → build → review until Done"><Icon name="run" size={12} /> Run</button
          >
        {/if}
      </div>
    </div>
  </div>
  {#if error}<p class="error pad">{error}</p>{/if}
  {#if editors.error}<p class="error pad">{editors.error}</p>{/if}
  {#if terminals.error}<p class="error pad">{terminals.error}</p>{/if}
  {#if changedOnDisk}<p class="warn">The doc changed on disk while you were editing. Saving overwrites that change.</p>{/if}
  {#if !editing}
    <div class="cps" title="Agents read these repositories too; the implementer may change the git repos among them (checkpointed, built, reviewed and delivered with the task)">
      <span class="meta">Related repos</span>
      {#key task.slug + task.repos.join()}<RepoList items={[...task.repos]} {repo} onchange={setRepos} />{/key}
    </div>
  {/if}
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
  {:else if view === "questions" && showQuestions}
    <QuestionsPane {repo} slug={task.slug} {version} {busy} {analysis} {onrefine} />
  {:else if view === "review"}
    <ReviewPane {repo} slug={task.slug} {version} />
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized above -->
    <article onclick={onDocClick} role="presentation">{@html html}</article>
  {/if}
</div>

<style>
  .bar {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 18px 10px;
    border-bottom: 1px solid var(--line);
  }
  .titlerow {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .slug {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--quiet);
  }
  .title {
    font-size: 15px;
    font-weight: 650;
    line-height: 1.3;
  }
  .status {
    height: 26px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 600;
    padding: 0 10px;
    border-color: transparent;
    background: var(--code-bg);
  }
  .s-analysis {
    color: var(--role-analyst);
    background: color-mix(in srgb, var(--role-analyst) 12%, transparent);
  }
  .s-ready {
    color: var(--accent-ink);
    background: var(--accent-soft);
  }
  .s-in_review {
    color: var(--role-reviewer);
    background: color-mix(in srgb, var(--role-reviewer) 13%, transparent);
  }
  .s-changes_requested {
    color: var(--bad);
    background: var(--bad-soft);
  }
  .s-done {
    color: var(--good);
    background: var(--good-soft);
  }
  .icon {
    width: 28px;
    padding: 0;
  }
  .toolrow {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .actions {
    display: flex;
    gap: 6px;
    margin-left: auto;
    align-items: center;
  }
  .detail {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
    border-left: 1px solid var(--line);
  }
  .count {
    margin-left: 5px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--accent);
    color: white;
    font-size: 10.5px;
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
