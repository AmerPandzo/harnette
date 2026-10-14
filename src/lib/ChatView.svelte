<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import Attachments from "./Attachments.svelte";
  import Icon from "./Icon.svelte";
  import { segments, type ChatAction } from "./actions";
  import { upload, savedPreview, type Pending } from "./attach";
  import { chat, endChat, interruptChat, sendChat } from "./chat.svelte";
  import { buildTimeline, shortPath, toolIcon, toolVerb } from "./feed";
  import { answer, run, startLoop, startRun } from "./run.svelte";
  import { remembered, STATUS_LABELS, type TaskSummary } from "./types";

  let {
    repo,
    tasks,
    task = $bindable(""),
    onopen,
  }: { repo: string; tasks: TaskSummary[]; task?: string; onopen: (tab: "run" | "board") => void } = $props();

  let text = $state("");
  let images = $state<Pending[]>([]);
  let composer = $state<HTMLElement>();
  let attach = $state<Attachments>();
  let feedEl: HTMLElement | undefined = $state();
  let error = $state("");
  /** Actions already clicked, by position in the feed. */
  let done = $state<Record<string, boolean>>({});

  const sections = $derived(buildTimeline(chat.events, "chat"));
  const md = (s: string) => DOMPurify.sanitize(marked.parse(s, { async: false }) as string);
  const short = (s: string) => shortPath(s, repo);
  const known = (slug: string) => tasks.some((t) => t.slug === slug);
  const slug = $derived(task || "_chat");

  $effect(() => {
    void chat.events.length;
    queueMicrotask(() => feedEl?.scrollTo({ top: feedEl.scrollHeight, behavior: "smooth" }));
  });

  async function send(ev?: Event) {
    ev?.preventDefault();
    if (!text.trim() && !images.length) return;
    try {
      const paths = await upload(repo, slug, images);
      const message = text.trim() || "See the attached image.";
      text = "";
      images = [];
      error = "";
      await sendChat({ repo, task, text: message, images: paths, model: remembered("kormilant.model", "") });
    } catch (e) {
      error = String(e);
    }
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  async function about(next: string) {
    // Another task is another conversation.
    if (chat.open || chat.events.length) await endChat();
    task = next;
  }

  function label(a: ChatAction): string {
    switch (a.action) {
      case "analyze":
        return `Start analysis ${a.slug}`;
      case "refine":
        return `Refine ${a.slug}`;
      case "run":
        return `Run ${a.slug} (implement → build → review)`;
      case "move":
        return `Move ${a.slug} to ${STATUS_LABELS[a.status]}`;
    }
  }
  /** Why the button is disabled, or "" when it can run. */
  function blocked(a: ChatAction): string {
    if (a.action === "analyze" && known(a.slug)) return `${a.slug} already exists`;
    if (a.action !== "analyze" && !known(a.slug)) return `no task ${a.slug}`;
    if (a.action !== "move" && run.running) return "a run is in progress";
    return "";
  }
  async function act(key: string, a: ChatAction) {
    const model = remembered("kormilant.model", "");
    try {
      if (a.action === "move") {
        await invoke("set_task_status", { repo, slug: a.slug, status: a.status });
      } else if (a.action === "run") {
        await startLoop({ repo, slug: a.slug, model });
      } else if (a.action === "analyze") {
        await startRun({ repo, slug: a.slug, mode: "analyze", ticket: a.ticket, model });
      } else {
        await startRun({ repo, slug: a.slug, mode: "refine", message: a.message, model });
      }
      done[key] = true;
      onopen(a.action === "move" ? "board" : "run");
    } catch (e) {
      error = String(e);
    }
  }
</script>

<div class="chatview">
  <div class="bar">
    <label class="about"
      >About
      <select value={task} onchange={(e) => about(e.currentTarget.value)}>
        <option value="">the repository</option>
        {#each tasks as t (t.slug)}<option value={t.slug}>{t.slug} — {t.title.replace(new RegExp(`^${t.slug}\\s*[—:-]\\s*`), "")}</option>{/each}
      </select>
    </label>
    <span class="meta">
      {#if chat.model}{chat.model} · {/if}{#if chat.usd}${chat.usd.toFixed(3)}{/if}
    </span>
    <button class="ghost" onclick={endChat} disabled={!chat.open && !chat.events.length} title="End this conversation and start fresh"
      ><Icon name="plus" size={13} /> New chat</button
    >
  </div>

  <div class="feed" bind:this={feedEl}>
    {#if !chat.events.length && !chat.busy}
      <div class="empty">
        <div class="emptyicon"><Icon name="chat" size={22} /></div>
        <h2>Talk to Claude</h2>
        <p>
          Ask about {task ? `task ${task}` : "the repository"}, think through a fix, or let it write a task doc. It reads freely, edits
          task docs, and asks you before anything else. Board actions it suggests appear as buttons.
        </p>
      </div>
    {/if}
    {#each sections as s, si (si)}
      {#if si > 0 && s.kind === "agent"}<div class="mark center">Reconnected: a new Claude process continues the conversation</div>{/if}
      {#each s.items as it, ii (ii)}
        {#if it.t === "note"}
          <div class="you">
            {it.text}
            {#if it.images.length}
              <div class="youimgs">
                {#each it.images as path (path)}
                  {#await savedPreview(repo, path) then src}<img {src} alt={path} title={path} />{:catch}<span>{path}</span>{/await}
                {/each}
              </div>
            {/if}
          </div>
        {:else if it.t === "text"}
          {#each segments(it.text) as seg, gi (gi)}
            {@const key = `${si}-${ii}-${gi}`}
            {#if seg.t === "md"}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized -->
              <div class="prose msg">{@html md(seg.text)}</div>
            {:else if seg.t === "action"}
              {@const why = blocked(seg.action)}
              <div class="action">
                <span class="aicon"><Icon name={seg.action.action === "move" ? "board" : "run"} size={13} /></span>
                <div class="abody">
                  <b>{label(seg.action)}</b>
                  {#if seg.action.action === "analyze"}<span class="adetail">{seg.action.ticket}</span>{/if}
                  {#if seg.action.action === "refine" && seg.action.message}<span class="adetail">{seg.action.message}</span>{/if}
                </div>
                {#if done[key]}
                  <span class="meta"><Icon name="check" size={12} /> Started</span>
                {:else}
                  <button class="primary small" disabled={!!why} title={why || "Runs only now, on your click"} onclick={() => act(key, seg.action)}
                    >{seg.action.action === "move" ? "Move" : "Start"}</button
                  >
                {/if}
              </div>
            {:else}
              <div class="callout"><Icon name="alert" size={14} /> Ignored a malformed action ({seg.why}): <code>{seg.raw}</code></div>
            {/if}
          {/each}
        {:else if it.t === "tool" || it.t === "explore"}
          {@const tools = it.t === "tool" ? [it] : it.tools}
          {@const t = tools[0]}
          <div class="tool" class:err={t.result?.is_error || t.decision?.allowed === false} class:waiting={t.waiting} title={t.result?.summary ?? ""}>
            <span class="ticon"><Icon name={it.t === "explore" ? "search" : toolIcon(t.tool)} size={13} /></span>
            <span class="verb">{it.t === "explore" ? "Explored" : toolVerb(t.tool)}</span>
            <span class="target"
              >{it.t === "explore"
                ? `${tools.length} items · ${tools.slice(0, 3).map((x) => short(x.summary).split("/").at(-1)).join(", ")}`
                : short(t.summary)}</span
            >
            {#if t.waiting}<span class="right warn">waiting for you</span>
            {:else if t.decision?.allowed === false}<span class="right bad">denied{t.decision.rule ? `: ${t.decision.rule}` : ""}</span>{/if}
          </div>
        {:else if it.t === "alarm"}
          <div class="callout" class:bad={it.e.action !== "warn"}><Icon name="alert" size={14} /> {it.e.message}</div>
        {:else if it.t === "error"}
          <div class="callout bad"><Icon name="alert" size={14} /> {it.text}</div>
        {/if}
      {/each}
      {#if s.exited && s.kind === "agent"}<div class="mark center">Claude's process ended. Your next message continues the conversation.</div>{/if}
    {/each}
    {#if chat.busy}<div class="mark"><span class="spinner"></span> Claude is answering…</div>{/if}
    {#each chat.asks as a (a.request_id)}
      <div class="ask">
        <div><Icon name={toolIcon(a.tool)} size={14} /> Allow <b>{a.tool}</b>?</div>
        <pre>{a.summary}</pre>
        <div class="aactions">
          <button class="danger" onclick={() => answer(a, false)}><Icon name="x" size={13} /> Deny</button>
          <button class="primary" onclick={() => answer(a, true)}><Icon name="check" size={13} /> Allow once</button>
        </div>
      </div>
    {/each}
    {#if chat.error || error}<div class="callout bad"><Icon name="alert" size={14} /> {chat.error || error}</div>{/if}
  </div>

  <form class="composer" onsubmit={send} bind:this={composer}>
    <div class="cbody">
      <Attachments bind:this={attach} bind:items={images} zone={composer} disabled={!repo} compact />
      <textarea
        rows="1"
        bind:value={text}
        onkeydown={onKey}
        onpaste={(e) => attach?.paste(e)}
        placeholder={repo ? "Message Claude (Enter to send, Shift+Enter for a new line)" : "Choose a repository first"}
        disabled={!repo}
      ></textarea>
    </div>
    <button type="button" class="ghost send" onclick={() => attach?.pick()} disabled={!repo} title="Attach images (or paste / drop them)"
      ><Icon name="paperclip" size={15} /></button
    >
    {#if chat.busy}
      <button type="button" class="send" onclick={interruptChat} title="Stop this answer (the chat stays open)"><Icon name="stop" size={12} /></button>
    {/if}
    <button type="submit" class="primary send" disabled={!repo || (!text.trim() && !images.length)} title="Send"><Icon name="send" size={14} /></button>
  </form>
</div>

<style>
  .chatview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .about {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--quiet);
    font-size: 12px;
    min-width: 0;
    flex: 1;
  }
  .about select {
    max-width: 420px;
    min-width: 0;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .feed {
    flex: 1;
    overflow: auto;
    padding: 16px 22px 28px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .feed > :global(*) {
    flex-shrink: 0;
  }
  .empty {
    margin: auto;
    text-align: center;
    max-width: 420px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    color: var(--quiet);
  }
  .empty h2 {
    margin: 6px 0 0;
    font-size: 16px;
    color: var(--ink);
  }
  .emptyicon {
    width: 48px;
    height: 48px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    background: var(--code-bg);
  }
  .msg {
    padding: 2px 2px;
  }
  .you {
    align-self: flex-end;
    max-width: 80%;
    background: var(--accent);
    color: #fff;
    padding: 7px 12px;
    border-radius: 12px 12px 4px 12px;
    white-space: pre-wrap;
    margin: 6px 0 2px;
  }
  .youimgs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 6px;
  }
  .youimgs img {
    max-width: 220px;
    max-height: 160px;
    border-radius: 6px;
    display: block;
  }
  .tool {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 6px;
    border-radius: 7px;
    min-width: 0;
    font-size: 12.5px;
  }
  .tool.waiting {
    background: var(--warn-soft);
  }
  .ticon {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    display: grid;
    place-items: center;
    flex: none;
    background: var(--code-bg);
    color: var(--quiet);
  }
  .tool.err .ticon {
    background: var(--bad-soft);
    color: var(--bad);
  }
  .verb {
    font-weight: 600;
    flex: none;
  }
  .target {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--quiet);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .right {
    margin-left: auto;
    flex: none;
    font-size: 11.5px;
    max-width: 50%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .right.warn {
    color: var(--warn);
  }
  .right.bad {
    color: var(--bad);
  }
  .action {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-left: 3px solid var(--accent);
    border-radius: 10px;
    background: var(--panel);
    box-shadow: var(--shadow-sm);
  }
  .aicon {
    color: var(--accent);
    display: grid;
  }
  .abody {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .adetail {
    color: var(--quiet);
    font-size: 12px;
    white-space: pre-wrap;
    max-height: 4.5em;
    overflow: hidden;
  }
  .ask {
    border: 1px solid var(--line);
    border-left: 3px solid var(--warn);
    border-radius: 10px;
    background: var(--panel);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .ask pre {
    margin: 0;
    white-space: pre-wrap;
    font-size: 12px;
  }
  .aactions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .callout {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--warn-soft);
  }
  .callout.bad {
    background: var(--bad-soft);
  }
  .mark {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--quiet);
    font-size: 12px;
    padding: 2px 4px;
  }
  .mark.center {
    justify-content: center;
  }
  .spinner {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, currentColor 25%, transparent);
    border-top-color: currentColor;
    animation: spin 0.8s linear infinite;
    color: var(--accent);
    display: inline-block;
  }
  .composer {
    display: flex;
    gap: 8px;
    align-items: flex-end;
    margin: 0 22px 16px;
    padding: 8px 8px 8px 12px;
    border: 1px solid var(--line-strong);
    border-radius: 12px;
    background: var(--panel);
    box-shadow: var(--shadow-md);
  }
  .composer:focus-within {
    border-color: var(--accent);
  }
  .cbody {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .composer textarea {
    flex: 1;
    border: none;
    box-shadow: none;
    padding: 4px 0;
    resize: none;
    max-height: 160px;
    field-sizing: content;
    background: transparent;
  }
  .send {
    flex: none;
    width: 32px;
    height: 32px;
    padding: 0;
    border-radius: 8px;
  }
</style>
