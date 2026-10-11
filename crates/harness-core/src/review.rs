//! Review findings and acceptance criteria, parsed from the task doc, plus archiving of
//! fully verified reviews into `<slug>.history.md`.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// `R2-3`
    pub id: String,
    pub round: u32,
    pub severity: String,
    pub text: String,
    pub ticked: bool,
    pub verified: bool,
    pub reopened: bool,
    /// The implementer's `Fixed:` note, if any.
    pub fixed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Criterion {
    pub text: String,
    pub done: bool,
}

fn section_lines<'a>(md: &'a str, heading: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut inside = false;
    for l in md.lines() {
        if let Some(h) = l.strip_prefix("## ") {
            inside = h.trim().to_lowercase().starts_with(&heading.to_lowercase());
            continue;
        }
        if inside {
            out.push(l);
        }
    }
    out
}

/// `- [ ] R2-3 [Major] text` → (ticked, id, round, severity, text)
fn finding_head(line: &str) -> Option<(bool, String, u32, String, String)> {
    let l = line.trim_start();
    let (ticked, rest) = match l.strip_prefix("- [ ] ") {
        Some(r) => (false, r),
        None => (true, l.strip_prefix("- [x] ").or_else(|| l.strip_prefix("- [X] "))?),
    };
    let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let (n, k) = id.strip_prefix('R')?.split_once('-')?;
    let round: u32 = n.parse().ok()?;
    let _: u32 = k.parse().ok()?;
    let rest = rest.trim_start();
    let (severity, text) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((sev, text)) => (sev.to_string(), text.trim().to_string()),
        None => (String::new(), rest.to_string()),
    };
    Some((ticked, id.to_string(), round, severity, text))
}

pub fn parse_findings(md: &str) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    for line in section_lines(md, "Review log") {
        if let Some((ticked, id, round, severity, text)) = finding_head(line) {
            findings.push(Finding {
                id,
                round,
                severity,
                verified: text.contains("Verified ✔") || text.contains("Verified ✓"),
                reopened: text.contains("Reopened:"),
                text,
                ticked,
                fixed: None,
            });
        } else if let Some(last) = findings.last_mut() {
            // Indented continuation lines belong to the finding above.
            let t = line.trim();
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some(note) = t.strip_prefix("Fixed:") {
                    last.fixed = Some(note.trim().to_string());
                }
                if t.contains("Verified ✔") || t.contains("Verified ✓") {
                    last.verified = true;
                }
                if t.contains("Reopened:") {
                    last.reopened = true;
                }
            }
        }
    }
    findings
}

pub fn parse_criteria(md: &str) -> Vec<Criterion> {
    section_lines(md, "Acceptance criteria")
        .into_iter()
        .filter_map(|l| {
            let t = l.trim_start();
            let (done, text) = match t.strip_prefix("- [ ] ") {
                Some(r) => (false, r),
                None => (true, t.strip_prefix("- [x] ").or_else(|| t.strip_prefix("- [X] "))?),
            };
            Some(Criterion { text: text.trim().to_string(), done })
        })
        .collect()
}

/// Moves fully verified `### Review N` sections (all findings ticked and Verified, or none at all),
/// except the latest review, out of the doc. Returns (new doc, text for the history file),
/// or None when nothing moves. Everything else in the doc stays byte-identical.
pub fn archive_verified(md: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let log_start = lines.iter().position(|l| l.trim_end().to_lowercase().starts_with("## review log"))?;
    let log_end = lines[log_start + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map(|p| log_start + 1 + p)
        .unwrap_or(lines.len());

    // [start, end) of each review subsection inside the log.
    let starts: Vec<usize> = (log_start + 1..log_end).filter(|&i| lines[i].starts_with("### Review")).collect();
    if starts.len() < 2 {
        return None;
    }
    let mut keep = vec![true; lines.len()];
    let mut moved = String::new();
    for (n, &start) in starts.iter().enumerate().take(starts.len() - 1) {
        let end = starts[n + 1];
        let body: String = lines[start..end].concat();
        let findings = parse_findings(&format!("## Review log\n{body}"));
        if findings.iter().all(|f| f.ticked && f.verified) {
            moved.push_str(&body);
            keep[start..end].iter_mut().for_each(|k| *k = false);
        }
    }
    if moved.is_empty() {
        return None;
    }
    let doc: String = lines.iter().zip(&keep).filter(|(_, k)| **k).map(|(l, _)| *l).collect();
    Some((doc, moved))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# T\n\n**Status:** In review\n\n## Acceptance criteria\n\n- [x] one\n- [ ] two\n\n## Review log\n\n### Review 1 (2026-10-01)\n- [x] R1-1 [Major] broken (a.rs:1) - expected: fixed Verified ✔\n  Fixed: patched a.rs\n- [x] R1-2 [Minor] nit\n  Fixed: renamed\n  Verified ✔\n\n### Review 2 (2026-10-02)\n- [ ] R2-1 [Blocker] crash (b.rs:9) - expected: no crash\n- [ ] R1-3 [Major] old issue Reopened: still wrong\n- [ ] garbage line\n- [ ] Rx-1 [Minor] bad id\n\n### Review 3 (2026-10-03)\nNo new findings.\n";

    #[test]
    fn findings_and_criteria() {
        let f = parse_findings(DOC);
        let ids: Vec<&str> = f.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["R1-1", "R1-2", "R2-1", "R1-3"]);
        assert!(f[0].ticked && f[0].verified);
        assert_eq!(f[0].fixed.as_deref(), Some("patched a.rs"));
        assert!(f[1].verified, "Verified on a continuation line counts");
        assert_eq!(f[2].severity, "Blocker");
        assert_eq!(f[2].text, "crash (b.rs:9) - expected: no crash");
        assert!(!f[2].ticked);
        assert!(f[3].reopened);
        let c = parse_criteria(DOC);
        assert_eq!(c, vec![Criterion { text: "one".into(), done: true }, Criterion { text: "two".into(), done: false }]);
    }

    #[test]
    fn archive_moves_only_verified_non_latest_reviews() {
        let (doc, moved) = archive_verified(DOC).unwrap();
        assert!(moved.starts_with("### Review 1"));
        assert!(!doc.contains("### Review 1"));
        assert!(doc.contains("### Review 2"), "open findings stay");
        assert!(doc.contains("### Review 3"), "the latest review always stays");
        assert_eq!(doc.len() + moved.len(), DOC.len(), "nothing else changes");
        assert!(archive_verified(&doc).is_none(), "idempotent");
    }

    #[test]
    fn archive_keeps_a_single_review() {
        assert!(archive_verified("## Review log\n\n### Review 1\nNo new findings.\n").is_none());
        assert!(archive_verified("# no log\n").is_none());
    }
}
