<script lang="ts">
  import Icon from "./Icon.svelte";
  import { run } from "./run.svelte";
  import { COLUMNS, type StatusKind, type TaskSummary } from "./types";

  const DOT: Record<string, string> = {
    analysis: "var(--role-analyst)",
    ready: "var(--accent)",
    in_review: "var(--role-reviewer)",
    changes_requested: "var(--bad)",
    done: "var(--good)",
    unknown: "var(--faint)",
  };

  let {
    tasks,
    selected,
    onselect,
    onmove,
  }: {
    tasks: TaskSummary[];
    selected: string | null;
    onselect: (slug: string) => void;
    /** A card was dropped on another column: change the doc's Status line. */
    onmove: (slug: string, to: StatusKind) => void;
  } = $props();

  // Pointer-driven drag (HTML5 drag-and-drop competes with the window's file-drop handling).
  type Drag = { slug: string; from: StatusKind; x: number; y: number; dx: number; dy: number; w: number; started: boolean; over: StatusKind | null };
  let drag = $state<Drag | null>(null);
  let dragged = $derived(drag?.started ? tasks.find((t) => t.slug === drag!.slug) : undefined);

  function down(e: PointerEvent, t: TaskSummary) {
    if (e.button !== 0 || (run.running && run.slug === t.slug)) return;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    drag = { slug: t.slug, from: t.status_kind, x: e.clientX, y: e.clientY, dx: e.clientX - r.left, dy: e.clientY - r.top, w: r.width, started: false, over: null };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up, { once: true });
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    if (!drag.started && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 5) return;
    drag.started = true;
    drag.x = e.clientX;
    drag.y = e.clientY;
    const col = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-kind]");
    const kind = (col?.dataset.kind ?? null) as StatusKind | null;
    drag.over = kind && kind !== "unknown" ? kind : null;
  }
  function up() {
    window.removeEventListener("pointermove", move);
    const d = drag;
    drag = null;
    if (!d) return;
    if (!d.started) onselect(d.slug);
    else if (d.over && d.over !== d.from) onmove(d.slug, d.over);
  }

  // An Unknown column appears only when some doc has a status the app does not recognise.
  let columns = $derived(
    tasks.some((t) => t.status_kind === "unknown")
      ? [...COLUMNS, { kind: "unknown" as StatusKind, label: "Other status" }]
      : COLUMNS,
  );

  const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const shortTitle = (t: TaskSummary) => t.title.replace(new RegExp(`^${escapeRe(t.slug)}\\s*[—-]\\s*`), "");
</script>

<div class="board">
  {#each columns as col (col.kind)}
    {@const items = tasks.filter((t) => t.status_kind === col.kind)}
    <section
      class="col"
      data-kind={col.kind}
      class:target={drag?.started && drag.over === col.kind && drag.from !== col.kind}
      style="--dot: {DOT[col.kind]}"
    >
      <h3><span class="dot"></span>{col.label} <span class="count">{items.length}</span></h3>
      <div class="cards">
        {#each items as t (t.slug)}
          {@const live = run.running && run.slug === t.slug}
          <button
            class="card"
            class:sel={selected === t.slug}
            class:live
            class:ghosted={drag?.started && drag.slug === t.slug}
            title={live ? "Running: cannot be moved" : "Drag to another column to change its status"}
            onpointerdown={(e) => down(e, t)}
            onkeydown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onselect(t.slug))}
          >
            <span class="slug">{t.slug}</span>
            <span class="title">{shortTitle(t)}</span>
            {#if live || t.criteria_total > 0 || t.open_questions > 0 || t.open_findings > 0 || col.kind === "unknown"}
              <span class="chips">
                {#if live}
                  <span class="pill accent"><span class="pulse"></span>{run.asks.length ? "needs you" : run.paused ? "paused" : run.role || "running"}</span>
                {/if}
                {#if t.criteria_total > 0}
                  <span class="chip" class:allok={t.criteria_done === t.criteria_total} title="Acceptance criteria met"
                    ><Icon name="check" size={11} stroke={2.5} /> {t.criteria_done}/{t.criteria_total}</span
                  >
                {/if}
                {#if t.open_questions > 0}
                  <span class="chip q" title="Open questions"><Icon name="question" size={11} /> {t.open_questions}</span>
                {/if}
                {#if t.open_findings > 0}
                  <span class="chip" class:bad={t.blockers > 0} title="Open review findings ({t.blockers} Blocker)">
                    <Icon name="flag" size={11} /> {t.open_findings}{t.blockers > 0 ? ` · ${t.blockers} Blocker` : ""}
                  </span>
                {/if}
                {#if col.kind === "unknown"}
                  <span class="chip">{t.status || "no Status line"}</span>
                {/if}
              </span>
            {/if}
          </button>
        {:else}
          <p class="empty">No tasks</p>
        {/each}
      </div>
    </section>
  {/each}
</div>

{#if dragged && drag}
  <div class="card floating" style="left: {drag.x - drag.dx}px; top: {drag.y - drag.dy}px; width: {drag.w}px">
    <span class="slug">{dragged.slug}</span>
    <span class="title">{shortTitle(dragged)}</span>
    {#if drag.over && drag.over !== drag.from}
      <span class="pill accent">→ {columns.find((c) => c.kind === drag?.over)?.label}</span>
    {/if}
  </div>
{/if}

<style>
  .col.target {
    background: color-mix(in srgb, var(--dot) 12%, transparent);
    box-shadow: inset 0 0 0 2px color-mix(in srgb, var(--dot) 55%, transparent);
  }
  .card.ghosted {
    opacity: 0.35;
  }
  .card.floating {
    position: fixed;
    z-index: 60;
    pointer-events: none;
    box-shadow: var(--shadow-lg);
    transform: rotate(1.5deg);
  }
  .board {
    display: flex;
    gap: 12px;
    padding: 4px 22px 16px;
    overflow: auto;
    align-items: stretch;
    height: 100%;
  }
  .col {
    flex: 1 0 190px;
    max-width: 300px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: color-mix(in srgb, var(--ink) 2.5%, transparent);
    border-radius: 12px;
    padding: 10px 8px 8px;
    min-height: 120px;
  }
  h3 {
    margin: 0 4px 2px;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--dot);
  }
  .count {
    font-weight: 500;
    color: var(--faint);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card {
    all: unset;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 10px 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 10px;
    box-shadow: var(--shadow-sm);
    cursor: grab;
    user-select: none;
    touch-action: none;
    transition:
      box-shadow 0.12s,
      border-color 0.12s,
      transform 0.12s;
  }
  .card:hover {
    box-shadow: var(--shadow-md);
    transform: translateY(-1px);
  }
  .card:focus-visible {
    border-color: var(--accent);
  }
  .card.sel {
    border-color: var(--accent);
    box-shadow:
      0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent),
      var(--shadow-sm);
  }
  .card.live {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--line));
  }
  .slug {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--quiet);
  }
  .title {
    font-weight: 600;
    line-height: 1.35;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 2px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
    height: 19px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--code-bg);
    color: var(--quiet);
  }
  .chip.allok {
    background: var(--good-soft);
    color: var(--good);
  }
  .chip.q {
    background: color-mix(in srgb, var(--role-analyst) 12%, transparent);
    color: var(--role-analyst);
  }
  .chip.bad {
    background: var(--bad-soft);
    color: var(--bad);
  }
  .pulse {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.4s ease-in-out infinite;
  }
  .empty {
    color: var(--faint);
    margin: 4px;
    font-size: 12px;
  }
</style>
