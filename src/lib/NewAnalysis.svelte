<script lang="ts">
  import Attachments from "./Attachments.svelte";
  import Icon from "./Icon.svelte";
  import { upload, type Pending } from "./attach";
  import { run, startRun } from "./run.svelte";
  import { remember, remembered } from "./types";

  let { repo, onclose, onstarted }: { repo: string; onclose: () => void; onstarted: () => void } = $props();

  let slug = $state("");
  let model = $state(remembered("kormilant.model", ""));
  let ticket = $state("");
  let error = $state("");
  let images = $state<Pending[]>([]);
  let sheet = $state<HTMLElement>();
  let attach = $state<Attachments>();

  async function start(ev: Event) {
    ev.preventDefault();
    remember("kormilant.model", model);
    try {
      const paths = await upload(repo, slug.trim(), images);
      await startRun({ repo, slug: slug.trim(), mode: "analyze", ticket, model, images: paths });
      onstarted();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <form class="sheet" onsubmit={start} bind:this={sheet}>
    <div class="head">
      <span class="badge"><Icon name="sparkles" size={15} /></span>
      <div>
        <h2>New analysis</h2>
        <p>The analyst reads the repo and writes <code>.ai-tasks/&lt;slug&gt;.md</code>. It changes no source files.</p>
      </div>
      <button type="button" class="ghost close" onclick={onclose} title="Close"><Icon name="x" size={15} /></button>
    </div>
    <div class="row">
      <label class="grow">Task<input bind:value={slug} placeholder="OAM-12345" required /></label>
      <label
        >Model
        <select bind:value={model}>
          <option value="">Default</option>
          <option value="haiku">Haiku</option>
          <option value="sonnet">Sonnet</option>
          <option value="opus">Opus</option>
        </select>
      </label>
    </div>
    <label
      >Ticket<textarea
        bind:value={ticket}
        rows="10"
        placeholder="Paste the ticket text, logs, links… and screenshots (⌘V)"
        required
        onpaste={(e) => attach?.paste(e)}
      ></textarea></label
    >
    <Attachments bind:this={attach} bind:items={images} zone={sheet} />
    {#if error}<p class="error">{error}</p>{/if}
    <div class="foot">
      <span class="hint">Unlisted commands ask you first.</span>
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="submit" class="primary" disabled={run.running || !repo}>
        <Icon name="run" size={13} /> Analyze
      </button>
    </div>
  </form>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.28);
    display: grid;
    place-items: center;
    z-index: 50;
    animation: fade 0.12s ease-out;
  }
  .sheet {
    width: min(620px, calc(100vw - 48px));
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 12px;
    box-shadow: var(--shadow-lg);
    padding: 18px 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    animation: rise 0.16s ease-out;
  }
  .head {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  .badge {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--role-analyst) 14%, transparent);
    color: var(--role-analyst);
    flex: none;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .head p {
    margin: 2px 0 0;
    color: var(--quiet);
    font-size: 12.5px;
  }
  .close {
    margin-left: auto;
    width: 28px;
    padding: 0;
  }
  .row {
    display: flex;
    gap: 10px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
    font-weight: 500;
    color: var(--quiet);
  }
  .grow {
    flex: 1;
  }
  textarea {
    resize: vertical;
    min-height: 160px;
  }
  .foot {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .hint {
    margin-right: auto;
    color: var(--faint);
    font-size: 12px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.99);
    }
  }
</style>
