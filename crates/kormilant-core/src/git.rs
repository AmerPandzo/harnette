//! Git plumbing for checkpoints and diffs. Every snapshot goes through a temporary index
//! (`GIT_INDEX_FILE`), so the user's staging area and branches are never touched.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, bail};
use serde::Serialize;

fn git_env<I, S>(repo: &Path, args: I, index: Option<&Path>) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    // Snapshot commits must work even when the user has no git identity configured.
    cmd.env("GIT_AUTHOR_NAME", "Kormilant")
        .env("GIT_AUTHOR_EMAIL", "kormilant@localhost")
        .env("GIT_COMMITTER_NAME", "Kormilant")
        .env("GIT_COMMITTER_EMAIL", "kormilant@localhost");
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let out = cmd.output().context("could not run git")?;
    if !out.status.success() {
        bail!("git failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn git<I, S>(repo: &Path, args: I) -> anyhow::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_env(repo, args, None)
}

/// A scratch index file, removed on drop.
struct TempIndex(PathBuf);

impl TempIndex {
    /// Starts as a copy of the real index so `git add -A` can reuse its stat cache.
    fn new(repo: &Path, copy_real: bool) -> anyhow::Result<Self> {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kormilant-index-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        if copy_real {
            let real = PathBuf::from(git(repo, ["rev-parse", "--path-format=absolute", "--git-path", "index"])?);
            if real.is_file() {
                std::fs::copy(&real, &path)?;
            }
        }
        Ok(Self(path))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn is_repo(repo: &Path) -> bool {
    git(repo, ["rev-parse", "--is-inside-work-tree"]).is_ok_and(|s| s == "true")
}

/// Current HEAD commit, or None in a repo without commits.
pub fn head(repo: &Path) -> anyhow::Result<Option<String>> {
    match git(repo, ["rev-parse", "--verify", "-q", "HEAD"]) {
        Ok(h) => Ok(Some(h)),
        Err(_) if is_repo(repo) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A commit object holding the working tree as it is now (tracked + untracked, ignores honoured).
/// It is not on any branch; callers keep it alive with a ref when needed.
pub fn snapshot(repo: &Path) -> anyhow::Result<String> {
    let index = TempIndex::new(repo, true)?;
    git_env(repo, ["add", "-A"], Some(&index.0))?;
    let tree = git_env(repo, ["write-tree"], Some(&index.0))?;
    let mut args = vec!["commit-tree".to_string(), tree, "-m".into(), "kormilant snapshot".into()];
    if let Some(h) = head(repo)? {
        args.extend(["-p".into(), h]);
    }
    git(repo, &args)
}

pub fn checkpoint_ref(slug: &str, round: u32) -> String {
    format!("refs/kormilant/{slug}/{round}")
}

/// Snapshot the working tree under `refs/kormilant/<slug>/<round>`.
pub fn checkpoint(repo: &Path, slug: &str, round: u32) -> anyhow::Result<String> {
    let commit = snapshot(repo)?;
    let r = checkpoint_ref(slug, round);
    git(repo, ["update-ref", &r, &commit])?;
    Ok(r)
}

/// Checkpoints made before the rename lived under `refs/harness/`; move them to `refs/kormilant/`.
/// Runs when a repo is opened. A ref that already exists under the new name is left as it is.
pub fn adopt_legacy_refs(repo: &Path) -> anyhow::Result<usize> {
    let out = git(repo, ["for-each-ref", "--format=%(refname) %(objectname)", "refs/harness/"])?;
    let mut moved = 0;
    for line in out.lines() {
        let Some((old, commit)) = line.split_once(' ') else { continue };
        let new = format!("refs/kormilant/{}", &old["refs/harness/".len()..]);
        if resolve(repo, &new).is_none() {
            git(repo, ["update-ref", &new, commit])?;
            moved += 1;
        }
        git(repo, ["update-ref", "-d", old, commit])?;
    }
    Ok(moved)
}

pub fn delivered_ref(slug: &str) -> String {
    format!("refs/kormilant/{slug}/delivered")
}

/// Remember the working tree right after a delivery: the next delivery of the same task
/// commits only what changed since then.
pub fn mark_delivered(repo: &Path, slug: &str) -> anyhow::Result<()> {
    let commit = snapshot(repo)?;
    git(repo, ["update-ref", &delivered_ref(slug), &commit])?;
    Ok(())
}

pub fn reviewed_ref(slug: &str) -> String {
    format!("refs/kormilant/{slug}/reviewed")
}

/// Remember the working tree the reviewer just saw, for incremental re-review.
pub fn mark_reviewed(repo: &Path, slug: &str) -> anyhow::Result<()> {
    let commit = snapshot(repo)?;
    git(repo, ["update-ref", &reviewed_ref(slug), &commit])?;
    Ok(())
}

/// Resolve a ref to a commit, or None when it does not exist.
pub fn resolve(repo: &Path, r: &str) -> Option<String> {
    git(repo, ["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")]).ok()
}

/// The tree a commit points at (two snapshots of the same files share it).
pub fn tree_of(repo: &Path, commit: &str) -> anyhow::Result<String> {
    git(repo, ["rev-parse", &format!("{commit}^{{tree}}")])
}

/// Commit only what changed between `base` and the working tree (in `files`) on top of HEAD,
/// with the user's own git identity. Work the user had before `base` stays out of the commit.
/// Returns the new commit; no ref, index or file is changed.
pub fn commit_changes_since(repo: &Path, base: &str, files: &[String], message: &str) -> anyhow::Result<String> {
    let head = head(repo)?.context("the repository has no commits yet")?;
    let now = snapshot(repo)?;
    let mut args: Vec<&str> = vec!["diff", "--binary", "--no-color", "--no-renames", base, &now, "--"];
    args.extend(files.iter().map(String::as_str));
    let patch = git(repo, &args)?;
    let index = TempIndex::new(repo, false)?;
    let patch_file = PathBuf::from(format!("{}.patch", index.0.display()));
    std::fs::write(&patch_file, format!("{patch}\n"))?;
    git_env(repo, ["read-tree", &head], Some(&index.0))?;
    // 3-way lets edits next to (not on) the user's own uncommitted lines apply; true overlaps still fail.
    let applied = git_env(repo, ["apply", "--cached", "--binary", "--3way", &patch_file.display().to_string()], Some(&index.0));
    let _ = std::fs::remove_file(&patch_file);
    if let Err(e) = applied {
        let msg = format!("{e:#}");
        // `git apply --3way` lists the files it could not merge as `U <path>`.
        let conflicts: Vec<&str> = msg.lines().filter_map(|l| l.trim().strip_prefix("U ")).collect();
        let which = if conflicts.is_empty() { files.join(", ") } else { conflicts.join(", ") };
        bail!(
            "The task's changes overlap uncommitted changes you made before the task in {which}. Commit or stash your own changes in that file first, then deliver."
        );
    }
    let tree = git_env(repo, ["write-tree"], Some(&index.0))?;
    // No Kormilant identity here: the commit is the user's.
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit-tree", &tree, "-p", &head, "-m", message])
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("git commit-tree failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Where a branch forked from HEAD, like a pull request's base.
pub fn merge_base(repo: &Path, branch: &str) -> anyhow::Result<String> {
    git(repo, ["merge-base", "HEAD", branch])
}

/// Files that differ between `base` and the working tree now (new and deleted files included).
pub fn changed_since(repo: &Path, base: &str) -> anyhow::Result<Vec<String>> {
    let now = snapshot(repo)?;
    let out = git(repo, ["diff", "--name-only", base, &now])?;
    Ok(out.lines().filter(|l| !l.is_empty()).map(String::from).collect())
}

/// `git diff --numstat` per file: (path, added, removed); binary files count as 0/0.
pub fn numstat(repo: &Path, base: &str) -> anyhow::Result<Vec<(String, u32, u32)>> {
    let now = snapshot(repo)?;
    let out = git(repo, ["diff", "--numstat", "--find-renames", base, &now])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let added = p.next()?.parse().unwrap_or(0);
            let removed = p.next()?.parse().unwrap_or(0);
            Some((p.next()?.to_string(), added, removed))
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Checkpoint {
    pub round: u32,
    pub git_ref: String,
    pub commit: String,
    pub created_unix: u64,
}

pub fn list_checkpoints(repo: &Path, slug: &str) -> anyhow::Result<Vec<Checkpoint>> {
    let out = git(
        repo,
        ["for-each-ref", "--format=%(refname) %(objectname) %(committerdate:unix)", &format!("refs/kormilant/{slug}/")],
    )?;
    let mut cps: Vec<Checkpoint> = out
        .lines()
        .filter_map(|l| {
            let mut parts = l.split(' ');
            let git_ref = parts.next()?.to_string();
            let commit = parts.next()?.to_string();
            let created_unix = parts.next()?.parse().ok()?;
            let round = git_ref.rsplit('/').next()?.parse().ok()?;
            Some(Checkpoint { round, git_ref, commit, created_unix })
        })
        .collect();
    cps.sort_by_key(|c| c.round);
    Ok(cps)
}

/// Patch from `base` (a ref or commit) to the current working tree, new files included.
pub fn diff(repo: &Path, base: &str) -> anyhow::Result<String> {
    let now = snapshot(repo)?;
    git(repo, ["diff", "--no-color", "--find-renames", base, &now])
}

/// Put the working tree back to a checkpoint: restore its files and delete files added since.
/// Ignored files are left alone; the user's index is not touched.
pub fn restore(repo: &Path, git_ref: &str) -> anyhow::Result<()> {
    let now = snapshot(repo)?;
    let added = git(repo, ["diff", "--name-only", "--no-renames", "--diff-filter=A", git_ref, &now])?;
    for file in added.lines().filter(|l| !l.is_empty()) {
        let path = repo.join(file);
        if path.starts_with(repo) {
            let _ = std::fs::remove_file(path);
        }
    }
    let index = TempIndex::new(repo, false)?;
    git_env(repo, ["read-tree", git_ref], Some(&index.0))?;
    git_env(repo, ["checkout-index", "-a", "-f"], Some(&index.0))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub(crate) fn temp_repo(name: &str) -> PathBuf {
        let repo = std::env::temp_dir().join(format!("kormilant-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&repo);
        fs::create_dir_all(&repo).unwrap();
        git(&repo, ["init", "-q"]).unwrap();
        fs::write(repo.join("a.txt"), "one\n").unwrap();
        fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        git(&repo, ["add", "-A"]).unwrap();
        git(&repo, ["commit", "-q", "-m", "init"]).unwrap();
        repo
    }

    #[test]
    fn checkpoint_diff_and_restore() {
        let repo = temp_repo("cp");
        fs::write(repo.join("a.txt"), "two\n").unwrap();
        fs::write(repo.join("b.txt"), "new\n").unwrap();
        fs::write(repo.join("ignored.txt"), "keep\n").unwrap();
        let head_before = head(&repo).unwrap();

        let r = checkpoint(&repo, "T-1", 1).unwrap();
        assert_eq!(r, "refs/kormilant/T-1/1");
        // The user's index and HEAD are untouched: b.txt is still untracked, nothing staged.
        assert_eq!(git(&repo, ["diff", "--cached", "--name-only"]).unwrap(), "");
        assert!(git(&repo, ["status", "--porcelain"]).unwrap().contains("?? b.txt"));
        assert_eq!(head(&repo).unwrap(), head_before);

        let cps = list_checkpoints(&repo, "T-1").unwrap();
        assert_eq!(cps.len(), 1);
        assert_eq!(cps[0].round, 1);

        fs::write(repo.join("a.txt"), "three\n").unwrap();
        fs::write(repo.join("c.txt"), "later\n").unwrap();
        fs::remove_file(repo.join("b.txt")).unwrap();
        let patch = diff(&repo, &r).unwrap();
        assert!(patch.contains("+three"));
        assert!(patch.contains("c.txt"));
        assert!(patch.contains("b.txt"));
        assert!(!patch.contains("ignored.txt"));

        mark_reviewed(&repo, "T-1").unwrap();
        assert_eq!(list_checkpoints(&repo, "T-1").unwrap().len(), 1, "the reviewed ref is not a checkpoint");
        fs::write(repo.join("d.txt"), "after review\n").unwrap();
        assert_eq!(changed_since(&repo, &reviewed_ref("T-1")).unwrap(), vec!["d.txt"]);
        let stats = numstat(&repo, &r).unwrap();
        assert!(stats.iter().any(|(p, a, _)| p == "c.txt" && *a == 1));
        assert!(resolve(&repo, &reviewed_ref("T-1")).is_some());
        assert!(resolve(&repo, "refs/kormilant/nope").is_none());

        let legacy = resolve(&repo, &r).unwrap();
        git(&repo, ["update-ref", "refs/harness/T-2/1", &legacy]).unwrap();
        git(&repo, ["update-ref", "refs/harness/T-1/1", &legacy]).unwrap();
        assert_eq!(adopt_legacy_refs(&repo).unwrap(), 1, "T-1/1 already exists under the new name");
        assert_eq!(list_checkpoints(&repo, "T-2").unwrap().len(), 1);
        assert_eq!(git(&repo, ["for-each-ref", "refs/harness/"]).unwrap(), "");
        fs::remove_file(repo.join("d.txt")).unwrap();

        restore(&repo, &r).unwrap();
        assert_eq!(fs::read_to_string(repo.join("a.txt")).unwrap(), "two\n");
        assert_eq!(fs::read_to_string(repo.join("b.txt")).unwrap(), "new\n");
        assert!(!repo.join("c.txt").exists(), "files added after the checkpoint are removed");
        assert_eq!(fs::read_to_string(repo.join("ignored.txt")).unwrap(), "keep\n");
        assert_eq!(git(&repo, ["diff", "--cached", "--name-only"]).unwrap(), "");
        fs::remove_dir_all(&repo).unwrap();
    }
}
