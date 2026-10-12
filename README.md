# Kormilant

Local-first control plane for AI coding agents: task specs, enforced guardrails, a run journal, live allow/deny and steering.
Phase 1: task board, run loop (implement → build → review) on Claude Code or Copilot CLI, policy screen, watchdog, review pane, delivery.

## Layout

- `crates/kormilant-core` — agent adapter (Claude Code stream-json control protocol), policy, journal, prompts. No UI code.
- `src-tauri` — Tauri 2 shell: commands `start_run`, `answer_permission`, `send_note`, `stop_run`; emits `run-event`.
- `src` — Svelte 5 UI.

## Run

```sh
npm install
npm run tauri dev
```

Packaged app (unsigned for distribution, ad-hoc signed for this Mac):

```sh
npm run tauri build                     # → target/release/bundle/macos/Kormilant.app
ditto target/release/bundle/macos/Kormilant.app /Applications/Kormilant.app
```

`kormilant-guard` is bundled into `Contents/MacOS/`. Started from Finder, the app takes its PATH from your login shell, so `claude`, `copilot`, build tools and `docker` resolve as in a terminal.

Terminal shell over the core (no GUI):

```sh
cargo run -p kormilant-core --example spike_cli -- <repo> <slug> <ticket-file> [model]
# then type: y | n | note <text> | stop
```

## Test

```sh
cargo test -p kormilant-core
npm run check
```

Requires the `claude` CLI on PATH, logged in.
