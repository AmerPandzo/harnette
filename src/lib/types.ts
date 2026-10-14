// Mirrors kormilant_core types (serde snake_case).

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
  /** Related repositories (the doc's `**Repos:**` line). */
  repos: string[];
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
  new_tokens: number;
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
  | { kind: "note"; text: string; images?: string[] }
  | { kind: "usage"; message_id: string; new_tokens: number }
  | { kind: "quota"; status: string; five_hour: number; seven_day: number }
  | Finished
  | { kind: "error"; message: string }
  | { kind: "exited"; code: number | null }
  | { kind: "phase"; round: number; role: string }
  | { kind: "checkpoint"; round: number; git_ref: string }
  | { kind: "build"; ok: boolean; command: string; duration_ms: number; tail: string; log: string }
  | { kind: "status_changed"; to: string; by: string; reason: string | null }
  | { kind: "session_ended"; ok: boolean; summary: string }
  | { kind: "alarm"; alarm: string; message: string; action: "warn" | "pause" | "stop" }
  | { kind: "paused"; reason: string }
  | { kind: "resumed" }
  | { kind: "info"; message: string }
  | { kind: "delivery_ready"; mode: string; branch: string; files: string[]; commands: string[] }
  | { kind: "delivered"; branch: string; commit: string | null; pushed: boolean; pr_url: string | null };

export type DeliveryPlan = {
  /** Member repo name; "" for the primary repo. */
  repo: string;
  slug: string;
  mode: string;
  branch: string;
  create_branch: boolean;
  files: string[];
  message: string;
  pr_title: string;
  pr_body: string;
  commands: string[];
  /** Member repos with changes, delivered after the primary repo. */
  related: DeliveryPlan[];
};

export type UsageSummary = {
  day_tokens: number;
  day_usd: number;
  day_runs: number;
  week_tokens: number;
  week_usd: number;
  week_runs: number;
  top_tasks: { repo: string; slug: string; new_tokens: number; usd: number }[];
};

export function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

export type Checkpoint = { round: number; git_ref: string; commit: string; created_unix: number };

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
