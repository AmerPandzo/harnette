// The chat: one Claude conversation per repo (optionally about a task), beside any run.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Ask, RunEvent } from "./types";

export const chat = $state({
  /** Number of the chat these events belong to (see `chat_send`); 0 = none yet. */
  id: 0,
  repo: "",
  /** Task slug the chat is about, "" = the repo. */
  task: "",
  events: [] as RunEvent[],
  asks: [] as Ask[],
  /** Claude's process is up. */
  open: false,
  /** A turn is in progress. */
  busy: false,
  error: "",
  model: "",
  /** The chat's cost so far (results carry running totals). */
  usd: 0,
});

let listening = false;
/** Events of a newer chat whose number `chat_send` has not returned yet. */
const early = new Map<number, RunEvent[]>();

export function initChatEvents() {
  if (listening) return;
  listening = true;
  listen<{ chat: number; event: RunEvent }>("chat-event", ({ payload: { chat: id, event: e } }) => {
    if (e.kind === "usage" || id < chat.id) return; // bookkeeping, or a chat that was replaced
    if (id > chat.id) {
      early.set(id, [...(early.get(id) ?? []), e]);
      return;
    }
    apply(e);
  });
}

function apply(e: RunEvent) {
  chat.events.push(e);
  if (e.kind === "permission_ask") chat.asks.push({ request_id: e.request_id, tool: e.tool, summary: e.summary });
  if (e.kind === "permission_answered") chat.asks = chat.asks.filter((a) => a.request_id !== e.request_id);
  if (e.kind === "session") chat.model = e.model;
  if (e.kind === "finished") {
    chat.busy = false;
    chat.usd = e.cost_usd;
  }
  if (e.kind === "exited") {
    chat.open = false;
    chat.busy = false;
    chat.asks = [];
  }
}

function clear() {
  chat.id = 0;
  chat.events = [];
  chat.asks = [];
  chat.open = false;
  chat.busy = false;
  chat.error = "";
  chat.model = "";
  chat.usd = 0;
}

export async function sendChat(args: { repo: string; task: string; text: string; images: string[]; model: string }) {
  // Another repo or task is another conversation.
  if (args.repo !== chat.repo || args.task !== chat.task) clear();
  chat.repo = args.repo;
  chat.task = args.task;
  chat.error = "";
  chat.busy = true;
  try {
    const id = await invoke<number>("chat_send", {
      repo: args.repo,
      task: args.task || null,
      text: args.text,
      images: args.images.length ? args.images : null,
      model: args.model || null,
    });
    if (id !== chat.id) {
      // A new process (the view marks where it starts): take the events that arrived before we knew its number.
      chat.id = id;
      chat.open = true;
      for (const e of early.get(id) ?? []) apply(e);
      for (const k of early.keys()) if (k <= id) early.delete(k);
    }
  } catch (e) {
    chat.busy = false;
    chat.error = String(e);
  }
}

export const interruptChat = () => invoke("chat_interrupt").catch((e) => (chat.error = String(e)));

export async function endChat() {
  await invoke("chat_end");
  clear();
}
