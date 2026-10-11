<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";

  // Mirrors harness_core::RunEvent (serde tag = "kind").
  type RunEvent =
    | { kind: "started"; run_id: string; slug: string }
    | { kind: "session"; session_id: string; model: string }
    | { kind: "text"; text: string }
    | { kind: "tool_use"; id: string; tool: string; summary: string }
    | { kind: "tool_result"; tool_use_id: string; is_error: boolean; summary: string }
    | { kind: "permission_auto"; request_id: string; tool: string; summary: string; allowed: boolean; rule: string }
    | { kind: "permission_ask"; request_id: string; tool: string; summary: string }
    | { kind: "permission_answered"; request_id: string; allowed: boolean; by: string }
    | { kind: "note"; text: string }
    | { kind: "quota"; five_hour: number; seven_day: number }
    | {
        kind: "finished";
        ok: boolean;
        result: string;
        cost_usd: number;
        input_tokens: number;
        output_tokens: number;
        duration_ms: number;
      }
    | { kind: "error"; message: string }
    | { kind: "exited"; code: number | null };

  type Ask = { request_id: string; tool: string; summary: string };

  const remembered = (key: string, fallback: string) => {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  };

  let repo = $state(remembered("harness.repo", ""));
  let slug = $state("");
  let model = $state(remembered("harness.model", ""));
  let ticket = $state("");
  let note = $state("");

  let running = $state(false);
  let error = $state("");
  let events = $state<RunEvent[]>([]);
  let asks = $state<Ask[]>([]);
  let quota = $state<{ five_hour: number; seven_day: number } | null>(null);
  let finished = $state<Extract<RunEvent, { kind: "finished" }> | null>(null);
  let model_used = $state("");

  // Tool results are shown under their tool call, not as separate rows.
  let results = $derived(
    new Map(events.filter((e) => e.kind === "tool_result").map((e) => [e.tool_use_id, e])),
  );
  let decisions = $derived(
    events.filter((e) => e.kind === "permission_auto" || e.kind === "permission_answered"),
  );
  let feed = $derived(events.filter((e) => e.kind !== "tool_result" && e.kind !== "quota"));

  let feedEl: HTMLElement | undefined = $state();

  onMount(() => {
    const unlisten = listen<RunEvent>("run-event", ({ payload: e }) => {
      events.push(e);
      if (e.kind === "permission_ask") asks.push({ request_id: e.request_id, tool: e.tool, summary: e.summary });
      if (e.kind === "permission_answered") asks = asks.filter((a) => a.request_id !== e.request_id);
      if (e.kind === "quota") quota = { five_hour: e.five_hour, seven_day: e.seven_day };
      if (e.kind === "session") model_used = e.model;
      if (e.kind === "finished") finished = e;
      if (e.kind === "exited") {
        running = false;
        asks = [];
      }
      queueMicrotask(() => feedEl?.scrollTo({ top: feedEl.scrollHeight, behavior: "smooth" }));
    });
    return () => {
      unlisten.then((f) => f());
    };
  });

  async function start(ev: Event) {
    ev.preventDefault();
    error = "";
    events = [];
    asks = [];
    finished = null;
    model_used = "";
    try {
      localStorage.setItem("harness.repo", repo);
      localStorage.setItem("harness.model", model);
    } catch {}
    try {
      running = true;
      await invoke("start_run", { repo, slug, ticket, model: model || null });
    } catch (e) {
      running = false;
      error = String(e);
    }
  }

  async function chooseRepo() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a repository", defaultPath: repo || undefined });
    if (typeof picked === "string") repo = picked;
  }

  async function answer(a: Ask, allow: boolean) {
    await invoke("answer_permission", { requestId: a.request_id, allow });
  }

  async function sendNote(ev: Event) {
    ev.preventDefault();
    if (!note.trim()) return;
    try {
      await invoke("send_note", { text: note.trim() });
      note = "";
    } catch (e) {
      error = String(e);
    }
  }

  const pct = (x: number) => `${Math.round(x * 100)}%`;
  const decisionFor = (toolSummary: string) => decisions.findLast((d) => "summary" in d && d.summary === toolSummary);
</script>

<div class="app">
  <header>
    <div class="brand">Harness <span class="tag">phase 0 spike</span></div>
    <div class="status">
      <span class="pill" class:live={running}>{running ? "Running" : finished ? (finished.ok ? "Finished" : "Failed") : "Idle"}</span>
      {#if model_used}<span class="meta">{model_used}</span>{/if}
      {#if quota}
        <span class="meter" title="Claude subscription usage, 5-hour window">
          5h <span class="bar"><span style="width: {pct(quota.five_hour)}"></span></span> {pct(quota.five_hour)}
        </span>
        <span class="meter" title="Claude subscription usage, 7-day window">
          7d <span class="bar"><span style="width: {pct(quota.seven_day)}"></span></span> {pct(quota.seven_day)}
        </span>
      {/if}
      {#if finished}
        <span class="meta">${finished.cost_usd.toFixed(4)} · {(finished.input_tokens / 1000).toFixed(0)}k in · {(finished.output_tokens / 1000).toFixed(1)}k out · {(finished.duration_ms / 1000).toFixed(0)}s</span>
      {/if}
    </div>
  </header>

  <aside>
    <form onsubmit={start}>
      <label
        >Repository
        <div class="picker">
          <input bind:value={repo} placeholder="Choose a folder or type ~/path" required disabled={running} />
          <button type="button" onclick={chooseRepo} disabled={running}>Choose…</button>
        </div>
      </label>
      <div class="row">
        <label>Task slug<input bind:value={slug} placeholder="OAM-12345" required disabled={running} /></label>
        <label
          >Model
          <select bind:value={model} disabled={running}>
            <option value="">Default</option>
            <option value="haiku">Haiku</option>
            <option value="sonnet">Sonnet</option>
            <option value="opus">Opus</option>
          </select>
        </label>
      </div>
      <label class="grow">Ticket<textarea bind:value={ticket} placeholder="Paste the ticket text" required disabled={running}></textarea></label>
      <div class="actions">
        <button type="submit" class="primary" disabled={running}>Analyze</button>
        <button type="button" onclick={() => invoke("stop_run")} disabled={!running}>Stop</button>
      </div>
      {#if error}<p class="error">{error}</p>{/if}
      <p class="hint">Analysis only: the agent may read the repo and write <code>.ai-tasks/&lt;slug&gt;.md</code>. Unlisted actions ask you.</p>
    </form>
  </aside>

  <main>
    {#each asks as a (a.request_id)}
      <div class="ask">
        <div>
          <div class="ask-title">Allow <b>{a.tool}</b>?</div>
          <code>{a.summary}</code>
        </div>
        <div class="ask-actions">
          <button class="deny" onclick={() => answer(a, false)}>Deny</button>
          <button class="primary" onclick={() => answer(a, true)}>Allow once</button>
        </div>
      </div>
    {/each}

    <section class="feed" bind:this={feedEl}>
      {#if feed.length === 0}
        <p class="empty">The journal of the next run appears here.</p>
      {/if}
      {#each feed as e, i (i)}
        {#if e.kind === "text"}
          <div class="row-ev text">{e.text}</div>
        {:else if e.kind === "tool_use"}
          {@const r = results.get(e.id)}
          {@const d = decisionFor(e.summary)}
          <div class="row-ev tool" class:err={r?.is_error}>
            <span class="tool-name">{e.tool}</span>
            <code class="summary">{e.summary}</code>
            {#if d && d.kind === "permission_auto"}
              <span class="badge" class:no={!d.allowed} title={d.rule}>{d.allowed ? "auto-allowed" : "auto-denied"}</span>
            {:else if d && d.kind === "permission_answered"}
              <span class="badge" class:no={!d.allowed}>{d.allowed ? "allowed" : "denied"} by {d.by}</span>
            {/if}
            {#if r}<div class="result">{r.summary}</div>{/if}
          </div>
        {:else if e.kind === "permission_ask"}
          <div class="row-ev sys">Waiting for you: {e.tool} <code>{e.summary}</code></div>
        {:else if e.kind === "note"}
          <div class="row-ev note">You: {e.text}</div>
        {:else if e.kind === "started"}
          <div class="row-ev sys">Run {e.run_id} started for {e.slug}</div>
        {:else if e.kind === "finished"}
          <div class="row-ev sys">{e.ok ? "Finished" : "Failed"} · ${e.cost_usd.toFixed(4)}</div>
        {:else if e.kind === "error"}
          <div class="row-ev error">{e.message}</div>
        {:else if e.kind === "exited"}
          <div class="row-ev sys">Agent exited ({e.code ?? "killed"})</div>
        {/if}
      {/each}
    </section>

    <form class="note-bar" onsubmit={sendNote}>
      <input bind:value={note} placeholder="Note to the worker, delivered after its current step" disabled={!running} />
      <button type="submit" disabled={!running || !note.trim()}>Send note</button>
    </form>
  </main>
</div>

<style>
  :root {
    --bg: #f6f5f2;
    --panel: #ffffff;
    --ink: #1d1d1b;
    --quiet: #6b6a66;
    --line: #e2e0da;
    --accent: #2f6fdb;
    --good: #2e7d4f;
    --bad: #c0392b;
    --warn-bg: #fff4e0;
    --code-bg: #f0eee9;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    font-size: 14px;
    color: var(--ink);
    background: var(--bg);
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --bg: #161615;
      --panel: #1f1f1d;
      --ink: #ecebe7;
      --quiet: #9a9993;
      --line: #33322f;
      --accent: #6b9cf0;
      --good: #5fbf86;
      --bad: #e0705f;
      --warn-bg: #3a2f1c;
      --code-bg: #2a2a27;
    }
  }
  :global(body) {
    margin: 0;
    background: var(--bg);
  }
  .app {
    display: grid;
    grid-template-columns: 340px 1fr;
    grid-template-rows: auto 1fr;
    height: 100vh;
  }
  header {
    grid-column: 1 / -1;
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    gap: 12px;
    flex-wrap: wrap;
  }
  .brand {
    font-weight: 700;
    font-size: 16px;
  }
  .tag {
    font-weight: 400;
    font-size: 12px;
    color: var(--quiet);
    margin-left: 6px;
  }
  .status {
    display: flex;
    gap: 14px;
    align-items: center;
    font-size: 12px;
    flex-wrap: wrap;
  }
  .pill {
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--line);
  }
  .pill.live {
    border-color: var(--accent);
    color: var(--accent);
  }
  .meta {
    color: var(--quiet);
  }
  .meter {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    color: var(--quiet);
  }
  .bar {
    width: 60px;
    height: 6px;
    background: var(--line);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  aside {
    border-right: 1px solid var(--line);
    background: var(--panel);
    overflow: auto;
  }
  aside form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    height: calc(100% - 32px);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--quiet);
  }
  label.grow {
    flex: 1;
  }
  .picker {
    display: flex;
    gap: 6px;
  }
  .picker input {
    flex: 1;
    min-width: 0;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 110px;
    gap: 8px;
  }
  input,
  select,
  textarea {
    font: inherit;
    color: var(--ink);
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 7px 9px;
  }
  textarea {
    flex: 1;
    min-height: 160px;
    resize: none;
  }
  button {
    font: inherit;
    padding: 7px 14px;
    border-radius: 6px;
    border: 1px solid var(--line);
    background: var(--panel);
    color: var(--ink);
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  button.deny {
    color: var(--bad);
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .hint {
    font-size: 12px;
    color: var(--quiet);
    margin: 0;
  }
  .error {
    color: var(--bad);
    white-space: pre-wrap;
    margin: 0;
  }
  main {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .ask {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin: 12px 16px 0;
    padding: 12px 14px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--warn-bg);
  }
  .ask-title {
    margin-bottom: 4px;
  }
  .ask-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
  .feed {
    flex: 1;
    overflow: auto;
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .empty {
    color: var(--quiet);
  }
  .row-ev {
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    overflow-wrap: anywhere;
  }
  .row-ev.text {
    white-space: pre-wrap;
  }
  .row-ev.sys {
    background: transparent;
    border: none;
    color: var(--quiet);
    font-size: 12px;
  }
  .row-ev.note {
    border-color: var(--accent);
  }
  .row-ev.error {
    color: var(--bad);
    white-space: pre-wrap;
  }
  .tool.err {
    border-color: var(--bad);
  }
  .tool-name {
    font-weight: 600;
    margin-right: 6px;
  }
  .badge {
    font-size: 11px;
    margin-left: 6px;
    color: var(--good);
  }
  .badge.no {
    color: var(--bad);
  }
  .result {
    margin-top: 4px;
    font-size: 12px;
    color: var(--quiet);
    white-space: pre-wrap;
    max-height: 4.5em;
    overflow: hidden;
  }
  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    background: var(--code-bg);
    padding: 1px 4px;
    border-radius: 4px;
  }
  .note-bar {
    display: flex;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
  .note-bar input {
    flex: 1;
  }
</style>
