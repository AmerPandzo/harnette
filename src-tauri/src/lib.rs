//! Tauri shell: exposes harness-core runs to the web UI as commands and `run-event` events.

use std::path::PathBuf;
use std::sync::Mutex;

use harness_core::{RunConfig, RunEvent, RunHandle, start_analyze};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_notification::NotificationExt;

#[derive(Default)]
struct AppState {
    run: Mutex<Option<RunHandle>>,
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn on_event(app: &AppHandle, event: RunEvent) {
    match &event {
        RunEvent::PermissionAsk { tool, summary, .. } => notify(app, &format!("Harness: allow {tool}?"), summary),
        RunEvent::Finished { ok, cost_usd, .. } => {
            let title = if *ok { "Harness: analysis finished" } else { "Harness: run failed" };
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

#[tauri::command]
async fn start_run(
    app: AppHandle,
    state: State<'_, AppState>,
    repo: String,
    slug: String,
    ticket: String,
    model: Option<String>,
) -> Result<String, String> {
    if state.run.lock().unwrap().as_ref().is_some_and(|h| !h.is_finished()) {
        return Err("A run is already in progress.".into());
    }
    let mut cfg = RunConfig::new(expand_home(repo.trim()), slug.trim(), ticket);
    cfg.model = model.filter(|m| !m.trim().is_empty());
    let sink_app = app.clone();
    let handle = start_analyze(cfg, move |e| on_event(&sink_app, e)).await.map_err(|e| format!("{e:#}"))?;
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
    handle.note(&text).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn stop_run(state: State<'_, AppState>) {
    if let Some(h) = state.run.lock().unwrap().as_ref() {
        h.stop();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![start_run, answer_permission, send_note, stop_run])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
