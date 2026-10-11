//! Claude Code adapter. Runs `claude -p` with the stream-json control protocol:
//! stdout carries events and permission requests, stdin carries our answers and steering notes.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use crate::event::{Parsed, RunEvent, parse_line, summarize_tool};
use crate::journal::{Journal, new_run_id};
use crate::policy::{self, Verdict};
use crate::prompt;
use crate::tasks;

/// What the run is for; picks the prompt.
#[derive(Debug, Clone)]
pub enum Mode {
    /// First analysis of a ticket: writes the task doc.
    Analyze { ticket: String },
    /// Continue an analysis after the human answered Open questions in the doc.
    Refine,
}

pub struct RunConfig {
    pub repo: PathBuf,
    pub slug: String,
    pub mode: Mode,
    pub model: Option<String>,
    pub ask_timeout: Duration,
    pub claude_bin: String,
}

impl RunConfig {
    pub fn new(repo: impl Into<PathBuf>, slug: impl Into<String>, mode: Mode) -> Self {
        Self {
            repo: repo.into(),
            slug: slug.into(),
            mode,
            model: None,
            ask_timeout: Duration::from_secs(10 * 60),
            claude_bin: "claude".into(),
        }
    }
}

type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

struct Inner {
    run_id: String,
    stdin: AsyncMutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
    finished: AtomicBool,
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

    pub fn stop(&self) {
        if let Some(kill) = self.inner.kill.lock().unwrap().take() {
            let _ = kill.send(());
        }
    }
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
        Mode::Refine if tasks::read(&cfg.repo, &cfg.slug).is_err() => {
            bail!("no task doc .ai-tasks/{}.md to refine", cfg.slug)
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

pub async fn start_run<F>(cfg: RunConfig, sink: F) -> anyhow::Result<RunHandle>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    validate(&cfg)?;
    let repo = cfg.repo.canonicalize()?;
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
        .arg("--allowedTools")
        .args(policy::PRE_ALLOWED_TOOLS);
    if let Some(model) = &cfg.model {
        cmd.args(["--model", model]);
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
        stdin: AsyncMutex::new(child.stdin.take()),
        pending: Mutex::new(HashMap::new()),
        kill: Mutex::new(Some(kill_tx)),
        finished: AtomicBool::new(false),
        emit: emit.clone(),
    });

    emit(RunEvent::Started { run_id: run_id.clone(), slug: cfg.slug.clone() });
    let text = match &cfg.mode {
        Mode::Analyze { ticket } => prompt::analyze(&cfg.slug, ticket),
        Mode::Refine => prompt::refine(&cfg.slug),
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
        let ask_timeout = cfg.ask_timeout;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                journal.lock().unwrap().raw(&line);
                match parse_line(&line) {
                    Parsed::Events(events) => {
                        for e in events {
                            let finished = matches!(e, RunEvent::Finished { .. });
                            (inner.emit)(e);
                            if finished {
                                // Closing stdin lets the CLI exit cleanly after its result.
                                inner.stdin.lock().await.take();
                            }
                        }
                    }
                    Parsed::PermissionRequest { request_id, tool, input } => {
                        handle_permission(&inner, &repo, ask_timeout, request_id, tool, input).await;
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
            inner.finished.store(true, Ordering::SeqCst);
            emit(RunEvent::Exited { code });
        });
    }

    Ok(RunHandle { inner })
}

async fn handle_permission(inner: &Arc<Inner>, repo: &std::path::Path, ask_timeout: Duration, request_id: String, tool: String, input: Value) {
    let summary = summarize_tool(&tool, &input);
    match policy::evaluate(&tool, &input, repo) {
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
