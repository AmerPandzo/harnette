//! GitHub Copilot CLI adapter over the Agent Client Protocol (`copilot --acp`): JSON-RPC 2.0 on stdio.
//! Copilot asks the host before tool calls (`session/request_permission`); the policy or the human answers.
//! ACP is turn-based, so steering notes go out as the next prompt when the current turn ends.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};

use crate::claude::{RunConfig, exclude_ai_tasks, prompt_text, signal, validate};
use crate::event::{RunEvent, summarize_tool, truncate};
use crate::journal::{Journal, new_run_id};
use crate::policy::{Policy, Role, Verdict};

type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

struct Inner {
    run_id: String,
    pid: Option<u32>,
    stdin: AsyncMutex<Option<ChildStdin>>,
    /// Permission asks waiting for the human, by tool call id.
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// Replies to our own JSON-RPC requests, by id.
    replies: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
    session_id: Mutex<Option<String>>,
    notes: Mutex<Vec<String>>,
    /// Assistant text of the current message, flushed as one Text event.
    text: Mutex<String>,
    last_text: Mutex<String>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    finished: AtomicBool,
    succeeded: AtomicBool,
    quota: AtomicBool,
    exited: Notify,
    emit: Emit,
}

#[derive(Clone)]
pub struct Handle {
    inner: Arc<Inner>,
}

impl Handle {
    pub fn run_id(&self) -> &str {
        &self.inner.run_id
    }

    pub fn answer(&self, request_id: &str, allow: bool) -> bool {
        let sender = self.inner.pending.lock().unwrap().remove(request_id);
        sender.is_some_and(|s| s.send(allow).is_ok())
    }

    /// Queue a steering note; Copilot gets it as the next prompt when its current turn ends.
    pub async fn note(&self, text: &str) -> anyhow::Result<()> {
        if self.is_finished() {
            bail!("the run has already finished");
        }
        self.inner.notes.lock().unwrap().push(text.to_string());
        (self.inner.emit)(RunEvent::Note { text: text.to_string() });
        Ok(())
    }

    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::SeqCst)
    }

    pub fn quota_exhausted(&self) -> bool {
        self.inner.quota.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) -> bool {
        loop {
            let notified = self.inner.exited.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_finished() {
                return self.inner.succeeded.load(Ordering::SeqCst);
            }
            notified.await;
        }
    }

    pub fn pause(&self) -> anyhow::Result<()> {
        signal(self.inner.pid, "STOP")
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        signal(self.inner.pid, "CONT")
    }

    pub fn stop(&self) {
        if let Some(kill) = self.inner.kill.lock().unwrap().take() {
            let _ = kill.send(());
        }
    }
}

async fn write_msg(inner: &Inner, value: &Value) -> anyhow::Result<()> {
    let mut guard = inner.stdin.lock().await;
    let Some(stdin) = guard.as_mut() else { bail!("stdin closed") };
    stdin.write_all(format!("{value}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

async fn request(inner: &Inner, method: &str, params: Value) -> anyhow::Result<Value> {
    let id = inner.next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = oneshot::channel();
    inner.replies.lock().unwrap().insert(id, tx);
    write_msg(inner, &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await?;
    let reply = rx.await.with_context(|| format!("Copilot exited before answering {method}"))?;
    if let Some(err) = reply.get("error") {
        let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        bail!("Copilot {method} failed: {message}");
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// Copilot reports an exhausted plan as assistant text, not as an error.
pub fn is_quota_message(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("quota") && (t.contains("exceeded") || t.contains("exhausted")) || t.contains("premium requests") && t.contains("limit")
}

fn flush_text(inner: &Inner) {
    let text = std::mem::take(&mut *inner.text.lock().unwrap());
    if text.trim().is_empty() {
        return;
    }
    if is_quota_message(&text) {
        inner.quota.store(true, Ordering::SeqCst);
    }
    *inner.last_text.lock().unwrap() = text.clone();
    (inner.emit)(RunEvent::Text { text });
}

/// An ACP tool call as one or more (tool, input) pairs in Claude Code's vocabulary, so the policy
/// applies unchanged. `None` = cannot classify; the human decides.
pub fn map_tool(tool_call: &Value) -> Option<Vec<(String, Value)>> {
    let kind = tool_call.get("kind").and_then(Value::as_str).unwrap_or("other");
    let raw = tool_call.get("rawInput").cloned().unwrap_or(Value::Null);
    let raw_str = |keys: &[&str]| keys.iter().find_map(|k| raw.get(*k).and_then(Value::as_str)).map(str::to_string);
    let mut paths: Vec<String> = tool_call
        .get("locations")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|l| l.get("path").and_then(Value::as_str).map(String::from)).collect())
        .unwrap_or_default();
    if paths.is_empty()
        && let Some(p) = raw_str(&["path", "file_path", "filePath"])
    {
        paths.push(p);
    }
    let per_path = |tool: &str| paths.iter().map(|p| (tool.to_string(), json!({ "file_path": p }))).collect::<Vec<_>>();
    match kind {
        // Without the exact command line, a title like "Run tests" must not match an allow rule.
        "execute" => raw_str(&["command", "cmd"]).map(|c| vec![("Bash".to_string(), json!({ "command": c }))]),
        "edit" | "delete" | "move" if !paths.is_empty() => Some(per_path("Edit")),
        "read" | "search" if !paths.is_empty() => Some(per_path("Read")),
        "read" | "search" => Some(vec![("Read".to_string(), json!({}))]),
        "fetch" => raw_str(&["url"]).map(|u| vec![("WebFetch".to_string(), json!({ "url": u }))]),
        "think" => Some(vec![("TodoWrite".to_string(), json!({}))]),
        _ => None,
    }
}

/// Combined verdict over every path a call touches: any deny wins, then any ask.
pub fn verdict_for(policy: &Policy, role: Role, tool_call: &Value, repo: &Path) -> Verdict {
    let Some(calls) = map_tool(tool_call) else { return Verdict::Ask };
    let mut allow_rule = None;
    let mut ask = false;
    for (tool, input) in &calls {
        match policy.evaluate(role, tool, input, repo) {
            Verdict::Deny(r) => return Verdict::Deny(r),
            Verdict::Ask => ask = true,
            Verdict::Allow(r) => {
                allow_rule.get_or_insert(r);
            }
        }
    }
    match allow_rule {
        Some(rule) if !ask => Verdict::Allow(rule),
        _ => Verdict::Ask,
    }
}

fn describe(tool_call: &Value) -> (String, String) {
    let title = tool_call.get("title").and_then(Value::as_str).unwrap_or_default();
    match map_tool(tool_call).and_then(|c| c.into_iter().next()) {
        Some((tool, input)) if input.as_object().is_some_and(|o| !o.is_empty()) => {
            let summary = summarize_tool(&tool, &input);
            (tool, summary)
        }
        _ => {
            let kind = tool_call.get("kind").and_then(Value::as_str).unwrap_or("other");
            (format!("copilot:{kind}"), truncate(title, 240))
        }
    }
}

/// Pick the ACP option for a decision; never an "always" option, so every call stays checked.
fn option_for(options: &Value, allow: bool) -> Value {
    let wanted = if allow { "allow_once" } else { "reject_once" };
    let prefix = if allow { "allow" } else { "reject" };
    let options = options.as_array().cloned().unwrap_or_default();
    let kind = |o: &Value| o.get("kind").and_then(Value::as_str).unwrap_or_default().to_string();
    let pick = options.iter().find(|o| kind(o) == wanted).or_else(|| options.iter().find(|o| kind(o).starts_with(prefix)));
    match pick.and_then(|o| o.get("optionId")) {
        Some(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

async fn reply(inner: &Inner, id: &Value, result: Value) {
    let _ = write_msg(inner, &json!({ "jsonrpc": "2.0", "id": id, "result": result })).await;
}

fn tool_result_text(update: &Value) -> String {
    let mut out = Vec::new();
    for c in update.get("content").and_then(Value::as_array).into_iter().flatten() {
        if let Some(t) = c.pointer("/content/text").and_then(Value::as_str) {
            out.push(t.to_string());
        } else if let Some(p) = c.get("path").and_then(Value::as_str) {
            out.push(format!("{} {p}", c.get("type").and_then(Value::as_str).unwrap_or("changed")));
        }
    }
    if out.is_empty()
        && let Some(raw) = update.get("rawOutput")
    {
        out.push(raw.as_str().map(String::from).unwrap_or_else(|| raw.to_string()));
    }
    truncate(&out.join("\n"), 240)
}

fn on_update(inner: &Inner, update: &Value) {
    match update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default() {
        "agent_message_chunk" => {
            if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                inner.text.lock().unwrap().push_str(t);
            }
        }
        "tool_call" => {
            flush_text(inner);
            let id = update.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
            let (tool, summary) = describe(update);
            let input = update.get("rawInput").cloned().unwrap_or(Value::Null);
            (inner.emit)(RunEvent::ToolUse { id, tool, summary, input });
        }
        "tool_call_update" => {
            let status = update.get("status").and_then(Value::as_str).unwrap_or_default();
            if status == "completed" || status == "failed" {
                let tool_use_id = update.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
                (inner.emit)(RunEvent::ToolResult { tool_use_id, is_error: status == "failed", summary: tool_result_text(update) });
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn on_permission(inner: Arc<Inner>, policy: &Policy, role: Role, repo: &Path, ask_timeout: Duration, id: Value, params: &Value) {
    flush_text(&inner);
    let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let options = params.get("options").cloned().unwrap_or(Value::Null);
    let request_id = tool_call.get("toolCallId").and_then(Value::as_str).map(String::from).unwrap_or_else(|| id.to_string());
    let (tool, summary) = describe(&tool_call);
    match verdict_for(policy, role, &tool_call, repo) {
        Verdict::Allow(rule) => {
            (inner.emit)(RunEvent::PermissionAuto { request_id, tool, summary, allowed: true, rule });
            reply(&inner, &id, option_for(&options, true)).await;
        }
        Verdict::Deny(rule) => {
            (inner.emit)(RunEvent::PermissionAuto { request_id, tool, summary, allowed: false, rule });
            reply(&inner, &id, option_for(&options, false)).await;
        }
        Verdict::Ask => {
            let (tx, rx) = oneshot::channel();
            inner.pending.lock().unwrap().insert(request_id.clone(), tx);
            (inner.emit)(RunEvent::PermissionAsk { request_id: request_id.clone(), tool, summary });
            tokio::spawn(async move {
                let (allowed, by) = match tokio::time::timeout(ask_timeout, rx).await {
                    Ok(Ok(allowed)) => (allowed, "human"),
                    Ok(Err(_)) => (false, "aborted"),
                    Err(_) => {
                        inner.pending.lock().unwrap().remove(&request_id);
                        (false, "timeout")
                    }
                };
                (inner.emit)(RunEvent::PermissionAnswered { request_id, allowed, by: by.into() });
                reply(&inner, &id, option_for(&options, allowed)).await;
            });
        }
    }
}

pub fn args(cfg: &RunConfig, policy: &Policy) -> Vec<String> {
    let mut args: Vec<String> = ["--acp", "--no-auto-update", "--no-ask-user", "--disallow-temp-dir"].map(String::from).to_vec();
    if let Some(model) = &cfg.model {
        args.extend(["--model".into(), model.clone()]);
    }
    // Denied even if Copilot would not ask (its own allow rules or auto-approved commands).
    for verb in &policy.git.deny {
        args.extend(["--deny-tool".into(), format!("shell(git {verb})")]);
    }
    args
}

pub async fn start<F>(cfg: RunConfig, sink: F) -> anyhow::Result<Handle>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    validate(&cfg)?;
    let repo = cfg.repo.canonicalize()?;
    let policy = Arc::new(Policy::load(&repo)?);
    let role = cfg.mode.role();
    let ask_timeout = cfg.ask_timeout.unwrap_or(Duration::from_secs(policy.shell.ask_timeout_min * 60));
    exclude_ai_tasks(&repo);
    let run_id = new_run_id();
    let journal = Arc::new(Mutex::new(Journal::open(&repo, &cfg.slug, &run_id)?));
    let emit: Emit = {
        let journal = journal.clone();
        Arc::new(move |e: RunEvent| {
            journal.lock().unwrap().event(&e);
            sink(e);
        })
    };

    let mut child = Command::new(&cfg.copilot_bin)
        .args(args(&cfg, &policy))
        .current_dir(&repo)
        // An inherited allow-all would skip the permission requests the policy depends on.
        .env_remove("COPILOT_ALLOW_ALL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start `{}`; is GitHub Copilot CLI installed and on PATH?", cfg.copilot_bin))?;

    let (kill_tx, mut kill_rx) = oneshot::channel();
    let inner = Arc::new(Inner {
        run_id: run_id.clone(),
        pid: child.id(),
        stdin: AsyncMutex::new(child.stdin.take()),
        pending: Mutex::new(HashMap::new()),
        replies: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        session_id: Mutex::new(None),
        notes: Mutex::new(Vec::new()),
        text: Mutex::new(String::new()),
        last_text: Mutex::new(String::new()),
        kill: Mutex::new(Some(kill_tx)),
        finished: AtomicBool::new(false),
        succeeded: AtomicBool::new(false),
        quota: AtomicBool::new(false),
        exited: Notify::new(),
        emit: emit.clone(),
    });
    emit(RunEvent::Started { run_id: run_id.clone(), slug: cfg.slug.clone() });

    if let Some(stderr) = child.stderr.take() {
        let journal = journal.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                journal.lock().unwrap().raw(&format!("stderr: {line}"));
            }
        });
    }

    let stdout = child.stdout.take().context("no stdout")?;
    let reader = {
        let inner = inner.clone();
        let journal = journal.clone();
        let policy = policy.clone();
        let repo = repo.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                journal.lock().unwrap().raw(&line);
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                let method = msg.get("method").and_then(Value::as_str);
                match (method, msg.get("id")) {
                    (Some("session/update"), _) => {
                        if let Some(update) = msg.pointer("/params/update") {
                            on_update(&inner, update);
                        }
                    }
                    (Some("session/request_permission"), Some(id)) => {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        on_permission(inner.clone(), &policy, role, &repo, ask_timeout, id.clone(), &params).await;
                    }
                    (Some(other), Some(id)) => {
                        // We announce no fs/terminal capabilities; anything else is unsupported.
                        let err = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{other} is not supported by Harness") } });
                        let _ = write_msg(&inner, &err).await;
                    }
                    (None, Some(id)) => {
                        if let Some(tx) = id.as_u64().and_then(|id| inner.replies.lock().unwrap().remove(&id)) {
                            let _ = tx.send(msg);
                        }
                    }
                    _ => {}
                }
            }
            // Fail every request still waiting for an answer.
            inner.replies.lock().unwrap().clear();
        })
    };

    let prompt = prompt_text(&cfg);
    journal.lock().unwrap().raw(&json!({ "harness_prompt": prompt }).to_string());
    let driver = {
        let inner = inner.clone();
        let repo = repo.clone();
        let model = cfg.model.clone();
        async move { drive(&inner, &repo, model, prompt).await }
    };

    let handle = Handle { inner: inner.clone() };
    tokio::spawn(async move {
        let started = Instant::now();
        let outcome = tokio::select! {
            r = driver => Some(r),
            _ = &mut kill_rx => None,
        };
        flush_text(&inner);
        let ok = match outcome {
            Some(Ok(stop_reason)) => {
                let quota = inner.quota.load(Ordering::SeqCst);
                if quota {
                    emit(RunEvent::Error { message: "Copilot is out of quota.".into() });
                }
                stop_reason == "end_turn" && !quota
            }
            Some(Err(e)) => {
                if is_quota_message(&format!("{e:#}")) {
                    inner.quota.store(true, Ordering::SeqCst);
                }
                emit(RunEvent::Error { message: format!("{e:#}") });
                false
            }
            None => {
                let sid = inner.session_id.lock().unwrap().clone();
                if let Some(sid) = sid {
                    let cancel = json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": sid } });
                    let _ = tokio::time::timeout(Duration::from_secs(1), write_msg(&inner, &cancel)).await;
                }
                emit(RunEvent::Error { message: "Run stopped by the human.".into() });
                false
            }
        };
        let result = inner.last_text.lock().unwrap().clone();
        let duration_ms = started.elapsed().as_millis() as u64;
        emit(RunEvent::Finished { ok, result, cost_usd: 0.0, input_tokens: 0, output_tokens: 0, new_tokens: 0, duration_ms });
        inner.succeeded.store(ok, Ordering::SeqCst);
        // Closing stdin ends `copilot --acp`; kill it if it lingers.
        inner.stdin.lock().await.take();
        let status = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(s) => s.ok(),
            Err(_) => {
                let _ = child.kill().await;
                child.wait().await.ok()
            }
        };
        let _ = reader.await;
        inner.finished.store(true, Ordering::SeqCst);
        emit(RunEvent::Exited { code: status.and_then(|s| s.code()) });
        inner.exited.notify_waiters();
    });

    Ok(handle)
}

/// initialize → session/new → prompt, then one more prompt per batch of queued notes. Returns the last stop reason.
async fn drive(inner: &Arc<Inner>, repo: &Path, model: Option<String>, prompt: String) -> anyhow::Result<String> {
    let caps = json!({ "protocolVersion": 1, "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false } });
    request(inner, "initialize", caps).await?;
    let session = request(inner, "session/new", json!({ "cwd": repo, "mcpServers": [] })).await?;
    let sid = session.get("sessionId").and_then(Value::as_str).context("Copilot returned no session id")?.to_string();
    let model = model
        .or_else(|| session.pointer("/models/currentModelId").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| "copilot".into());
    inner.session_id.lock().unwrap().replace(sid.clone());
    (inner.emit)(RunEvent::Session { session_id: sid.clone(), model: format!("copilot/{model}") });

    let mut text = prompt;
    loop {
        let result = request(inner, "session/prompt", json!({ "sessionId": sid, "prompt": [{ "type": "text", "text": text }] })).await?;
        flush_text(inner);
        let stop = result.get("stopReason").and_then(Value::as_str).unwrap_or("unknown").to_string();
        if stop != "end_turn" || inner.quota.load(Ordering::SeqCst) {
            return Ok(stop);
        }
        let notes = std::mem::take(&mut *inner.notes.lock().unwrap());
        if notes.is_empty() {
            return Ok(stop);
        }
        text = notes.iter().map(|n| format!("Note from the human supervisor: {n}")).collect::<Vec<_>>().join("\n\n");
        text.push_str("\n\nTake this into account and continue the task; keep following the earlier rules.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn maps_acp_tool_kinds_onto_the_policy() {
        let p = Policy::default();
        let r = repo();
        let inside = r.join("src/a.js").display().to_string();
        let v = |tc: Value| verdict_for(&p, Role::Implementer, &tc, &r);
        assert!(matches!(v(json!({ "kind": "edit", "locations": [{ "path": inside }] })), Verdict::Allow(_)));
        assert!(matches!(v(json!({ "kind": "edit", "rawInput": { "path": "/etc/hosts" } })), Verdict::Deny(_)));
        assert!(matches!(v(json!({ "kind": "execute", "rawInput": { "command": "git push origin main" } })), Verdict::Deny(_)));
        // No command line: the title alone never matches an allow rule.
        assert!(matches!(v(json!({ "kind": "execute", "title": "git status" })), Verdict::Ask));
        assert!(matches!(v(json!({ "kind": "read", "locations": [{ "path": inside }] })), Verdict::Allow(_)));
        assert!(matches!(v(json!({ "kind": "other", "title": "mystery" })), Verdict::Ask));
        // A move touching one allowed and one forbidden path is denied.
        let mv = json!({ "kind": "move", "locations": [{ "path": inside }, { "path": r.join(".git/config").display().to_string() }] });
        assert!(matches!(v(mv), Verdict::Deny(_)));
        // Analyst may only write the task docs.
        assert!(matches!(
            verdict_for(&p, Role::Analyst, &json!({ "kind": "edit", "locations": [{ "path": inside }] }), &r),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn picks_once_options_only() {
        let opts = json!([
            { "optionId": "aa", "kind": "allow_always" },
            { "optionId": "a1", "kind": "allow_once" },
            { "optionId": "r1", "kind": "reject_once" },
        ]);
        assert_eq!(option_for(&opts, true)["outcome"]["optionId"], "a1");
        assert_eq!(option_for(&opts, false)["outcome"]["optionId"], "r1");
        assert_eq!(option_for(&json!([]), true)["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn quota_text() {
        assert!(is_quota_message("Error: You have exceeded your monthly quota (Request ID: 85)"));
        assert!(!is_quota_message("All tests pass."));
    }

    fn fake_repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("harness-acp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ai-tasks")).unwrap();
        std::fs::write(dir.join(".ai-tasks/T.md"), "# T\n\n**Status:** Ready for implementation\n").unwrap();
        dir
    }

    fn fake_cfg(repo: &Path, mode: crate::claude::Mode) -> RunConfig {
        let mut cfg = RunConfig::new(repo, "T", mode);
        cfg.copilot_bin = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_acp.py").into();
        cfg.ask_timeout = Some(Duration::from_secs(20));
        cfg
    }

    /// The whole ACP path against a scripted agent: policy allow, policy deny, a human ask, a note as the next prompt.
    #[tokio::test(flavor = "multi_thread")]
    async fn drives_a_scripted_acp_agent() {
        let repo = fake_repo("work");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cfg = fake_cfg(&repo, crate::claude::Mode::Implement { round: 1, build_command: None });
        let handle = start(cfg, move |e| {
            let _ = tx.send(e);
        })
        .await
        .unwrap();
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            if let RunEvent::PermissionAsk { request_id, .. } = &e {
                handle.note("hurry").await.unwrap();
                assert!(handle.answer(request_id, true));
            }
            let exited = matches!(e, RunEvent::Exited { .. });
            events.push(e);
            if exited {
                break;
            }
        }
        assert!(handle.wait().await, "{events:#?}");
        let auto: Vec<(String, bool)> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::PermissionAuto { tool, allowed, .. } => Some((tool.clone(), *allowed)),
                _ => None,
            })
            .collect();
        assert_eq!(auto, vec![("Edit".to_string(), true), ("Bash".to_string(), false)]);
        let asks: Vec<&RunEvent> = events.iter().filter(|e| matches!(e, RunEvent::PermissionAsk { .. })).collect();
        assert_eq!(asks.len(), 1, "make deploy is unlisted, so the human decides");
        let texts: Vec<&str> = events.iter().filter_map(|e| if let RunEvent::Text { text } = e { Some(text.as_str()) } else { None }).collect();
        assert!(texts.contains(&"outcomes once,no,once"), "{texts:?}");
        assert!(texts.contains(&"second turn saw: NOTE"), "{texts:?}");
        assert!(events.iter().any(|e| matches!(e, RunEvent::Session { model, .. } if model == "copilot/fake-model")));
        assert!(events.iter().any(|e| matches!(e, RunEvent::ToolResult { is_error: true, .. })), "the denied push failed");
        assert!(!handle.quota_exhausted());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reports_an_exhausted_plan() {
        let repo = fake_repo("quota");
        let cfg = fake_cfg(&repo, crate::claude::Mode::Analyze { ticket: "SCENARIO:quota".into() });
        let handle = start(cfg, |_| {}).await.unwrap();
        assert!(!handle.wait().await);
        assert!(handle.quota_exhausted());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn args_deny_git_verbs_and_never_allow_all() {
        let cfg = RunConfig::new("/tmp", "x", crate::claude::Mode::Refine);
        let a = args(&cfg, &Policy::default()).join(" ");
        assert!(a.contains("--deny-tool shell(git push)"));
        assert!(a.starts_with("--acp"));
        assert!(!a.contains("allow-all") && !a.contains("yolo"));
    }
}
