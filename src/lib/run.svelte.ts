// The single active run, shared by every view.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Ask, Finished, RunEvent } from "./types";

export const run = $state({
  running: false,
  slug: "",
  mode: "",
  error: "",
  model: "",
  events: [] as RunEvent[],
  asks: [] as Ask[],
  quota: null as { status: string; five_hour: number; seven_day: number } | null,
  paused: null as string | null,
  alarms: [] as { at: number; alarm: string; message: string; action: string }[],
  /** Latest warning not yet dismissed. */
  warning: null as string | null,
  startedAt: 0,
  tokens: 0,
  usd: 0,
  role: "",
  /** A loop finished Done and delivery waits for approval. */
  deliveryReady: null as { slug: string } | null,
  finished: null as Finished | null,
  ended: null as { ok: boolean; summary: string } | null,
});

const exitListeners: (() => void)[] = [];
let listening = false;

export function onRunExit(f: () => void) {
  exitListeners.push(f);
}

export function initRunEvents() {
  if (listening) return;
  listening = true;
  const seen = new Set<string>();
  listen<RunEvent>("run-event", ({ payload: e }) => {
    if (e.kind === "usage") {
      // Usage is bookkeeping, not a journal row.
      if (!seen.has(e.message_id)) {
        seen.add(e.message_id);
        run.tokens += e.new_tokens;
      }
      return;
    }
    run.events.push(e);
    if (e.kind === "alarm") {
      run.alarms.push({ at: Date.now(), alarm: e.alarm, message: e.message, action: e.action });
      if (e.action === "warn") run.warning = e.message;
    }
    if (e.kind === "paused") run.paused = e.reason;
    if (e.kind === "resumed") run.paused = null;
    if (e.kind === "phase") run.role = e.role;
    if (e.kind === "delivery_ready") run.deliveryReady = { slug: run.slug };
    if (e.kind === "delivered") run.deliveryReady = null;
    if (e.kind === "permission_ask") run.asks.push({ request_id: e.request_id, tool: e.tool, summary: e.summary });
    if (e.kind === "permission_answered") run.asks = run.asks.filter((a) => a.request_id !== e.request_id);
    if (e.kind === "quota") run.quota = { status: e.status, five_hour: e.five_hour, seven_day: e.seven_day };
    if (e.kind === "session") run.model = e.model;
    if (e.kind === "finished") {
      run.finished = e;
      run.usd += e.cost_usd;
    }
    // An agent exiting ends only its own asks; a loop keeps running until the session ends.
    if (e.kind === "exited") run.asks = [];
    if (e.kind === "session_ended") {
      run.running = false;
      run.asks = [];
      run.paused = null;
      run.ended = { ok: e.ok, summary: e.summary };
      exitListeners.forEach((f) => f());
    }
  });
}

function begin(slug: string, mode: string) {
  run.error = "";
  run.events = [];
  run.asks = [];
  run.finished = null;
  run.ended = null;
  run.model = "";
  run.slug = slug;
  run.mode = mode;
  run.running = true;
  run.paused = null;
  run.warning = null;
  run.startedAt = Date.now();
  run.tokens = 0;
  run.usd = 0;
  run.role = mode === "loop" ? "" : "analyst";
}

async function guarded(f: () => Promise<unknown>) {
  try {
    await f();
  } catch (e) {
    run.running = false;
    run.error = String(e);
    throw e;
  }
}

export async function startRun(args: { repo: string; slug: string; mode: "analyze" | "refine"; ticket?: string; model?: string }) {
  begin(args.slug, args.mode);
  await guarded(() => invoke("start_run", { ...args, ticket: args.ticket ?? null, model: args.model || null }));
}

export async function startLoop(args: { repo: string; slug: string; model?: string }) {
  begin(args.slug, "loop");
  await guarded(() => invoke("start_loop", { ...args, model: args.model || null }));
}

export const answer = (a: Ask, allow: boolean) => invoke("answer_permission", { requestId: a.request_id, allow });
export const stopRun = () => invoke("stop_run");
export const pauseRun = () => invoke("pause_run");
export const resumeRun = () => invoke("resume_run");
export const sendNote = (text: string) => invoke("send_note", { text });
