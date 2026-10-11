<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { run } from "./run.svelte";
  import { fmtTokens, type UsageSummary } from "./types";

  let summary = $state<UsageSummary | null>(null);
  let now = $state(Date.now());

  const load = () => invoke<UsageSummary>("usage_summary").then((s) => (summary = s));
  const pct = (x: number) => `${Math.round(x * 100)}%`;
  const elapsed = (ms: number) => {
    const s = Math.floor(ms / 1000);
    return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  };
  const name = (p: string) => p.split("/").filter(Boolean).at(-1) ?? p;

  $effect(() => {
    // Reload totals whenever a session ends.
    void run.ended;
    load();
  });

  onMount(() => {
    const t = setInterval(() => {
      now = Date.now();
      if (run.running) load();
    }, 5000);
    const clock = setInterval(() => (now = Date.now()), 1000);
    return () => {
      clearInterval(t);
      clearInterval(clock);
    };
  });
</script>

<div class="maint">
  <section class="card">
    <h3>Claude subscription</h3>
    {#if run.quota}
      <div class="meter">
        <span>5-hour window</span>
        <span class="bar" class:hot={run.quota.five_hour >= 0.9}><span style="width: {pct(run.quota.five_hour)}"></span></span>
        <b>{pct(run.quota.five_hour)}</b>
      </div>
      <div class="meter">
        <span>7-day window</span>
        <span class="bar" class:hot={run.quota.seven_day >= 0.9}><span style="width: {pct(run.quota.seven_day)}"></span></span>
        <b>{pct(run.quota.seven_day)}</b>
      </div>
      <p class="meta">Status: {run.quota.status || "unknown"} · updated by the last run</p>
    {:else}
      <p class="meta">Shown after the first run of this session.</p>
    {/if}
  </section>

  <section class="card">
    <h3>On shift</h3>
    {#if run.running}
      <dl>
        <dt>Task</dt><dd><code>{run.slug}</code> · {run.mode === "loop" ? "loop" : run.mode}</dd>
        <dt>Worker</dt><dd>Claude Code{run.model ? ` (${run.model})` : ""} as {run.role || "starting"}</dd>
        <dt>Elapsed</dt><dd>{elapsed(now - run.startedAt)}</dd>
        <dt>Tokens</dt><dd>{fmtTokens(run.tokens)} new</dd>
        <dt>Cost</dt><dd>${run.usd.toFixed(4)} <span class="meta">(finished agent runs)</span></dd>
        <dt>State</dt><dd>{run.paused ? `Paused: ${run.paused}` : run.asks.length ? "Waiting for you" : "Working"}</dd>
      </dl>
    {:else}
      <p class="meta">No worker on shift.{run.ended ? ` Last: ${run.ended.summary}` : ""}</p>
    {/if}
  </section>

  <section class="card">
    <h3>Usage <span class="meta">(all repositories, API-equivalent cost)</span></h3>
    {#if summary}
      <table>
        <thead><tr><th></th><th>Runs</th><th>New tokens</th><th>Cost</th></tr></thead>
        <tbody>
          <tr><td>Last 24 h</td><td>{summary.day_runs}</td><td>{fmtTokens(summary.day_tokens)}</td><td>${summary.day_usd.toFixed(2)}</td></tr>
          <tr><td>Last 7 days</td><td>{summary.week_runs}</td><td>{fmtTokens(summary.week_tokens)}</td><td>${summary.week_usd.toFixed(2)}</td></tr>
        </tbody>
      </table>
      {#if summary.top_tasks.length}
        <h4>Most expensive tasks, last 7 days</h4>
        <table>
          <tbody>
            {#each summary.top_tasks as t (t.repo + t.slug)}
              <tr><td><code>{t.slug}</code> <span class="meta">{name(t.repo)}</span></td><td>{fmtTokens(t.new_tokens)}</td><td>${t.usd.toFixed(2)}</td></tr>
            {/each}
          </tbody>
        </table>
      {/if}
    {/if}
  </section>

  <section class="card">
    <h3>Alarms <span class="meta">(this app session)</span></h3>
    {#each [...run.alarms].reverse() as a (a.at + a.message)}
      <p class="alarm" class:stop={a.action !== "warn"}>
        <span class="meta">{new Date(a.at).toLocaleTimeString()}</span> {a.message} <b>{a.action}</b>
      </p>
    {:else}
      <p class="meta">None. Budgets are set under [budget] in the Policy tab.</p>
    {/each}
  </section>
</div>

<style>
  .maint {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 12px;
    padding: 16px;
    overflow: auto;
    height: calc(100% - 32px);
    align-content: start;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 12px 14px;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 14px;
  }
  h4 {
    margin: 12px 0 4px;
    font-size: 12px;
    color: var(--quiet);
    font-weight: 600;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
    font-weight: 400;
  }
  .meter {
    display: grid;
    grid-template-columns: 110px 1fr 44px;
    gap: 8px;
    align-items: center;
    margin-bottom: 6px;
    font-size: 13px;
  }
  .bar {
    height: 8px;
    background: var(--line);
    border-radius: 4px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .bar.hot span {
    background: var(--bad);
  }
  dl {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 4px 10px;
    margin: 0;
    font-size: 13px;
  }
  dt {
    color: var(--quiet);
  }
  dd {
    margin: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  th {
    text-align: left;
    color: var(--quiet);
    font-weight: 400;
    font-size: 12px;
  }
  td,
  th {
    padding: 3px 4px;
  }
  .alarm {
    margin: 0 0 6px;
    font-size: 13px;
  }
  .alarm.stop b {
    color: var(--bad);
  }
</style>
