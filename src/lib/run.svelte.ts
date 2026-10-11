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
  quota: null as { five_hour: number; seven_day: number } | null,
  finished: null as Finished | null,
});

const exitListeners: (() => void)[] = [];
let listening = false;

export function onRunExit(f: () => void) {
  exitListeners.push(f);
}

export function initRunEvents() {
  if (listening) return;
  listening = true;
  listen<RunEvent>("run-event", ({ payload: e }) => {
    run.events.push(e);
    if (e.kind === "permission_ask") run.asks.push({ request_id: e.request_id, tool: e.tool, summary: e.summary });
    if (e.kind === "permission_answered") run.asks = run.asks.filter((a) => a.request_id !== e.request_id);
    if (e.kind === "quota") run.quota = { five_hour: e.five_hour, seven_day: e.seven_day };
    if (e.kind === "session") run.model = e.model;
    if (e.kind === "finished") run.finished = e;
    if (e.kind === "exited") {
      run.running = false;
      run.asks = [];
      exitListeners.forEach((f) => f());
    }
  });
}

export async function startRun(args: { repo: string; slug: string; mode: "analyze" | "refine"; ticket?: string; model?: string }) {
  run.error = "";
  run.events = [];
  run.asks = [];
  run.finished = null;
  run.model = "";
  run.slug = args.slug;
  run.mode = args.mode;
  run.running = true;
  try {
    await invoke("start_run", { ...args, ticket: args.ticket ?? null, model: args.model || null });
  } catch (e) {
    run.running = false;
    run.error = String(e);
    throw e;
  }
}

export const answer = (a: Ask, allow: boolean) => invoke("answer_permission", { requestId: a.request_id, allow });
export const stopRun = () => invoke("stop_run");
export const sendNote = (text: string) => invoke("send_note", { text });
