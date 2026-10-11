//! Tauri shell: exposes harness-core runs and task docs to the web UI
//! as commands plus `run-event` / `tasks-changed` events.

use std::path::PathBuf;
use std::sync::Mutex;

use harness_core::tasks::{self, StatusKind, TaskSummary};
use harness_core::watch::{TaskWatcher, watch_tasks};
use harness_core::{Mode, RunConfig, RunEvent, RunHandle, start_run as start_core_run};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
struct AppState {
    run: Mutex<Option<RunHandle>>,
    watcher: Mutex<Option<TaskWatcher>>,
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn on_event(app: &AppHandle, event: RunEvent) {
    match &event {
        RunEvent::PermissionAsk { tool, summary, .. } => notify(app, &format!("Harness: allow {tool}?"), summary),
        RunEvent::Finished { ok, cost_usd, .. } => {
            let title = if *ok { "Harness: run finished" } else { "Harness: run failed" };
            notify(app, title, &format!("Cost ${cost_usd:.4}"));
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
) -> Result<String, String> {
    if state.run.lock().unwrap().as_ref().is_some_and(|h| !h.is_finished()) {
        return Err("A run is already in progress.".into());
    }
    let mode = match mode.as_str() {
        "analyze" => Mode::Analyze { ticket: ticket.unwrap_or_default() },
        "refine" => Mode::Refine,
        other => return Err(format!("unknown run mode {other:?}")),
    };
    let mut cfg = RunConfig::new(expand_home(repo.trim()), slug.trim(), mode);
    cfg.model = model.filter(|m| !m.trim().is_empty());
    let sink_app = app.clone();
    let handle = start_core_run(cfg, move |e| on_event(&sink_app, e)).await.map_err(err)?;
    let run_id = handle.run_id().to_string();
    state.run.lock().unwrap().replace(handle);
    Ok(run_id)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            start_run,
            answer_permission,
            send_note,
            stop_run,
            watch_repo,
            list_tasks,
            read_task,
            write_task,
            set_task_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
