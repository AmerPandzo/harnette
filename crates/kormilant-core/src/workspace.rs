//! A task's member repositories: related repos (the doc's `**Repos:**` line) that are git repositories.
//! The implementer may change them; the loop checkpoints, builds, reviews and delivers them next to the primary repo.

use std::path::{Path, PathBuf};

use crate::{git, tasks};

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// Folder name, unique within the task: the prefix of its paths in diffs and file lists.
    pub name: String,
    pub path: PathBuf,
}

/// Members of `slug` (none when the doc has no `**Repos:**` line or cannot be read).
pub fn members(repo: &Path, slug: &str) -> Vec<Member> {
    let named = tasks::read(repo, slug).map(|md| tasks::related(&md)).unwrap_or_default();
    name_members(tasks::related_dirs(repo, &named).into_iter().filter(|d| git::is_repo(d)).collect())
}

fn name_members(dirs: Vec<PathBuf>) -> Vec<Member> {
    let mut out: Vec<Member> = Vec::new();
    for path in dirs {
        let base = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
        let mut name = base.clone();
        let mut n = 2;
        while out.iter().any(|m| m.name == name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        out.push(Member { name, path });
    }
    out
}

/// Where a member's changes start: its first checkpoint of the task, else HEAD.
pub fn base(member: &Member, slug: &str) -> anyhow::Result<Option<String>> {
    Ok(match git::list_checkpoints(&member.path, slug)?.first() {
        Some(c) => Some(c.git_ref.clone()),
        None => git::head(&member.path)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_folder_names() {
        let m = name_members(vec!["/a/lib".into(), "/b/lib".into(), "/c/api".into()]);
        let names: Vec<&str> = m.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["lib", "lib-2", "api"]);
    }

    #[test]
    fn members_are_the_git_repos_among_related_dirs() {
        let primary = git::tests::temp_repo("ws-primary");
        let member = git::tests::temp_repo("ws-member");
        let plain = std::env::temp_dir().join(format!("kormilant-ws-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        let doc = format!("# T\n\n**Status:** Ready for implementation\n\n**Repos:** {}, {}\n", member.display(), plain.display());
        tasks::write(&primary, "T", &doc).unwrap();
        let m = members(&primary, "T");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, member.canonicalize().unwrap());
    }
}
