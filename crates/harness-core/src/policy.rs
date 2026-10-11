//! Guardrail policy, loaded from `<repo>/.ai-tasks/policy.toml` over built-in defaults.
//! The same `evaluate` runs in `harness-guard` (every tool call) and in the host handler (asks),
//! so both layers always agree.

use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow(String),
    Deny(String),
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Analyst,
    Implementer,
    Reviewer,
}

impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "analyst" => Ok(Self::Analyst),
            "implementer" => Ok(Self::Implementer),
            "reviewer" => Ok(Self::Reviewer),
            other => anyhow::bail!("unknown role {other:?}"),
        }
    }
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Analyst => "analyst",
            Self::Implementer => "implementer",
            Self::Reviewer => "reviewer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnQuota {
    Ask,
    Reassign,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditScope {
    Task,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOutOfScope {
    Warn,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnBreach {
    Warn,
    Pause,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    None,
    Commit,
    Push,
    Pr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Ask,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Roles {
    pub analyst: String,
    pub implementer: String,
    pub reviewer: String,
    pub on_quota_exhausted: OnQuota,
}

impl Roles {
    /// The worker configured for a role (validated when the policy loads).
    pub fn worker(&self, role: Role) -> Worker {
        let spec = match role {
            Role::Analyst => &self.analyst,
            Role::Implementer => &self.implementer,
            Role::Reviewer => &self.reviewer,
        };
        Worker::parse(spec).unwrap_or_default()
    }
}

/// Which agent CLI fills a role: `claude`, `claude:<model>`, `copilot` or `copilot:<model>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Worker {
    pub agent: Agent,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Agent {
    #[default]
    Claude,
    Copilot,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Copilot => "copilot",
        }
    }
}

impl Worker {
    pub fn parse(spec: &str) -> anyhow::Result<Self> {
        let (name, model) = match spec.trim().split_once(':') {
            Some((n, m)) => (n.trim(), Some(m.trim()).filter(|m| !m.is_empty()).map(String::from)),
            None => (spec.trim(), None),
        };
        let agent = match name {
            "claude" => Agent::Claude,
            "copilot" => Agent::Copilot,
            other => anyhow::bail!("unknown worker {other:?}: use \"claude\", \"copilot\", or \"<worker>:<model>\""),
        };
        Ok(Self { agent, model })
    }

    pub fn claude() -> Self {
        Self::default()
    }

    pub fn label(&self) -> String {
        match &self.model {
            Some(m) => format!("{}:{m}", self.agent.as_str()),
            None => self.agent.as_str().to_string(),
        }
    }
}

impl Default for Roles {
    fn default() -> Self {
        Self {
            analyst: "claude".into(),
            implementer: "claude".into(),
            reviewer: "claude".into(),
            on_quota_exhausted: OnQuota::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git {
    pub deny: Vec<String>,
}

impl Default for Git {
    fn default() -> Self {
        let verbs = ["push", "commit", "reset", "stash", "checkout", "rebase", "merge", "switch", "restore", "clean"];
        Self { deny: verbs.map(String::from).to_vec() }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    /// Command prefixes, matched word by word: `git diff` allows `git diff --stat`.
    pub allow: Vec<String>,
    /// Pipes, chains, redirects and subshells are refused with a reason, so the agent retries plainly.
    pub single_command: bool,
    pub unlisted: Access,
    pub ask_timeout_min: u64,
}

impl Default for Shell {
    fn default() -> Self {
        let allow = [
            "ls", "pwd", "wc", "head", "tail", "cat", "rg", "grep", "tree", "file", "git status", "git log", "git diff",
            "git show", "git blame", "git ls-files", "git grep", "git rev-parse", "git branch --show-current",
        ];
        Self { allow: allow.map(String::from).to_vec(), single_command: true, unlisted: Access::Ask, ask_timeout_min: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Read {
    pub outside_repo: Access,
}

impl Default for Read {
    fn default() -> Self {
        Self { outside_repo: Access::Ask }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Edits {
    pub scope: EditScope,
    pub on_out_of_scope: OnOutOfScope,
}

impl Default for Edits {
    fn default() -> Self {
        Self { scope: EditScope::Task, on_out_of_scope: OnOutOfScope::Warn }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub access: Access,
}

impl Default for Web {
    fn default() -> Self {
        Self { access: Access::Ask }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    /// New tokens (input + cache writes + output) across all runs of a task; cache reads are not counted.
    pub tokens_per_task: Option<u64>,
    /// API-equivalent dollars across all runs of a task; also passed to the CLI as a hard cap per run.
    pub usd_per_task: Option<f64>,
    pub minutes_per_run: Option<u64>,
    pub idle_minutes: Option<u64>,
    pub on_breach: OnBreach,
}

impl Default for Budget {
    fn default() -> Self {
        Self { tokens_per_task: None, usd_per_task: None, minutes_per_run: None, idle_minutes: None, on_breach: OnBreach::Pause }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    pub command: Option<String>,
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunLimits {
    pub max_rounds: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_rounds: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Delivery {
    pub mode: DeliveryMode,
    pub approval: Approval,
    pub branch: String,
    pub protected: Vec<String>,
    pub force_push: bool,
}

impl Default for Delivery {
    fn default() -> Self {
        Self {
            mode: DeliveryMode::None,
            approval: Approval::Ask,
            branch: "ai/{slug}".into(),
            protected: ["main", "master", "develop", "release/*"].map(String::from).to_vec(),
            force_push: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub roles: Roles,
    pub git: Git,
    pub shell: Shell,
    pub read: Read,
    pub edits: Edits,
    pub web: Web,
    pub budget: Budget,
    pub build: Build,
    pub run: RunLimits,
    pub delivery: Delivery,
}

pub fn policy_path(repo: &Path) -> PathBuf {
    repo.join(".ai-tasks").join("policy.toml")
}

/// Commented template; parses to `Policy::default()` (tested).
pub fn default_toml() -> &'static str {
    r#"# Harness policy for this repository. Missing keys use these defaults; a list replaces the default list.

[roles]                       # factory of one: the same worker may fill every role
analyst = "claude"
implementer = "claude"        # "claude" | "copilot", optionally with a model: "copilot:claude-sonnet-5"
reviewer = "claude"
on_quota_exhausted = "ask"    # when a worker runs out of quota: "ask" | "reassign" (to claude) | "stop"

[git]                         # applies to agents; delivery is done by the app
deny = ["push", "commit", "reset", "stash", "checkout", "rebase", "merge", "switch", "restore", "clean"]

[shell]
# Command prefixes matched word by word ("git diff" allows "git diff --stat"). Add build tools here, e.g. "mvn", "./mvnw", "npm test".
allow = ["ls", "pwd", "wc", "head", "tail", "cat", "rg", "grep", "tree", "file",
         "git status", "git log", "git diff", "git show", "git blame", "git ls-files", "git grep", "git rev-parse",
         "git branch --show-current"]
single_command = true         # refuse pipes, &&, ;, redirects and subshells with a reason
unlisted = "ask"              # "ask" | "deny" | "allow"
ask_timeout_min = 10          # unanswered asks are denied after this

[read]
outside_repo = "ask"          # Read/Grep/Glob outside the repository: "ask" | "deny" | "allow"

[edits]                       # implementer only (analyst and reviewer write only .ai-tasks/*.md)
scope = "task"                # "task" | "repo"
on_out_of_scope = "warn"      # "warn" | "block"

[web]
access = "ask"                # WebFetch / WebSearch: "ask" | "deny" | "allow"

[budget]                      # watchdog: 80% warns, 100% applies on_breach
# tokens_per_task = 2_000_000 # new tokens across all runs of a task (cache reads not counted)
# usd_per_task = 5.0          # API-equivalent dollars across all runs of a task; hard cap per run
# minutes_per_run = 45        # wall clock of one Analyze/Refine run or one whole loop
# idle_minutes = 10           # no agent activity for this long
on_breach = "pause"           # "warn" | "pause" | "stop"

[build]                       # used by the run loop from phase 1c
# command = "mvn -B verify"
requires = []                 # e.g. ["docker"]

[run]
max_rounds = 3

[delivery]                    # runs only after the Done gate passes (phase 1f)
mode = "none"                 # "none" | "commit" | "push" | "pr"
approval = "ask"              # "ask" | "auto"
branch = "ai/{slug}"
protected = ["main", "master", "develop", "release/*"]
force_push = false
"#
}

impl Policy {
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let policy: Self = toml::from_str(text).context("invalid policy.toml")?;
        for (role, spec) in [("analyst", &policy.roles.analyst), ("implementer", &policy.roles.implementer), ("reviewer", &policy.roles.reviewer)] {
            Worker::parse(spec).with_context(|| format!("invalid policy.toml: roles.{role}"))?;
        }
        Ok(policy)
    }

    /// Defaults overridden by the repo's `.ai-tasks/policy.toml`, if present.
    pub fn load(repo: &Path) -> anyhow::Result<Self> {
        let path = policy_path(repo);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text).with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn evaluate(&self, role: Role, tool: &str, input: &Value, repo: &Path) -> Verdict {
        let field = |k: &str| input.get(k).and_then(Value::as_str);
        match tool {
            "Read" | "NotebookRead" | "Grep" | "Glob" | "LS" => {
                let path = field("file_path").or_else(|| field("notebook_path")).or_else(|| field("path"));
                match path {
                    Some(p) if !is_inside(&resolve(repo, p), repo) => access(self.read.outside_repo, "reading outside the repository"),
                    _ => Verdict::Allow("read inside the repository".into()),
                }
            }
            "TodoWrite" => Verdict::Allow("planning tool".into()),
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                let Some(p) = field("file_path").or_else(|| field("notebook_path")) else {
                    return Verdict::Deny("write without a file path".into());
                };
                write_verdict(role, &resolve(repo, p), repo)
            }
            "Bash" => self.bash(field("command").unwrap_or_default(), repo),
            "WebFetch" | "WebSearch" => access(self.web.access, "web access"),
            _ => Verdict::Ask,
        }
    }

    fn bash(&self, command: &str, repo: &Path) -> Verdict {
        let words: Vec<&str> =
            command.split(|c: char| c.is_whitespace() || ";|&()`".contains(c)).filter(|w| !w.is_empty()).collect();
        if let Some(verb) = git_verbs(&words).into_iter().find(|v| self.git.deny.iter().any(|d| d == v)) {
            return Verdict::Deny(format!("git {verb} is not allowed for agents; Harness handles delivery."));
        }
        if SHELL_OPERATORS.iter().any(|op| command.contains(op)) {
            return if self.shell.single_command {
                Verdict::Deny(
                    "Run one plain command per call: no pipes, &&, ;, redirects or subshells. Retry without them.".into(),
                )
            } else {
                access(self.shell.unlisted, "this compound command")
            };
        }
        let mut plain: Vec<&str> = command.split_whitespace().collect();
        if plain.first() == Some(&"git") {
            // `git -C <dir> diff` is `git diff` run elsewhere: match on the subcommand, and treat
            // a directory outside the repo as a read outside the repo.
            let mut i = 1;
            while i < plain.len() && plain[i].starts_with('-') {
                if plain[i] == "-C" {
                    match plain.get(i + 1) {
                        Some(dir) if !is_inside_or_same(&resolve(repo, dir), repo) => {
                            return access(self.read.outside_repo, "git in a directory outside the repository");
                        }
                        _ => {}
                    }
                }
                i += if matches!(plain[i], "-C" | "-c") { 2 } else { 1 };
            }
            plain.drain(1..i.min(plain.len()));
        }
        let listed = self.shell.allow.iter().find(|entry| {
            let prefix: Vec<&str> = entry.split_whitespace().collect();
            !prefix.is_empty() && plain.starts_with(&prefix)
        });
        match listed {
            Some(entry) => Verdict::Allow(format!("shell.allow: {entry}")),
            None => access(self.shell.unlisted, "this command"),
        }
    }
}

const SHELL_OPERATORS: &[&str] = &["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"];

fn access(a: Access, what: &str) -> Verdict {
    match a {
        Access::Allow => Verdict::Allow(format!("policy allows {what}")),
        Access::Ask => Verdict::Ask,
        Access::Deny => Verdict::Deny(format!("Policy does not allow {what}.")),
    }
}

fn write_verdict(role: Role, path: &Path, repo: &Path) -> Verdict {
    let tasks = repo.join(".ai-tasks");
    if path == policy_path(repo) {
        return Verdict::Deny("Agents may not change the Harness policy.".into());
    }
    if is_inside(path, &tasks.join("runs")) {
        return Verdict::Deny("Run journals are written by Harness only.".into());
    }
    match role {
        Role::Analyst | Role::Reviewer => {
            let is_task_doc = path.parent() == Some(tasks.as_path()) && path.extension().is_some_and(|e| e == "md");
            if is_task_doc {
                Verdict::Allow("task doc in .ai-tasks/".into())
            } else {
                Verdict::Deny(format!(
                    "The {} may write only task docs (.ai-tasks/<slug>.md), not source files.",
                    role.as_str()
                ))
            }
        }
        Role::Implementer => {
            if !is_inside(path, repo) {
                Verdict::Deny("Writes outside the repository are not allowed.".into())
            } else if is_inside(path, &repo.join(".git")) {
                Verdict::Deny("Agents may not write inside .git/.".into())
            } else {
                Verdict::Allow("write inside the repository".into())
            }
        }
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
    // Lexical normalization: `..` must not escape the checks.
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

fn is_inside_or_same(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir)
}

fn is_inside(path: &Path, dir: &Path) -> bool {
    path.starts_with(dir) && path != dir
}

/// PreToolUse hook answer for `harness-guard`. Never fails open: any error asks the human.
pub fn hook_response(repo: &Path, role: Role, hook_input: &str) -> String {
    let (decision, reason) = match decide_hook(repo, role, hook_input) {
        Ok(Verdict::Allow(rule)) => ("allow", rule),
        Ok(Verdict::Deny(reason)) => ("deny", reason),
        Ok(Verdict::Ask) => ("ask", "Harness asks the human supervisor.".to_string()),
        Err(e) => ("ask", format!("harness-guard could not decide ({e:#}); asking the human.")),
    };
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

fn decide_hook(repo: &Path, role: Role, hook_input: &str) -> anyhow::Result<Verdict> {
    let v: Value = serde_json::from_str(hook_input).context("hook input is not JSON")?;
    let tool = v.get("tool_name").and_then(Value::as_str).context("hook input has no tool_name")?;
    let input = v.get("tool_input").cloned().unwrap_or(Value::Null);
    Ok(Policy::load(repo)?.evaluate(role, tool, &input, repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo() -> PathBuf {
        PathBuf::from("/work/repo")
    }

    fn eval(p: &Policy, role: Role, tool: &str, input: Value) -> Verdict {
        p.evaluate(role, tool, &input, &repo())
    }

    fn bash(p: &Policy, cmd: &str) -> Verdict {
        eval(p, Role::Analyst, "Bash", json!({ "command": cmd }))
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(Policy::from_toml(default_toml()).unwrap(), Policy::default());
        assert_eq!(Policy::from_toml("").unwrap(), Policy::default());
    }

    #[test]
    fn toml_overrides_and_validation() {
        let p = Policy::from_toml("[shell]\nallow = [\"mvn\"]\nunlisted = \"deny\"\n[run]\nmax_rounds = 5").unwrap();
        assert_eq!(p.shell.allow, vec!["mvn"]);
        assert_eq!(p.shell.unlisted, Access::Deny);
        assert!(p.shell.single_command, "unset keys keep defaults");
        assert_eq!(p.run.max_rounds, 5);
        assert!(Policy::from_toml("[shell]\nalow = []").is_err(), "typo must be rejected");
        assert!(Policy::from_toml("[delivery]\nmode = \"yolo\"").is_err());
    }

    #[test]
    fn analyst_and_reviewer_write_only_task_docs() {
        let p = Policy::default();
        let w = |role, path: &str| eval(&p, role, "Write", json!({ "file_path": path }));
        assert!(matches!(w(Role::Analyst, ".ai-tasks/T-1.md"), Verdict::Allow(_)));
        assert!(matches!(w(Role::Reviewer, "/work/repo/.ai-tasks/T-1.md"), Verdict::Allow(_)));
        assert!(matches!(w(Role::Analyst, "src/main.rs"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Analyst, ".ai-tasks/../src/x.md"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Analyst, ".ai-tasks/runs/T-1/x.md"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Analyst, ".ai-tasks/notes/x.md"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Analyst, ".ai-tasks/policy.toml"), Verdict::Deny(_)));
        assert!(matches!(eval(&p, Role::Analyst, "Edit", json!({})), Verdict::Deny(_)));
    }

    #[test]
    fn worker_specs() {
        assert_eq!(Worker::parse("claude").unwrap(), Worker { agent: Agent::Claude, model: None });
        assert_eq!(
            Worker::parse(" copilot:claude-sonnet-5 ").unwrap(),
            Worker { agent: Agent::Copilot, model: Some("claude-sonnet-5".into()) }
        );
        assert_eq!(Worker::parse("copilot:").unwrap().model, None);
        assert!(Worker::parse("codex").is_err());
        let err = Policy::from_toml("[roles]\nimplementer = \"gpt\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("roles.implementer"));
        let p = Policy::from_toml("[roles]\nimplementer = \"copilot\"\n").unwrap();
        assert_eq!(p.roles.worker(Role::Implementer).agent, Agent::Copilot);
        assert_eq!(p.roles.worker(Role::Reviewer).agent, Agent::Claude);
    }

    #[test]
    fn implementer_writes_inside_repo_but_not_git_runs_or_policy() {
        let p = Policy::default();
        let w = |path: &str| eval(&p, Role::Implementer, "Edit", json!({ "file_path": path }));
        assert!(matches!(w("src/main.rs"), Verdict::Allow(_)));
        assert!(matches!(w(".ai-tasks/T-1.md"), Verdict::Allow(_)));
        assert!(matches!(w(".git/config"), Verdict::Deny(_)));
        assert!(matches!(w(".ai-tasks/policy.toml"), Verdict::Deny(_)));
        assert!(matches!(w(".ai-tasks/runs/x"), Verdict::Deny(_)));
        assert!(matches!(w("/etc/hosts"), Verdict::Deny(_)));
    }

    #[test]
    fn reads_outside_the_repo_follow_policy() {
        let p = Policy::default();
        let r = |input| eval(&p, Role::Analyst, "Read", input);
        assert!(matches!(r(json!({ "file_path": "/work/repo/src/a.rs" })), Verdict::Allow(_)));
        assert_eq!(r(json!({ "file_path": "/Users/me/.ssh/id_rsa" })), Verdict::Ask);
        assert_eq!(r(json!({ "file_path": "/work/repo/../other/x" })), Verdict::Ask);
        assert!(matches!(eval(&p, Role::Analyst, "Grep", json!({ "pattern": "x" })), Verdict::Allow(_)));
        let strict = Policy::from_toml("[read]\noutside_repo = \"deny\"").unwrap();
        assert!(matches!(eval(&strict, Role::Analyst, "Glob", json!({ "pattern": "*", "path": "/tmp" })), Verdict::Deny(_)));
    }

    #[test]
    fn shell_rules() {
        let p = Policy::default();
        assert!(matches!(bash(&p, "git push origin main"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "git -C other stash"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "ls && git commit -m x"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "git log --oneline -5"), Verdict::Allow(_)));
        assert!(matches!(bash(&p, "git -C /work/repo diff -- src/a.rs"), Verdict::Allow(_)));
        assert!(matches!(bash(&p, "git -C . --no-pager log"), Verdict::Allow(_)));
        assert_eq!(bash(&p, "git -C /other/repo diff"), Verdict::Ask, "outside the repo follows read.outside_repo");
        assert!(matches!(bash(&p, "git -C /work/repo push"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "ls -la src"), Verdict::Allow(_)));
        assert_eq!(bash(&p, "git branch -D main"), Verdict::Ask, "only --show-current is listed");
        assert_eq!(bash(&p, "mvn test"), Verdict::Ask);
        assert_eq!(bash(&p, "rm -rf target"), Verdict::Ask);
        assert!(matches!(bash(&p, "git log | head"), Verdict::Deny(r) if r.contains("one plain command")));
        let loose = Policy::from_toml("[shell]\nsingle_command = false\nallow = [\"mvn\"]").unwrap();
        assert_eq!(bash(&loose, "git log | head"), Verdict::Ask);
        assert!(matches!(bash(&loose, "mvn -B verify"), Verdict::Allow(_)));
        assert!(matches!(bash(&loose, "mvnw test"), Verdict::Ask), "prefix is word-wise, not character-wise");
    }

    #[test]
    fn web_and_unknown_tools() {
        let p = Policy::default();
        assert_eq!(eval(&p, Role::Analyst, "WebFetch", json!({ "url": "https://x" })), Verdict::Ask);
        assert_eq!(eval(&p, Role::Analyst, "Task", json!({})), Verdict::Ask);
        assert!(matches!(eval(&p, Role::Analyst, "TodoWrite", json!({})), Verdict::Allow(_)));
    }

    #[test]
    fn hook_response_shapes() {
        let decision = |input: &str| -> String {
            let out: Value = serde_json::from_str(&hook_response(Path::new("/nonexistent/repo"), Role::Analyst, input)).unwrap();
            out["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string()
        };
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#), "allow");
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"git push"}}"#), "deny");
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"mvn test"}}"#), "ask");
        assert_eq!(decision("not json"), "ask");
        assert_eq!(decision(r#"{"tool_input":{}}"#), "ask");
    }
}
