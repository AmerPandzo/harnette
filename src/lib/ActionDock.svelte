<script lang="ts">
  // Everything that waits for the human, in one place: permission asks, a paused run, a warning.
  import Icon from "./Icon.svelte";
  import { answer, resumeRun, run, stopRun } from "./run.svelte";
  import { toolIcon } from "./feed";

  /** Lift above the Run view's message box. */
  let { raised = false }: { raised?: boolean } = $props();
</script>

{#if run.asks.length || run.paused || run.warning}
  <div class="dock" class:raised>
    {#each run.asks as a (a.request_id)}
      <div class="card ask">
        <div class="top">
          <span class="icon"><Icon name={toolIcon(a.tool)} size={14} /></span>
          <span class="title">Allow <b>{a.tool}</b>?</span>
          <span class="who">{run.slug}{run.role ? ` · ${run.role}` : ""}</span>
        </div>
        <pre class="cmd">{a.summary}</pre>
        <div class="actions">
          <button class="danger" onclick={() => answer(a, false)}><Icon name="x" size={13} /> Deny</button>
          <button class="primary" onclick={() => answer(a, true)}><Icon name="check" size={13} /> Allow once</button>
        </div>
      </div>
    {/each}
    {#if run.paused}
      <div class="card paused">
        <div class="top">
          <span class="icon"><Icon name="pause" size={14} /></span>
          <span class="title"><b>{run.slug}</b> is paused</span>
        </div>
        <p>{run.paused}</p>
        <div class="actions">
          <button class="danger" onclick={stopRun}><Icon name="stop" size={12} /> Stop</button>
          <button class="primary" onclick={resumeRun}><Icon name="run" size={12} /> Resume</button>
        </div>
      </div>
    {:else if run.warning}
      <div class="card warn">
        <div class="top">
          <span class="icon"><Icon name="alert" size={14} /></span>
          <span class="title">Heads-up</span>
        </div>
        <p>{run.warning}</p>
        <div class="actions"><button onclick={() => (run.warning = null)}>Dismiss</button></div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .dock {
    position: fixed;
    right: 18px;
    bottom: 18px;
    width: min(420px, calc(100vw - 36px));
    display: flex;
    flex-direction: column;
    gap: 10px;
    z-index: 40;
  }
  .dock.raised {
    bottom: 88px;
  }
  .card {
    background: var(--raised);
    border: 1px solid var(--line);
    border-radius: 12px;
    box-shadow: var(--shadow-lg);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    animation: rise 0.18s ease-out;
    border-left: 3px solid var(--accent);
  }
  .card.paused {
    border-left-color: var(--bad);
  }
  .card.warn {
    border-left-color: var(--warn);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .icon {
    width: 24px;
    height: 24px;
    border-radius: 6px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent-ink);
  }
  .paused .icon {
    background: var(--bad-soft);
    color: var(--bad);
  }
  .warn .icon {
    background: var(--warn-soft);
    color: var(--warn);
  }
  .who {
    margin-left: auto;
    color: var(--faint);
    font-size: 11.5px;
  }
  .cmd {
    margin: 0;
    padding: 8px 10px;
    background: var(--code-bg);
    border-radius: 6px;
    font-size: 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 140px;
    overflow: auto;
  }
  p {
    margin: 0;
    color: var(--quiet);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(10px);
    }
  }
</style>
