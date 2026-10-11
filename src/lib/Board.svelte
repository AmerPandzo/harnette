<script lang="ts">
  import { COLUMNS, type StatusKind, type TaskSummary } from "./types";

  let {
    tasks,
    selected,
    onselect,
  }: { tasks: TaskSummary[]; selected: string | null; onselect: (slug: string) => void } = $props();

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
    <section class="col">
      <h3>{col.label} <span class="count">{items.length}</span></h3>
      {#each items as t (t.slug)}
        <button class="card" class:sel={selected === t.slug} onclick={() => onselect(t.slug)}>
          <span class="slug">{t.slug}</span>
          <span class="title">{shortTitle(t)}</span>
          <span class="chips">
            {#if t.criteria_total > 0}
              <span class="chip" title="Acceptance criteria met">✓ {t.criteria_done}/{t.criteria_total}</span>
            {/if}
            {#if t.open_questions > 0}
              <span class="chip q" title="Open questions">? {t.open_questions}</span>
            {/if}
            {#if t.open_findings > 0}
              <span class="chip" class:bad={t.blockers > 0} title="Open review findings ({t.blockers} Blocker)">
                ⚑ {t.open_findings}{t.blockers > 0 ? ` · ${t.blockers} Blocker` : ""}
              </span>
            {/if}
            {#if col.kind === "unknown"}
              <span class="chip">{t.status || "no Status line"}</span>
            {/if}
          </span>
        </button>
      {:else}
        <p class="empty">—</p>
      {/each}
    </section>
  {/each}
</div>

<style>
  .board {
    display: flex;
    gap: 10px;
    padding: 12px 16px;
    overflow-x: auto;
    align-items: flex-start;
    height: calc(100% - 24px);
  }
  .col {
    flex: 1 0 170px;
    max-width: 280px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h3 {
    margin: 0 0 2px;
    font-size: 12px;
    font-weight: 600;
    color: var(--quiet);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .count {
    font-weight: 400;
    margin-left: 4px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    padding: 10px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  .card.sel {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .slug {
    font-family: ui-monospace, Menlo, monospace;
    font-size: 11px;
    color: var(--quiet);
  }
  .title {
    font-weight: 600;
    line-height: 1.3;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 2px;
  }
  .chip {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid var(--line);
    color: var(--quiet);
  }
  .chip.q {
    color: var(--accent);
    border-color: var(--accent);
  }
  .chip.bad {
    color: var(--bad);
    border-color: var(--bad);
  }
  .empty {
    color: var(--quiet);
    margin: 0;
    font-size: 12px;
  }
</style>
