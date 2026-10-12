<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import DOMPurify from "dompurify";
  import { marked } from "marked";

  type Question = { index: number; text: string; decided: string | null; answer: string | null; options: string[] };

  let {
    repo,
    slug,
    version,
    busy,
    analysis,
    onrefine,
  }: {
    repo: string;
    slug: string;
    version: number;
    busy: boolean;
    /** Refine only runs while the task is in Analysis. */
    analysis: boolean;
    onrefine: (message: string) => void;
  } = $props();

  let questions = $state<Question[]>([]);
  /** Typed answers by question text, so a doc reload does not lose them. */
  let drafts = $state<Record<string, string>>({});
  let message = $state("");
  let error = $state("");
  let saved = $state("");

  $effect(() => {
    const s = slug;
    void version;
    invoke<Question[]>("task_questions", { repo, slug: s })
      .then((q) => {
        questions = q;
        for (const x of q) if (drafts[x.text] === undefined) drafts[x.text] = x.answer ?? "";
        error = "";
      })
      .catch((e) => (error = String(e)));
  });

  $effect(() => {
    // Another task: start clean.
    void slug;
    drafts = {};
    message = "";
    saved = "";
  });

  let open = $derived(questions.filter((q) => !q.decided));
  let decided = $derived(questions.filter((q) => q.decided));
  let changed = $derived(open.filter((q) => (drafts[q.text] ?? "").trim() !== (q.answer ?? "").trim()));
  let answered = $derived(open.filter((q) => (drafts[q.text] ?? "").trim()));

  // Agent-written text: sanitize before it reaches the webview.
  const inline = (md: string) => DOMPurify.sanitize(marked.parseInline(md, { async: false }) as string);
  /** Paragraphs, with the doc's hard-wrapped lines joined. */
  const block = (md: string) => DOMPurify.sanitize(marked.parse(md, { async: false }) as string);

  function pick(q: Question, option: string) {
    drafts[q.text] = drafts[q.text]?.trim() === option ? "" : option;
  }

  async function save(): Promise<boolean> {
    if (changed.length === 0) return true;
    try {
      await invoke("save_answers", { repo, slug, answers: changed.map((q) => [q.index, drafts[q.text] ?? ""]) });
      saved = `Saved ${changed.length} answer${changed.length === 1 ? "" : "s"} to the doc.`;
      error = "";
      return true;
    } catch (e) {
      error = String(e);
      return false;
    }
  }

  async function send() {
    if (await save()) {
      onrefine(message);
      message = "";
    }
  }
</script>

<div class="qs">
  {#if error}<p class="error">{error}</p>{/if}

  {#if open.length === 0}
    <p class="empty">No open questions.{analysis ? " You can still send the analyst a message below." : ""}</p>
  {/if}

  {#each open as q, n (q.index)}
    <div class="q">
      <div class="qtext"><span class="num">{n + 1}</span><div class="qbody">{@html block(q.text)}</div></div>
      {#if q.options.length}
        <div class="opts">
          {#each q.options as o, i (i)}
            <button class="opt" class:on={(drafts[q.text] ?? "").trim() === o} onclick={() => pick(q, o)}>
              {@html inline(o)}{#if i === 0}<span class="rec">recommended</span>{/if}
            </button>
          {/each}
        </div>
      {/if}
      <textarea
        rows="2"
        bind:value={drafts[q.text]}
        placeholder={q.options.length ? "Pick an option or write your own answer" : "Your answer"}
        disabled={busy}
      ></textarea>
    </div>
  {/each}

  {#if decided.length}
    <details class="decided">
      <summary>Decided ({decided.length})</summary>
      {#each decided as q (q.index)}
        <div class="d"><span>{@html inline(q.text)}</span><span class="dv">→ {@html inline(q.decided ?? "")}</span></div>
      {/each}
    </details>
  {/if}

  <label class="msg">
    Message to the analyst <span class="quiet">(optional: extra requirements, corrections, or a question)</span>
    <textarea rows="3" bind:value={message} placeholder="e.g. Also check the export path; keep the API unchanged." disabled={busy}></textarea>
  </label>

  <div class="actions">
    {#if saved}<span class="quiet">{saved}</span>{/if}
    <button onclick={save} disabled={busy || changed.length === 0} title="Write the answers into the doc without running the analyst">Save answers</button>
    <button
      class="primary"
      onclick={send}
      disabled={busy || (answered.length === 0 && !message.trim())}
      title="Save the answers, then the analyst turns them into decisions and updates the plan"
    >
      Send to analyst
    </button>
  </div>
  <p class="hint">
    {#if analysis}
      Answers go into the doc under each question. The analyst marks them <b>Decided</b>, updates the plan, and moves the task
      to Ready when nothing blocks implementation.
    {:else}
      The analyst marks the answers <b>Decided</b>. If one adds work, it appends steps to the plan and moves the task back to
      <b>Ready</b>, so <b>Run</b> implements just the new part and the reviewer checks it. Otherwise the status stays.
    {/if}
  </p>
</div>

<style>
  .qs {
    flex: 1;
    overflow: auto;
    padding: 12px 16px 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .q {
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--bg);
  }
  .qtext {
    display: flex;
    gap: 8px;
    line-height: 1.45;
  }
  .qbody :global(p) {
    margin: 0 0 4px;
  }
  .opt :global(code) {
    background: none;
    padding: 0;
    color: inherit;
  }
  .num {
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
    display: grid;
    place-items: center;
  }
  .opts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding-left: 28px;
  }
  .opt {
    border-radius: 999px;
    font-size: 12px;
    padding: 3px 10px;
  }
  .opt.on {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }
  .rec {
    margin-left: 6px;
    font-size: 10px;
    opacity: 0.7;
  }
  .q textarea {
    margin-left: 28px;
    resize: vertical;
    font: inherit;
    font-size: 13px;
  }
  .decided summary {
    cursor: pointer;
    color: var(--quiet);
    font-size: 12px;
  }
  .d {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 0 6px 12px;
    font-size: 12.5px;
    border-bottom: 1px solid var(--line);
  }
  .dv {
    color: var(--good);
  }
  .msg {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }
  .msg textarea {
    resize: vertical;
    font: inherit;
    font-size: 13px;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    align-items: center;
  }
  .quiet,
  .empty,
  .hint {
    color: var(--quiet);
    font-size: 12px;
  }
  .empty {
    margin: 0;
  }
  .hint {
    margin: 0;
  }
  .error {
    margin: 0;
  }
</style>
