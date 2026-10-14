<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";

  type Access = "allow" | "ask" | "deny";
  type Policy = {
    roles: { analyst: string; implementer: string; reviewer: string; on_quota_exhausted: "ask" | "reassign" | "stop" };
    git: { deny: string[] };
    shell: { allow: string[]; single_command: boolean; unlisted: Access; ask_timeout_min: number };
    read: { outside_repo: Access };
    edits: { scope: "task" | "repo"; on_out_of_scope: "warn" | "block" };
    web: { access: Access };
    budget: {
      tokens_per_task: number | null;
      usd_per_task: number | null;
      minutes_per_run: number | null;
      idle_minutes: number | null;
      on_breach: "warn" | "pause" | "stop";
    };
    build: { command: string | null; requires: string[] };
    run: { max_rounds: number };
    delivery: { mode: "none" | "commit" | "push" | "pr"; approval: "ask" | "auto"; branch: string; protected: string[]; branch_at_start: boolean };
    copilot: { model: string; instructions: string; excluded_tools: string[] };
    plugins: { enabled: string[] };
  };
  type FormData = {
    values: Policy;
    defaults: Policy;
    detected: { command: string; allow: string; docker: boolean } | null;
    exists: boolean;
  };

  let { repo, onsaved }: { repo: string; onsaved: () => void } = $props();

  let data = $state<FormData | null>(null);
  let form = $state<Policy | null>(null);
  let saved = $state("");
  let error = $state("");
  let message = $state("");
  let addAllow = $state("");
  let addProtected = $state("");

  const dirty = $derived(form !== null && JSON.stringify(form) !== saved);

  async function load() {
    try {
      const d = await invoke<FormData>("policy_form", { repo });
      data = d;
      form = structuredClone(d.values);
      saved = JSON.stringify(d.values);
      error = "";
    } catch (e) {
      error = String(e);
    }
  }

  $effect(() => {
    void repo;
    message = "";
    load();
  });

  /** Only the keys the form edits; everything else in the file stays as written. */
  function values(p: Policy) {
    const { delivery, ...rest } = p;
    const { force_push: _f, ...deliveryRest } = delivery as Policy["delivery"] & { force_push?: boolean };
    return { ...rest, delivery: deliveryRest };
  }

  async function save() {
    if (!form) return;
    try {
      await invoke("save_policy_values", { repo, values: values($state.snapshot(form) as Policy) });
      message = "Saved. Applies to the next run.";
      await load();
      onsaved();
    } catch (e) {
      error = String(e);
      message = "";
    }
  }

  // ---- Autonomy presets (the spec's autonomy dial) ----
  type Preset = { name: string; hint: string; apply: (p: Policy) => void; matches: (p: Policy) => boolean };
  const presets: Preset[] = [
    {
      name: "Supervised",
      hint: "Anything not allowed is asked; asks wait 30 min; delivery and quota hand-overs need you.",
      apply: (p) => {
        p.shell.unlisted = "ask";
        p.read.outside_repo = "ask";
        p.web.access = "ask";
        p.shell.ask_timeout_min = 30;
        p.roles.on_quota_exhausted = "ask";
        p.budget.on_breach = "pause";
        p.delivery.approval = "ask";
      },
      matches: (p) =>
        p.shell.unlisted === "ask" && p.read.outside_repo === "ask" && p.web.access === "ask" && p.shell.ask_timeout_min >= 30 &&
        p.roles.on_quota_exhausted === "ask" && p.budget.on_breach === "pause" && p.delivery.approval === "ask",
    },
    {
      name: "Balanced",
      hint: "The defaults: unlisted commands are asked (10 min), budgets pause, delivery asks.",
      apply: (p) => {
        p.shell.unlisted = "ask";
        p.read.outside_repo = "ask";
        p.web.access = "ask";
        p.shell.ask_timeout_min = 10;
        p.roles.on_quota_exhausted = "ask";
        p.budget.on_breach = "pause";
        p.delivery.approval = "ask";
      },
      matches: (p) =>
        p.shell.unlisted === "ask" && p.read.outside_repo === "ask" && p.web.access === "ask" && p.shell.ask_timeout_min < 30 &&
        p.roles.on_quota_exhausted === "ask" && p.budget.on_breach === "pause" && p.delivery.approval === "ask",
    },
    {
      name: "Autonomous",
      hint: "Never waits for you: what is not allowed is denied, quota goes to Claude, budgets stop, delivery runs itself.",
      apply: (p) => {
        p.shell.unlisted = "deny";
        p.read.outside_repo = "deny";
        p.web.access = "deny";
        p.roles.on_quota_exhausted = "reassign";
        p.budget.on_breach = "stop";
        p.delivery.approval = "auto";
      },
      matches: (p) =>
        p.shell.unlisted === "deny" && p.read.outside_repo === "deny" && p.web.access === "deny" &&
        p.roles.on_quota_exhausted === "reassign" && p.budget.on_breach === "stop" && p.delivery.approval === "auto",
    },
  ];
  const activePreset = $derived(form ? (presets.find((p) => p.matches(form!))?.name ?? "Custom") : "");

  // ---- Workers ----
  const roles = ["analyst", "implementer", "reviewer"] as const;
  function workerOf(spec: string) {
    const [agent, ...m] = spec.split(":");
    return { agent: agent.trim(), model: m.join(":").trim() };
  }
  function setWorker(role: (typeof roles)[number], agent: string, model: string) {
    if (!form) return;
    form.roles[role] = model.trim() ? `${agent}:${model.trim()}` : agent;
  }

  // ---- Shell packs ----
  const packs: Record<string, string[]> = {
    Maven: ["mvn", "./mvnw"],
    Gradle: ["./gradlew", "gradle"],
    npm: ["npm test", "npm run", "npx"],
    Cargo: ["cargo test", "cargo build", "cargo check", "cargo clippy"],
    Go: ["go test", "go build", "go vet"],
  };
  function hasPack(cmds: string[]) {
    return !!form && cmds.every((c) => form!.shell.allow.includes(c));
  }
  function togglePack(cmds: string[]) {
    if (!form) return;
    form.shell.allow = hasPack(cmds)
      ? form.shell.allow.filter((c) => !cmds.includes(c))
      : [...form.shell.allow, ...cmds.filter((c) => !form!.shell.allow.includes(c))];
  }
  function addTo(list: string[], value: string) {
    const v = value.trim();
    if (v && !list.includes(v)) list.push(v);
  }

  const gitVerbs = $derived(
    form && data ? [...new Set([...data.defaults.git.deny, "cherry-pick", "tag", "am", "apply", ...form.git.deny])] : [],
  );
  function toggleVerb(v: string) {
    if (!form) return;
    form.git.deny = form.git.deny.includes(v) ? form.git.deny.filter((x) => x !== v) : [...form.git.deny, v];
  }

  // ---- Budgets: a checkbox turns a limit on (with a sensible start value) or off (null) ----
  type BudgetKey = "tokens_per_task" | "usd_per_task" | "minutes_per_run" | "idle_minutes";
  const budgets: { key: BudgetKey; label: string; min: number; max: number; step: number; start: number; unit: string }[] = [
    { key: "tokens_per_task", label: "Tokens per task", min: 100_000, max: 10_000_000, step: 100_000, start: 2_000_000, unit: "tokens" },
    { key: "usd_per_task", label: "Cost per task", min: 0.5, max: 50, step: 0.5, start: 5, unit: "$" },
    { key: "minutes_per_run", label: "Time per run", min: 5, max: 240, step: 5, start: 45, unit: "min" },
    { key: "idle_minutes", label: "Idle", min: 2, max: 60, step: 1, start: 10, unit: "min" },
  ];
  function fmtBudget(key: BudgetKey, v: number) {
    if (key === "tokens_per_task") return v >= 1_000_000 ? `${(v / 1_000_000).toFixed(1)}M` : `${Math.round(v / 1000)}k`;
    if (key === "usd_per_task") return `$${v.toFixed(2)}`;
    return `${v} min`;
  }

  // ---- Plugins: installed ones, plus enabled ids that are no longer installed ----
  let installed = $state<string[]>([]);
  invoke<string[]>("list_plugins").then((p) => (installed = p), () => {});
  const plugins = $derived(form ? [...new Set([...installed, ...form.plugins.enabled])] : []);
  function togglePlugin(id: string) {
    if (!form) return;
    const on = form.plugins.enabled;
    form.plugins.enabled = on.includes(id) ? on.filter((x) => x !== id) : [...on, id];
  }

  const docker = $derived(!!form?.build.requires.includes("docker"));
  function setDocker(on: boolean) {
    if (!form) return;
    form.build.requires = on ? [...form.build.requires.filter((r) => r !== "docker"), "docker"] : form.build.requires.filter((r) => r !== "docker");
  }
</script>

{#snippet seg<T extends string>(value: T, options: T[], set: (v: T) => void, labels: Record<string, string> = {})}
  <span class="seg">
    {#each options as o (o)}
      <button class:on={value === o} onclick={() => set(o)}>{labels[o] ?? o}</button>
    {/each}
  </span>
{/snippet}

{#snippet chips(list: string[], remove: (v: string) => void)}
  {#each list as c (c)}
    <span class="chip"><code>{c}</code><button class="x" onclick={() => remove(c)} title="Remove">×</button></span>
  {/each}
{/snippet}

{#if error}<p class="error pad">{error}</p>{/if}
{#if form && data}
  <div class="bar">
    <div class="presets">
      <span class="label">Autonomy</span>
      {#each presets as p (p.name)}
        <button class:on={activePreset === p.name} title={p.hint} onclick={() => p.apply(form!)}>{p.name}</button>
      {/each}
      {#if activePreset === "Custom"}<span class="meta">Custom</span>{/if}
    </div>
    <div class="actions">
      {#if message}<span class="ok">{message}</span>{/if}
      {#if !data.exists}<span class="meta">Not created yet: these are the defaults.</span>{/if}
      <button onclick={() => (form = JSON.parse(saved))} disabled={!dirty}>Revert</button>
      <button class="primary" onclick={save} disabled={!dirty && data.exists}>Save</button>
    </div>
  </div>

  <div class="grid">
    <section>
      <h3>Workers</h3>
      {#each roles as role (role)}
        {@const w = workerOf(form.roles[role])}
        <div class="row">
          <span class="name">{role[0].toUpperCase() + role.slice(1)}</span>
          <select value={w.agent} onchange={(e) => setWorker(role, e.currentTarget.value, w.model)}>
            <option value="claude">Claude Code</option>
            <option value="copilot">Copilot CLI</option>
          </select>
          <input
            class="grow"
            value={w.model}
            placeholder={w.agent === "copilot" ? form.copilot.model || "Copilot default" : "toolbar model"}
            onchange={(e) => setWorker(role, w.agent, e.currentTarget.value)}
          />
        </div>
      {/each}
      <div class="row">
        <span class="name">Out of quota</span>
        {@render seg(form.roles.on_quota_exhausted, ["ask", "reassign", "stop"], (v) => (form!.roles.on_quota_exhausted = v), {
          reassign: "hand to Claude",
        })}
      </div>
    </section>

    <section>
      <h3>Commands</h3>
      <div class="row">
        <span class="name">Packs</span>
        {#each Object.entries(packs) as [name, cmds] (name)}
          <button class="pack" class:on={hasPack(cmds)} onclick={() => togglePack(cmds)} title={cmds.join(", ")}>{name}</button>
        {/each}
      </div>
      <div class="chips">
        {@render chips(form.shell.allow, (c) => (form!.shell.allow = form!.shell.allow.filter((x) => x !== c)))}
        <input
          class="add"
          bind:value={addAllow}
          placeholder="+ command prefix"
          onkeydown={(e) => {
            if (e.key === "Enter") {
              addTo(form!.shell.allow, addAllow);
              addAllow = "";
            }
          }}
        />
      </div>
      <p class="hint">Prefixes match word by word: <code>git diff</code> allows <code>git diff --stat</code>.</p>
      <div class="row">
        <span class="name">Anything else</span>
        {@render seg(form.shell.unlisted, ["ask", "deny", "allow"] as Access[], (v) => (form!.shell.unlisted = v))}
      </div>
      <div class="row">
        <label class="name"><input type="checkbox" bind:checked={form.shell.single_command} /> One command per call</label>
        <span class="hint">refuse pipes, <code>&amp;&amp;</code>, redirects (with a reason the agent can act on)</span>
      </div>
      <div class="row">
        <span class="name">Unanswered asks</span>
        <input type="range" min="1" max="60" bind:value={form.shell.ask_timeout_min} />
        <span class="val">denied after {form.shell.ask_timeout_min} min</span>
      </div>
    </section>

    <section>
      <h3>Git, reach and edits</h3>
      <p class="hint">
        Git commands agents <b>may</b> run (unticked = blocked). Branches and commits are the app's job, not the agent's:
        see <b>Delivery</b> below.
      </p>
      <div class="verbs">
        {#each gitVerbs as v (v)}
          {#if v === "push"}
            <label class="verb blocked" title="Never: only the app pushes, after you approve delivery"
              ><input type="checkbox" disabled /> push</label
            >
          {:else}
            <label class="verb" class:blocked={form.git.deny.includes(v)}
              ><input type="checkbox" checked={!form.git.deny.includes(v)} onchange={() => toggleVerb(v)} /> {v}</label
            >
          {/if}
        {/each}
      </div>
      <div class="row">
        <span class="name">Read outside repo</span>
        {@render seg(form.read.outside_repo, ["ask", "deny", "allow"] as Access[], (v) => (form!.read.outside_repo = v))}
      </div>
      <div class="row">
        <span class="name">Web</span>
        {@render seg(form.web.access, ["ask", "deny", "allow"] as Access[], (v) => (form!.web.access = v))}
      </div>
      <div class="row">
        <span class="name">Implementer edits</span>
        {@render seg(form.edits.scope, ["task", "repo"], (v) => (form!.edits.scope = v), { task: "task files", repo: "whole repo" })}
      </div>
      {#if form.edits.scope === "task"}
        <div class="row">
          <span class="name">Other files</span>
          {@render seg(form.edits.on_out_of_scope, ["warn", "block"], (v) => (form!.edits.on_out_of_scope = v))}
        </div>
        <p class="hint">Task files = paths the task doc names in backticks (e.g. <code>src/a.js</code>, <code>src/util/</code>). A doc naming none has no limit.</p>
      {/if}
    </section>

    <section>
      <h3>Budgets</h3>
      {#each budgets as b (b.key)}
        {@const v = form.budget[b.key]}
        <div class="row">
          <label class="name">
            <input
              type="checkbox"
              checked={v !== null}
              onchange={(e) => (form!.budget[b.key] = e.currentTarget.checked ? b.start : null)}
            />
            {b.label}
          </label>
          {#if v !== null}
            <input type="range" min={b.min} max={b.max} step={b.step} bind:value={form.budget[b.key]} />
            <span class="val">{fmtBudget(b.key, v)}</span>
          {:else}
            <span class="hint">no limit</span>
          {/if}
        </div>
      {/each}
      <div class="row">
        <span class="name">At 100%</span>
        {@render seg(form.budget.on_breach, ["warn", "pause", "stop"], (v) => (form!.budget.on_breach = v))}
      </div>
      <p class="hint">80% always warns. Copilot runs report no tokens or cost; time and idle limits still apply.</p>
    </section>

    <section>
      <h3>Build and rounds</h3>
      <div class="row">
        <span class="name">Build</span>
        <input
          class="grow mono"
          value={form.build.command ?? ""}
          placeholder="none"
          onchange={(e) => (form!.build.command = e.currentTarget.value.trim() || null)}
        />
      </div>
      {#if data.detected && data.detected.command !== form.build.command}
        <div class="row">
          <span class="name"></span>
          <button
            class="link"
            onclick={() => {
              form!.build.command = data!.detected!.command;
              addTo(form!.shell.allow, data!.detected!.allow);
              if (data!.detected!.docker) setDocker(true);
            }}>Use detected: <code>{data.detected.command}</code>{data.detected.docker ? " + Docker" : ""}</button
          >
        </div>
      {/if}
      <div class="row">
        <label class="name"><input type="checkbox" checked={docker} onchange={(e) => setDocker(e.currentTarget.checked)} /> Needs Docker</label>
        <span class="hint">checked before every build (Testcontainers, OrbStack)</span>
      </div>
      <div class="row">
        <span class="name">Max rounds</span>
        <input type="range" min="1" max="10" bind:value={form.run.max_rounds} />
        <span class="val">{form.run.max_rounds}</span>
      </div>
    </section>

    <section>
      <h3>Delivery</h3>
      <div class="row">
        <span class="name">After Done</span>
        {@render seg(form.delivery.mode, ["none", "commit", "push", "pr"], (v) => (form!.delivery.mode = v), {
          none: "nothing",
          pr: "PR",
        })}
      </div>
      <div class="row">
        <span class="name">Task branch</span>
        <input class="grow mono" bind:value={form.delivery.branch} />
      </div>
      <div class="row">
        <span class="name"></span>
        <label class="name wide"
          ><input type="checkbox" bind:checked={form.delivery.branch_at_start} /> Switch to it when a run starts (from a protected branch like develop)</label
        >
      </div>
      {#if form.delivery.mode !== "none"}
        <div class="row">
          <span class="name">Approval</span>
          {@render seg(form.delivery.approval, ["ask", "auto"], (v) => (form!.delivery.approval = v))}
        </div>
      {/if}
      <p class="hint">Never touched (globs allowed):</p>
      <div class="chips">
        {@render chips(form.delivery.protected, (c) => (form!.delivery.protected = form!.delivery.protected.filter((x) => x !== c)))}
        <input
          class="add"
          bind:value={addProtected}
          placeholder="+ branch"
          onkeydown={(e) => {
            if (e.key === "Enter") {
              addTo(form!.delivery.protected, addProtected);
              addProtected = "";
            }
          }}
        />
      </div>
      <p class="hint">Kormilant never force-pushes.</p>
    </section>

    <section>
      <h3>Skills & plugins</h3>
      {#if plugins.length}
        <div class="row">
          {#each plugins as id (id)}
            <button class="pack" class:on={form.plugins.enabled.includes(id)} onclick={() => togglePlugin(id)} title={id}>
              {id.split("@")[0]}{#if !installed.includes(id)} (not installed){/if}
            </button>
          {/each}
        </div>
      {:else}
        <p class="hint">No Claude Code plugins installed (<code>~/.claude/plugins/</code>).</p>
      {/if}
      <p class="hint">Claude workers get only the plugins switched on here (your user settings are not read). Their skills and hooks come with them.</p>
    </section>

    {#if roles.some((r) => workerOf(form!.roles[r]).agent === "copilot")}
      <section>
        <h3>Copilot</h3>
        <div class="row">
          <span class="name">Default model</span>
          <input class="grow mono" bind:value={form.copilot.model} />
        </div>
        <div class="row">
          <span class="name">Instructions</span>
          <input class="grow mono" bind:value={form.copilot.instructions} placeholder="none" />
        </div>
        <p class="hint">Appended to every Copilot prompt when the file exists (Copilot CLI does not read IntelliJ's).</p>
        <p class="hint">Hidden tools:</p>
        <div class="chips">
          {@render chips(form.copilot.excluded_tools, (c) => (form!.copilot.excluded_tools = form!.copilot.excluded_tools.filter((x) => x !== c)))}
        </div>
      </section>
    {/if}
  </div>
{/if}

<style>
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .presets,
  .actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .presets button.on,
  .pack.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }
  .label,
  .meta {
    color: var(--quiet);
    font-size: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(360px, 1fr));
    gap: 12px;
    padding: 12px 16px;
    overflow: auto;
    flex: 1;
    min-height: 0;
    align-content: start;
  }
  section {
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  h3 {
    margin: 0 0 4px;
    font-size: 13px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .name {
    width: 130px;
    flex-shrink: 0;
    font-size: 12.5px;
  }
  label.name {
    width: auto;
    min-width: 130px;
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .grow {
    flex: 1;
    min-width: 80px;
  }
  .mono,
  code {
    font-family: ui-monospace, Menlo, monospace;
    font-size: 12px;
  }
  .val {
    font-size: 12px;
    min-width: 64px;
  }
  input[type="range"] {
    flex: 1;
    min-width: 100px;
  }
  .hint {
    color: var(--quiet);
    font-size: 12px;
    margin: 0;
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
    padding: 3px 10px;
    font-size: 12px;
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--ink);
  }
  .chips,
  .verbs {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    background: var(--code-bg);
    border-radius: 4px;
    padding: 1px 2px 1px 6px;
  }
  .x {
    border: none;
    background: none;
    padding: 0 4px;
    color: var(--quiet);
    cursor: pointer;
  }
  .add {
    width: 140px;
    font-size: 12px;
  }
  .verb {
    font-size: 12px;
    display: inline-flex;
    gap: 3px;
    align-items: center;
    min-width: 92px;
  }
  .verb.blocked {
    color: var(--bad);
    text-decoration: line-through;
    text-decoration-color: color-mix(in srgb, var(--bad) 50%, transparent);
  }
  .name.wide {
    width: auto;
    flex: 1;
    min-width: 0;
    font-weight: 400;
    white-space: normal;
  }
  .pack {
    font-size: 12px;
    padding: 2px 8px;
  }
  .link {
    border: none;
    background: none;
    color: var(--accent);
    cursor: pointer;
    padding: 0;
    font-size: 12px;
  }
  .ok {
    color: var(--good);
    font-size: 12px;
  }
  .pad {
    padding: 6px 16px;
    margin: 0;
  }
</style>
