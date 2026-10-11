<script lang="ts">
  import { remember, remembered } from "./types";
  import { pauseRun, resumeRun, run, sendNote, startRun, stopRun } from "./run.svelte";

  let { repo }: { repo: string } = $props();

  let slug = $state("");
  let model = $state(remembered("kormilant.model", ""));
  let ticket = $state("");
  let note = $state("");
  let feedEl: HTMLElement | undefined = $state();

  // Tool results and decisions are shown on their tool-call row, not as separate rows.
  let results = $derived(new Map(run.events.filter((e) => e.kind === "tool_result").map((e) => [e.tool_use_id, e])));
  let decisions = $derived(
    run.events.filter((e) => e.kind === "permission_auto" || e.kind === "permission_answered"),
  );
  let feed = $derived(run.events.filter((e) => e.kind !== "tool_result" && e.kind !== "quota"));
  const decisionFor = (summary: string) => decisions.findLast((d) => "summary" in d && d.summary === summary);

  $effect(() => {
    void feed.length;
    queueMicrotask(() => feedEl?.scrollTo({ top: feedEl.scrollHeight, behavior: "smooth" }));
  });

  async function analyze(ev: Event) {
    ev.preventDefault();
    remember("kormilant.model", model);
    try {
      await startRun({ repo, slug: slug.trim(), mode: "analyze", ticket, model });
      ticket = "";
    } catch {}
  }

  async function submitNote(ev: Event) {
    ev.preventDefault();
    if (!note.trim()) return;
    try {
      await sendNote(note.trim());
      note = "";
    } catch (e) {
      run.error = String(e);
    }
  }
</script>

<div class="runview">
  <aside>
    <form onsubmit={analyze}>
      <h3>New analysis <span class="in">in {repo.split("/").filter(Boolean).at(-1) ?? "no repository"}</span></h3>
      <div class="row">
        <label>Task slug<input bind:value={slug} placeholder="OAM-12345" required disabled={run.running} /></label>
        <label
          >Model
          <select bind:value={model} disabled={run.running}>
            <option value="">Default</option>
            <option value="haiku">Haiku</option>
            <option value="sonnet">Sonnet</option>
            <option value="opus">Opus</option>
          </select>
        </label>
      </div>
      <label class="grow">Ticket<textarea bind:value={ticket} placeholder="Paste the ticket text" required disabled={run.running}></textarea></label>
      <div class="actions">
        <button type="submit" class="primary" disabled={run.running || !repo}>Analyze</button>
        {#if run.paused}
          <button type="button" onclick={resumeRun}>Resume</button>
        {:else}
          <button type="button" onclick={pauseRun} disabled={!run.running}>Pause</button>
        {/if}
        <button type="button" onclick={stopRun} disabled={!run.running}>Stop</button>
      </div>
      {#if run.error}<p class="error">{run.error}</p>{/if}
      <p class="hint">
        Analysis only: the agent may read the repo and write <code>.ai-tasks/&lt;slug&gt;.md</code>. Unlisted actions ask you.
      </p>
    </form>
  </aside>

  <main>
    <section class="feed" bind:this={feedEl}>
      {#if feed.length === 0}
        <p class="empty">The journal of the next run appears here.</p>
      {/if}
      {#each feed as e, i (i)}
        {#if e.kind === "text"}
          <div class="ev text">{e.text}</div>
        {:else if e.kind === "tool_use"}
          {@const r = results.get(e.id)}
          {@const d = decisionFor(e.summary)}
          <div class="ev tool" class:err={r?.is_error}>
            <span class="tool-name">{e.tool}</span>
            <code>{e.summary}</code>
            {#if d && d.kind === "permission_auto"}
              <span class="badge" class:no={!d.allowed} title={d.rule}>{d.allowed ? "auto-allowed" : "auto-denied"}</span>
            {:else if d && d.kind === "permission_answered"}
              <span class="badge" class:no={!d.allowed}>{d.allowed ? "allowed" : "denied"} by {d.by}</span>
            {/if}
            {#if r}<div class="result">{r.summary}</div>{/if}
          </div>
        {:else if e.kind === "permission_ask"}
          <div class="ev sys">Waiting for you: {e.tool} <code>{e.summary}</code></div>
        {:else if e.kind === "note"}
          <div class="ev note">You: {e.text}</div>
        {:else if e.kind === "started"}
          <div class="ev sys">Agent run {e.run_id} started for {e.slug}</div>
        {:else if e.kind === "finished"}
          <div class="ev sys">{e.ok ? "Finished" : "Failed"} · ${e.cost_usd.toFixed(4)}</div>
        {:else if e.kind === "error"}
          <div class="ev error">{e.message}</div>
        {:else if e.kind === "exited"}
          <div class="ev sys">Agent exited ({e.code ?? "killed"})</div>
        {:else if e.kind === "phase"}
          <div class="phase">Round {e.round} · {e.role === "implementer" ? "Implementer" : "Reviewer"}</div>
        {:else if e.kind === "checkpoint"}
          <div class="ev sys">Checkpoint {e.round} saved <code>{e.git_ref}</code></div>
        {:else if e.kind === "build"}
          <details class="ev build" class:err={!e.ok}>
            <summary>
              Build <code>{e.command}</code> <b class:no={!e.ok}>{e.ok ? "green" : "red"}</b> · {(e.duration_ms / 1000).toFixed(1)}s
            </summary>
            <pre>{e.tail}</pre>
          </details>
        {:else if e.kind === "status_changed"}
          <div class="ev sys">Status → <b>{e.to}</b> by {e.by}{e.reason ? `: ${e.reason}` : ""}</div>
        {:else if e.kind === "alarm"}
          <div class="ev alarm" class:stop={e.action !== "warn"}>⚠ {e.message}{e.action !== "warn" ? ` → ${e.action}` : ""}</div>
        {:else if e.kind === "paused"}
          <div class="ev sys">Paused: {e.reason}</div>
        {:else if e.kind === "resumed"}
          <div class="ev sys">Resumed</div>
        {:else if e.kind === "info"}
          <div class="ev sys">{e.message}</div>
        {:else if e.kind === "delivery_ready"}
          <div class="ev note">Ready to deliver ({e.mode} to <code>{e.branch}</code>, {e.files.length} file{e.files.length === 1 ? "" : "s"}): approve in the banner or from the task.</div>
        {:else if e.kind === "delivered"}
          <div class="ended">Delivered to <code>{e.branch}</code>{e.commit ? ` · ${e.commit.slice(0, 8)}` : ""}{e.pushed ? " · pushed" : ""}{e.pr_url ? ` · ${e.pr_url}` : ""}</div>
        {:else if e.kind === "session_ended"}
          <div class="ended" class:err={!e.ok}>{e.ok ? "✓" : "■"} {e.summary}</div>
        {/if}
      {/each}
    </section>

    <form class="note-bar" onsubmit={submitNote}>
      <input bind:value={note} placeholder="Note to the worker, delivered after its current step" disabled={!run.running} />
      <button type="submit" disabled={!run.running || !note.trim()}>Send note</button>
    </form>
  </main>
</div>

<style>
  .runview {
    display: grid;
    grid-template-columns: 320px 1fr;
    height: 100%;
    min-height: 0;
  }
  aside {
    border-right: 1px solid var(--line);
    background: var(--panel);
    overflow: auto;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    box-sizing: border-box;
    height: 100%;
  }
  h3 {
    margin: 0;
    font-size: 14px;
  }
  .in {
    font-weight: 400;
    color: var(--quiet);
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
  .row {
    display: grid;
    grid-template-columns: 1fr 100px;
    gap: 8px;
  }
  textarea {
    flex: 1;
    min-height: 140px;
    resize: none;
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
  main {
    display: flex;
    flex-direction: column;
    min-height: 0;
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
  .ev {
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--panel);
    border: 1px solid var(--line);
    overflow-wrap: anywhere;
  }
  .ev.text {
    white-space: pre-wrap;
  }
  .ev.sys {
    background: transparent;
    border: none;
    color: var(--quiet);
    font-size: 12px;
  }
  .ev.note {
    border-color: var(--accent);
  }
  .ev.error {
    color: var(--bad);
    white-space: pre-wrap;
  }
  .tool.err {
    border-color: var(--bad);
  }
  .phase {
    margin-top: 10px;
    font-weight: 600;
    font-size: 13px;
    border-bottom: 1px solid var(--line);
    padding-bottom: 4px;
  }
  .build pre {
    margin: 6px 0 0;
    font-size: 11.5px;
    white-space: pre-wrap;
    color: var(--quiet);
  }
  .build summary {
    cursor: pointer;
  }
  .build.err {
    border-color: var(--bad);
  }
  b.no {
    color: var(--bad);
  }
  .ev.alarm {
    border-color: #d99a2b;
    background: var(--warn-bg);
  }
  .ev.alarm.stop {
    border-color: var(--bad);
  }
  .ended {
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--accent-soft);
    font-weight: 600;
  }
  .ended.err {
    background: var(--warn-bg);
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
  .note-bar {
    flex-direction: row;
    height: auto;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid var(--line);
    background: var(--panel);
  }
  .note-bar input {
    flex: 1;
  }
</style>
