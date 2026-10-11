<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import Board from "$lib/Board.svelte";
  import RunView from "$lib/RunView.svelte";
  import TaskDetail from "$lib/TaskDetail.svelte";
  import { answer, initRunEvents, onRunExit, run, startRun } from "$lib/run.svelte";
  import { remember, remembered, type TaskSummary } from "$lib/types";

  let repoInput = $state(remembered("harness.repo", ""));
  let repo = $state(""); // the watched, resolved repo
  let repoError = $state("");
  let tasks = $state<TaskSummary[]>([]);
  let selected = $state<string | null>(null);
  let version = $state(0);
  let tab = $state<"board" | "run">("board");
  let recent = $state<string[]>(JSON.parse(remembered("harness.recentRepos", "[]")));
  const repoName = (p: string) => p.split("/").filter(Boolean).at(-1) ?? p;

  let selectedTask = $derived(tasks.find((t) => t.slug === selected) ?? null);
  const pct = (x: number) => `${Math.round(x * 100)}%`;

  async function refresh() {
    if (!repo) return;
    try {
      tasks = await invoke<TaskSummary[]>("list_tasks", { repo });
      version++;
    } catch (e) {
      repoError = String(e);
    }
  }

  async function connect(path: string) {
    if (!path.trim()) return;
    try {
      repo = await invoke<string>("watch_repo", { repo: path });
      repoInput = repo;
      repoError = "";
      selected = null;
      remember("harness.repo", repo);
      recent = [repo, ...recent.filter((r) => r !== repo)].slice(0, 8);
      remember("harness.recentRepos", JSON.stringify(recent));
      await refresh();
    } catch (e) {
      repoError = String(e);
    }
  }

  async function chooseRepo() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a repository", defaultPath: repo || undefined });
    if (typeof picked === "string") await connect(picked);
  }

  async function refine() {
    if (!selectedTask) return;
    try {
      await startRun({ repo, slug: selectedTask.slug, mode: "refine", model: remembered("harness.model", "") });
      tab = "run";
    } catch {
      tab = "run"; // the error shows in the Run view
    }
  }

  onMount(() => {
    initRunEvents();
    onRunExit(refresh);
    // Editors and agents write in bursts; coalesce into one refresh.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unlisten = listen("tasks-changed", () => {
      clearTimeout(timer);
      timer = setTimeout(refresh, 150);
    });
    if (repoInput) connect(repoInput);
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="app">
  <header>
    <div class="brand">Harness</div>
    <form class="repo" onsubmit={(e) => (e.preventDefault(), connect(repoInput))}>
      <input bind:value={repoInput} placeholder="Choose a repository or type ~/path" />
      <button type="button" onclick={chooseRepo}>Choose…</button>
      {#if recent.length > 1}
        <select
          title="Recent repositories"
          value=""
          onchange={(e) => {
            const v = (e.target as HTMLSelectElement).value;
            (e.target as HTMLSelectElement).value = "";
            if (v) connect(v);
          }}
        >
          <option value="" disabled>Recent</option>
          {#each recent.filter((r) => r !== repo) as r (r)}
            <option value={r}>{repoName(r)}</option>
          {/each}
        </select>
      {/if}
    </form>
    <nav>
      <button class:on={tab === "board"} onclick={() => (tab = "board")}>Board</button>
      <button class:on={tab === "run"} onclick={() => (tab = "run")}>
        Run{#if run.running}<span class="dot" title="Running"></span>{/if}
      </button>
    </nav>
    <div class="status">
      {#if run.quota}
        <span class="meter" title="Claude subscription usage, 5-hour window">
          5h <span class="bar"><span style="width: {pct(run.quota.five_hour)}"></span></span>
          {pct(run.quota.five_hour)}
        </span>
        <span class="meter" title="Claude subscription usage, 7-day window">
          7d <span class="bar"><span style="width: {pct(run.quota.seven_day)}"></span></span>
          {pct(run.quota.seven_day)}
        </span>
      {/if}
      {#if run.finished}
        <span class="meta" title="Last run">${run.finished.cost_usd.toFixed(4)} · {(run.finished.duration_ms / 1000).toFixed(0)}s</span>
      {/if}
    </div>
  </header>

  {#if repoError}<p class="error banner">{repoError}</p>{/if}

  {#each run.asks as a (a.request_id)}
    <div class="ask">
      <div>
        <div>{run.slug}: allow <b>{a.tool}</b>?</div>
        <code>{a.summary}</code>
      </div>
      <div class="ask-actions">
        <button class="deny" onclick={() => answer(a, false)}>Deny</button>
        <button class="primary" onclick={() => answer(a, true)}>Allow once</button>
      </div>
    </div>
  {/each}

  <div class="body">
    {#if tab === "board"}
      {#if !repo}
        <p class="empty">Choose a repository to see its tasks.</p>
      {:else}
        <div class="boardview" class:split={selectedTask}>
          <div class="boardpane">
            <div class="boardbar">
              <span class="where">
                <b>{repoName(repo)}</b>
                <span class="meta">{tasks.length} task{tasks.length === 1 ? "" : "s"} in <code>.ai-tasks/</code></span>
              </span>
              <button class="primary" onclick={() => (tab = "run")}>New analysis</button>
            </div>
            <Board {tasks} {selected} onselect={(s) => (selected = selected === s ? null : s)} />
          </div>
          {#if selectedTask}
            <TaskDetail {repo} task={selectedTask} {version} busy={run.running} onrefine={refine} onclose={() => (selected = null)} />
          {/if}
        </div>
      {/if}
    {:else}
      <RunView {repo} />
    {/if}
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex-wrap: wrap;
  }
  .brand {
    font-weight: 700;
    font-size: 16px;
  }
  .repo {
    display: flex;
    gap: 6px;
    flex: 1;
    min-width: 260px;
    max-width: 520px;
  }
  .repo input {
    flex: 1;
    min-width: 0;
    padding: 5px 8px;
  }
  nav {
    display: flex;
    gap: 4px;
  }
  nav button {
    border-color: transparent;
    background: transparent;
  }
  nav button.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-left: 6px;
    border-radius: 50%;
    background: var(--accent);
    vertical-align: middle;
  }
  .status {
    display: flex;
    gap: 14px;
    align-items: center;
    font-size: 12px;
    margin-left: auto;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
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
  .banner {
    padding: 8px 16px;
  }
  .ask {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin: 10px 16px 0;
    padding: 10px 14px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--warn-bg);
  }
  .ask-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
  .body {
    flex: 1;
    min-height: 0;
  }
  .empty {
    color: var(--quiet);
    padding: 24px 16px;
  }
  .boardview {
    display: grid;
    grid-template-columns: 1fr;
    height: 100%;
  }
  .boardview.split {
    grid-template-columns: minmax(0, 1fr) minmax(380px, 46%);
  }
  .boardpane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .where {
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 15px;
  }
  .repo select {
    padding: 5px 6px;
    max-width: 120px;
  }
  .boardbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 16px 0;
  }
</style>
