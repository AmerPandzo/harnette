//! Tauri shell: exposes harness-core runs and task docs to the web UI
//! as commands plus `run-event` / `tasks-changed` events.

use std::path::PathBuf;
use std::sync::Mutex;

use harness_core::policy::{Policy, policy_path, suggested_toml};
use harness_core::review::{Criterion, Finding, parse_criteria, parse_findings};
use harness_core::tasks::{self, StatusKind, TaskSummary};
use harness_core::watch::{TaskWatcher, watch_tasks};
use harness_core::delivery::{self, Outcome, Plan};
use harness_core::git::{self, Checkpoint};
use harness_core::ledger::{self, Entry, Summary};
use harness_core::watchdog::PriorUsage;
use harness_core::{LoopConfig, Mode, RunConfig, RunEvent, Session, start_loop as start_core_loop, start_single};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
struct AppState {
    run: Mutex<Option<Session>>,
    watcher: Mutex<Option<TaskWatcher>>,
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn ledger_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("harness")).join("usage.jsonl")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What the sink needs to know about its session to book usage in the ledger.
struct Ctx {
    app: AppHandle,
    repo: String,
    slug: String,
    role: Mutex<String>,
}

/// Canonical repo path plus this task's earlier usage, so budgets span runs.
fn prior_usage(app: &AppHandle, repo: &std::path::Path, slug: &str) -> (String, PriorUsage) {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf()).display().to_string();
    let (tokens, usd) = ledger::task_totals(&ledger::read(&ledger_path(app)), &repo, slug);
    (repo, PriorUsage { tokens, usd })
}

fn session_sink(ctx: Ctx) -> impl Fn(RunEvent) + Send + Sync + 'static {
    move |e: RunEvent| {
        match &e {
            RunEvent::Phase { role, .. } => *ctx.role.lock().unwrap() = role.clone(),
            RunEvent::Finished { new_tokens, cost_usd, .. } => {
                let entry = Entry {
                    ts_ms: now_ms(),
                    repo: ctx.repo.clone(),
                    slug: ctx.slug.clone(),
                    role: ctx.role.lock().unwrap().clone(),
                    new_tokens: *new_tokens,
                    usd: *cost_usd,
                };
                let _ = ledger::append(&ledger_path(&ctx.app), &entry);
            }
            _ => {}
        }
        on_event(&ctx.app, e);
    }
}

fn on_event(app: &AppHandle, event: RunEvent) {
    match &event {
        RunEvent::Alarm { message, action, .. } => {
            let title = match action.as_str() {
                "pause" => "Harness: paused",
                "stop" => "Harness: stopped",
                _ => "Harness: warning",
            };
            notify(app, title, message);
        }
        RunEvent::PermissionAsk { tool, summary, .. } => notify(app, &format!("Harness: allow {tool}?"), summary),
        RunEvent::Build { ok: false, command, .. } => notify(app, "Harness: build is red", command),
        RunEvent::DeliveryReady { mode, branch, .. } => notify(app, "Harness: ready to deliver", &format!("{mode} to {branch}: approve in the app")),
        RunEvent::Delivered { branch, pr_url, .. } => {
            notify(app, "Harness: delivered", pr_url.as_deref().unwrap_or(branch));
        }
        RunEvent::SessionEnded { ok, summary } => {
            notify(app, if *ok { "Harness: finished" } else { "Harness: stopped" }, summary);
        }
        _ => {}
    }
    let _ = app.emit("run-event", &event);
}

/// Accept `~/…` paths as typed in a terminal.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    slug: String,
    mode: String,
    ticket: Option<String>,
    model: Option<String>,
) -> Result<(), String> {
    if busy(&state) {
        return Err("A run is already in progress.".into());
    }
    let mode = match mode.as_str() {
        "analyze" => Mode::Analyze { ticket: ticket.unwrap_or_default() },
        "refine" => Mode::Refine,
        other => return Err(format!("unknown run mode {other:?}")),
    };
    let repo = expand_home(repo.trim());
    let slug = slug.trim().to_string();
    let (repo_key, prior) = prior_usage(&app, &repo, &slug);
    let mut cfg = RunConfig::new(repo, &slug, mode);
    cfg.model = model.filter(|m| !m.trim().is_empty());
    cfg.prior = prior;
    let ctx = Ctx { app: app.clone(), repo: repo_key, slug, role: Mutex::new("analyst".into()) };
    let session = start_single(cfg, session_sink(ctx)).await.map_err(err)?;
    state.run.lock().unwrap().replace(session);
    Ok(())
}

fn busy(state: &AppState) -> bool {
    state.run.lock().unwrap().as_ref().is_some_and(|s| !s.is_finished())
}

/// Implement → build → review until Done (or a stop condition).
#[tauri::command]
async fn start_loop(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    slug: String,
    model: Option<String>,
    base: Option<String>,
) -> Result<(), String> {
    if busy(&state) {
        return Err("A run is already in progress.".into());
    }
    let repo = expand_home(repo.trim());
    let slug = slug.trim().to_string();
    let (repo_key, prior) = prior_usage(&app, &repo, &slug);
    let mut cfg = LoopConfig::new(repo, &slug);
    cfg.model = model.filter(|m| !m.trim().is_empty());
    cfg.base = base.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    cfg.prior = prior;
    let ctx = Ctx { app: app.clone(), repo: repo_key, slug, role: Mutex::new("implementer".into()) };
    let session = start_core_loop(cfg, session_sink(ctx)).await.map_err(err)?;
    state.run.lock().unwrap().replace(session);
    Ok(())
}

#[derive(serde::Serialize)]
struct FileStat {
    path: String,
    added: u32,
    removed: u32,
}

#[derive(serde::Serialize)]
struct ReviewData {
    /// What the diff is against, in words.
    base_label: String,
    files: Vec<FileStat>,
    patch: String,
    truncated: bool,
    findings: Vec<Finding>,
    criteria: Vec<Criterion>,
}

const PATCH_LIMIT: usize = 2_000_000;

/// The task's change against a base: `checkpoint` (before the first implement round), `head`,
/// or a branch name (diffed from its merge-base with HEAD, like a pull request).
#[tauri::command]
fn task_review(repo: String, slug: String, base: String) -> Result<ReviewData, String> {
    let repo = expand_home(&repo);
    let md = tasks::read(&repo, &slug).map_err(err)?;
    let findings = parse_findings(&md);
    let criteria = parse_criteria(&md);
    if !git::is_repo(&repo) {
        return Ok(ReviewData {
            base_label: "not a git repository".into(),
            files: vec![],
            patch: String::new(),
            truncated: false,
            findings,
            criteria,
        });
    }
    let (base_ref, base_label) = match base.trim() {
        "" | "checkpoint" => match git::list_checkpoints(&repo, &slug).map_err(err)?.first() {
            Some(c) => (c.git_ref.clone(), format!("checkpoint {} (before the first implement round)", c.round)),
            None => (git::head(&repo).map_err(err)?.ok_or("no commits yet")?, "HEAD (no checkpoint yet)".into()),
        },
        "head" => (git::head(&repo).map_err(err)?.ok_or("no commits yet")?, "HEAD".into()),
        branch => (git::merge_base(&repo, branch).map_err(err)?, format!("merge-base with {branch}")),
    };
    let files = git::numstat(&repo, &base_ref)
        .map_err(err)?
        .into_iter()
        .map(|(path, added, removed)| FileStat { path, added, removed })
        .collect();
    let mut patch = git::diff(&repo, &base_ref).map_err(err)?;
    let truncated = patch.len() > PATCH_LIMIT;
    if truncated {
        let cut = (0..=PATCH_LIMIT).rev().find(|i| patch.is_char_boundary(*i)).unwrap_or(0);
        patch.truncate(cut);
    }
    Ok(ReviewData { base_label, files, patch, truncated, findings, criteria })
}

/// What delivery would do now, or why it is refused (gate).
#[tauri::command]
fn delivery_plan(repo: String, slug: String) -> Result<Plan, String> {
    let repo = expand_home(&repo);
    let policy = Policy::load(&repo).map_err(err)?;
    delivery::plan(&repo, &slug, &policy).map_err(err)
}

/// Re-checks the gate and delivers. Pushing happens only if the repo's policy says push or pr.
#[tauri::command]
fn deliver(state: State<'_, AppState>, repo: String, slug: String) -> Result<Outcome, String> {
    if busy(&state) {
        return Err("Wait for the running session to finish.".into());
    }
    let repo = expand_home(&repo);
    let policy = Policy::load(&repo).map_err(err)?;
    let plan = delivery::plan(&repo, &slug, &policy).map_err(err)?;
    delivery::execute(&repo, &plan).map_err(err)
}

#[tauri::command]
fn list_checkpoints(repo: String, slug: String) -> Result<Vec<Checkpoint>, String> {
    let repo = expand_home(&repo);
    if !git::is_repo(&repo) {
        return Ok(Vec::new());
    }
    git::list_checkpoints(&repo, &slug).map_err(err)
}

#[tauri::command]
fn restore_checkpoint(state: State<'_, AppState>, repo: String, slug: String, round: u32) -> Result<(), String> {
    if busy(&state) {
        return Err("Stop the running session before restoring a checkpoint.".into());
    }
    if !tasks::valid_slug(&slug) {
        return Err("invalid slug".into());
    }
    git::restore(&expand_home(&repo), &git::checkpoint_ref(&slug, round)).map_err(err)
}

#[tauri::command]
fn answer_permission(state: State<'_, AppState>, request_id: String, allow: bool) -> bool {
    state.run.lock().unwrap().as_ref().is_some_and(|h| h.answer(&request_id, allow))
}

#[tauri::command]
async fn send_note(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let handle = state.run.lock().unwrap().clone().ok_or("No run in progress.")?;
    handle.note(&text).await.map_err(err)
}

#[tauri::command]
fn pause_run(state: State<'_, AppState>) {
    if let Some(s) = state.run.lock().unwrap().as_ref() {
        s.pause("Paused by you.");
    }
}

#[tauri::command]
fn resume_run(state: State<'_, AppState>) {
    if let Some(s) = state.run.lock().unwrap().as_ref() {
        s.resume();
    }
}

/// Rolling 24 h / 7 day usage across all repos, from the ledger.
#[tauri::command]
fn usage_summary(app: AppHandle) -> Summary {
    ledger::summary(&ledger::read(&ledger_path(&app)), now_ms())
}

#[tauri::command]
fn stop_run(state: State<'_, AppState>) {
    if let Some(h) = state.run.lock().unwrap().as_ref() {
        h.stop();
    }
}

/// Watch the repo's task docs; replaces any previous watch. Returns the resolved repo path.
#[tauri::command]
fn watch_repo(app: AppHandle, state: State<'_, AppState>, repo: String) -> Result<String, String> {
    let path = expand_home(repo.trim());
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    let watcher = watch_tasks(&path, move || {
        let _ = app.emit("tasks-changed", ());
    })
    .map_err(err)?;
    state.watcher.lock().unwrap().replace(watcher);
    Ok(path.display().to_string())
}

#[tauri::command]
fn list_tasks(repo: String) -> Result<Vec<TaskSummary>, String> {
    tasks::list(&expand_home(&repo)).map_err(err)
}

#[tauri::command]
fn read_task(repo: String, slug: String) -> Result<String, String> {
    tasks::read(&expand_home(&repo), &slug).map_err(err)
}

#[tauri::command]
fn write_task(repo: String, slug: String, content: String) -> Result<(), String> {
    tasks::write(&expand_home(&repo), &slug, &content).map_err(err)
}

#[tauri::command]
fn set_task_status(repo: String, slug: String, status: StatusKind) -> Result<(), String> {
    tasks::set_status(&expand_home(&repo), &slug, status).map_err(err)
}

/// The repo's policy file, or the commented template when it has none yet.
#[tauri::command]
fn read_policy(repo: String) -> Result<(String, bool), String> {
    let path = policy_path(&expand_home(&repo));
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((text, true)),
        // No policy yet: the template, with the build command guessed from the repo's files.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((suggested_toml(&expand_home(&repo)), false)),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

/// Validates before writing, so a typo never reaches harness-guard.
#[tauri::command]
fn save_policy(repo: String, text: String) -> Result<(), String> {
    Policy::from_toml(&text).map_err(err)?;
    let path = policy_path(&expand_home(&repo));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            start_run,
            start_loop,
            list_checkpoints,
            restore_checkpoint,
            task_review,
            delivery_plan,
            deliver,
            answer_permission,
            send_note,
            stop_run,
            pause_run,
            resume_run,
            usage_summary,
            watch_repo,
            list_tasks,
            read_task,
            write_task,
            set_task_status,
            read_policy,
            save_policy
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
