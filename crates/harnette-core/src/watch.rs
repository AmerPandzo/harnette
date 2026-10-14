//! Watches `<repo>/.ai-tasks/` so the board follows edits made outside the app.

use std::path::Path;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::tasks::tasks_dir;

/// Stops watching when dropped.
pub struct TaskWatcher {
    _inner: RecommendedWatcher,
}

/// Calls `on_change` for every create/modify/remove of a top-level `*.md` in `.ai-tasks/`.
/// Non-recursive: run journals under `runs/` change constantly and are not tasks.
pub fn watch_tasks<F>(repo: &Path, on_change: F) -> anyhow::Result<TaskWatcher>
where
    F: Fn() + Send + 'static,
{
    let dir = tasks_dir(repo);
    std::fs::create_dir_all(&dir)?;
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_))
            && event.paths.iter().any(|p| p.extension().is_some_and(|e| e == "md"));
        if relevant {
            on_change();
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(TaskWatcher { _inner: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn reports_doc_changes_but_not_run_journals() {
        let repo = std::env::temp_dir().join(format!("harnette-watch-{}", std::process::id()));
        let dir = tasks_dir(&repo);
        std::fs::create_dir_all(dir.join("runs/T-1")).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch_tasks(&repo, move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));

        std::fs::write(dir.join("runs/T-1/events.md"), "x").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let journal_events = rx.try_iter().count();

        std::fs::write(dir.join("T-1.md"), "# T-1\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok(), "doc change not reported");
        assert_eq!(journal_events, 0, "run journal changes must not be reported");
        std::fs::remove_dir_all(&repo).unwrap();
    }
}
