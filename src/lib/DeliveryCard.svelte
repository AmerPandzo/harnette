<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { DeliveryPlan } from "./types";

  type Outcome = { repo: string; branch: string; commit: string | null; pushed: boolean; pr_url: string | null; related: Outcome[] };
  const describe = (o: Outcome) =>
    `${o.repo ? `${o.repo}: ` : ""}delivered to ${o.branch}${o.commit ? ` (${o.commit.slice(0, 8)})` : ""}${o.pushed ? ", pushed" : ""}${o.pr_url ? `: ${o.pr_url}` : ""}`;

  let { repo, slug, onclose }: { repo: string; slug: string; onclose: () => void } = $props();

  let plan = $state<DeliveryPlan | null>(null);
  let error = $state("");
  let result = $state("");
  let busy = $state(false);

  $effect(() => {
    invoke<DeliveryPlan>("delivery_plan", { repo, slug })
      .then((p) => {
        plan = p;
        error = "";
      })
      .catch((e) => (error = String(e)));
  });

  async function approve() {
    busy = true;
    try {
      const o = await invoke<Outcome>("deliver", { repo, slug });
      result = [o, ...o.related].filter((x) => x.commit || x.pushed).map(describe).join("\n") || "Nothing was committed.";
    } catch (e) {
      error = String(e);
    }
    busy = false;
  }
</script>

<div class="card">
  <div class="head">
    <b>Deliver {slug}</b>
    <button onclick={onclose} title="Close">✕</button>
  </div>
  {#if result}
    <p class="ok">{result}</p>
  {:else if error}
    <p class="error">{error}</p>
  {:else if plan}
    {#if plan.mode === "none"}
      <p>Delivery is off for this repository (<code>[delivery] mode = "none"</code> in the Policy tab). The changes stay in your working tree.</p>
    {:else}
      {#each [plan, ...plan.related].filter((p) => p.files.length) as p (p.repo)}
        <p>
          {#if p.repo}<b>{p.repo}</b> (member repo): {/if}<b>{p.mode}</b> to <code>{p.branch}</code>{p.create_branch ? " (new branch)" : ""} ·
          {p.files.length} file{p.files.length === 1 ? "" : "s"}
        </p>
        <ul class="files">
          {#each p.files as f (f)}<li><code>{f}</code></li>{/each}
        </ul>
        <p class="meta">Harnette will run{p.repo ? ` in ${p.repo}` : ""}:</p>
        <pre>{p.commands.join("\n")}</pre>
      {/each}
      <p class="meta">Commit message:</p>
      <pre>{plan.message}</pre>
      <div class="actions">
        <button onclick={onclose}>Not now</button>
        <button class="primary" onclick={approve} disabled={busy}>Approve and {plan.mode === "commit" ? "commit" : plan.mode === "push" ? "push" : "open PR"}</button>
      </div>
    {/if}
  {:else}
    <p class="meta">Checking the gate…</p>
  {/if}
</div>

<style>
  .card {
    margin: 10px 16px 0;
    padding: 12px 14px;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--panel);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .head button {
    border: none;
    background: none;
  }
  .files {
    margin: 4px 0;
    padding-left: 18px;
    font-size: 12.5px;
  }
  pre {
    background: var(--code-bg);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 12px;
    white-space: pre-wrap;
    margin: 4px 0 8px;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
    margin: 6px 0 0;
  }
  .ok {
    white-space: pre-line;
    color: var(--good);
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
</style>
