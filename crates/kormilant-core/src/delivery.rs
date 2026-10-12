//! Delivery: after the Done gate, the app (never an agent) commits, pushes or opens a PR,
//! as `[delivery]` in the policy says. Planning and execution are separate so the plan can be
//! shown for approval exactly as it will run.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::policy::{Delivery, DeliveryMode, Policy};
use crate::review::parse_findings;
use crate::tasks::{self, StatusKind};

/// Written by the loop after every build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastBuild {
    pub ok: bool,
    pub command: String,
    /// Snapshot tree the build ran on.
    pub tree: String,
    pub ts_ms: u64,
}

pub fn last_build_path(repo: &Path, slug: &str) -> PathBuf {
    tasks::tasks_dir(repo).join("runs").join(slug).join("last-build.json")
}

pub fn record_build(repo: &Path, slug: &str, build: &LastBuild) -> anyhow::Result<()> {
    let path = last_build_path(repo, slug);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(build)?)?;
    Ok(())
}

fn read_last_build(repo: &Path, slug: &str) -> Option<LastBuild> {
    serde_json::from_str(&std::fs::read_to_string(last_build_path(repo, slug)).ok()?).ok()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Plan {
    pub slug: String,
    pub mode: String,
    /// Changes since this ref are delivered (the task's first checkpoint).
    pub base: String,
    pub branch: String,
    pub create_branch: bool,
    pub files: Vec<String>,
    pub message: String,
    pub pr_title: String,
    pub pr_body: String,
    /// The exact commands `execute` runs, for the approval card.
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Outcome {
    pub branch: String,
    pub commit: Option<String>,
    pub pushed: bool,
    pub pr_url: Option<String>,
}

pub fn is_protected(branch: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => branch.starts_with(prefix),
        None => branch == p,
    })
}

fn section(md: &str, heading: &str) -> String {
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
    out.join("\n").trim().to_string()
}

fn latest_review(md: &str) -> String {
    let log = section(md, "Review log");
    match log.rfind("### Review") {
        Some(i) => log[i..].trim().to_string(),
        None => String::new(),
    }
}

/// The user's own git: no Kormilant identity, so commits are authored by the user.
fn user_git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git").arg("-C").arg(repo).args(args).output().context("could not run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn current_branch(repo: &Path) -> Option<String> {
    user_git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().filter(|b| !b.is_empty())
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    user_git(repo, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok()
}

fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// With `branch_at_start`, put the work on the task branch before the first edit: from a protected or
/// detached HEAD, switch to `delivery.branch` (creating it at HEAD if needed). Uncommitted changes come along.
/// Returns what was done, or None when nothing needed doing.
pub fn start_branch(repo: &Path, slug: &str, d: &Delivery) -> anyhow::Result<Option<String>> {
    if !d.branch_at_start {
        return Ok(None);
    }
    let current = current_branch(repo);
    if let Some(b) = &current
        && !is_protected(b, &d.protected)
    {
        return Ok(None);
    }
    let branch = d.branch.replace("{slug}", slug);
    if is_protected(&branch, &d.protected) {
        bail!("delivery.branch {branch} is protected; set a non-protected name.");
    }
    let from = current.unwrap_or_else(|| "a detached HEAD".into());
    if branch_exists(repo, &branch) {
        user_git(repo, &["switch", &branch]).with_context(|| format!("could not switch to the existing task branch {branch}"))?;
        Ok(Some(format!("Switched from {from} to the existing task branch {branch}.")))
    } else {
        user_git(repo, &["switch", "-c", &branch])?;
        Ok(Some(format!("Created task branch {branch} from {from} and switched to it; uncommitted changes came along.")))
    }
}

/// Checks the gate and works out what delivery would do, without changing anything.
pub fn plan(repo: &Path, slug: &str, policy: &Policy) -> anyhow::Result<Plan> {
    let d: &Delivery = &policy.delivery;
    let md = tasks::read(repo, slug)?;
    let summary = tasks::parse(slug, &md);
    if summary.status_kind != StatusKind::Done {
        bail!("Delivery runs only for Done tasks; {slug} is {}.", summary.status_kind.label());
    }
    let open: Vec<String> = parse_findings(&md)
        .into_iter()
        .filter(|f| !f.ticked && matches!(f.severity.as_str(), "Blocker" | "Major"))
        .map(|f| f.id)
        .collect();
    if !open.is_empty() {
        bail!("Open Blocker/Major findings: {}.", open.join(", "));
    }
    if !git::is_repo(repo) {
        bail!("Not a git repository.");
    }
    if let Some(command) = &policy.build.command {
        let last = read_last_build(repo, slug).with_context(|| format!("No Kormilant build recorded for {slug}; run the loop first."))?;
        if !last.ok {
            bail!("The last build (`{}`) is red.", last.command);
        }
        if last.command != *command {
            bail!("The last build ran `{}`, but the policy's build command is now `{command}`. Run the loop again.", last.command);
        }
        let now = git::tree_of(repo, &git::snapshot(repo)?)?;
        if now != last.tree {
            bail!("Files changed after the last green build; run the loop again so the delivered code is the code that was built.");
        }
    }
    // Changes since the task started, or since its last delivery when it came back for more work.
    let delivered = git::delivered_ref(slug);
    let base = match git::list_checkpoints(repo, slug)?.first() {
        _ if git::resolve(repo, &delivered).is_some() => delivered,
        Some(c) => c.git_ref.clone(),
        None => git::head(repo)?.context("no commits yet")?,
    };
    let files = git::changed_since(repo, &base)?;

    let task_branch = d.branch.replace("{slug}", slug);
    let current = current_branch(repo);
    let (branch, create_branch) = match &current {
        Some(b) if !is_protected(b, &d.protected) => (b.clone(), false),
        _ => (task_branch, true),
    };
    if create_branch && branch_exists(repo, &branch) && d.mode != DeliveryMode::None {
        bail!("Branch {branch} already exists. Switch to it, or delete it, and deliver again.");
    }
    if is_protected(&branch, &d.protected) {
        bail!("{branch} is protected; set delivery.branch to a non-protected name.");
    }

    let title = summary.title.clone();
    let pr_title = match title.strip_prefix(slug) {
        Some(rest) => format!("{slug}: {}", rest.trim_start_matches([' ', '—', '-', ':']).trim()),
        None => format!("{slug}: {title}"),
    };
    let problem = section(&md, "Problem");
    let first_para = problem.split("\n\n").next().unwrap_or("").trim().to_string();
    let message = format!("{pr_title}\n\n{first_para}\n\nKormilant task: {slug}");
    let pr_body = format!(
        "## Problem\n\n{problem}\n\n## Implementation notes\n\n{}\n\n## Review\n\n{}\n\n---\nDelivered by Kormilant (task `{slug}`).",
        section(&md, "Implementation notes"),
        latest_review(&md)
    );

    let mode = match d.mode {
        DeliveryMode::None => "none",
        DeliveryMode::Commit => "commit",
        DeliveryMode::Push => "push",
        DeliveryMode::Pr => "pr",
    };
    let mut commands = Vec::new();
    if d.mode != DeliveryMode::None {
        if files.is_empty() {
            bail!("Nothing to deliver: no files changed since the task started.");
        }
        if create_branch {
            commands.push(format!("create branch {} there and switch to it (working tree untouched)", quote(&branch)));
        }
        commands.push(format!(
            "commit the task's changes since {base} ({} file{}) as {}",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            quote(&pr_title)
        ));
    }
    if matches!(d.mode, DeliveryMode::Push | DeliveryMode::Pr) {
        // Never --force, whatever delivery.force_push says.
        commands.push(format!("git push -u origin {}", quote(&branch)));
    }
    if d.mode == DeliveryMode::Pr {
        commands.push(format!("gh pr create --head {} --title {} --body-file <PR body>", quote(&branch), quote(&pr_title)));
    }

    Ok(Plan { slug: slug.to_string(), mode: mode.into(), base, branch, create_branch, files, message, pr_title, pr_body, commands })
}

/// Runs a plan made by `plan` (call `plan` again right before, so the gate is fresh).
pub fn execute(repo: &Path, plan: &Plan) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome { branch: plan.branch.clone(), commit: None, pushed: false, pr_url: None };
    if plan.mode == "none" {
        return Ok(outcome);
    }
    // Commit only the task's own changes on top of HEAD; the user's earlier uncommitted work
    // stays in the working tree as it was.
    let old_head = git::head(repo)?.context("the repository has no commits yet")?;
    let commit = git::commit_changes_since(repo, &plan.base, &plan.files, &plan.message)?;
    if plan.create_branch {
        user_git(repo, &["update-ref", &format!("refs/heads/{}", plan.branch), &commit])?;
        user_git(repo, &["symbolic-ref", "HEAD", &format!("refs/heads/{}", plan.branch)])?;
    } else {
        user_git(repo, &["update-ref", "HEAD", &commit, &old_head])?;
    }
    // Delivered paths now match HEAD in the index; anything else the user staged stays staged.
    let mut reset = vec!["reset", "-q", "--"];
    reset.extend(plan.files.iter().map(String::as_str));
    user_git(repo, &reset)?;
    git::mark_delivered(repo, &plan.slug)?;
    outcome.commit = Some(commit);
    if plan.mode == "push" || plan.mode == "pr" {
        user_git(repo, &["push", "-u", "origin", &plan.branch])?;
        outcome.pushed = true;
    }
    if plan.mode == "pr" {
        let body = std::env::temp_dir().join(format!("kormilant-pr-{}-{}.md", plan.slug, std::process::id()));
        std::fs::write(&body, &plan.pr_body)?;
        let out = Command::new("gh")
            .current_dir(repo)
            .args(["pr", "create", "--head", &plan.branch, "--title", &plan.pr_title, "--body-file"])
            .arg(&body)
            .output()
            .context("could not run gh; is the GitHub CLI installed?")?;
        let _ = std::fs::remove_file(&body);
        if !out.status.success() {
            bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        outcome.pr_url = String::from_utf8_lossy(&out.stdout).lines().rev().find(|l| l.starts_with("http")).map(String::from);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Approval;
    use std::fs;

    const DONE: &str = "# T-1 — Fix the thing\n\n**Status:** Done\n\n## Problem\n\nIt breaks.\n\nMore detail.\n\n## Acceptance criteria\n\n- [x] works\n\n## Implementation notes\n\nChanged a.txt.\n\n## Review log\n\n### Review 1 (2026-10-11)\n\n- [ ] R1-1 [Minor] nit\n";

    fn setup(name: &str, status_doc: &str) -> PathBuf {
        let repo = crate::git::tests::temp_repo(name);
        user_git(&repo, &["config", "user.name", "Test User"]).unwrap();
        user_git(&repo, &["config", "user.email", "user@example.com"]).unwrap();
        fs::write(repo.join(".git/info/exclude"), ".ai-tasks/\n").unwrap();
        // The user's own uncommitted work before the task: must not be delivered.
        fs::write(repo.join("mine.txt"), "user work\n").unwrap();
        git::checkpoint(&repo, "T-1", 1).unwrap();
        fs::write(repo.join("a.txt"), "fixed\n").unwrap();
        tasks::write(&repo, "T-1", status_doc).unwrap();
        repo
    }

    fn policy(mode: DeliveryMode, build: Option<&str>) -> Policy {
        let mut p = Policy::default();
        p.delivery.mode = mode;
        p.delivery.approval = Approval::Auto;
        p.build.command = build.map(String::from);
        p
    }

    fn green(repo: &Path, command: &str) {
        let tree = git::tree_of(repo, &git::snapshot(repo).unwrap()).unwrap();
        record_build(repo, "T-1", &LastBuild { ok: true, command: command.into(), tree, ts_ms: 0 }).unwrap();
    }

    #[test]
    fn branch_at_start_moves_work_off_a_protected_branch() {
        let repo = setup("start", DONE);
        user_git(&repo, &["branch", "-M", "develop"]).unwrap();
        let mut d = Policy::default().delivery;
        assert_eq!(start_branch(&repo, "T-1", &d).unwrap(), None, "off by default");
        d.branch_at_start = true;
        let msg = start_branch(&repo, "T-1", &d).unwrap().unwrap();
        assert!(msg.contains("Created task branch ai/T-1 from develop"), "{msg}");
        assert_eq!(current_branch(&repo).as_deref(), Some("ai/T-1"));
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "fixed\n", "uncommitted work came along");
        assert_eq!(start_branch(&repo, "T-1", &d).unwrap(), None, "already on a non-protected branch");
        user_git(&repo, &["switch", "develop"]).unwrap();
        assert!(start_branch(&repo, "T-1", &d).unwrap().unwrap().contains("existing task branch ai/T-1"));
        d.branch = "release/{slug}".into();
        user_git(&repo, &["switch", "develop"]).unwrap();
        assert!(start_branch(&repo, "T-1", &d).is_err(), "never a protected task branch");
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn gate_refusals() {
        let repo = setup("gate", &DONE.replace("**Status:** Done", "**Status:** In review"));
        let err = |p: &Policy| plan(&repo, "T-1", p).unwrap_err().to_string();
        assert!(err(&policy(DeliveryMode::Commit, None)).contains("only for Done"));

        tasks::write(&repo, "T-1", &DONE.replace("[Minor] nit", "[Blocker] crash")).unwrap();
        assert!(err(&policy(DeliveryMode::Commit, None)).contains("R1-1"));

        tasks::write(&repo, "T-1", DONE).unwrap();
        assert!(err(&policy(DeliveryMode::Commit, Some("make test"))).contains("No Kormilant build"));
        green(&repo, "make test");
        assert!(plan(&repo, "T-1", &policy(DeliveryMode::Commit, Some("make test"))).is_ok());
        assert!(err(&policy(DeliveryMode::Commit, Some("make check"))).contains("build command is now"));
        fs::write(repo.join("a.txt"), "changed after the build\n").unwrap();
        assert!(err(&policy(DeliveryMode::Commit, Some("make test"))).contains("after the last green build"));
        let tree = git::tree_of(&repo, &git::snapshot(&repo).unwrap()).unwrap();
        record_build(&repo, "T-1", &LastBuild { ok: false, command: "make test".into(), tree, ts_ms: 0 }).unwrap();
        assert!(err(&policy(DeliveryMode::Commit, Some("make test"))).contains("red"));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn commit_on_protected_branch_creates_task_branch() {
        let repo = setup("commit", DONE);
        user_git(&repo, &["branch", "-M", "main"]).unwrap();
        let p = plan(&repo, "T-1", &policy(DeliveryMode::Commit, None)).unwrap();
        assert_eq!(p.branch, "ai/T-1");
        assert!(p.create_branch);
        assert_eq!(p.files, vec!["a.txt"], "the user's earlier work is not delivered");
        assert_eq!(p.pr_title, "T-1: Fix the thing");
        assert!(p.message.contains("It breaks.") && !p.message.contains("More detail"));

        let out = execute(&repo, &p).unwrap();
        assert!(out.commit.is_some() && !out.pushed);
        assert_eq!(current_branch(&repo).as_deref(), Some("ai/T-1"));
        assert_eq!(user_git(&repo, &["log", "-1", "--format=%an <%ae>"]).unwrap(), "Test User <user@example.com>");
        assert_eq!(user_git(&repo, &["show", "--name-only", "--format=", "HEAD"]).unwrap(), "a.txt");
        assert!(user_git(&repo, &["status", "--porcelain"]).unwrap().contains("?? mine.txt"));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn commit_on_feature_branch_stays_there() {
        let repo = setup("feature", DONE);
        user_git(&repo, &["switch", "-q", "-c", "feature/x"]).unwrap();
        let p = plan(&repo, "T-1", &policy(DeliveryMode::Commit, None)).unwrap();
        assert_eq!((p.branch.as_str(), p.create_branch), ("feature/x", false));
        execute(&repo, &p).unwrap();
        assert_eq!(current_branch(&repo).as_deref(), Some("feature/x"));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn a_second_delivery_commits_only_the_new_work() {
        let repo = setup("again", DONE);
        user_git(&repo, &["switch", "-q", "-c", "feature/x"]).unwrap();
        let pol = policy(DeliveryMode::Commit, None);
        execute(&repo, &plan(&repo, "T-1", &pol).unwrap()).unwrap();
        assert!(plan(&repo, "T-1", &pol).unwrap_err().to_string().contains("Nothing to deliver"));

        // The task came back (a decision added work) and the agent changed a.txt again.
        fs::write(repo.join("a.txt"), "fixed\nand more\n").unwrap();
        let p = plan(&repo, "T-1", &pol).unwrap();
        assert_eq!(p.files, ["a.txt"]);
        assert!(p.base.ends_with("/delivered"));
        let second = execute(&repo, &p).unwrap().commit.unwrap();
        let diff = user_git(&repo, &["show", "--format=", &second]).unwrap();
        assert!(diff.contains("+and more") && !diff.contains("+fixed"), "{diff}");
        assert_eq!(fs::read_to_string(repo.join("mine.txt")).unwrap(), "user work\n", "the user's own work stays uncommitted");
        assert!(user_git(&repo, &["status", "--porcelain"]).unwrap().contains("mine.txt"));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn push_and_pr_plans_are_exact_and_never_force() {
        let repo = setup("plans", DONE);
        user_git(&repo, &["branch", "-M", "develop"]).unwrap();
        let mut pol = policy(DeliveryMode::Pr, None);
        pol.delivery.force_push = true;
        let p = plan(&repo, "T-1", &pol).unwrap();
        assert_eq!(
            p.commands,
            vec![
                "create branch ai/T-1 there and switch to it (working tree untouched)",
                "commit the task's changes since refs/kormilant/T-1/1 (1 file) as 'T-1: Fix the thing'",
                "git push -u origin ai/T-1",
                "gh pr create --head ai/T-1 --title 'T-1: Fix the thing' --body-file <PR body>",
            ]
        );
        assert!(p.commands.iter().all(|c| !c.contains("--force")));
        assert!(p.pr_body.contains("### Review 1") && p.pr_body.contains("Changed a.txt."));

        pol.delivery.branch = "release/{slug}".into();
        assert!(plan(&repo, "T-1", &pol).unwrap_err().to_string().contains("protected"));
        let none = plan(&repo, "T-1", &policy(DeliveryMode::None, None)).unwrap();
        assert!(none.commands.is_empty());
        fs::remove_dir_all(&repo).unwrap();
    }

    fn lines(changed: &[(usize, &str)]) -> String {
        (1..=12).map(|i| changed.iter().find(|(n, _)| *n == i).map(|(_, t)| t.to_string()).unwrap_or(format!("line {i}")) + "\n").collect()
    }

    #[test]
    fn same_file_keeps_the_users_earlier_change_out_of_the_commit() {
        let repo = crate::git::tests::temp_repo("samefile");
        user_git(&repo, &["config", "user.name", "Test User"]).unwrap();
        user_git(&repo, &["config", "user.email", "user@example.com"]).unwrap();
        fs::write(repo.join(".git/info/exclude"), ".ai-tasks/\n").unwrap();
        fs::write(repo.join("a.txt"), lines(&[])).unwrap();
        user_git(&repo, &["commit", "-q", "-am", "twelve lines"]).unwrap();
        fs::write(repo.join("a.txt"), lines(&[(1, "user edit")])).unwrap();
        git::checkpoint(&repo, "T-1", 1).unwrap();
        fs::write(repo.join("a.txt"), lines(&[(1, "user edit"), (12, "agent edit")])).unwrap();
        tasks::write(&repo, "T-1", DONE).unwrap();
        user_git(&repo, &["switch", "-q", "-c", "feature/y"]).unwrap();

        let p = plan(&repo, "T-1", &policy(DeliveryMode::Commit, None)).unwrap();
        execute(&repo, &p).unwrap();
        let committed = user_git(&repo, &["show", "HEAD:a.txt"]).unwrap();
        assert!(committed.contains("agent edit") && !committed.contains("user edit"), "only the agent's hunk is committed");
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), lines(&[(1, "user edit"), (12, "agent edit")]), "working tree untouched");
        let diff = user_git(&repo, &["diff", "HEAD", "--", "a.txt"]).unwrap();
        assert!(diff.contains("+user edit") && !diff.contains("agent edit"), "the user's change is still uncommitted");
        assert_eq!(user_git(&repo, &["diff", "--cached", "--name-only"]).unwrap(), "", "index matches HEAD for delivered files");

        // Overlapping edits cannot be separated: refuse instead of guessing.
        fs::write(repo.join("a.txt"), lines(&[(5, "user again")])).unwrap();
        user_git(&repo, &["commit", "-q", "-am", "reset"]).unwrap();
        fs::write(repo.join("a.txt"), lines(&[(5, "user dirty")])).unwrap();
        git::checkpoint(&repo, "T-2", 1).unwrap();
        fs::write(repo.join("a.txt"), lines(&[(5, "agent over it")])).unwrap();
        tasks::write(&repo, "T-2", &DONE.replace("T-1", "T-2")).unwrap();
        let p2 = plan(&repo, "T-2", &policy(DeliveryMode::Commit, None)).unwrap();
        assert!(execute(&repo, &p2).unwrap_err().to_string().contains("overlap"));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn protected_patterns() {
        let pats = vec!["main".to_string(), "release/*".to_string()];
        assert!(is_protected("main", &pats));
        assert!(is_protected("release/1.2", &pats));
        assert!(!is_protected("mainline", &pats));
        assert!(!is_protected("ai/T-1", &pats));
    }
}
