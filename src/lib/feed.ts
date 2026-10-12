// Turns the flat run-event stream into a timeline: one section per agent run, tool calls joined with
// their result and permission decision, runs of reads folded into one "explored" row.

import type { Finished, RunEvent } from "./types";

type Ev<K extends RunEvent["kind"]> = Extract<RunEvent, { kind: K }>;

export type Diff = { path: string; removed: string[]; added: string[]; more: number };

export type Decision = { allowed: boolean; by: string; rule?: string };

export type ToolItem = {
  t: "tool";
  id: string;
  tool: string;
  summary: string;
  diff?: Diff;
  result?: { is_error: boolean; summary: string };
  decision?: Decision;
  waiting: boolean;
};

export type Item =
  | { t: "text"; text: string }
  | ToolItem
  | { t: "explore"; tools: ToolItem[] }
  | { t: "note"; text: string }
  | { t: "build"; e: Ev<"build"> }
  | { t: "status"; e: Ev<"status_changed"> }
  | { t: "checkpoint"; e: Ev<"checkpoint"> }
  | { t: "alarm"; e: Ev<"alarm"> }
  | { t: "info"; text: string }
  | { t: "error"; text: string }
  | { t: "paused"; reason: string }
  | { t: "resumed" }
  | { t: "delivery_ready"; e: Ev<"delivery_ready"> }
  | { t: "delivered"; e: Ev<"delivered"> }
  | { t: "ended"; e: Ev<"session_ended"> };

export type Section = {
  kind: "agent" | "system";
  role: string;
  round: number | null;
  model: string;
  finished: Finished | null;
  exited: boolean;
  items: Item[];
};

const READ_ONLY = new Set(["Read", "Grep", "Glob", "LS", "NotebookRead"]);

export function toolIcon(tool: string): string {
  if (["Read", "NotebookRead"].includes(tool)) return "file";
  if (["Edit", "Write", "MultiEdit", "NotebookEdit"].includes(tool)) return "edit";
  if (tool === "Bash") return "terminal";
  if (["Grep", "Glob", "LS"].includes(tool)) return "search";
  if (["WebFetch", "WebSearch"].includes(tool)) return "globe";
  if (tool === "TodoWrite") return "check";
  return "zap";
}

export function toolVerb(tool: string): string {
  return (
    {
      Read: "Read",
      Edit: "Edit",
      MultiEdit: "Edit",
      Write: "Write",
      Bash: "Run",
      Grep: "Search",
      Glob: "Find",
      LS: "List",
      WebFetch: "Fetch",
      WebSearch: "Web search",
      TodoWrite: "Plan",
    } as Record<string, string>
  )[tool] ?? tool;
}

/** Paths inside the repo are shown relative to it. */
export function shortPath(s: string, repo: string): string {
  const root = repo.endsWith("/") ? repo : repo + "/";
  return s.split(root).join("");
}

export function buildTimeline(events: RunEvent[], initialRole: string): Section[] {
  const sections: Section[] = [];
  let pendingRole = initialRole;
  let pendingRound: number | null = null;
  const byRequest = new Map<string, ToolItem>();

  const current = (): Section => {
    const last = sections.at(-1);
    if (last) return last;
    const s: Section = { kind: "system", role: "", round: null, model: "", finished: null, exited: false, items: [] };
    sections.push(s);
    return s;
  };
  const system = (): Section => {
    const last = sections.at(-1);
    if (last && (last.kind === "system" || !last.exited)) return last;
    const s: Section = { kind: "system", role: "", round: null, model: "", finished: null, exited: false, items: [] };
    sections.push(s);
    return s;
  };
  /** The latest tool call with this summary that has no decision yet. */
  const toolFor = (summary: string): ToolItem | undefined => {
    for (let i = sections.length - 1; i >= 0; i--) {
      const items = sections[i].items;
      for (let j = items.length - 1; j >= 0; j--) {
        const it = items[j];
        if (it.t === "tool" && it.summary === summary && !it.decision) return it;
      }
    }
    return undefined;
  };
  const tools = new Map<string, ToolItem>();

  for (const e of events) {
    switch (e.kind) {
      case "phase":
        pendingRole = e.role;
        pendingRound = e.round;
        break;
      case "started":
        sections.push({ kind: "agent", role: pendingRole || "agent", round: pendingRound, model: "", finished: null, exited: false, items: [] });
        break;
      case "session":
        current().model = e.model;
        break;
      case "text":
        current().items.push({ t: "text", text: e.text });
        break;
      case "tool_use": {
        const ev = e as Ev<"tool_use"> & { diff?: Diff };
        const item: ToolItem = { t: "tool", id: ev.id, tool: ev.tool, summary: ev.summary, diff: ev.diff, waiting: false };
        tools.set(ev.id, item);
        current().items.push(item);
        break;
      }
      case "tool_result": {
        const item = tools.get(e.tool_use_id);
        if (item) item.result = { is_error: e.is_error, summary: e.summary };
        break;
      }
      case "permission_auto": {
        const item = toolFor(e.summary);
        if (item) item.decision = { allowed: e.allowed, by: "policy", rule: e.rule };
        break;
      }
      case "permission_ask": {
        const item = toolFor(e.summary);
        if (item) {
          item.waiting = true;
          byRequest.set(e.request_id, item);
        }
        break;
      }
      case "permission_answered": {
        const item = byRequest.get(e.request_id);
        if (item) {
          item.waiting = false;
          item.decision = { allowed: e.allowed, by: e.by };
        }
        break;
      }
      case "note":
        current().items.push({ t: "note", text: e.text });
        break;
      case "finished":
        current().finished = e;
        break;
      case "exited":
        current().exited = true;
        for (const it of current().items) if (it.t === "tool") it.waiting = false;
        break;
      case "error":
        current().items.push({ t: "error", text: e.message });
        break;
      case "build":
        system().items.push({ t: "build", e });
        break;
      case "status_changed":
        system().items.push({ t: "status", e });
        break;
      case "checkpoint":
        system().items.push({ t: "checkpoint", e });
        break;
      case "alarm":
        current().items.push({ t: "alarm", e });
        break;
      case "info":
        system().items.push({ t: "info", text: e.message });
        break;
      case "paused":
        current().items.push({ t: "paused", reason: e.reason });
        break;
      case "resumed":
        current().items.push({ t: "resumed" });
        break;
      case "delivery_ready":
        system().items.push({ t: "delivery_ready", e });
        break;
      case "delivered":
        system().items.push({ t: "delivered", e });
        break;
      case "session_ended":
        system().items.push({ t: "ended", e });
        break;
    }
  }

  // Fold runs of two or more successful read-only calls into one row.
  for (const s of sections) {
    const out: Item[] = [];
    let run: ToolItem[] = [];
    const flush = () => {
      if (run.length >= 2) out.push({ t: "explore", tools: run });
      else out.push(...run);
      run = [];
    };
    for (const it of s.items) {
      if (it.t === "tool" && READ_ONLY.has(it.tool) && !it.waiting && !it.result?.is_error && it.decision?.allowed !== false) {
        run.push(it);
      } else {
        flush();
        out.push(it);
      }
    }
    flush();
    s.items = out;
  }
  return sections.filter((s) => s.kind === "agent" || s.items.length > 0);
}

export function fmtDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}
