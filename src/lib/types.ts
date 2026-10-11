// Mirrors harness_core types (serde snake_case).

export type StatusKind = "analysis" | "ready" | "in_review" | "changes_requested" | "done" | "unknown";

export type TaskSummary = {
  slug: string;
  title: string;
  status: string;
  status_kind: StatusKind;
  criteria_done: number;
  criteria_total: number;
  open_findings: number;
  blockers: number;
  open_questions: number;
  modified_ms: number;
};

export const COLUMNS: { kind: StatusKind; label: string }[] = [
  { kind: "analysis", label: "Analysis" },
  { kind: "ready", label: "Ready" },
  { kind: "in_review", label: "In review" },
  { kind: "changes_requested", label: "Changes requested" },
  { kind: "done", label: "Done" },
];

export const STATUS_LABELS: Record<Exclude<StatusKind, "unknown">, string> = {
  analysis: "Analysis",
  ready: "Ready for implementation",
  in_review: "In review",
  changes_requested: "Changes requested",
  done: "Done",
};

export type Finished = {
  kind: "finished";
  ok: boolean;
  result: string;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  duration_ms: number;
};

export type RunEvent =
  | { kind: "started"; run_id: string; slug: string }
  | { kind: "session"; session_id: string; model: string }
  | { kind: "text"; text: string }
  | { kind: "tool_use"; id: string; tool: string; summary: string }
  | { kind: "tool_result"; tool_use_id: string; is_error: boolean; summary: string }
  | { kind: "permission_auto"; request_id: string; tool: string; summary: string; allowed: boolean; rule: string }
  | { kind: "permission_ask"; request_id: string; tool: string; summary: string }
  | { kind: "permission_answered"; request_id: string; allowed: boolean; by: string }
  | { kind: "note"; text: string }
  | { kind: "quota"; five_hour: number; seven_day: number }
  | Finished
  | { kind: "error"; message: string }
  | { kind: "exited"; code: number | null };

export type Ask = { request_id: string; tool: string; summary: string };

export function remembered(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

export function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}
