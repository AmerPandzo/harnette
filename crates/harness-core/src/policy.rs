//! Spike-sized guardrail policy for the Analyze role.
//! Later phases load this from `.ai-tasks/policy.toml`; the decision shape stays the same.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow(String),
    Deny(String),
    Ask,
}

/// Tools the CLI may use without asking us (passed as `--allowedTools`).
pub const PRE_ALLOWED_TOOLS: &[&str] = &["Read", "Grep", "Glob"];

const READ_ONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob", "TodoWrite"];
const WRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
const DENIED_GIT: &[&str] = &["push", "commit", "reset", "stash", "checkout", "rebase", "merge", "switch", "restore", "clean"];
const READ_ONLY_GIT: &[&str] = &["status", "log", "diff", "show", "blame", "ls-files", "grep", "branch", "rev-parse"];
const READ_ONLY_COMMANDS: &[&str] = &["ls", "pwd", "wc", "head", "tail", "cat", "rg", "grep", "tree", "file"];
/// Anything that chains, pipes, redirects or substitutes is not a single plain command.
const SHELL_OPERATORS: &[&str] = &["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"];

pub fn evaluate(tool: &str, input: &Value, repo: &Path) -> Verdict {
    if READ_ONLY_TOOLS.contains(&tool) {
        return Verdict::Allow("read-only tool".into());
    }
    if WRITE_TOOLS.contains(&tool) {
        let path = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str);
        return match path {
            Some(p) if is_inside(&resolve(repo, p), &repo.join(".ai-tasks")) => {
                Verdict::Allow("write inside .ai-tasks/".into())
            }
            _ => Verdict::Deny("Analysis only: you may write only the task doc under .ai-tasks/.".into()),
        };
    }
    if tool == "Bash" {
        let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
        return evaluate_bash(command);
    }
    Verdict::Ask
}

fn evaluate_bash(command: &str) -> Verdict {
    let words: Vec<&str> = command.split(|c: char| c.is_whitespace() || ";|&()`".contains(c)).filter(|w| !w.is_empty()).collect();
    if let Some(verb) = git_verbs(&words).into_iter().find(|v| DENIED_GIT.contains(v)) {
        return Verdict::Deny(format!("git {verb} is not allowed for agents; Harness handles delivery."));
    }
    if SHELL_OPERATORS.iter().any(|op| command.contains(op)) {
        return Verdict::Ask;
    }
    match words.as_slice() {
        ["git", rest @ ..] if git_verbs(&words).first().is_some_and(|v| READ_ONLY_GIT.contains(v)) && !rest.is_empty() => {
            Verdict::Allow("read-only git".into())
        }
        [cmd, ..] if READ_ONLY_COMMANDS.contains(cmd) => Verdict::Allow("read-only command".into()),
        _ => Verdict::Ask,
    }
}

/// The subcommand after each `git` word, skipping global flags such as `-C <dir>` or `-c k=v`.
fn git_verbs<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut verbs = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if words[i] == "git" {
            let mut j = i + 1;
            while j < words.len() && words[j].starts_with('-') {
                j += if matches!(words[j], "-C" | "-c") { 2 } else { 1 };
            }
            if let Some(v) = words.get(j) {
                verbs.push(*v);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    verbs
}

fn resolve(repo: &Path, p: &str) -> PathBuf {
    let joined = if Path::new(p).is_absolute() { PathBuf::from(p) } else { repo.join(p) };
    // Lexical normalization: `..` must not escape the check.
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn is_inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir) && path != dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo() -> PathBuf {
        PathBuf::from("/work/repo")
    }

    fn bash(cmd: &str) -> Verdict {
        evaluate("Bash", &json!({ "command": cmd }), &repo())
    }

    #[test]
    fn writes_only_inside_ai_tasks() {
        let write = |p: &str| evaluate("Write", &json!({ "file_path": p }), &repo());
        assert!(matches!(write("/work/repo/.ai-tasks/x.md"), Verdict::Allow(_)));
        assert!(matches!(write(".ai-tasks/x.md"), Verdict::Allow(_)));
        assert!(matches!(write("/work/repo/src/main.rs"), Verdict::Deny(_)));
        assert!(matches!(write("/work/repo/.ai-tasks/../src/main.rs"), Verdict::Deny(_)));
        assert!(matches!(write("/work/repo/.ai-tasks-evil/x.md"), Verdict::Deny(_)));
        assert!(matches!(evaluate("Edit", &json!({}), &repo()), Verdict::Deny(_)));
    }

    #[test]
    fn denies_dangerous_git_anywhere_in_the_command() {
        assert!(matches!(bash("git push origin main"), Verdict::Deny(_)));
        assert!(matches!(bash("git -C other stash"), Verdict::Deny(_)));
        assert!(matches!(bash("ls && git commit -m x"), Verdict::Deny(_)));
        assert!(matches!(bash("echo hi; git reset --hard"), Verdict::Deny(_)));
    }

    #[test]
    fn allows_plain_read_only_commands() {
        assert!(matches!(bash("git status"), Verdict::Allow(_)));
        assert!(matches!(bash("git log --oneline -5"), Verdict::Allow(_)));
        assert!(matches!(bash("ls -la src"), Verdict::Allow(_)));
    }

    #[test]
    fn asks_for_everything_else() {
        assert_eq!(bash("mvn test"), Verdict::Ask);
        assert_eq!(bash("git log | head"), Verdict::Ask);
        assert_eq!(bash("cat a > b"), Verdict::Ask);
        assert_eq!(bash("rm -rf target"), Verdict::Ask);
        assert_eq!(bash("git"), Verdict::Ask);
        assert_eq!(evaluate("WebFetch", &json!({ "url": "https://x" }), &repo()), Verdict::Ask);
    }
}
