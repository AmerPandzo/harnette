//! Per-run journal under `<repo>/.ai-tasks/runs/<slug>/<run-id>/`.
//! `events.jsonl` holds normalized events; `raw.jsonl` holds the agent's own output for debugging.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::event::RunEvent;

pub struct Journal {
    dir: PathBuf,
    events: File,
    raw: File,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub fn new_run_id() -> String {
    format!("run-{}", now_ms())
}

impl Journal {
    pub fn open(repo: &Path, slug: &str, run_id: &str) -> anyhow::Result<Self> {
        let dir = repo.join(".ai-tasks").join("runs").join(slug).join(run_id);
        fs::create_dir_all(&dir)?;
        let append = |name: &str| OpenOptions::new().create(true).append(true).open(dir.join(name));
        Ok(Self { events: append("events.jsonl")?, raw: append("raw.jsonl")?, dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn event(&mut self, event: &RunEvent) {
        let line = json!({ "ts": now_ms() as u64, "event": event });
        // A journal write failure must never stop the run.
        let _ = writeln!(self.events, "{line}");
    }

    pub fn raw(&mut self, line: &str) {
        let _ = writeln!(self.raw, "{line}");
    }
}

/// Today's date as YYYY-MM-DD (UTC), for review headings.
pub fn today() -> String {
    let days = (now_ms() / 86_400_000) as i64;
    // Civil-from-days (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}
