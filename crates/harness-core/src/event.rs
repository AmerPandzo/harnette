//! Normalized run events, parsed from Claude Code's stream-json output.
//! The UI, the journal and the CLI all consume these instead of raw agent JSON.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEvent {
    Started { run_id: String, slug: String },
    Session { session_id: String, model: String },
    Text { text: String },
    ToolUse {
        id: String,
        tool: String,
        summary: String,
        /// Full tool input for policy checks; not sent to the UI or journal (Write content can be large).
        #[serde(skip)]
        input: Value,
    },
    ToolResult { tool_use_id: String, is_error: bool, summary: String },
    /// Decided by policy without asking the human.
    PermissionAuto { request_id: String, tool: String, summary: String, allowed: bool, rule: String },
    /// Waiting for the human.
    PermissionAsk { request_id: String, tool: String, summary: String },
    PermissionAnswered { request_id: String, allowed: bool, by: String },
    Note { text: String },
    Quota { five_hour: f64, seven_day: f64 },
    Finished {
        ok: bool,
        result: String,
        cost_usd: f64,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    },
    Error { message: String },
    Exited { code: Option<i32> },
}

/// One line of agent output, classified.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    Events(Vec<RunEvent>),
    PermissionRequest { request_id: String, tool: String, input: Value },
    Ignored,
}

const SUMMARY_MAX: usize = 240;

pub fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// Short human-readable description of a tool call.
pub fn summarize_tool(tool: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let s = match tool {
        "Bash" => field("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "Grep" | "Glob" => field("pattern"),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        _ => None,
    };
    truncate(&s.unwrap_or_else(|| input.to_string()), SUMMARY_MAX)
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn parse_line(line: &str) -> Parsed {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Parsed::Ignored;
    };
    let str_at = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            Parsed::Events(vec![RunEvent::Session { session_id: str_at(&v, "session_id"), model: str_at(&v, "model") }])
        }
        Some("assistant") => {
            let content = v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
            let events: Vec<RunEvent> = content
                .iter()
                .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = str_at(c, "text");
                        (!text.trim().is_empty()).then_some(RunEvent::Text { text })
                    }
                    Some("tool_use") => {
                        let tool = str_at(c, "name");
                        let input = c.get("input").cloned().unwrap_or(Value::Null);
                        Some(RunEvent::ToolUse { id: str_at(c, "id"), summary: summarize_tool(&tool, &input), tool, input })
                    }
                    _ => None,
                })
                .collect();
            Parsed::Events(events)
        }
        Some("user") => {
            // Replayed user messages (our notes) have string content; tool results come as an array.
            let content = v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
            let events = content
                .iter()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("tool_result"))
                .map(|c| RunEvent::ToolResult {
                    tool_use_id: str_at(c, "tool_use_id"),
                    is_error: c.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                    summary: truncate(&tool_result_text(c.get("content").unwrap_or(&Value::Null)), SUMMARY_MAX),
                })
                .collect();
            Parsed::Events(events)
        }
        Some("rate_limit_event") => {
            let util = |w: &str| {
                v.pointer(&format!("/rate_limit_info/unifiedWindows/{w}/utilization"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            };
            Parsed::Events(vec![RunEvent::Quota { five_hour: util("five_hour"), seven_day: util("seven_day") }])
        }
        Some("result") => {
            let num = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
            let usage = |k: &str| num(&format!("/usage/{k}"));
            Parsed::Events(vec![RunEvent::Finished {
                ok: str_at(&v, "subtype") == "success" && !v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                result: str_at(&v, "result"),
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                input_tokens: usage("input_tokens") + usage("cache_creation_input_tokens") + usage("cache_read_input_tokens"),
                output_tokens: usage("output_tokens"),
                duration_ms: num("/duration_ms"),
            }])
        }
        Some("control_request") if v.pointer("/request/subtype").and_then(Value::as_str) == Some("can_use_tool") => {
            Parsed::PermissionRequest {
                request_id: str_at(&v, "request_id"),
                tool: v.pointer("/request/tool_name").and_then(Value::as_str).unwrap_or_default().to_string(),
                input: v.pointer("/request/input").cloned().unwrap_or(Value::Null),
            }
        }
        _ => Parsed::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_permission_request() {
        let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch x"}}}"#;
        assert_eq!(
            parse_line(line),
            Parsed::PermissionRequest { request_id: "r1".into(), tool: "Bash".into(), input: json!({"command":"touch x"}) }
        );
    }

    #[test]
    fn parses_tool_use_and_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"Hi"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/r/a.md"}}]}}"#;
        assert_eq!(
            parse_line(line),
            Parsed::Events(vec![
                RunEvent::Text { text: "Hi".into() },
                RunEvent::ToolUse {
                    id: "t1".into(),
                    tool: "Read".into(),
                    summary: "/r/a.md".into(),
                    input: json!({"file_path":"/r/a.md"})
                },
            ])
        );
    }

    #[test]
    fn parses_tool_result_and_skips_replayed_notes() {
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"Denied"}]}}"#;
        assert_eq!(
            parse_line(result),
            Parsed::Events(vec![RunEvent::ToolResult { tool_use_id: "t1".into(), is_error: true, summary: "Denied".into() }])
        );
        let replay = r#"{"type":"user","message":{"role":"user","content":"a note"}}"#;
        assert_eq!(parse_line(replay), Parsed::Events(vec![]));
    }

    #[test]
    fn parses_quota_and_result() {
        let quota = r#"{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.25},"seven_day":{"utilization":0.5}}}}"#;
        assert_eq!(parse_line(quota), Parsed::Events(vec![RunEvent::Quota { five_hour: 0.25, seven_day: 0.5 }]));
        let result = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","total_cost_usd":0.01,"duration_ms":900,"usage":{"input_tokens":4,"cache_creation_input_tokens":10,"cache_read_input_tokens":20,"output_tokens":7}}"#;
        assert_eq!(
            parse_line(result),
            Parsed::Events(vec![RunEvent::Finished {
                ok: true,
                result: "done".into(),
                cost_usd: 0.01,
                input_tokens: 34,
                output_tokens: 7,
                duration_ms: 900
            }])
        );
    }

    #[test]
    fn ignores_noise() {
        assert_eq!(parse_line("not json"), Parsed::Ignored);
        assert_eq!(parse_line(r#"{"type":"system","subtype":"hook_started"}"#), Parsed::Ignored);
    }
}
