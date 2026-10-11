# Harness

Local-first control plane for AI coding agents: task specs, enforced guardrails, a run journal, live allow/deny and steering.
Phase 0 spike: one window that runs **Analyze** with Claude Code.

## Layout

- `crates/harness-core` — agent adapter (Claude Code stream-json control protocol), policy, journal, prompts. No UI code.
- `src-tauri` — Tauri 2 shell: commands `start_run`, `answer_permission`, `send_note`, `stop_run`; emits `run-event`.
- `src` — Svelte 5 UI.

## Run

```sh
npm install
npm run tauri dev
```

Terminal shell over the core (no GUI):

```sh
cargo run -p harness-core --example spike_cli -- <repo> <slug> <ticket-file> [model]
# then type: y | n | note <text> | stop
```

## Test

```sh
cargo test -p harness-core
npm run check
```

Requires the `claude` CLI on PATH, logged in.
