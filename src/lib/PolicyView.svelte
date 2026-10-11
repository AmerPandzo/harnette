<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import PolicyForm from "./PolicyForm.svelte";
  import { remember, remembered } from "./types";

  let { repo }: { repo: string } = $props();

  let text = $state("");
  let saved = $state("");
  let exists = $state(false);
  let error = $state("");
  let message = $state("");

  let dirty = $derived(text !== saved);
  let mode = $state<"screen" | "toml">(remembered("harness.policyMode", "screen") === "toml" ? "toml" : "screen");

  function setMode(m: "screen" | "toml") {
    if (dirty && !confirm("Discard unsaved TOML changes?")) return;
    mode = m;
    remember("harness.policyMode", m);
  }

  $effect(() => {
    const r = repo;
    void mode; // re-read when switching, so both views show the file as saved
    invoke<[string, boolean]>("read_policy", { repo: r })
      .then(([t, e]) => {
        text = saved = t;
        exists = e;
        error = message = "";
      })
      .catch((e) => (error = String(e)));
  });

  async function save() {
    try {
      await invoke("save_policy", { repo, text });
      saved = text;
      exists = true;
      error = "";
      message = "Saved. Applies to the next run.";
    } catch (e) {
      error = String(e);
      message = "";
    }
  }
</script>

<div class="policy">
  <div class="tabs">
    <span class="seg">
      <button class:on={mode === "screen"} onclick={() => setMode("screen")}>Screen</button>
      <button class:on={mode === "toml"} onclick={() => setMode("toml")}>TOML</button>
    </span>
    <span class="meta">
      <code>.ai-tasks/policy.toml</code> · every agent tool call is checked against it; agents cannot edit it.
    </span>
  </div>
  {#if mode === "screen"}
    <PolicyForm {repo} onsaved={() => {}} />
  {:else}
  <div class="bar">
    <div>
      <b>.ai-tasks/policy.toml</b>
      <span class="meta">
        {exists ? "" : "Not created yet: showing the defaults. Save to create it."}
      </span>
    </div>
    <div class="actions">
      <button onclick={() => (text = saved)} disabled={!dirty}>Revert</button>
      <button class="primary" onclick={save} disabled={!dirty && exists}>Save</button>
    </div>
  </div>
  {#if error}<p class="error pad">{error}</p>{/if}
  {#if message}<p class="ok pad">{message}</p>{/if}
  <textarea bind:value={text} spellcheck="false" oninput={() => (message = "")}></textarea>
  <p class="hint pad">
    Every tool call an agent makes is checked against this file by <code>harness-guard</code>. Agents cannot edit it.
  </p>
  {/if}
</div>

<style>
  .tabs {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
  }
  .seg button {
    border: none;
    border-radius: 0;
    padding: 3px 12px;
    font-size: 12px;
  }
  .seg button.on {
    background: var(--accent-soft);
  }
  .policy {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    gap: 8px;
  }
  .meta {
    color: var(--quiet);
    font-size: 12px;
    margin-left: 8px;
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .pad {
    padding: 6px 16px;
    margin: 0;
  }
  .ok {
    color: var(--good);
  }
  .hint {
    color: var(--quiet);
    font-size: 12px;
  }
  textarea {
    flex: 1;
    margin: 12px 16px 0;
    resize: none;
    font-family: ui-monospace, Menlo, monospace;
    font-size: 12.5px;
    line-height: 1.5;
  }
</style>
