<script lang="ts">
  import DOMPurify from "dompurify";
  import { marked } from "marked";
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import { buildTimeline, fmtDuration, shortPath, toolIcon, toolVerb, type Section, type ToolItem } from "./feed";
  import { pauseRun, resumeRun, run, sendNote, stopRun } from "./run.svelte";
  import Attachments from "./Attachments.svelte";
  import { savedPreview, upload, type Pending } from "./attach";
  import { fmtTokens } from "./types";

  let { repo, onnew }: { repo: string; onnew: () => void } = $props();

  let note = $state("");
  let noteImages = $state<Pending[]>([]);
  let composer = $state<HTMLElement>();
  let attach = $state<Attachments>();
  let feedEl: HTMLElement | undefined = $state();
  let open = $state<Record<string, boolean>>({});
  let now = $state(Date.now());
  let stick = true;

  let sections = $derived(buildTimeline(run.events, run.mode === "loop" ? "" : "analyst"));

  // Agent-written markdown: sanitize before it reaches the webview, which can call Tauri commands.
  const md = (s: string) => DOMPurify.sanitize(marked.parse(s, { async: false }) as string);
  const short = (s: string) => shortPath(s, repo);

  const ROLE: Record<string, { label: string; icon: string; color: string }> = {
    analyst: { label: "Analyst", icon: "sparkles", color: "var(--role-analyst)" },
    implementer: { label: "Implementer", icon: "edit", color: "var(--role-implementer)" },
    reviewer: { label: "Reviewer", icon: "eye", color: "var(--role-reviewer)" },
  };
  const role = (r: string) => ROLE[r] ?? { label: r || "Agent", icon: "zap", color: "var(--accent)" };
  const modeLabel = (m: string) => ({ analyze: "Analysis", refine: "Refine", loop: "Implement → build → review" })[m] ?? m;

  /** The trail of steps at the top: each agent run and build, in order. */
  let trail = $derived.by(() => {
    const out: { label: string; icon: string; state: "done" | "active" | "bad"; color: string }[] = [];
    for (const s of sections) {
      if (s.kind === "agent") {
        const r = role(s.role);
        out.push({
          label: r.label + (s.round ? ` ${s.round}` : ""),
          icon: r.icon,
          color: r.color,
          state: !s.exited ? "active" : s.finished?.ok === false ? "bad" : "done",
        });
      }
      for (const it of s.items)
        if (it.t === "build") out.push({ label: "Build", icon: "hammer", color: "var(--quiet)", state: it.e.ok ? "done" : "bad" });
    }
    return out;
  });

  let status = $derived(
    run.running
      ? run.paused
        ? { label: "Paused", cls: "warn" }
        : run.asks.length
          ? { label: "Waiting for you", cls: "warn" }
          : { label: "Running", cls: "accent" }
      : run.ended
        ? run.ended.ok
          ? { label: "Finished", cls: "good" }
          : { label: "Stopped", cls: "bad" }
        : run.error
          ? { label: "Failed to start", cls: "bad" }
          : null,
  );

  function onScroll() {
    if (!feedEl) return;
    stick = feedEl.scrollHeight - feedEl.scrollTop - feedEl.clientHeight < 80;
  }
  $effect(() => {
    void run.events.length;
    if (stick) queueMicrotask(() => feedEl?.scrollTo({ top: feedEl.scrollHeight, behavior: "smooth" }));
  });

  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  async function submitNote(ev?: Event) {
    ev?.preventDefault();
    if ((!note.trim() && !noteImages.length) || !run.running) return;
    try {
      const paths = await upload(repo, run.slug, noteImages);
      await sendNote(note.trim() || "See the attached image.", paths);
      note = "";
      noteImages = [];
    } catch (e) {
      run.error = String(e);
    }
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submitNote();
    }
  }

  const toggle = (k: string) => (open[k] = !open[k]);
  const sectionCost = (s: Section) =>
    s.finished ? `${fmtDuration(s.finished.duration_ms)} · ${fmtTokens(s.finished.new_tokens)} tok · $${s.finished.cost_usd.toFixed(3)}` : "";
  const diffSize = (t: ToolItem) => (t.diff ? `+${t.diff.added.length}${t.diff.removed.length ? ` −${t.diff.removed.length}` : ""}` : "");
</script>

{#snippet toolRow(t: ToolItem, nested = false)}
  {@const expandable = !!(t.result?.summary || t.diff)}
  {@const isOpen = open[t.id] ?? (!!t.diff && t.diff.added.length + t.diff.removed.length <= 14)}
  <div class="tool" class:nested class:err={t.result?.is_error || t.decision?.allowed === false} class:waiting={t.waiting}>
    <button class="trow" onclick={() => expandable && (open[t.id] = !isOpen)} class:static={!expandable}>
      <span class="ticon"><Icon name={toolIcon(t.tool)} size={13} /></span>
      <span class="verb">{toolVerb(t.tool)}</span>
      <span class="target">{short(t.summary)}</span>
      {#if t.diff}<span class="dsize">{diffSize(t)}</span>{/if}
      <span class="right">
        {#if t.waiting}
          <span class="pill warn"><span class="pulse"></span>waiting for you</span>
        {:else if t.decision && !t.decision.allowed}
          <span class="pill bad" title={t.decision.rule ?? ""}>denied{t.decision.by === "policy" ? "" : ` by ${t.decision.by}`}</span>
        {:else if t.decision && t.decision.by !== "policy"}
          <span class="pill good">allowed by {t.decision.by}</span>
        {/if}
        {#if t.result}
          <span class="state" class:bad={t.result.is_error}><Icon name={t.result.is_error ? "x" : "check"} size={12} stroke={2.5} /></span>
        {:else if !t.waiting && run.running}
          <span class="spinner"></span>
        {/if}
        {#if expandable}<span class="chev" class:open={isOpen}><Icon name="chevron-right" size={12} /></span>{/if}
      </span>
    </button>
    {#if isOpen}
      {#if t.diff}
        <pre class="diff">{#each t.diff.removed as l, i (i)}<span class="del">- {l}
</span>{/each}{#each t.diff.added as l, i (i)}<span class="add">+ {l}
</span>{/each}{#if t.diff.more}<span class="more">… {t.diff.more} more lines</span>{/if}</pre>
      {/if}
      {#if t.result?.summary && (t.result.is_error || !t.diff)}
        <pre class="out" class:bad={t.result.is_error}>{t.result.summary}</pre>
      {/if}
    {/if}
  </div>
{/snippet}

<div class="runview">
  {#if run.events.length === 0 && !run.running}
    <div class="empty">
      <div class="emptyicon"><Icon name="run" size={22} /></div>
      <h2>No run yet</h2>
      <p>Start an analysis from a ticket, or pick a task on the Board and press <b>Run</b>.</p>
      {#if run.error}<p class="error">{run.error}</p>{/if}
      <button class="primary" onclick={onnew} disabled={!repo}><Icon name="plus" size={13} /> New analysis</button>
    </div>
  {:else}
    <div class="head">
      <div class="title">
        <span class="slug">{run.slug}</span>
        <span class="mode">{modeLabel(run.mode)}</span>
        {#if status}<span class="pill {status.cls}">{#if status.cls === "accent"}<span class="pulse"></span>{/if}{status.label}</span>{/if}
      </div>
      <div class="stats">
        {#if run.running}<span title="Elapsed"><Icon name="clock" size={13} /> {fmtDuration(now - run.startedAt)}</span>{/if}
        <span title="New tokens this session"><Icon name="zap" size={13} /> {fmtTokens(run.tokens)}</span>
        <span title="Cost of finished agent runs (API-equivalent)"><Icon name="coins" size={13} /> ${run.usd.toFixed(3)}</span>
      </div>
      <div class="ctl">
        {#if run.running}
          {#if run.paused}
            <button onclick={resumeRun}><Icon name="run" size={12} /> Resume</button>
          {:else}
            <button onclick={pauseRun}><Icon name="pause" size={12} /> Pause</button>
          {/if}
          <button class="danger" onclick={stopRun}><Icon name="stop" size={12} /> Stop</button>
        {:else}
          <button onclick={onnew} disabled={!repo}><Icon name="plus" size={13} /> New analysis</button>
        {/if}
      </div>
    </div>
    {#if trail.length}
      <div class="trail">
        {#each trail as step, i (i)}
          {#if i > 0}<span class="link"></span>{/if}
          <span class="step {step.state}" style="--c: {step.color}">
            {#if step.state === "active"}<span class="pulse"></span>{:else}<Icon
                name={step.state === "bad" ? "x" : "check"}
                size={11}
                stroke={2.5}
              />{/if}
            {step.label}
          </span>
        {/each}
      </div>
    {/if}

    <div class="feed" bind:this={feedEl} onscroll={onScroll}>
      {#if run.error}<div class="callout bad"><Icon name="alert" size={14} /> {run.error}</div>{/if}
      {#each sections as s, si (si)}
        {#if s.kind === "agent"}
          {@const r = role(s.role)}
          <section class="agent" style="--c: {r.color}">
            <header>
              <span class="avatar"><Icon name={r.icon} size={14} /></span>
              <span class="who">{r.label}</span>{#if s.round}<span class="round">round {s.round}</span>{/if}
              {#if s.model}<span class="model">{s.model}</span>{/if}
              <span class="cost">
                {#if s.finished}{sectionCost(s)}{:else if !s.exited && run.running}<span class="working"><span class="spinner"></span> working</span>{/if}
              </span>
            </header>
            <div class="items">
              {#each s.items as it, ii (ii)}
                {#if it.t === "text"}
                  <div class="prose msg">{@html md(it.text)}</div>
                {:else if it.t === "tool"}
                  {@render toolRow(it)}
                {:else if it.t === "explore"}
                  {@const key = `${si}-${ii}`}
                  <div class="tool">
                    <button class="trow" onclick={() => toggle(key)}>
                      <span class="ticon"><Icon name="search" size={13} /></span>
                      <span class="verb">Explored</span>
                      <span class="target"
                        >{it.tools.length} items · {it.tools
                          .slice(0, 3)
                          .map((t) => short(t.summary).split("/").at(-1))
                          .join(", ")}{it.tools.length > 3 ? "…" : ""}</span
                      >
                      <span class="right"><span class="chev" class:open={open[key]}><Icon name="chevron-right" size={12} /></span></span>
                    </button>
                    {#if open[key]}
                      <div class="nestwrap">
                        {#each it.tools as t (t.id)}{@render toolRow(t, true)}{/each}
                      </div>
                    {/if}
                  </div>
                {:else if it.t === "note"}
                  <div class="you">
                    <span class="youlabel">You</span>{it.text}
                    {#if it.images.length}
                      <div class="youimgs">
                        {#each it.images as path (path)}
                          {#await savedPreview(repo, path) then src}
                            <img {src} alt={path} title={path} />
                          {:catch}
                            <span class="faint">{path}</span>
                          {/await}
                        {/each}
                      </div>
                    {/if}
                  </div>
                {:else if it.t === "alarm"}
                  <div class="callout" class:bad={it.e.action !== "warn"}>
                    <Icon name="alert" size={14} />
                    {it.e.message}{it.e.action !== "warn" ? ` → ${it.e.action}` : ""}
                  </div>
                {:else if it.t === "error"}
                  <div class="callout bad"><Icon name="alert" size={14} /> {it.text}</div>
                {:else if it.t === "paused"}
                  <div class="mark"><Icon name="pause" size={12} /> Paused: {it.reason}</div>
                {:else if it.t === "resumed"}
                  <div class="mark"><Icon name="run" size={12} /> Resumed</div>
                {/if}
              {/each}
              {#if s.items.length === 0 && !s.exited}
                <div class="mark"><span class="spinner"></span> Starting…</div>
              {/if}
            </div>
          </section>
        {:else}
          <div class="system">
            {#each s.items as it, ii (ii)}
              {#if it.t === "build"}
                {@const key = `b-${si}-${ii}`}
                <div class="build" class:bad={!it.e.ok}>
                  <button class="trow" onclick={() => toggle(key)}>
                    <span class="bicon"><Icon name="hammer" size={14} /></span>
                    <span class="verb">{it.e.ok ? "Build passed" : "Build failed"}</span>
                    <code>{it.e.command}</code>
                    <span class="right">
                      <span class="faint">{fmtDuration(it.e.duration_ms)}</span>
                      <span class="chev" class:open={open[key]}><Icon name="chevron-right" size={12} /></span>
                    </span>
                  </button>
                  {#if open[key]}<pre class="out">{it.e.tail}</pre>{/if}
                </div>
              {:else if it.t === "status"}
                <div class="divider">
                  <span>Status → <b>{it.e.to}</b> <span class="faint">by {it.e.by}</span></span>
                </div>
                {#if it.e.reason}<div class="mark center">{it.e.reason}</div>{/if}
              {:else if it.t === "checkpoint"}
                <div class="mark"><Icon name="branch" size={12} /> Checkpoint {it.e.round} <code>{it.e.git_ref}</code></div>
              {:else if it.t === "info"}
                <div class="mark"><Icon name="dot" size={12} /> {it.text}</div>
              {:else if it.t === "delivery_ready"}
                <div class="callout accent">
                  <Icon name="package" size={14} /> Ready to deliver: {it.e.mode} to <code>{it.e.branch}</code>, {it.e.files.length} file{it.e.files.length === 1
                    ? ""
                    : "s"}. Approve it from the task.
                </div>
              {:else if it.t === "delivered"}
                <div class="callout good">
                  <Icon name="package" size={14} /> Delivered to <code>{it.e.branch}</code>{it.e.commit ? ` · ${it.e.commit.slice(0, 8)}` : ""}{it.e.pushed
                    ? " · pushed"
                    : ""}{#if it.e.pr_url}
                    · {it.e.pr_url}{/if}
                </div>
              {:else if it.t === "ended"}
                <div class="ended" class:bad={!it.e.ok}>
                  <span class="eicon"><Icon name={it.e.ok ? "check" : "stop"} size={16} stroke={2.5} /></span>
                  <div>
                    <b>{it.e.summary}</b>
                    <div class="faint">{fmtTokens(run.tokens)} tokens · ${run.usd.toFixed(3)}</div>
                  </div>
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      {/each}
    </div>

    <form class="composer" onsubmit={submitNote} bind:this={composer}>
      <div class="cbody">
      <Attachments bind:this={attach} bind:items={noteImages} zone={composer} disabled={!run.running} compact />
      <textarea
        rows="1"
        bind:value={note}
        onkeydown={onKey}
        onpaste={(e) => attach?.paste(e)}
        placeholder={run.running ? "Message the agent — it reads it after its current step (Enter to send, Shift+Enter for a new line)" : "No agent is running"}
        disabled={!run.running}
      ></textarea>
      </div>
      <button type="button" class="ghost send" onclick={() => attach?.pick()} disabled={!run.running} title="Attach images (or paste / drop them)"
        ><Icon name="paperclip" size={15} /></button
      >
      <button type="submit" class="primary send" disabled={!run.running || (!note.trim() && !noteImages.length)} title="Send"
        ><Icon name="send" size={14} /></button
      >
    </form>
  {/if}
</div>

<style>
  .runview {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .empty {
    margin: auto;
    text-align: center;
    max-width: 380px;
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
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 22px 10px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .slug {
    font-weight: 650;
    font-size: 15px;
  }
  .mode {
    color: var(--quiet);
  }
  .stats {
    display: flex;
    gap: 14px;
    margin-left: auto;
    color: var(--quiet);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .stats span {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .ctl {
    display: flex;
    gap: 6px;
  }
  .trail {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px;
    padding: 0 22px 12px;
    border-bottom: 1px solid var(--line);
  }
  .step {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 9px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 500;
    background: color-mix(in srgb, var(--c) 10%, transparent);
    color: var(--c);
  }
  .step.done {
    color: var(--quiet);
    background: var(--code-bg);
  }
  .step.bad {
    color: var(--bad);
    background: var(--bad-soft);
  }
  .link {
    width: 14px;
    height: 1px;
    background: var(--line-strong);
  }
  .pulse {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.4s ease-in-out infinite;
  }
  .feed {
    flex: 1;
    overflow: auto;
    padding: 16px 22px 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .feed > :global(*) {
    flex-shrink: 0;
  }
  .agent {
    border: 1px solid var(--line);
    background: var(--panel);
    border-radius: 12px;
    box-shadow: var(--shadow-sm);
    overflow: hidden;
  }
  .agent header {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 9px 14px;
    border-bottom: 1px solid var(--line);
    background: color-mix(in srgb, var(--c) 4%, var(--panel));
  }
  .avatar {
    width: 24px;
    height: 24px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    color: #fff;
    background: var(--c);
  }
  .who {
    font-weight: 600;
  }
  .round {
    font-weight: 400;
    color: var(--quiet);
  }
  .model {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--quiet);
    background: var(--code-bg);
    padding: 1px 6px;
    border-radius: 4px;
  }
  .cost {
    margin-left: auto;
    color: var(--quiet);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .working {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--c);
  }
  .items {
    padding: 8px 14px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .msg {
    padding: 6px 2px;
  }
  .tool {
    border-radius: 7px;
  }
  .tool.waiting {
    background: var(--warn-soft);
  }
  .trow {
    all: unset;
    box-sizing: border-box;
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: 7px;
    cursor: pointer;
    min-width: 0;
  }
  .trow:hover {
    background: var(--hover);
  }
  .trow.static {
    cursor: default;
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
  .dsize {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--good);
    flex: none;
  }
  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .state {
    color: var(--good);
    display: inline-flex;
  }
  .state.bad {
    color: var(--bad);
  }
  .chev {
    color: var(--faint);
    display: inline-flex;
    transition: transform 0.12s;
  }
  .chev.open {
    transform: rotate(90deg);
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
  .nestwrap {
    margin: 2px 0 4px 17px;
    padding-left: 10px;
    border-left: 2px solid var(--line);
  }
  .out,
  .diff {
    margin: 2px 0 6px 36px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--code-bg);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 260px;
    overflow: auto;
  }
  .out {
    color: var(--quiet);
  }
  .out.bad {
    color: var(--bad);
    background: var(--bad-soft);
  }
  .diff {
    padding: 6px 0;
    white-space: pre;
    overflow-x: auto;
  }
  .diff span {
    display: block;
    padding: 0 10px;
  }
  .diff .add {
    background: color-mix(in srgb, var(--good) 13%, transparent);
  }
  .diff .del {
    background: color-mix(in srgb, var(--bad) 13%, transparent);
  }
  .diff .more {
    color: var(--faint);
  }
  .you {
    align-self: flex-end;
    max-width: 80%;
    background: var(--accent);
    color: #fff;
    padding: 7px 12px;
    border-radius: 12px 12px 4px 12px;
    white-space: pre-wrap;
    margin: 4px 0;
  }
  .youlabel {
    display: block;
    font-size: 10.5px;
    opacity: 0.75;
    font-weight: 600;
  }
  .callout {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--warn-soft);
    color: var(--ink);
  }
  .callout :global(svg) {
    margin-top: 2px;
    color: var(--warn);
  }
  .callout.bad {
    background: var(--bad-soft);
  }
  .callout.bad :global(svg) {
    color: var(--bad);
  }
  .callout.good {
    background: var(--good-soft);
  }
  .callout.good :global(svg) {
    color: var(--good);
  }
  .callout.accent {
    background: var(--accent-soft);
  }
  .callout.accent :global(svg) {
    color: var(--accent);
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
  .mark code {
    font-size: 11px;
  }
  .system {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .build {
    border: 1px solid var(--line);
    border-left: 3px solid var(--good);
    border-radius: 10px;
    background: var(--panel);
    padding: 4px 6px;
  }
  .build.bad {
    border-left-color: var(--bad);
  }
  .bicon {
    color: var(--good);
    display: inline-flex;
  }
  .build.bad .bicon {
    color: var(--bad);
  }
  .build code {
    font-size: 11.5px;
  }
  .build .out {
    margin-left: 6px;
  }
  .faint {
    color: var(--faint);
    font-size: 11.5px;
  }
  .divider {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--quiet);
    font-size: 12px;
  }
  .divider::before,
  .divider::after {
    content: "";
    flex: 1;
    height: 1px;
    background: var(--line);
  }
  .ended {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 12px;
    background: var(--good-soft);
    border: 1px solid color-mix(in srgb, var(--good) 30%, transparent);
  }
  .ended.bad {
    background: var(--bad-soft);
    border-color: color-mix(in srgb, var(--bad) 30%, transparent);
  }
  .eicon {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    background: var(--good);
    flex: none;
  }
  .ended.bad .eicon {
    background: var(--bad);
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
    border: 1px solid rgba(255, 255, 255, 0.35);
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
    width: 32px;
    height: 32px;
    padding: 0;
    border-radius: 8px;
  }
</style>
