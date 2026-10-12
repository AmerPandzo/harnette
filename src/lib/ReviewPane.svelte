<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { editorName, openInEditor } from "./editor.svelte";

  type Finding = {
    id: string;
    round: number;
    severity: string;
    text: string;
    ticked: boolean;
    verified: boolean;
    reopened: boolean;
    fixed: string | null;
  };
  type ReviewData = {
    base_label: string;
    files: { path: string; added: number; removed: number }[];
    patch: string;
    truncated: boolean;
    findings: Finding[];
    criteria: { text: string; done: boolean }[];
  };

  let { repo, slug, version }: { repo: string; slug: string; version: number } = $props();

  let base = $state("checkpoint");
  let branch = $state("develop");
  let data = $state<ReviewData | null>(null);
  let error = $state("");
  let open = $state<Record<string, boolean>>({});

  $effect(() => {
    const args = { repo, slug, base: base === "branch" ? branch.trim() || "develop" : base };
    void version;
    invoke<ReviewData>("task_review", args)
      .then((d) => {
        data = d;
        error = "";
      })
      .catch((e) => (error = String(e)));
  });

  const stateOf = (f: Finding) =>
    f.reopened && !f.ticked ? "reopened" : f.verified ? "verified" : f.ticked ? "fixed" : "open";
  const order = { open: 0, reopened: 1, fixed: 2, verified: 3 } as const;
  let findings = $derived(data ? [...data.findings].sort((a, b) => order[stateOf(a)] - order[stateOf(b)]) : []);

  // Split the unified diff per file so each file can be collapsed.
  let filePatches = $derived.by(() => {
    const out = new Map<string, string[]>();
    if (!data) return out;
    let current: string[] | null = null;
    for (const line of data.patch.split("\n")) {
      if (line.startsWith("diff --git ")) {
        const m = / b\/(.+)$/.exec(line);
        current = [];
        out.set(m ? m[1] : line, current);
      } else if (current && !line.startsWith("index ")) {
        current.push(line);
      }
    }
    return out;
  });

  /** Where to open a changed file: the first changed line on the new side; null when the file was deleted. */
  function firstLine(path: string): number | null {
    const lines = filePatches.get(path) ?? [];
    if (lines.includes("+++ /dev/null")) return null;
    for (const l of lines) {
      const m = /^@@ -\d+(?:,\d+)? \+(\d+)/.exec(l);
      if (m) return Math.max(1, Number(m[1]));
    }
    return 1;
  }

  // Review findings cite code as `path:line`; make those references open the editor.
  const REF = /([\w.\/-]+\.[\w]+):(\d+)/g;
  function segments(text: string): ({ text: string } | { path: string; line: number; text: string })[] {
    const out: ({ text: string } | { path: string; line: number; text: string })[] = [];
    let last = 0;
    for (const m of text.matchAll(REF)) {
      if (m.index > last) out.push({ text: text.slice(last, m.index) });
      out.push({ path: m[1], line: Number(m[2]), text: m[0] });
      last = m.index + m[0].length;
    }
    if (last < text.length) out.push({ text: text.slice(last) });
    return out;
  }

  const cls = (l: string) =>
    l.startsWith("+++") || l.startsWith("---")
      ? "meta"
      : l.startsWith("+")
        ? "add"
        : l.startsWith("-")
          ? "del"
          : l.startsWith("@@")
            ? "hunk"
            : "";
</script>

<div class="review">
  <div class="bar">
    <label
      >Compare with
      <select bind:value={base}>
        <option value="checkpoint">Before the first round</option>
        <option value="head">HEAD (uncommitted changes)</option>
        <option value="branch">Branch (merge-base)…</option>
      </select>
    </label>
    {#if base === "branch"}<input bind:value={branch} placeholder="develop" />{/if}
    {#if data}<span class="meta">{data.base_label}</span>{/if}
  </div>
  {#if error}<p class="error pad">{error}</p>{/if}

  {#if data}
    <div class="cols">
      <section>
        <h4>Acceptance criteria <span class="meta">{data.criteria.filter((c) => c.done).length}/{data.criteria.length}</span></h4>
        {#each data.criteria as c, i (i)}
          <p class="crit" class:done={c.done}>{c.done ? "☑" : "☐"} {c.text}</p>
        {:else}
          <p class="meta">No criteria in the doc.</p>
        {/each}
      </section>
      <section>
        <h4>Findings <span class="meta">{findings.filter((f) => stateOf(f) === "open" || stateOf(f) === "reopened").length} open</span></h4>
        {#each findings as f (f.id)}
          <div class="finding {stateOf(f)}">
            <span class="id">{f.id}</span>
            {#if f.severity}<span class="sev {f.severity.toLowerCase()}">{f.severity}</span>{/if}
            <span class="st">{stateOf(f)}</span>
            <div class="ftext">
              {#each segments(f.text) as seg, i (i)}{#if "path" in seg}<button
                    class="ref"
                    title={`Open in ${editorName()}`}
                    onclick={() => openInEditor(repo, seg.path, seg.line)}>{seg.text}</button
                  >{:else}{seg.text}{/if}{/each}
            </div>
            {#if f.fixed}<div class="fixed">Fixed: {f.fixed}</div>{/if}
          </div>
        {:else}
          <p class="meta">No findings yet.</p>
        {/each}
      </section>
    </div>

    <h4 class="pad">Changes <span class="meta">{data.files.length} file{data.files.length === 1 ? "" : "s"}{data.truncated ? " · diff truncated at 2 MB" : ""}</span></h4>
    {#each data.files as file (file.path)}
      <div class="file">
        <div class="frow">
          <button class="fhead" onclick={() => (open[file.path] = !open[file.path])}>
            <span>{open[file.path] ? "▾" : "▸"}</span>
            <code>{file.path}</code>
            <span class="add">+{file.added}</span>
            <span class="del">−{file.removed}</span>
          </button>
          {#if firstLine(file.path) !== null}
            <button class="ref open" title={`Open in ${editorName()} at the first change`} onclick={() => openInEditor(repo, file.path, firstLine(file.path) ?? undefined)}
              >Open</button
            >
          {/if}
        </div>
        {#if open[file.path]}
          <pre class="diff">{#each filePatches.get(file.path) ?? [] as line, i (i)}<span class={cls(line)}>{line}
</span>{/each}</pre>
        {/if}
      </div>
    {:else}
      <p class="meta pad">No changes against this base.</p>
    {/each}
  {/if}
</div>

<style>
  .review {
    flex: 1;
    overflow: auto;
    padding-bottom: 24px;
  }
  .bar {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    padding: 10px 14px;
    border-bottom: 1px solid var(--line);
    font-size: 12px;
  }
  .bar label {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--quiet);
  }
  .bar select,
  .bar input {
    padding: 4px 6px;
  }
  .bar input {
    width: 120px;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
    font-weight: 400;
  }
  .pad {
    padding: 0 14px;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
    padding: 4px 14px;
  }
  h4 {
    font-size: 13px;
    margin: 12px 0 6px;
  }
  .crit {
    margin: 0 0 4px;
    font-size: 12.5px;
  }
  .crit.done {
    color: var(--quiet);
  }
  .finding {
    border: 1px solid var(--line);
    border-left: 3px solid var(--bad);
    border-radius: 6px;
    padding: 6px 8px;
    margin-bottom: 6px;
    font-size: 12.5px;
  }
  .finding.reopened {
    border-left-color: #d99a2b;
  }
  .finding.fixed {
    border-left-color: var(--accent);
  }
  .finding.verified {
    border-left-color: var(--good);
    opacity: 0.75;
  }
  .id {
    font-family: ui-monospace, Menlo, monospace;
    font-weight: 600;
  }
  .sev {
    margin-left: 6px;
    font-size: 11px;
    padding: 0 6px;
    border-radius: 999px;
    border: 1px solid var(--line);
  }
  .sev.blocker {
    color: var(--bad);
    border-color: var(--bad);
  }
  .sev.major {
    color: #b7791f;
    border-color: #b7791f;
  }
  .st {
    float: right;
    color: var(--quiet);
    font-size: 11px;
  }
  .ftext {
    margin-top: 3px;
  }
  .fixed {
    margin-top: 3px;
    color: var(--quiet);
  }
  .file {
    margin: 0 14px 6px;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
  }
  .frow {
    display: flex;
    align-items: center;
    background: var(--panel);
  }
  .ref {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    font-family: ui-monospace, Menlo, monospace;
    font-size: 0.95em;
    cursor: pointer;
  }
  .ref:hover {
    text-decoration: underline;
  }
  .ref.open {
    font-family: inherit;
    font-size: 12px;
    padding: 0 10px;
  }
  .fhead {
    display: flex;
    gap: 8px;
    align-items: center;
    width: 100%;
    text-align: left;
    border: none;
    border-radius: 0;
    background: var(--panel);
  }
  .fhead code {
    flex: 1;
    background: none;
  }
  .add {
    color: var(--good);
  }
  .del {
    color: var(--bad);
  }
  .diff {
    margin: 0;
    padding: 8px 0;
    font-size: 12px;
    line-height: 1.45;
    overflow-x: auto;
    background: var(--bg);
  }
  .diff span {
    display: block;
    padding: 0 10px;
    white-space: pre;
  }
  .diff .add {
    background: color-mix(in srgb, var(--good) 14%, transparent);
    color: var(--ink);
  }
  .diff .del {
    background: color-mix(in srgb, var(--bad) 14%, transparent);
    color: var(--ink);
  }
  .diff .hunk {
    color: var(--accent);
  }
  .diff .meta {
    color: var(--quiet);
  }
</style>
