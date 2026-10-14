// The chat: one Claude conversation per repo (optionally about a task), beside any run.
// Each repo's chat is saved in the browser storage, so it survives an app restart; the next message
// then continues the Claude session (`--resume`).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { remember, remembered, type Ask, type RunEvent } from "./types";

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
  /** The chat's cost so far. */
  usd: 0,
  /** Claude's session id, to resume it in a new process. */
  sessionId: "",
});

/** Cost before the current process (each process's results count from zero). */
let usdBase = 0;
const MAX_SAVED_EVENTS = 400;
const savedKey = (repo: string) => `harnette.chat:${repo}`;

type Saved = { task: string; sessionId: string; usd: number; events: RunEvent[] };

function save() {
  if (!chat.repo) return;
  // Tool inputs can be whole files; the view does not need them.
  const events = chat.events.slice(-MAX_SAVED_EVENTS).map((e) => ("input" in e ? { ...e, input: undefined } : e));
  const saved: Saved = { task: chat.task, sessionId: chat.sessionId, usd: chat.usd, events };
  remember(savedKey(chat.repo), JSON.stringify(saved));
}

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
  if (e.kind === "session") {
    chat.model = e.model;
    chat.sessionId = e.session_id;
  }
  if (e.kind === "finished") {
    chat.busy = false;
    chat.usd = usdBase + e.cost_usd;
    save();
  }
  if (e.kind === "exited") {
    chat.open = false;
    chat.busy = false;
    chat.asks = [];
    save();
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
  chat.sessionId = "";
  usdBase = 0;
}

/** Show `repo`'s saved chat. A live chat of another repo is ended (it stays saved). */
export async function loadChat(repo: string) {
  if (chat.repo === repo) return;
  if (chat.open) await invoke("chat_end").catch(() => {});
  clear();
  chat.repo = repo;
  try {
    const saved: Saved | null = JSON.parse(remembered(savedKey(repo), "null"));
    if (saved) {
      chat.task = saved.task;
      chat.sessionId = saved.sessionId;
      chat.usd = usdBase = saved.usd;
      chat.events = saved.events;
    }
  } catch {
    // A corrupt entry is just no saved chat.
  }
}

export async function sendChat(args: { repo: string; task: string; text: string; images: string[]; model: string }) {
  // Another repo or task is another conversation.
  if (args.repo !== chat.repo || args.task !== chat.task) clear();
  const resume = chat.open ? null : chat.sessionId || null;
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
      resume,
    });
    if (id !== chat.id) {
      // A new process (the view marks where it starts): take the events that arrived before we knew its number.
      chat.id = id;
      chat.open = true;
      usdBase = chat.usd;
      for (const e of early.get(id) ?? []) apply(e);
      for (const k of early.keys()) if (k <= id) early.delete(k);
    }
  } catch (e) {
    chat.busy = false;
    chat.error = String(e);
  }
}

export const interruptChat = () => invoke("chat_interrupt").catch((e) => (chat.error = String(e)));

/** New chat: end the process and forget the conversation. */
export async function endChat() {
  await invoke("chat_end");
  const repo = chat.repo;
  clear();
  chat.repo = repo;
  remember(savedKey(repo), "null");
}
