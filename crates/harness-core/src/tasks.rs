//! Task store over `<repo>/.ai-tasks/<slug>.md`. The markdown file is the source of truth;
//! the app parses only the Status line, the Acceptance criteria checklist and Review findings.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Analysis,
    Ready,
    InReview,
    ChangesRequested,
    Done,
    Unknown,
}

impl StatusKind {
    /// The canonical words written to a doc's Status line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Analysis => "Analysis",
            Self::Ready => "Ready for implementation",
            Self::InReview => "In review",
            Self::ChangesRequested => "Changes requested",
            Self::Done => "Done",
            Self::Unknown => "Unknown",
        }
    }

    /// Prefix match, so `Done (with notes)` is still Done.
    pub fn of(status: &str) -> Self {
        let s = status.trim().to_lowercase();
        [Self::Analysis, Self::Ready, Self::InReview, Self::ChangesRequested, Self::Done]
            .into_iter()
            .find(|k| s.starts_with(&k.label().to_lowercase()))
            .unwrap_or(Self::Unknown)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TaskSummary {
    pub slug: String,
    pub title: String,
    pub status: String,
    pub status_kind: StatusKind,
    pub criteria_done: u32,
    pub criteria_total: u32,
    pub open_findings: u32,
    pub blockers: u32,
    pub open_questions: u32,
    pub modified_ms: u64,
}

const STATUS_PREFIX: &str = "**Status:**";

pub fn tasks_dir(repo: &Path) -> PathBuf {
    repo.join(".ai-tasks")
}

pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty() && !slug.contains("..") && slug.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn doc_path(repo: &Path, slug: &str) -> anyhow::Result<PathBuf> {
    if !valid_slug(slug) {
        bail!("invalid slug {slug:?}: use letters, digits, '.', '_' or '-'");
    }
    Ok(tasks_dir(repo).join(format!("{slug}.md")))
}

/// Lines of the `## <heading>` section, up to the next `## ` heading.
fn section<'a>(md: &'a str, heading: &str) -> impl Iterator<Item = &'a str> {
    let mut lines = md.lines();
    let wanted = heading.to_lowercase();
    let found = lines.by_ref().any(|l| {
        l.strip_prefix("## ").is_some_and(|h| h.trim().to_lowercase().starts_with(&wanted))
    });
    lines.take_while(move |l| found && !l.starts_with("## "))
}

fn checkbox(line: &str) -> Option<bool> {
    let l = line.trim_start();
    if l.starts_with("- [ ]") {
        Some(false)
    } else if l.starts_with("- [x]") || l.starts_with("- [X]") {
        Some(true)
    } else {
        None
    }
}

/// `- [ ] R2-3 [Blocker] …` → Some(is_blocker) for an unticked finding.
fn open_finding(line: &str) -> Option<bool> {
    let rest = line.trim_start().strip_prefix("- [ ] R")?;
    let (n, k) = rest.split_once('-')?;
    let k_digits: String = k.chars().take_while(char::is_ascii_digit).collect();
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) || k_digits.is_empty() {
        return None;
    }
    Some(rest.contains("[Blocker]"))
}

pub fn parse(slug: &str, md: &str) -> TaskSummary {
    let title = md
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .unwrap_or_else(|| slug.to_string());
    let status = md
        .lines()
        .find_map(|l| l.trim_start().strip_prefix(STATUS_PREFIX))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let criteria: Vec<bool> = section(md, "Acceptance criteria").filter_map(checkbox).collect();
    let findings: Vec<bool> = section(md, "Review log").filter_map(open_finding).collect();
    let open_questions = section(md, "Open questions")
        .filter(|l| {
            let t = l.trim_start();
            (t.starts_with("- ") || t.starts_with("* ") || t.chars().next().is_some_and(|c| c.is_ascii_digit()))
                && !t.to_lowercase().contains("decided")
                && checkbox(t) != Some(true)
        })
        .count() as u32;
    TaskSummary {
        slug: slug.to_string(),
        title,
        status_kind: StatusKind::of(&status),
        status,
        criteria_done: criteria.iter().filter(|d| **d).count() as u32,
        criteria_total: criteria.len() as u32,
        open_findings: findings.len() as u32,
        blockers: findings.iter().filter(|b| **b).count() as u32,
        open_questions,
        modified_ms: 0,
    }
}

pub fn list(repo: &Path) -> anyhow::Result<Vec<TaskSummary>> {
    let dir = tasks_dir(repo);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(slug) = name.strip_suffix(".md") else { continue };
        if !path.is_file() || slug.ends_with(".history") || !valid_slug(slug) {
            continue;
        }
        let md = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut summary = parse(slug, &md);
        summary.modified_ms = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        tasks.push(summary);
    }
    tasks.sort_by_key(|t| std::cmp::Reverse(t.modified_ms));
    Ok(tasks)
}

pub fn read(repo: &Path, slug: &str) -> anyhow::Result<String> {
    let path = doc_path(repo, slug)?;
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

pub fn write(repo: &Path, slug: &str, content: &str) -> anyhow::Result<()> {
    let path = doc_path(repo, slug)?;
    fs::create_dir_all(tasks_dir(repo))?;
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))
}

/// Rewrite only the Status line (inserting one under the title if the doc has none).
pub fn with_status(md: &str, kind: StatusKind) -> String {
    let line = format!("{STATUS_PREFIX} {}", kind.label());
    let mut replaced = false;
    let mut out: Vec<String> = md
        .lines()
        .map(|l| {
            if !replaced && l.trim_start().starts_with(STATUS_PREFIX) {
                replaced = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !replaced {
        let at = out.iter().position(|l| l.starts_with("# ")).map(|i| i + 1).unwrap_or(0);
        out.splice(at..at, ["".to_string(), line]);
    }
    let mut s = out.join("\n");
    if md.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn set_status(repo: &Path, slug: &str, kind: StatusKind) -> anyhow::Result<()> {
    if kind == StatusKind::Unknown {
        bail!("cannot set status Unknown");
    }
    let md = read(repo, slug)?;
    write(repo, slug, &with_status(&md, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# T-1 — Fix it\n\n**Status:** Changes requested (round 2)\n\n## Problem\nx\n\n## Acceptance criteria\n\n- [x] one\n- [ ] two\n  - [X] nested three\n\n## Open questions\n\n- Should we? Decided: yes\n- What about Y?\n1. And Z?\n\n## Review log\n\n### Review 1\n- [x] R1-1 [Major] done (a:1)\n- [ ] R1-2 [Blocker] broken (b:2)\n- [ ] R1-3 [Minor] nit\n- [ ] not a finding\n";

    #[test]
    fn parses_summary() {
        let s = parse("T-1", DOC);
        assert_eq!(s.title, "T-1 — Fix it");
        assert_eq!(s.status, "Changes requested (round 2)");
        assert_eq!(s.status_kind, StatusKind::ChangesRequested);
        assert_eq!((s.criteria_done, s.criteria_total), (2, 3));
        assert_eq!((s.open_findings, s.blockers), (2, 1));
        assert_eq!(s.open_questions, 2);
    }

    #[test]
    fn status_kinds_match_by_prefix() {
        assert_eq!(StatusKind::of("Ready for implementation"), StatusKind::Ready);
        assert_eq!(StatusKind::of(" done — shipped"), StatusKind::Done);
        assert_eq!(StatusKind::of("In review"), StatusKind::InReview);
        assert_eq!(StatusKind::of("Blocked"), StatusKind::Unknown);
        assert_eq!(StatusKind::of(""), StatusKind::Unknown);
    }

    #[test]
    fn missing_sections_count_zero() {
        let s = parse("x", "no title here");
        assert_eq!(s.title, "x");
        assert_eq!(s.status_kind, StatusKind::Unknown);
        assert_eq!((s.criteria_total, s.open_findings, s.open_questions), (0, 0, 0));
    }

    #[test]
    fn with_status_rewrites_only_the_status_line() {
        let out = with_status(DOC, StatusKind::Done);
        assert!(out.contains("**Status:** Done\n"));
        assert_eq!(out.lines().count(), DOC.lines().count());
        assert_eq!(out.replace("**Status:** Done", ""), DOC.replace("**Status:** Changes requested (round 2)", ""));
        let inserted = with_status("# T\n\nbody\n", StatusKind::Analysis);
        assert_eq!(inserted, "# T\n\n**Status:** Analysis\n\nbody\n");
    }

    #[test]
    fn list_skips_runs_history_and_junk() {
        let repo = std::env::temp_dir().join(format!("harness-tasks-{}", std::process::id()));
        let dir = tasks_dir(&repo);
        fs::create_dir_all(dir.join("runs/T-1")).unwrap();
        fs::write(dir.join("T-1.md"), DOC).unwrap();
        fs::write(dir.join("T-1.history.md"), "# old").unwrap();
        fs::write(dir.join("runs/T-1/x.md"), "# run").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        let slugs: Vec<String> = list(&repo).unwrap().into_iter().map(|t| t.slug).collect();
        assert_eq!(slugs, vec!["T-1"]);
        set_status(&repo, "T-1", StatusKind::InReview).unwrap();
        assert_eq!(parse("T-1", &read(&repo, "T-1").unwrap()).status_kind, StatusKind::InReview);
        assert!(read(&repo, "../etc/passwd").is_err());
        fs::remove_dir_all(&repo).unwrap();
    }
}
