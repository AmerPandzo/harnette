//! Usage ledger: one JSON line per finished agent run, kept in the app data dir across repos.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub ts_ms: u64,
    pub repo: String,
    pub slug: String,
    pub role: String,
    pub new_tokens: u64,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskUsage {
    pub repo: String,
    pub slug: String,
    pub new_tokens: u64,
    pub usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Summary {
    pub day_tokens: u64,
    pub day_usd: f64,
    pub day_runs: u32,
    pub week_tokens: u64,
    pub week_usd: f64,
    pub week_runs: u32,
    /// Most expensive tasks of the last 7 days.
    pub top_tasks: Vec<TaskUsage>,
}

pub fn append(path: &Path, entry: &Entry) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(entry)?)?;
    Ok(())
}

/// Unreadable lines are skipped: the ledger is informational and must never block a run.
pub fn read(path: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn task_totals(entries: &[Entry], repo: &str, slug: &str) -> (u64, f64) {
    entries
        .iter()
        .filter(|e| e.repo == repo && e.slug == slug)
        .fold((0, 0.0), |(t, u), e| (t + e.new_tokens, u + e.usd))
}

const DAY_MS: u64 = 86_400_000;

/// Rolling windows: the last 24 hours and the last 7 days.
pub fn summary(entries: &[Entry], now_ms: u64) -> Summary {
    let mut s = Summary::default();
    let mut tasks: Vec<TaskUsage> = Vec::new();
    for e in entries.iter().filter(|e| e.ts_ms + 7 * DAY_MS > now_ms) {
        s.week_tokens += e.new_tokens;
        s.week_usd += e.usd;
        s.week_runs += 1;
        if e.ts_ms + DAY_MS > now_ms {
            s.day_tokens += e.new_tokens;
            s.day_usd += e.usd;
            s.day_runs += 1;
        }
        match tasks.iter_mut().find(|t| t.repo == e.repo && t.slug == e.slug) {
            Some(t) => {
                t.new_tokens += e.new_tokens;
                t.usd += e.usd;
            }
            None => tasks.push(TaskUsage { repo: e.repo.clone(), slug: e.slug.clone(), new_tokens: e.new_tokens, usd: e.usd }),
        }
    }
    tasks.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    tasks.truncate(5);
    s.top_tasks = tasks;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts_ms: u64, slug: &str, tokens: u64, usd: f64) -> Entry {
        Entry { ts_ms, repo: "/r".into(), slug: slug.into(), role: "analyst".into(), new_tokens: tokens, usd }
    }

    #[test]
    fn append_read_and_sum() {
        let path = std::env::temp_dir().join(format!("harnette-ledger-{}/usage.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        append(&path, &entry(1, "A", 100, 0.5)).unwrap();
        append(&path, &entry(2, "A", 50, 0.25)).unwrap();
        append(&path, &entry(3, "B", 10, 0.1)).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"garbage\n").unwrap();
        let all = read(&path);
        assert_eq!(all.len(), 3);
        assert_eq!(task_totals(&all, "/r", "A"), (150, 0.75));
        assert_eq!(task_totals(&all, "/other", "A"), (0, 0.0));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rolling_windows_and_top_tasks() {
        let now = 30 * DAY_MS;
        let all = vec![
            entry(now - 1000, "A", 100, 1.0),
            entry(now - 2 * DAY_MS, "B", 200, 3.0),
            entry(now - 8 * DAY_MS, "C", 999, 9.0),
        ];
        let s = summary(&all, now);
        assert_eq!((s.day_tokens, s.day_runs), (100, 1));
        assert_eq!((s.week_tokens, s.week_runs), (300, 2));
        assert_eq!(s.top_tasks.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(), vec!["B", "A"]);
    }
}
