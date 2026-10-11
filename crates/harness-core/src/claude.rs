//! Claude Code adapter. Runs `claude -p` with the stream-json control protocol:
//! stdout carries events and permission requests, stdin carries our answers and steering notes.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};

use crate::event::{Parsed, RunEvent, parse_line, summarize_tool};
use crate::journal::{Journal, new_run_id};
use crate::policy::{Policy, Role, Verdict};
use crate::prompt;
use crate::watchdog::PriorUsage;
use crate::tasks;

/// What the run is for; picks the prompt.
#[derive(Debug, Clone)]
pub enum Mode {
    /// First analysis of a ticket: writes the task doc.
    Analyze { ticket: String },
    /// Continue an analysis after the human answered Open questions in the doc.
    Refine,
    /// Implement the Proposed fix or the open review findings.
    Implement { round: u32, build_command: Option<String> },
    /// Fresh-session review of the change against the task doc.
    Review { round: u32, diff_path: String, build: Option<BuildSummary> },
}

/// What the reviewer is told about Harness's own build.
#[derive(Debug, Clone)]
pub struct BuildSummary {
    pub ok: bool,
    pub command: String,
    pub tail: String,
}

impl Mode {
    pub fn role(&self) -> Role {
        match self {
            Mode::Analyze { .. } | Mode::Refine => Role::Analyst,
            Mode::Implement { .. } => Role::Implementer,
            Mode::Review { .. } => Role::Reviewer,
        }
    }
}

pub struct RunConfig {
    pub repo: PathBuf,
    pub slug: String,
    pub mode: Mode,
    pub model: Option<String>,
    /// Overrides `shell.ask_timeout_min` from the policy.
    pub ask_timeout: Option<Duration>,
    pub claude_bin: String,
    /// Path to `harness-guard`; found next to the running executable when unset.
    pub guard_bin: Option<PathBuf>,
    /// Hard dollar cap the CLI enforces for this run (`--max-budget-usd`).
    pub max_budget_usd: Option<f64>,
    /// Usage of this task before this run (from the ledger), for the watchdog's budgets.
    pub prior: PriorUsage,
}

impl RunConfig {
    pub fn new(repo: impl Into<PathBuf>, slug: impl Into<String>, mode: Mode) -> Self {
        Self {
            repo: repo.into(),
            slug: slug.into(),
            mode,
            model: None,
            ask_timeout: None,
            claude_bin: "claude".into(),
            guard_bin: None,
            max_budget_usd: None,
            prior: PriorUsage::default(),
        }
    }
}

type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

struct Inner {
    run_id: String,
    pid: Option<u32>,
    stdin: AsyncMutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    finished: AtomicBool,
    /// The agent reported a successful result.
    succeeded: AtomicBool,
    exited: Notify,
    emit: Emit,
}

/// Control surface for a running agent; cheap to clone.
#[derive(Clone)]
pub struct RunHandle {
    inner: Arc<Inner>,
}

impl RunHandle {
    pub fn run_id(&self) -> &str {
        &self.inner.run_id
    }

    /// Answer a pending permission request. Returns false if it was already answered or timed out.
    pub fn answer(&self, request_id: &str, allow: bool) -> bool {
        let sender = self.inner.pending.lock().unwrap().remove(request_id);
        sender.is_some_and(|s| s.send(allow).is_ok())
    }

    /// Send a steering note; the agent sees it after its current tool call.
    pub async fn note(&self, text: &str) -> anyhow::Result<()> {
        let msg = json!({ "type": "user", "message": { "role": "user", "content": format!("Note from the human supervisor: {text}") } });
        write_line(&self.inner, &msg).await.context("the run has already finished")?;
        (self.inner.emit)(RunEvent::Note { text: text.to_string() });
        Ok(())
    }

    /// True once the agent process has exited.
    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::SeqCst)
    }

    /// Wait until the agent process has exited; true if it finished its task successfully.
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

    /// Freeze the agent process (SIGSTOP); a command it already started keeps running.
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

#[cfg(unix)]
fn signal(pid: Option<u32>, sig: &str) -> anyhow::Result<()> {
    let pid = pid.context("agent process id unknown")?;
    let ok = std::process::Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).status()?.success();
    if !ok {
        bail!("could not send SIG{sig} to the agent");
    }
    Ok(())
}

#[cfg(not(unix))]
fn signal(_pid: Option<u32>, _sig: &str) -> anyhow::Result<()> {
    bail!("pausing an agent is not supported on this platform yet")
}

async fn write_line(inner: &Inner, value: &Value) -> anyhow::Result<()> {
    let mut guard = inner.stdin.lock().await;
    let Some(stdin) = guard.as_mut() else { bail!("stdin closed") };
    stdin.write_all(format!("{value}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

async fn respond(inner: &Inner, request_id: &str, allow: bool, input: Value, message: &str) {
    let decision = if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": message })
    };
    let msg = json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": decision } });
    let _ = write_line(inner, &msg).await;
}

fn validate(cfg: &RunConfig) -> anyhow::Result<()> {
    if !cfg.repo.is_dir() {
        bail!("repo is not a directory: {}", cfg.repo.display());
    }
    if !tasks::valid_slug(&cfg.slug) {
        bail!("invalid slug {:?}: use letters, digits, '.', '_' or '-'", cfg.slug);
    }
    match &cfg.mode {
        Mode::Analyze { ticket } if ticket.trim().is_empty() => bail!("ticket text is empty"),
        Mode::Refine | Mode::Implement { .. } | Mode::Review { .. } if tasks::read(&cfg.repo, &cfg.slug).is_err() => {
            bail!("no task doc .ai-tasks/{}.md", cfg.slug)
        }
        _ => Ok(()),
    }
}

/// Keep `.ai-tasks/` out of commits without touching the repo's tracked `.gitignore`.
fn exclude_ai_tasks(repo: &std::path::Path) {
    let exclude = repo.join(".git/info/exclude");
    if !repo.join(".git").is_dir() {
        return;
    }
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == ".ai-tasks/") {
        let _ = std::fs::create_dir_all(repo.join(".git/info"));
        let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
        let _ = std::fs::write(&exclude, format!("{current}{sep}.ai-tasks/\n"));
    }
}

/// `harness-guard` sits next to the app binary (`target/debug/`), or one level up for examples.
fn find_guard(cfg: &RunConfig) -> anyhow::Result<PathBuf> {
    if let Some(p) = &cfg.guard_bin {
        return Ok(p.clone());
    }
    if let Some(p) = std::env::var_os("HARNESS_GUARD") {
        return Ok(PathBuf::from(p));
    }
    let name = format!("harness-guard{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe()?;
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
        .with_context(|| format!("{name} not found next to {}; build it with `cargo build -p harness-core --bin harness-guard`", exe.display()))
}

/// POSIX single-quote for the hook command line.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub async fn start_run<F>(cfg: RunConfig, sink: F) -> anyhow::Result<RunHandle>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    validate(&cfg)?;
    let repo = cfg.repo.canonicalize()?;
    let policy = Arc::new(Policy::load(&repo)?);
    let role = cfg.mode.role();
    let ask_timeout = cfg.ask_timeout.unwrap_or(Duration::from_secs(policy.shell.ask_timeout_min * 60));
    let guard = find_guard(&cfg)?;
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

    let mut cmd = Command::new(&cfg.claude_bin);
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .args(["--permission-prompts", "host", "--permission-prompt-tool", "stdio"])
        // User-level hooks and permission rules could decide prompts before we see them.
        .args(["--setting-sources", "project,local"])
        // harness-guard sees every tool call; anything it cannot allow or deny comes back to us as a prompt.
        .args(["--settings", &guard_settings(&guard, &repo, role)]);
    if let Some(model) = &cfg.model {
        cmd.args(["--model", model]);
    }
    if let Some(usd) = cfg.max_budget_usd {
        cmd.args(["--max-budget-usd", &format!("{usd:.2}")]);
    }
    let mut child = cmd
        .current_dir(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("could not start `{}`; is Claude Code installed and on PATH?", cfg.claude_bin))?;

    let (kill_tx, kill_rx) = oneshot::channel();
    let inner = Arc::new(Inner {
        run_id: run_id.clone(),
        pid: child.id(),
        stdin: AsyncMutex::new(child.stdin.take()),
        pending: Mutex::new(HashMap::new()),
        kill: Mutex::new(Some(kill_tx)),
        finished: AtomicBool::new(false),
        succeeded: AtomicBool::new(false),
        exited: Notify::new(),
        emit: emit.clone(),
    });

    emit(RunEvent::Started { run_id: run_id.clone(), slug: cfg.slug.clone() });
    let text = match &cfg.mode {
        Mode::Analyze { ticket } => prompt::analyze(&cfg.slug, ticket),
        Mode::Refine => prompt::refine(&cfg.slug),
        Mode::Implement { round, build_command } => prompt::implement(&cfg.slug, *round, build_command.as_deref()),
        Mode::Review { round, diff_path, build } => prompt::review(&cfg.slug, *round, diff_path, build.as_ref()),
    };
    let first = json!({ "type": "user", "message": { "role": "user", "content": text } });
    write_line(&inner, &first).await?;

    let stderr_tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = stderr_tail.clone();
        let journal = journal.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                journal.lock().unwrap().raw(&format!("stderr: {line}"));
                let mut t = tail.lock().unwrap();
                t.push_back(line);
                if t.len() > 20 {
                    t.pop_front();
                }
            }
        });
    }

    let stdout = child.stdout.take().context("no stdout")?;
    let reader = {
        let inner = inner.clone();
        let repo = repo.clone();
        let policy = policy.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            // The CLI repeats a message's usage once per content block; report it once.
            let mut usage_seen = std::collections::HashSet::new();
            while let Ok(Some(line)) = lines.next_line().await {
                journal.lock().unwrap().raw(&line);
                match parse_line(&line) {
                    Parsed::Events(events) => {
                        for e in events {
                            if let RunEvent::Usage { message_id, .. } = &e
                                && !usage_seen.insert(message_id.clone())
                            {
                                continue;
                            }
                            let finished = matches!(e, RunEvent::Finished { .. });
                            if let RunEvent::Finished { ok: true, .. } = e {
                                inner.succeeded.store(true, Ordering::SeqCst);
                            }
                            // harness-guard decides allow/deny silently; mirror its verdict into the journal.
                            let guard_verdict = match &e {
                                RunEvent::ToolUse { id, tool, summary, input } => {
                                    match policy.evaluate(role, tool, input, &repo) {
                                        Verdict::Allow(rule) => Some((id.clone(), tool.clone(), summary.clone(), true, rule)),
                                        Verdict::Deny(rule) => Some((id.clone(), tool.clone(), summary.clone(), false, rule)),
                                        Verdict::Ask => None,
                                    }
                                }
                                _ => None,
                            };
                            (inner.emit)(e);
                            if let Some((request_id, tool, summary, allowed, rule)) = guard_verdict {
                                (inner.emit)(RunEvent::PermissionAuto { request_id, tool, summary, allowed, rule });
                            }
                            if finished {
                                // Closing stdin lets the CLI exit cleanly after its result.
                                inner.stdin.lock().await.take();
                            }
                        }
                    }
                    Parsed::PermissionRequest { request_id, tool, input } => {
                        handle_permission(&inner, &policy, role, &repo, ask_timeout, request_id, tool, input).await;
                    }
                    Parsed::Ignored => {}
                }
            }
        })
    };

    {
        let emit = emit.clone();
        let inner = inner.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = kill_rx => {
                    let _ = child.kill().await;
                    emit(RunEvent::Error { message: "Run stopped by the human.".into() });
                    child.wait().await
                }
            };
            let _ = reader.await;
            let code = status.ok().and_then(|s| s.code());
            if code.is_some_and(|c| c != 0) {
                let tail: Vec<String> = stderr_tail.lock().unwrap().iter().cloned().collect();
                if !tail.is_empty() {
                    emit(RunEvent::Error { message: tail.join("\n") });
                }
            }
            if code != Some(0) {
                inner.succeeded.store(false, Ordering::SeqCst);
            }
            inner.finished.store(true, Ordering::SeqCst);
            emit(RunEvent::Exited { code });
            inner.exited.notify_waiters();
        });
    }

    Ok(RunHandle { inner })
}

fn guard_settings(guard: &Path, repo: &Path, role: Role) -> String {
    let command = format!(
        "{} --repo {} --role {}",
        sh_quote(&guard.display().to_string()),
        sh_quote(&repo.display().to_string()),
        role.as_str()
    );
    json!({ "hooks": { "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": command }] }] } }).to_string()
}

#[allow(clippy::too_many_arguments)]
async fn handle_permission(
    inner: &Arc<Inner>,
    policy: &Policy,
    role: Role,
    repo: &std::path::Path,
    ask_timeout: Duration,
    request_id: String,
    tool: String,
    input: Value,
) {
    let summary = summarize_tool(&tool, &input);
    match policy.evaluate(role, &tool, &input, repo) {
        Verdict::Allow(rule) => {
            (inner.emit)(RunEvent::PermissionAuto { request_id: request_id.clone(), tool, summary, allowed: true, rule });
            respond(inner, &request_id, true, input, "").await;
        }
        Verdict::Deny(reason) => {
            (inner.emit)(RunEvent::PermissionAuto { request_id: request_id.clone(), tool, summary, allowed: false, rule: reason.clone() });
            respond(inner, &request_id, false, input, &reason).await;
        }
        Verdict::Ask => {
            let (tx, rx) = oneshot::channel();
            inner.pending.lock().unwrap().insert(request_id.clone(), tx);
            (inner.emit)(RunEvent::PermissionAsk { request_id: request_id.clone(), tool, summary });
            // Waiting must not block the reader: other events keep flowing while the human decides.
            let inner = inner.clone();
            tokio::spawn(async move {
                let (allowed, by) = match tokio::time::timeout(ask_timeout, rx).await {
                    Ok(Ok(allowed)) => (allowed, "human"),
                    Ok(Err(_)) => (false, "aborted"),
                    Err(_) => {
                        inner.pending.lock().unwrap().remove(&request_id);
                        (false, "timeout")
                    }
                };
                (inner.emit)(RunEvent::PermissionAnswered { request_id: request_id.clone(), allowed, by: by.into() });
                let message = match by {
                    "timeout" => "No answer from the human in time; denied. Continue without it.",
                    _ => "Denied by the human supervisor. Continue without it.",
                };
                respond(&inner, &request_id, allowed, input, message).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_command_quotes_paths() {
        assert_eq!(sh_quote("/a b/it's"), r#"'/a b/it'\''s'"#);
        let s: Value = serde_json::from_str(&guard_settings(Path::new("/x/harness-guard"), Path::new("/my repo"), Role::Analyst)).unwrap();
        let cmd = s["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, "'/x/harness-guard' --repo '/my repo' --role analyst");
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "*");
    }
}
