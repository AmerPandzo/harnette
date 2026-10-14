// Board actions the chat proposes as ```kormilant-action {json}``` blocks. Nothing here runs them:
// the chat view shows a button per action and runs it only on the human's click.

export type ChatAction =
  | { action: "analyze"; slug: string; ticket: string }
  | { action: "refine"; slug: string; message: string }
  | { action: "run"; slug: string }
  | { action: "move"; slug: string; status: "analysis" | "ready" | "in_review" | "changes_requested" | "done" };

export type Segment = { t: "md"; text: string } | { t: "action"; action: ChatAction } | { t: "bad"; raw: string; why: string };

const STATUSES = ["analysis", "ready", "in_review", "changes_requested", "done"];
const SLUG = /^[A-Za-z0-9._-]+$/;

/** Agent-written JSON: accept only the four shapes, with a valid slug. */
export function toAction(raw: string): ChatAction | string {
  let v: Record<string, unknown>;
  try {
    v = JSON.parse(raw);
  } catch {
    return "not valid JSON";
  }
  const str = (k: string) => (typeof v?.[k] === "string" ? (v[k] as string).trim() : "");
  const slug = str("slug");
  if (!SLUG.test(slug) || slug.includes("..")) return "missing or invalid slug";
  switch (v.action) {
    case "analyze":
      return str("ticket") ? { action: "analyze", slug, ticket: str("ticket") } : "analyze needs a ticket";
    case "refine":
      return { action: "refine", slug, message: str("message") };
    case "run":
      return { action: "run", slug };
    case "move":
      return STATUSES.includes(str("status")) ? ({ action: "move", slug, status: str("status") } as ChatAction) : "unknown status";
    default:
      return `unknown action ${JSON.stringify(v?.action)}`;
  }
}

/** Split assistant text into markdown and action blocks. */
export function segments(text: string): Segment[] {
  const out: Segment[] = [];
  const re = /```kormilant-action[^\n]*\n([\s\S]*?)```/g;
  let at = 0;
  for (let m; (m = re.exec(text)); ) {
    if (m.index > at) out.push({ t: "md", text: text.slice(at, m.index) });
    const a = toAction(m[1].trim());
    out.push(typeof a === "string" ? { t: "bad", raw: m[1].trim(), why: a } : { t: "action", action: a });
    at = m.index + m[0].length;
  }
  if (at < text.length) out.push({ t: "md", text: text.slice(at) });
  return out.filter((s) => s.t !== "md" || s.text.trim());
}
