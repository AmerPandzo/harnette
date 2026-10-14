//! Tauri shell: exposes harnette-core runs and task docs to the web UI
//! as commands plus `run-event` / `tasks-changed` events.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use harnette_core::policy::{Detected, Policy, apply_values, detect_build, policy_path, suggested_toml};
use harnette_core::review::{Criterion, Finding, parse_criteria, parse_findings};
use harnette_core::tasks::{self, Question, StatusKind, TaskSummary, expand_home};
use harnette_core::watch::{TaskWatcher, watch_tasks};
use harnette_core::attach;
use harnette_core::workspace;
use harnette_core::delivery::{self, Outcome, Plan};
use harnette_core::editor::{self, Editor};
use harnette_core::git::{self, Checkpoint};
use harnette_core::ledger::{self, Entry, Summary};
use harnette_core::watchdog::{Action, PriorUsage, Watchdog};
use harnette_core::claude::{self, CHAT_SLUG};
use harnette_core::{LoopConfig, Mode, RunConfig, RunEvent, Session, start_loop as start_core_loop, start_single};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
struct AppState {
    run: Mutex<Option<Session>>,
    chat: Mutex<Option<Chat>>,
    watcher: Mutex<Option<TaskWatcher>>,
}

/// The chat's Claude process and what it is about; runs beside any run.
struct Chat {
    handle: claude::Handle,
    repo: String,
    slug: String,
    id: u64,
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn ledger_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("harnette")).join("usage.jsonl")
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

/// Numbers chats, so the UI can drop late events of a chat it already replaced.
static CHAT_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(serde::Serialize, Clone)]
struct ChatEvent<'a> {
    chat: u64,
    event: &'a RunEvent,
}

/// Chat events go out as `chat-event`. Each turn's result carries the turn's tokens but the process's
/// running cost, so the ledger and the watchdog get the cost difference to the previous turn.
/// Budgets are the task's (`_chat` has its own); time budgets do not apply, a chat waits for you by design.
fn chat_sink(app: AppHandle, repo: String, slug: String, chat: u64, dog: Watchdog) -> impl Fn(RunEvent) + Send + Sync + 'static {
    let last_usd = Mutex::new(0.0f64);
    let dog = Mutex::new(dog);
    move |e: RunEvent| {
        let observed = match &e {
            RunEvent::Finished { ok, result, cost_usd, input_tokens, output_tokens, new_tokens, duration_ms } => {
                let mut last = last_usd.lock().unwrap();
                let usd = (cost_usd - *last).max(0.0);
                *last = *cost_usd;
                let entry = Entry { ts_ms: now_ms(), repo: repo.clone(), slug: slug.clone(), role: "chat".into(), new_tokens: *new_tokens, usd };
                let _ = ledger::append(&ledger_path(&app), &entry);
                let turn = RunEvent::Finished {
                    ok: *ok,
                    result: result.clone(),
                    cost_usd: usd,
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    new_tokens: *new_tokens,
                    duration_ms: *duration_ms,
                };
                dog.lock().unwrap().observe(&turn, Instant::now())
            }
            other => dog.lock().unwrap().observe(other, Instant::now()),
        };
        if let RunEvent::PermissionAsk { tool, summary, .. } = &e {
            notify(&app, &format!("Harnette chat: allow {tool}?"), summary);
        }
        let _ = app.emit("chat-event", ChatEvent { chat, event: &e });
        for alarm in observed {
            let action = alarm.action.as_str();
            let message = match alarm.action {
                Action::Warn => alarm.message.clone(),
                Action::Pause => format!("{} Stopped this answer; your next message continues the chat.", alarm.message),
                Action::Stop => format!("{} Ended the chat.", alarm.message),
            };
            notify(&app, "Harnette chat: budget", &message);
            let event = RunEvent::Alarm { alarm: alarm.kind, message, action: action.into() };
            let _ = app.emit("chat-event", ChatEvent { chat, event: &event });
            let handle = app.state::<AppState>().chat.lock().unwrap().as_ref().filter(|c| c.id == chat).map(|c| c.handle.clone());
            match (alarm.action, handle) {
                (Action::Pause, Some(h)) => {
                    tauri::async_runtime::spawn(async move {
                        let _ = h.interrupt().await;
                    });
                }
                (Action::Stop, Some(h)) => h.stop(),
                _ => {}
            }
        }
    }
}

fn on_event(app: &AppHandle, event: RunEvent) {
    match &event {
        RunEvent::Alarm { message, action, .. } => {
            let title = match action.as_str() {
                "pause" => "Harnette: paused",
                "stop" => "Harnette: stopped",
                _ => "Harnette: warning",
            };
            notify(app, title, message);
        }
        RunEvent::PermissionAsk { tool, summary, .. } => notify(app, &format!("Harnette: allow {tool}?"), summary),
        RunEvent::Build { ok: false, command, .. } => notify(app, "Harnette: build is red", command),
        RunEvent::DeliveryReady { mode, branch, .. } => notify(app, "Harnette: ready to deliver", &format!("{mode} to {branch}: approve in the app")),
        RunEvent::Delivered { branch, pr_url, .. } => {
            notify(app, "Harnette: delivered", pr_url.as_deref().unwrap_or(branch));
        }
        RunEvent::SessionEnded { ok, summary } => {
            notify(app, if *ok { "Harnette: finished" } else { "Harnette: stopped" }, summary);
        }
        _ => {}
    }
    let _ = app.emit("run-event", &event);
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per command argument from the UI
async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    slug: String,
    mode: String,
    ticket: Option<String>,
    model: Option<String>,
    message: Option<String>,
    images: Option<Vec<String>>,
    repos: Option<Vec<String>>,
) -> Result<(), String> {
    if busy(&state) {
        return Err("A run is already in progress.".into());
    }
    let mode = match mode.as_str() {
        "analyze" => Mode::Analyze { ticket: ticket.unwrap_or_default() },
        "refine" => Mode::Refine { message },
        other => return Err(format!("unknown run mode {other:?}")),
    };
    let repo = expand_home(repo.trim());
    let slug = slug.trim().to_string();
    let (repo_key, prior) = prior_usage(&app, &repo, &slug);
    let mut cfg = RunConfig::new(repo, &slug, mode);
    cfg.model = model.filter(|m| !m.trim().is_empty());
    cfg.prior = prior;
    cfg.images = assets(images)?;
    cfg.related = related_repos(repos.unwrap_or_default())?;
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
    let mut files: Vec<FileStat> = git::numstat(&repo, &base_ref)
        .map_err(err)?
        .into_iter()
        .map(|(path, added, removed)| FileStat { path, added, removed })
        .collect();
    let mut patch = git::diff(&repo, &base_ref).map_err(err)?;
    // Member repos: their changes since the task's first checkpoint there, paths prefixed with the member name.
    for m in workspace::members(&repo, &slug) {
        let Some(base) = workspace::base(&m, &slug).map_err(err)? else { continue };
        files.extend(git::numstat(&m.path, &base).map_err(err)?.into_iter().map(|(path, added, removed)| FileStat {
            path: format!("{}/{path}", m.name),
            added,
            removed,
        }));
        patch.push('\n');
        patch.push_str(&git::diff_as(&m.path, &base, &m.name).map_err(err)?);
    }
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
    let repo = expand_home(&repo);
    let git_ref = git::checkpoint_ref(&slug, round);
    git::restore(&repo, &git_ref).map_err(err)?;
    for m in workspace::members(&repo, &slug) {
        if git::resolve(&m.path, &git_ref).is_some() {
            git::restore(&m.path, &git_ref).map_err(|e| format!("member repo {}: {e:#}", m.name))?;
        }
    }
    Ok(())
}

#[tauri::command]
fn answer_permission(state: State<'_, AppState>, request_id: String, allow: bool) -> bool {
    state.run.lock().unwrap().as_ref().is_some_and(|h| h.answer(&request_id, allow))
        || state.chat.lock().unwrap().as_ref().is_some_and(|c| c.handle.answer(&request_id, allow))
}

/// Send a chat message. Starts the chat's Claude process when there is none, or when the repo or
/// task differs from the running chat's (that one is stopped). Returns the chat's number.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per command argument from the UI
async fn chat_send(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    task: Option<String>,
    text: String,
    images: Option<Vec<String>>,
    model: Option<String>,
    resume: Option<String>,
) -> Result<u64, String> {
    let images = assets(images)?;
    let path = expand_home(repo.trim());
    let repo = path.canonicalize().unwrap_or_else(|_| path.clone()).display().to_string();
    let slug = task.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| CHAT_SLUG.into());
    let same = {
        let mut chat = state.chat.lock().unwrap();
        match chat.as_ref() {
            Some(c) if !c.handle.is_finished() && c.repo == repo && c.slug == slug => Some((c.handle.clone(), c.id)),
            _ => {
                if let Some(old) = chat.take() {
                    old.handle.stop();
                }
                None
            }
        }
    };
    if let Some((handle, id)) = same {
        return handle.say(&text, &images).await.map(|_| id).map_err(err);
    }
    let id = CHAT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let mut cfg = RunConfig::new(&path, &slug, Mode::Chat { message: text });
    cfg.model = model.filter(|m| !m.trim().is_empty());
    cfg.images = images;
    cfg.resume = resume.filter(|r| !r.trim().is_empty());
    let (_, prior) = prior_usage(&app, &path, &slug);
    let dog = Watchdog::new(Policy::load(&path).map_err(err)?.budget, prior, Instant::now());
    // The CLI enforces the dollar budget as a hard cap per process.
    cfg.max_budget_usd = dog.remaining_usd();
    let handle = claude::start(cfg, chat_sink(app, repo.clone(), slug.clone(), id, dog)).await.map_err(err)?;
    state.chat.lock().unwrap().replace(Chat { handle, repo, slug, id });
    Ok(id)
}

/// Stop the chat's current answer; the chat stays open.
#[tauri::command]
async fn chat_interrupt(state: State<'_, AppState>) -> Result<(), String> {
    let handle = state.chat.lock().unwrap().as_ref().map(|c| c.handle.clone()).ok_or("No chat is open.")?;
    handle.interrupt().await.map_err(err)
}

/// End the chat (New chat): its process exits.
#[tauri::command]
fn chat_end(state: State<'_, AppState>) {
    if let Some(c) = state.chat.lock().unwrap().take() {
        c.handle.stop();
    }
}

#[tauri::command]
async fn send_note(state: State<'_, AppState>, text: String, images: Option<Vec<String>>) -> Result<(), String> {
    let images = assets(images)?;
    let handle = state.run.lock().unwrap().clone().ok_or("No run in progress.")?;
    handle.note(&text, &images).await.map_err(err)
}

fn assets(images: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let images = images.unwrap_or_default();
    match images.iter().find(|p| !attach::is_asset(p)) {
        Some(p) => Err(format!("not an attached image: {p}")),
        None => Ok(images),
    }
}

/// Save a pasted or picked image under `.ai-tasks/<slug>.assets/`; returns its repo-relative path.
#[tauri::command]
fn attach_image(repo: String, slug: String, mime: String, data: String) -> Result<String, String> {
    attach::save_base64(&expand_home(repo.trim()), slug.trim(), &mime, &data).map_err(err)
}

/// A file dropped on the window, as `(mime, base64)`.
#[tauri::command]
fn read_image_file(path: String) -> Result<(String, String), String> {
    attach::read_file(Path::new(&path)).map_err(err)
}

#[tauri::command]
fn image_data(repo: String, path: String) -> Result<String, String> {
    attach::data_url(&expand_home(repo.trim()), &path).map_err(err)
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
    // Not every folder is a git repo; a failure here only means there is nothing to adopt.
    let _ = git::adopt_legacy_refs(&path);
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
fn task_questions(repo: String, slug: String) -> Result<Vec<Question>, String> {
    Ok(tasks::questions(&tasks::read(&expand_home(&repo), &slug).map_err(err)?))
}

/// Writes answers given in the app under their questions in the doc. `answers`: (question index, text).
#[tauri::command]
fn save_answers(repo: String, slug: String, answers: Vec<(usize, String)>) -> Result<(), String> {
    tasks::save_answers(&expand_home(&repo), &slug, &answers).map_err(err)
}

/// Claude Code plugins installed on this machine, for the policy's `[plugins]` checkboxes.
#[tauri::command]
fn list_plugins() -> Vec<String> {
    harnette_core::policy::installed_plugins()
}

#[tauri::command]
fn list_editors() -> Vec<Editor> {
    editor::installed()
}

/// Opens a repo file (never one outside the repo) in one of the known editors.
#[tauri::command]
fn open_in_editor(repo: String, path: String, line: Option<u32>, editor: String) -> Result<(), String> {
    editor::open(&expand_home(&repo), &path, line, &editor).map_err(err)
}

#[tauri::command]
fn write_task(repo: String, slug: String, content: String) -> Result<(), String> {
    tasks::write(&expand_home(&repo), &slug, &content).map_err(err)
}

/// Trimmed related repo paths, each an existing directory.
fn related_repos(repos: Vec<String>) -> Result<Vec<String>, String> {
    let repos: Vec<String> = repos.into_iter().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect();
    match repos.iter().find(|r| !expand_home(r).is_dir()) {
        Some(r) => Err(format!("not a directory: {r}")),
        None => Ok(repos),
    }
}

/// Replace the task's `**Repos:**` line.
#[tauri::command]
fn set_task_repos(repo: String, slug: String, repos: Vec<String>) -> Result<(), String> {
    tasks::set_related(&expand_home(&repo), &slug, &related_repos(repos)?).map_err(err)
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

/// Validates before writing, so a typo never reaches harnette-guard.
#[tauri::command]
fn save_policy(repo: String, text: String) -> Result<(), String> {
    Policy::from_toml(&text).map_err(err)?;
    let path = policy_path(&expand_home(&repo));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

#[derive(serde::Serialize)]
struct PolicyForm {
    values: Policy,
    defaults: Policy,
    detected: Option<Detected>,
    exists: bool,
}

/// The policy as form values (file or, when there is none, the detected template).
#[tauri::command]
fn policy_form(repo: String) -> Result<PolicyForm, String> {
    let path = expand_home(&repo);
    let (text, exists) = read_policy(repo)?;
    Ok(PolicyForm { values: Policy::from_toml(&text).map_err(err)?, defaults: Policy::default(), detected: detect_build(&path), exists })
}

/// Save form values into the policy file, keeping its comments. Returns the new text.
#[tauri::command]
fn save_policy_values(repo: String, values: serde_json::Value) -> Result<String, String> {
    let (text, _) = read_policy(repo.clone())?;
    let out = apply_values(&text, &values).map_err(err)?;
    save_policy(repo, out.clone())?;
    Ok(out)
}

/// An app started from Finder gets launchd's PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), which has no
/// `claude`, `copilot`, `mvn` or `docker`. Ask the user's login shell for its PATH and put it first.
/// Call before any thread starts: it sets an environment variable.
pub fn adopt_login_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let Ok(mut child) = std::process::Command::new(&shell)
        .args(["-ilc", "printf '\\n__KPATH__%s__KPATH__' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    // A shell profile that waits for input must not hang the app.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while matches!(child.try_wait(), Ok(None)) {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut so, &mut out);
    }
    let Some(login) = out.split("__KPATH__").nth(1) else { return };
    let current = std::env::var("PATH").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let merged: Vec<&str> = login.split(':').chain(current.split(':')).filter(|d| !d.is_empty() && seen.insert(*d)).collect();
    // SAFETY: called from main before the Tauri runtime or any other thread exists.
    unsafe { std::env::set_var("PATH", merged.join(":")) };
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
            attach_image,
            read_image_file,
            image_data,
            start_loop,
            list_checkpoints,
            restore_checkpoint,
            task_review,
            delivery_plan,
            deliver,
            answer_permission,
            send_note,
            chat_send,
            chat_interrupt,
            chat_end,
            stop_run,
            pause_run,
            resume_run,
            usage_summary,
            watch_repo,
            list_tasks,
            read_task,
            list_editors,
            list_plugins,
            task_questions,
            save_answers,
            open_in_editor,
            write_task,
            set_task_status,
            set_task_repos,
            read_policy,
            save_policy,
            policy_form,
            save_policy_values
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quitting must not leave an agent running (or frozen by Pause) behind.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                if let Some(session) = state.run.lock().unwrap().as_ref() {
                    session.shutdown();
                }
                if let Some(chat) = state.chat.lock().unwrap().as_ref() {
                    chat.handle.kill_now();
                }
            }
        });
}
