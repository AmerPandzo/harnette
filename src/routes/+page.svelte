<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import ActionDock from "$lib/ActionDock.svelte";
  import Board from "$lib/Board.svelte";
  import ChatView from "$lib/ChatView.svelte";
  import { chat, initChatEvents, loadChat } from "$lib/chat.svelte";
  import Icon from "$lib/Icon.svelte";
  import NewAnalysis from "$lib/NewAnalysis.svelte";
  import DeliveryCard from "$lib/DeliveryCard.svelte";
  import MaintenanceView from "$lib/MaintenanceView.svelte";
  import PolicyView from "$lib/PolicyView.svelte";
  import RunView from "$lib/RunView.svelte";
  import TaskDetail from "$lib/TaskDetail.svelte";
  import { initRunEvents, onRunExit, run, startLoop, startRun } from "$lib/run.svelte";
  import { remember, remembered, type StatusKind, type TaskSummary } from "$lib/types";

  let repoInput = $state(remembered("kormilant.repo", ""));
  let repo = $state(""); // the watched, resolved repo
  let repoError = $state("");
  let tasks = $state<TaskSummary[]>([]);
  let selected = $state<string | null>(null);
  let version = $state(0);
  let tab = $state<"board" | "run" | "chat" | "policy" | "maintenance">("board");
  /** Task the chat is about ("" = the repo). */
  let chatTask = $state("");

  function openChat() {
    // A fresh chat is about the task open on the board; a saved one keeps its task.
    chatTask = chat.open || chat.events.length ? chat.task : (selected ?? "");
    tab = "chat";
  }
  let recent = $state<string[]>(JSON.parse(remembered("kormilant.recentRepos", "[]")));
  let showNew = $state(false);
  let repoMenu = $state(false);
  const repoName = (p: string) => p.split("/").filter(Boolean).at(-1) ?? p;

  let selectedTask = $derived(tasks.find((t) => t.slug === selected) ?? null);
  /** Slug whose delivery card is open. */
  let delivering = $state<string | null>(null);

  $effect(() => {
    // A loop that ends Done with approval = "ask" opens the card.
    if (run.deliveryReady) {
      delivering = run.deliveryReady.slug;
      run.deliveryReady = null;
    }
  });
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
      remember("kormilant.repo", repo);
      await loadChat(repo);
      recent = [repo, ...recent.filter((r) => r !== repo)].slice(0, 8);
      remember("kormilant.recentRepos", JSON.stringify(recent));
      await refresh();
    } catch (e) {
      repoError = String(e);
    }
  }

  async function chooseRepo() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a repository", defaultPath: repo || undefined });
    if (typeof picked === "string") await connect(picked);
  }

  async function refine(message = "", images: string[] = []) {
    if (!selectedTask) return;
    try {
      await startRun({ repo, slug: selectedTask.slug, mode: "refine", model: remembered("kormilant.model", ""), message, images });
      tab = "run";
    } catch {
      tab = "run"; // the error shows in the Run view
    }
  }

  async function moveTask(slug: string, status: StatusKind) {
    // Show it in the new column right away; the file watcher confirms with the real doc.
    const before = tasks;
    tasks = tasks.map((t) => (t.slug === slug ? { ...t, status_kind: status } : t));
    try {
      await invoke("set_task_status", { repo, slug, status });
      repoError = "";
    } catch (e) {
      tasks = before;
      repoError = String(e);
    }
  }

  async function runTask(base = "") {
    if (!selectedTask) return;
    try {
      await startLoop({ repo, slug: selectedTask.slug, model: remembered("kormilant.model", ""), base });
    } catch {}
    tab = "run";
  }

  onMount(() => {
    initRunEvents();
    initChatEvents();
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
  <aside class="sidebar">
    <div class="traffic" data-tauri-drag-region></div>
    <div class="repo-wrap">
      <button class="repo-btn" onclick={() => (repoMenu = !repoMenu)} title={repo || "Choose a repository"}>
        <span class="repo-icon"><Icon name="folder" size={14} /></span>
        <span class="repo-name">{repo ? repoName(repo) : "Choose repository"}</span>
        <Icon name="chevron" size={13} />
      </button>
      {#if repoMenu}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="menu-scrim" onclick={() => (repoMenu = false)}></div>
        <div class="menu">
          {#each recent as r (r)}
            <button class="menu-item" class:on={r === repo} onclick={() => ((repoMenu = false), connect(r))}>
              <Icon name="folder" size={13} />
              <span class="mi-name">{repoName(r)}</span>
              <span class="mi-path">{r.replace(/^\/Users\/[^/]+/, "~")}</span>
            </button>
          {/each}
          <form class="menu-path" onsubmit={(e) => (e.preventDefault(), (repoMenu = false), connect(repoInput))}>
            <input bind:value={repoInput} placeholder="~/path/to/repo — Enter to open" />
          </form>
          <button class="menu-item" onclick={() => ((repoMenu = false), chooseRepo())}><Icon name="plus" size={13} /> Choose folder…</button>
        </div>
      {/if}
    </div>

    <nav>
      <button class:on={tab === "board"} onclick={() => (tab = "board")}><Icon name="board" size={15} /> Board</button>
      <button class:on={tab === "run"} onclick={() => (tab = "run")}>
        <Icon name="run" size={15} /> Run
        {#if run.running}<span class="live" class:warn={run.asks.length || run.paused}></span>{/if}
      </button>
      <button class:on={tab === "chat"} onclick={openChat} disabled={!repo}>
        <Icon name="chat" size={15} /> Chat
        {#if chat.asks.length}<span class="live warn"></span>{:else if chat.busy}<span class="live"></span>{/if}
      </button>
      <button class:on={tab === "policy"} onclick={() => (tab = "policy")} disabled={!repo}><Icon name="shield" size={15} /> Policy</button>
      <button class:on={tab === "maintenance"} onclick={() => (tab = "maintenance")}>
        <Icon name="gauge" size={15} /> Usage
        {#if run.alarms.length}<span class="badge">{run.alarms.length}</span>{/if}
      </button>
    </nav>

    <button class="new primary" onclick={() => (showNew = true)} disabled={!repo || run.running}><Icon name="plus" size={14} /> New analysis</button>

    <div class="foot">
      {#if run.running}
        <button class="running" onclick={() => (tab = "run")}>
          <span class="live" class:warn={run.asks.length || run.paused}></span>
          <span class="r-slug">{run.slug}</span>
          <span class="r-meta">{run.asks.length ? "needs you" : run.paused ? "paused" : run.role || "running"}</span>
        </button>
      {/if}
      {#if run.quota}
        <div class="meter" title="Claude subscription usage, 5-hour window">
          <span>5h</span><span class="bar"><span style="width: {pct(run.quota.five_hour)}"></span></span><span>{pct(run.quota.five_hour)}</span>
        </div>
        <div class="meter" title="Claude subscription usage, 7-day window">
          <span>7d</span><span class="bar"><span style="width: {pct(run.quota.seven_day)}"></span></span><span>{pct(run.quota.seven_day)}</span>
        </div>
      {/if}
    </div>
  </aside>

  <main>
    <div class="topbar" data-tauri-drag-region>
      <h1 data-tauri-drag-region>
        {({ board: "Board", run: "Run", chat: "Chat", policy: "Policy", maintenance: "Usage" } as const)[tab]}
        {#if tab === "board" && repo}<span class="sub">{tasks.length} task{tasks.length === 1 ? "" : "s"} in <code>.ai-tasks/</code></span>{/if}
        {#if tab === "policy" && repo}<span class="sub"><code>.ai-tasks/policy.toml</code> · checked on every agent tool call</span>{/if}
      </h1>
    </div>

    {#if repoError}<p class="error banner">{repoError}</p>{/if}

    <div class="body">
      {#if tab === "board"}
        {#if !repo}
          <div class="empty">
            <div class="emptyicon"><Icon name="folder" size={22} /></div>
            <h2>Choose a repository</h2>
            <p>Kormilant reads its tasks from <code>.ai-tasks/</code>.</p>
            <button class="primary" onclick={chooseRepo}>Choose folder…</button>
          </div>
        {:else}
          <div class="boardview" class:split={selectedTask}>
            <Board {tasks} {selected} onselect={(s) => (selected = selected === s ? null : s)} onmove={moveTask} />
            {#if selectedTask}
              <TaskDetail
                {repo}
                task={selectedTask}
                {version}
                busy={run.running}
                onrefine={refine}
                onrun={runTask}
                ondeliver={() => (delivering = selectedTask?.slug ?? null)}
                onclose={() => (selected = null)}
              />
            {/if}
          </div>
        {/if}
      {:else if tab === "chat"}
        <ChatView {repo} {tasks} bind:task={chatTask} onopen={(t) => (tab = t)} />
      {:else if tab === "policy"}
        <PolicyView {repo} />
      {:else if tab === "maintenance"}
        <MaintenanceView />
      {:else}
        <RunView {repo} onnew={() => (showNew = true)} />
      {/if}
    </div>
  </main>
</div>

{#if delivering && repo}
  <DeliveryCard {repo} slug={delivering} onclose={() => ((delivering = null), refresh())} />
{/if}
{#if showNew && repo}
  <NewAnalysis {repo} onclose={() => (showNew = false)} onstarted={() => ((showNew = false), (tab = "run"))} />
{/if}
<ActionDock raised={tab === "run"} />

<style>
  .app {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    height: 100vh;
  }
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 10px 12px;
    background: var(--sidebar);
    border-right: 1px solid var(--line);
    min-height: 0;
  }
  .traffic {
    height: 40px;
    flex: none;
  }
  .repo-wrap {
    position: relative;
  }
  .repo-btn {
    width: 100%;
    justify-content: flex-start;
    height: 34px;
    padding: 0 8px;
    background: transparent;
    border-color: transparent;
    box-shadow: none;
  }
  .repo-btn:hover:not(:disabled) {
    background: var(--hover);
  }
  .repo-icon {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: #fff;
  }
  .repo-name {
    flex: 1;
    text-align: left;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .menu-scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .menu {
    position: absolute;
    top: 38px;
    left: 0;
    width: 320px;
    z-index: 31;
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow-lg);
    padding: 5px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .menu-item {
    justify-content: flex-start;
    background: transparent;
    border-color: transparent;
    box-shadow: none;
    height: 30px;
    padding: 0 8px;
    color: var(--ink);
  }
  .menu-item:hover:not(:disabled),
  .menu-item.on {
    background: var(--hover);
  }
  .mi-name {
    font-weight: 500;
  }
  .mi-path {
    margin-left: auto;
    color: var(--faint);
    font-size: 11px;
    font-weight: 400;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 160px;
  }
  .menu-path {
    padding: 4px;
  }
  .menu-path input {
    width: 100%;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-top: 6px;
  }
  nav button {
    justify-content: flex-start;
    gap: 9px;
    height: 30px;
    padding: 0 9px;
    border-color: transparent;
    background: transparent;
    box-shadow: none;
    color: var(--quiet);
  }
  nav button:hover:not(:disabled) {
    background: var(--hover);
    color: var(--ink);
  }
  nav button.on,
  nav button.on:hover:not(:disabled) {
    background: var(--raised);
    color: var(--ink);
    box-shadow: var(--shadow-sm);
    border-color: var(--line);
  }
  .live {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    margin-left: auto;
    animation: pulse 1.4s ease-in-out infinite;
  }
  .live.warn {
    background: var(--warn);
  }
  .badge {
    margin-left: auto;
    font-size: 10.5px;
    min-width: 17px;
    height: 17px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--warn);
    color: #fff;
    display: grid;
    place-items: center;
  }
  .new {
    margin-top: 10px;
  }
  .foot {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .running {
    justify-content: flex-start;
    height: auto;
    padding: 8px 10px;
    gap: 8px;
    text-align: left;
  }
  .running .live {
    margin-left: 0;
  }
  .r-slug {
    font-weight: 600;
  }
  .r-meta {
    margin-left: auto;
    color: var(--quiet);
    font-size: 11.5px;
    font-weight: 400;
  }
  .meter {
    display: grid;
    grid-template-columns: 20px 1fr 32px;
    align-items: center;
    gap: 6px;
    color: var(--quiet);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    padding: 0 4px;
  }
  .meter span:last-child {
    text-align: right;
  }
  .bar {
    height: 5px;
    background: var(--line);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
  }
  main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    background: var(--bg);
  }
  .topbar {
    height: 46px;
    flex: none;
    display: flex;
    align-items: center;
    padding: 6px 22px 0;
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .sub {
    font-size: 12px;
    font-weight: 400;
    color: var(--quiet);
  }
  .banner {
    padding: 8px 22px;
  }
  .body {
    flex: 1;
    min-height: 0;
  }
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--quiet);
    text-align: center;
  }
  .empty h2 {
    margin: 6px 0 0;
    font-size: 16px;
    color: var(--ink);
  }
  .empty p {
    margin: 0 0 10px;
  }
  .emptyicon {
    width: 48px;
    height: 48px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .boardview {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  .boardview.split {
    grid-template-columns: minmax(0, 1fr) minmax(440px, 50%);
  }
</style>
