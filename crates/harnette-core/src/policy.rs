//! Guardrail policy, loaded from `<repo>/.ai-tasks/policy.toml` over built-in defaults.
//! The same `evaluate` runs in `harnette-guard` (every tool call) and in the host handler (asks),
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
    /// Free-form chat with the human: reads freely, writes task docs, asks for anything else.
    Chat,
}

impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "analyst" => Ok(Self::Analyst),
            "implementer" => Ok(Self::Implementer),
            "reviewer" => Ok(Self::Reviewer),
            "chat" => Ok(Self::Chat),
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
            Self::Chat => "chat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnQuota {
    Ask,
    Reassign,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditScope {
    Task,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOutOfScope {
    Warn,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnBreach {
    Warn,
    Pause,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    None,
    Commit,
    Push,
    Pr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Ask,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
            Role::Chat => return Worker::claude(),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git {
    pub deny: Vec<String>,
}

/// Git verbs agents can never run, whatever `[git] deny` says: pushing is the app's job, after approval.
pub const ALWAYS_DENIED_GIT: &[&str] = &["push"];

impl Git {
    /// `deny` plus the verbs that cannot be lifted.
    pub fn denied(&self) -> Vec<&str> {
        let mut v: Vec<&str> = ALWAYS_DENIED_GIT.to_vec();
        v.extend(self.deny.iter().map(String::as_str).filter(|d| !ALWAYS_DENIED_GIT.contains(d)));
        v
    }
}

impl Default for Git {
    fn default() -> Self {
        let verbs = ["push", "commit", "reset", "stash", "checkout", "rebase", "merge", "switch", "restore", "clean"];
        Self { deny: verbs.map(String::from).to_vec() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Read {
    pub outside_repo: Access,
}

impl Default for Read {
    fn default() -> Self {
        Self { outside_repo: Access::Ask }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Web {
    pub access: Access,
}

impl Default for Web {
    fn default() -> Self {
        Self { access: Access::Ask }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    pub command: Option<String>,
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunLimits {
    pub max_rounds: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_rounds: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Delivery {
    pub mode: DeliveryMode,
    pub approval: Approval,
    pub branch: String,
    pub protected: Vec<String>,
    pub force_push: bool,
    /// Switch to the task branch when a loop starts (only from a protected or detached branch), so the
    /// work happens on its own branch from the first edit. Off: the branch is made at delivery.
    pub branch_at_start: bool,
}

impl Default for Delivery {
    fn default() -> Self {
        Self {
            mode: DeliveryMode::None,
            approval: Approval::Ask,
            branch: "ai/{slug}".into(),
            protected: ["main", "master", "develop", "release/*"].map(String::from).to_vec(),
            force_push: false,
            branch_at_start: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    pub copilot: CopilotCfg,
    pub plugins: Plugins,
    /// Related repositories of the task being run (its `**Repos:**` line): read like the repo itself.
    /// Set per run, never from the file.
    #[serde(skip)]
    pub extra_read: Vec<PathBuf>,
    /// The task's member repos (related git repos): the implementer may change them. Set per run.
    #[serde(skip)]
    pub extra_write: Vec<PathBuf>,
}

/// Claude Code plugins (and their skills) agents get. Runs skip user settings, so none load unless listed here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Plugins {
    /// `<name>@<marketplace>` ids, as in `~/.claude/plugins/installed_plugins.json`.
    pub enabled: Vec<String>,
}

/// Plugin ids installed for Claude Code on this machine; empty when there is no plugin file.
pub fn installed_plugins() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let text = std::fs::read_to_string(PathBuf::from(home).join(".claude/plugins/installed_plugins.json")).unwrap_or_default();
    plugin_ids(&text)
}

fn plugin_ids(json: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let mut ids: Vec<String> = v.get("plugins").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    ids.sort();
    ids
}

/// How Harnette runs Copilot CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CopilotCfg {
    /// Model for roles that say just "copilot" (Copilot's own default may be a small model).
    pub model: String,
    /// Copilot CLI does not read IntelliJ's global instructions; this file is appended to every
    /// Copilot prompt when it exists. Empty = none.
    pub instructions: String,
    /// Tools hidden from Copilot (sub-agents would bypass the turn-level view Harnette has).
    pub excluded_tools: Vec<String>,
}

impl Default for CopilotCfg {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-5".into(),
            instructions: "~/.config/github-copilot/intellij/global-copilot-instructions.md".into(),
            excluded_tools: ["task", "run_dynamic_workflow", "dynamic_workflows_manage"].map(String::from).to_vec(),
        }
    }
}

pub fn policy_path(repo: &Path) -> PathBuf {
    repo.join(".ai-tasks").join("policy.toml")
}

/// Commented template; parses to `Policy::default()` (tested).
pub fn default_toml() -> &'static str {
    r#"# Harnette policy for this repository. Missing keys use these defaults; a list replaces the default list.

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
scope = "task"                # "task": only files the task doc names (in backticks) | "repo": anywhere
on_out_of_scope = "warn"      # "warn" (allowed, raises an alarm) | "block" (denied with a reason)

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
branch_at_start = false       # true: the app switches to the task branch when a run starts (from a protected branch)

[copilot]                     # when a role is "copilot"
model = "claude-sonnet-5"     # for roles that say just "copilot"
instructions = "~/.config/github-copilot/intellij/global-copilot-instructions.md"  # appended to Copilot prompts if it exists
excluded_tools = ["task", "run_dynamic_workflow", "dynamic_workflows_manage"]   # no sub-agents

[plugins]                     # Claude Code plugins (skills) Claude workers get; your user settings are not read
enabled = []                  # e.g. ["ponytail@ponytail", "mattpocock-skills@claude-plugins-official"]
"#
}

/// What the repository is built and tested with, guessed from its files.
#[derive(Debug, PartialEq, Serialize)]
pub struct Detected {
    pub command: &'static str,
    /// Shell prefix the agents need to run it.
    pub allow: &'static str,
    pub docker: bool,
}

pub fn detect_build(repo: &Path) -> Option<Detected> {
    let has = |f: &str| repo.join(f).exists();
    let (command, allow) = if has("mvnw") {
        ("./mvnw -B verify", "./mvnw")
    } else if has("pom.xml") {
        ("mvn -B verify", "mvn")
    } else if has("gradlew") {
        ("./gradlew build", "./gradlew")
    } else if has("package.json") {
        ("npm test", "npm test")
    } else if has("Cargo.toml") {
        ("cargo test", "cargo test")
    } else if has("go.mod") {
        ("go test ./...", "go test")
    } else {
        return None;
    };
    Some(Detected { command, allow, docker: uses_testcontainers(repo, 2) })
}

/// Testcontainers in a build file at the root or `depth` module levels below.
fn uses_testcontainers(dir: &Path, depth: u32) -> bool {
    let builds = ["pom.xml", "build.gradle", "build.gradle.kts"];
    if builds.iter().any(|b| std::fs::read_to_string(dir.join(b)).is_ok_and(|t| t.contains("testcontainers"))) {
        return true;
    }
    depth > 0
        && std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let name = e.file_name();
            let hidden = name.to_string_lossy().starts_with('.') || name == "node_modules" || name == "target";
            !hidden && e.path().is_dir() && uses_testcontainers(&e.path(), depth - 1)
        })
}

/// The template with the build section filled in from what the repository looks like.
pub fn suggested_toml(repo: &Path) -> String {
    let text = default_toml().to_string();
    let Some(d) = detect_build(repo) else { return text };
    let requires = if d.docker { r#"["docker"]"# } else { "[]" };
    text.replacen(
        "# command = \"mvn -B verify\"\nrequires = []                 # e.g. [\"docker\"]",
        &format!("command = \"{}\"           # detected\nrequires = {requires}", d.command),
        1,
    )
    .replacen("\"git branch --show-current\"]", &format!("\"git branch --show-current\", \"{}\"]", d.allow), 1)
}

/// Write form values (a JSON object shaped like `Policy`) into the policy text. Only keys whose value
/// changed are touched, so comments and layout survive; `null` removes a key (back to its default).
pub fn apply_values(text: &str, values: &Value) -> anyhow::Result<String> {
    use toml_edit::{DocumentMut, Item, Table};
    let mut doc: DocumentMut = text.parse().context("policy.toml is not valid TOML")?;
    let sections = values.as_object().context("values must be an object")?;
    for (section, fields) in sections {
        let Some(fields) = fields.as_object() else { continue };
        if !doc.contains_key(section) {
            doc.insert(section, Item::Table(Table::new()));
        }
        let table = doc[section.as_str()].as_table_mut().with_context(|| format!("[{section}] is not a table"))?;
        for (key, v) in fields {
            if v.is_null() {
                table.remove(key);
                continue;
            }
            let new = to_toml_value(key, v)?;
            match table.get_mut(key).and_then(Item::as_value_mut) {
                Some(old) if toml_eq(old, &new) => {}
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = new;
                    *old.decor_mut() = decor;
                }
                None => {
                    table.insert(key, Item::Value(new));
                }
            }
        }
    }
    let out = doc.to_string();
    Policy::from_toml(&out)?;
    Ok(out)
}

fn to_toml_value(key: &str, v: &Value) -> anyhow::Result<toml_edit::Value> {
    Ok(match v {
        Value::Bool(b) => (*b).into(),
        // Dollar amounts are floats even when the form sends a whole number.
        Value::Number(n) if key == "usd_per_task" => n.as_f64().unwrap_or_default().into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64().unwrap_or_default().into(),
        },
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for i in items {
                arr.push(to_toml_value(key, i)?);
            }
            toml_edit::Value::Array(arr)
        }
        other => anyhow::bail!("unsupported value for {key}: {other}"),
    })
}

fn toml_eq(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
    use toml_edit::Value as V;
    match (a, b) {
        (V::String(x), V::String(y)) => x.value() == y.value(),
        (V::Integer(x), V::Integer(y)) => x.value() == y.value(),
        (V::Float(x), V::Float(y)) => x.value() == y.value(),
        (V::Boolean(x), V::Boolean(y)) => x.value() == y.value(),
        (V::Array(x), V::Array(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| toml_eq(a, b)),
        _ => false,
    }
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
                    Some(p) if !is_inside(&resolve(repo, p), repo) => match self.related_of(&resolve(repo, p)) {
                        Some(dir) => Verdict::Allow(format!("read inside related repo {}", dir.display())),
                        None => access(self.read.outside_repo, "reading outside the repository"),
                    },
                    _ => Verdict::Allow("read inside the repository".into()),
                }
            }
            "TodoWrite" => Verdict::Allow("planning tool".into()),
            "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                let Some(p) = field("file_path").or_else(|| field("notebook_path")) else {
                    return Verdict::Deny("write without a file path".into());
                };
                write_verdict(role, &resolve(repo, p), repo, &self.extra_read, &self.extra_write)
            }
            "Bash" => self.bash(field("command").unwrap_or_default(), repo),
            "WebFetch" | "WebSearch" => access(self.web.access, "web access"),
            _ => Verdict::Ask,
        }
    }

    /// `evaluate`, plus `[edits]` task scope for the implementer: writes to files the task doc does
    /// not name warn or block. The task's own doc is always in scope.
    pub fn evaluate_task(&self, role: Role, tool: &str, input: &Value, repo: &Path, slug: &str) -> Verdict {
        let verdict = self.evaluate(role, tool, input, repo);
        let path = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(Value::as_str);
        let is_write = matches!(tool, "Write" | "Edit" | "MultiEdit" | "NotebookEdit");
        let (Verdict::Allow(_), true, Role::Implementer, EditScope::Task, Some(path)) = (&verdict, is_write, role, self.edits.scope, path)
        else {
            return verdict;
        };
        let full = resolve(repo, path);
        // In a member repo, files are named `<rel>` or `<member folder>/<rel>`.
        let member = self.extra_write.iter().find(|m| is_inside(&full, m));
        let root = member.map_or(repo, PathBuf::as_path);
        let rel = full.strip_prefix(root).unwrap_or(&full).to_string_lossy().replace('\\', "/");
        if member.is_none() && rel == format!(".ai-tasks/{slug}.md") {
            return verdict;
        }
        let scope = crate::tasks::read(repo, slug).map(|d| task_scope(&d)).unwrap_or_default();
        let prefixed = member.and_then(|m| m.file_name()).map(|n| format!("{}/{rel}", n.to_string_lossy()));
        if scope.is_empty() || in_scope(&scope, &rel) || prefixed.is_some_and(|p| in_scope(&scope, &p)) {
            return verdict;
        }
        match self.edits.on_out_of_scope {
            OnOutOfScope::Warn => Verdict::Allow(format!("{OUT_OF_SCOPE}: {rel} (edits.on_out_of_scope = warn)")),
            OnOutOfScope::Block => Verdict::Deny(format!(
                "{rel} is not among the files the task doc names. Do not change it; if the fix really needs it, \
                 explain why under Implementation notes and finish, so the human can widen the scope."
            )),
        }
    }

    /// The related repo `path` lies in, if any.
    fn related_of(&self, path: &Path) -> Option<&PathBuf> {
        self.extra_read.iter().find(|dir| is_inside_or_same(path, dir))
    }

    fn bash(&self, command: &str, repo: &Path) -> Verdict {
        let words: Vec<&str> =
            command.split(|c: char| c.is_whitespace() || ";|&()`".contains(c)).filter(|w| !w.is_empty()).collect();
        if let Some(verb) = git_verbs(&words).into_iter().find(|v| self.git.denied().contains(v)) {
            return Verdict::Deny(format!("git {verb} is not allowed for agents; Harnette handles delivery."));
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
                        Some(dir) if !is_inside_or_same(&resolve(repo, dir), repo) && self.related_of(&resolve(repo, dir)).is_none() => {
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

/// Files and folders a task doc names: backticked paths anywhere (`src/a.js`, `src/util/`,
/// `src/a.js:12` evidence) and `(path:line)` references in review findings. Empty = no scope known.
pub fn task_scope(doc: &str) -> Vec<String> {
    let is_path_char = |c: char| c.is_ascii_alphanumeric() || "._/-".contains(c);
    let mut out: Vec<String> = Vec::new();
    let mut add = |token: &str| {
        let token = token.trim().trim_start_matches("./");
        // `src/a.js:12-14` → `src/a.js`
        let token = match token.split_once(':') {
            Some((path, line)) if line.chars().all(|c| c.is_ascii_digit() || c == '-') => path,
            _ => token,
        };
        let looks_like_path = !token.is_empty()
            && token.chars().all(is_path_char)
            && (token.contains('/') || token.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && (1..=5).contains(&ext.len())))
            && (!token.starts_with('.') || token.starts_with(".ai-tasks/"));
        if looks_like_path && !out.iter().any(|t| t == token) {
            out.push(token.to_string());
        }
    };
    for (i, part) in doc.split('`').enumerate() {
        if i % 2 == 1 {
            add(part);
        }
    }
    for chunk in doc.split('(').skip(1) {
        if let Some((inner, _)) = chunk.split_once(')')
            && inner.contains(':')
        {
            add(inner);
        }
    }
    out
}

/// Is `rel` (repo-relative, `/`-separated) covered by the scope? A bare file name matches anywhere.
pub fn in_scope(scope: &[String], rel: &str) -> bool {
    scope.iter().any(|s| {
        if let Some(dir) = s.strip_suffix('/') {
            rel.starts_with(&format!("{dir}/"))
        } else if s.contains('/') {
            rel == s || rel.starts_with(&format!("{s}/"))
        } else {
            rel.rsplit('/').next() == Some(s.as_str())
        }
    })
}

fn write_verdict(role: Role, path: &Path, repo: &Path, related: &[PathBuf], members: &[PathBuf]) -> Verdict {
    let tasks = repo.join(".ai-tasks");
    if path == policy_path(repo) {
        return Verdict::Deny("Agents may not change the Harnette policy.".into());
    }
    if is_inside(path, &tasks.join("runs")) {
        return Verdict::Deny("Run journals are written by Harnette only.".into());
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
        Role::Chat => {
            let is_task_doc = path.parent() == Some(tasks.as_path()) && path.extension().is_some_and(|e| e == "md");
            if is_task_doc {
                Verdict::Allow("task doc in .ai-tasks/".into())
            } else if is_inside(path, &repo.join(".git")) {
                Verdict::Deny("Agents may not write inside .git/.".into())
            } else if is_inside(path, repo) || related.iter().any(|d| is_inside(path, d)) {
                Verdict::Ask
            } else {
                Verdict::Deny("Writes outside the repository and its related repos are not allowed.".into())
            }
        }
        Role::Implementer => {
            if let Some(member) = members.iter().find(|m| is_inside(path, m)) {
                return if is_inside(path, &member.join(".git")) {
                    Verdict::Deny("Agents may not write inside .git/.".into())
                } else {
                    Verdict::Allow(format!("write inside member repo {}", member.display()))
                };
            }
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

/// Prefix of the rule text for an allowed write outside the task scope (adapters raise an alarm on it).
pub const OUT_OF_SCOPE: &str = "outside the task scope";

/// PreToolUse hook answer for `harnette-guard`. Never fails open: any error asks the human.
/// Directories beyond the repo for one run: related repos (readable) and member repos (writable by the implementer).
#[derive(Debug, Clone, Default)]
pub struct RunDirs {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
}

pub fn hook_response(repo: &Path, role: Role, slug: Option<&str>, dirs: &RunDirs, hook_input: &str) -> String {
    let (decision, reason) = match decide_hook(repo, role, slug, dirs, hook_input) {
        Ok(Verdict::Allow(rule)) => ("allow", rule),
        Ok(Verdict::Deny(reason)) => ("deny", reason),
        Ok(Verdict::Ask) => ("ask", "Harnette asks the human supervisor.".to_string()),
        Err(e) => ("ask", format!("harnette-guard could not decide ({e:#}); asking the human.")),
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

fn decide_hook(repo: &Path, role: Role, slug: Option<&str>, dirs: &RunDirs, hook_input: &str) -> anyhow::Result<Verdict> {
    let v: Value = serde_json::from_str(hook_input).context("hook input is not JSON")?;
    let tool = v.get("tool_name").and_then(Value::as_str).context("hook input has no tool_name")?;
    let input = v.get("tool_input").cloned().unwrap_or(Value::Null);
    let mut policy = Policy::load(repo)?;
    policy.extra_read = dirs.read.clone();
    policy.extra_write = dirs.write.clone();
    Ok(match slug {
        Some(slug) => policy.evaluate_task(role, tool, &input, repo, slug),
        None => policy.evaluate(role, tool, &input, repo),
    })
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
    fn push_stays_denied_when_removed_from_the_list() {
        let p = Policy::from_toml("[git]\ndeny = [\"checkout\"]\n[shell]\nallow = [\"git\"]").unwrap();
        assert!(matches!(bash(&p, "git push origin HEAD"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "git -C . push --force"), Verdict::Deny(_)));
        assert!(matches!(bash(&p, "git checkout -b x"), Verdict::Deny(_)));
        assert!(!matches!(bash(&p, "git commit -m x"), Verdict::Deny(_)), "only the listed verbs and push");
        assert_eq!(p.git.denied(), ["push", "checkout"]);
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
    fn suggested_policy_detects_the_build() {
        let dir = std::env::temp_dir().join(format!("harnette-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("svc")).unwrap();
        assert_eq!(suggested_toml(&dir), default_toml(), "nothing recognised: plain template");
        std::fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        std::fs::write(dir.join("svc/pom.xml"), "<artifactId>testcontainers</artifactId>").unwrap();
        let p = Policy::from_toml(&suggested_toml(&dir)).unwrap();
        assert_eq!(p.build.command.as_deref(), Some("mvn -B verify"));
        assert_eq!(p.build.requires, vec!["docker".to_string()]);
        assert!(p.shell.allow.contains(&"mvn".to_string()));
        std::fs::write(dir.join("mvnw"), "").unwrap();
        assert_eq!(detect_build(&dir).unwrap().command, "./mvnw -B verify");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn form_values_keep_comments_and_touch_only_changes() {
        let values = json!({
            "shell": { "unlisted": "deny", "allow": Policy::default().shell.allow, "ask_timeout_min": 30 },
            "budget": { "usd_per_task": 5, "tokens_per_task": null },
            "build": { "command": "mvn -B verify" },
        });
        let out = apply_values(default_toml(), &values).unwrap();
        let p = Policy::from_toml(&out).unwrap();
        assert_eq!(p.shell.unlisted, Access::Deny);
        assert_eq!(p.shell.ask_timeout_min, 30);
        assert_eq!(p.budget.usd_per_task, Some(5.0));
        assert_eq!(p.build.command.as_deref(), Some("mvn -B verify"));
        assert!(out.contains(r#"unlisted = "deny"              # "ask" | "deny" | "allow""#), "comment kept:\n{out}");
        // The unchanged multi-line allow list keeps its layout.
        assert!(out.contains("allow = [\"ls\", \"pwd\""));
        assert!(out.contains("         \"git status\""));
        // A form that changes nothing changes nothing.
        let same = serde_json::to_value(Policy::default()).unwrap();
        assert_eq!(apply_values(default_toml(), &same).unwrap(), default_toml());
        assert!(apply_values(default_toml(), &json!({ "roles": { "implementer": "gpt" } })).is_err());
        // A file written before a key existed gets it added.
        let old = "[delivery]\nmode = \"none\"\n";
        let out = apply_values(old, &json!({ "delivery": { "branch_at_start": true } })).unwrap();
        assert!(Policy::from_toml(&out).unwrap().delivery.branch_at_start, "{out}");
    }

    #[test]
    fn task_scope_from_the_doc() {
        let doc = "## Analysis\n`src/math.js:2-4` has no checks; `npm test` runs `node --test`.\n\n## Proposed fix\n1. Create `src/clamp.js` and `clamp.test.js`.\n2. Touch `src/util/`.\n\n## Review log\n- [ ] R1-1 [Minor] nit (src/extra.js:9) - expected: x\n";
        let scope = task_scope(doc);
        assert_eq!(scope, vec!["src/math.js", "src/clamp.js", "clamp.test.js", "src/util/", "src/extra.js"]);
        assert!(in_scope(&scope, "src/clamp.js"));
        assert!(in_scope(&scope, "test/clamp.test.js"), "a bare name matches in any folder");
        assert!(in_scope(&scope, "src/util/deep/x.js"));
        assert!(!in_scope(&scope, "src/other.js"));
        assert!(!in_scope(&scope, "package.json"));
    }

    #[test]
    fn edits_outside_the_task_scope_warn_or_block() {
        let dir = std::env::temp_dir().join(format!("harnette-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ai-tasks")).unwrap();
        std::fs::write(dir.join(".ai-tasks/T.md"), "# T\n\n## Proposed fix\n1. Edit `src/a.js`.\n").unwrap();
        let edit = |p: &str| json!({ "file_path": dir.join(p).display().to_string() });
        let warn = Policy::default();
        assert!(matches!(warn.evaluate_task(Role::Implementer, "Edit", &edit("src/a.js"), &dir, "T"), Verdict::Allow(r) if !r.starts_with(OUT_OF_SCOPE)));
        assert!(matches!(warn.evaluate_task(Role::Implementer, "Edit", &edit("src/b.js"), &dir, "T"), Verdict::Allow(r) if r.starts_with(OUT_OF_SCOPE)));
        assert!(matches!(warn.evaluate_task(Role::Implementer, "Edit", &edit(".ai-tasks/T.md"), &dir, "T"), Verdict::Allow(r) if !r.starts_with(OUT_OF_SCOPE)));
        let block = Policy::from_toml("[edits]\non_out_of_scope = \"block\"\n").unwrap();
        assert!(matches!(block.evaluate_task(Role::Implementer, "Write", &edit("src/b.js"), &dir, "T"), Verdict::Deny(r) if r.contains("src/b.js")));
        let repo_scope = Policy::from_toml("[edits]\nscope = \"repo\"\non_out_of_scope = \"block\"\n").unwrap();
        assert!(matches!(repo_scope.evaluate_task(Role::Implementer, "Edit", &edit("src/b.js"), &dir, "T"), Verdict::Allow(_)));
        // Other roles and reads are unaffected; a doc naming no files has no scope.
        assert!(matches!(block.evaluate_task(Role::Implementer, "Read", &edit("src/b.js"), &dir, "T"), Verdict::Allow(_)));
        std::fs::write(dir.join(".ai-tasks/T.md"), "# T\n\nFix the bug.\n").unwrap();
        assert!(matches!(block.evaluate_task(Role::Implementer, "Edit", &edit("src/b.js"), &dir, "T"), Verdict::Allow(_)));
        let _ = std::fs::remove_dir_all(&dir);
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
    fn chat_writes_task_docs_and_asks_for_the_rest() {
        let p = Policy { extra_read: vec![PathBuf::from("/work/lib")], ..Policy::default() };
        let w = |path: &str| eval(&p, Role::Chat, "Edit", json!({ "file_path": path }));
        assert!(matches!(w("/work/repo/.ai-tasks/T-1.md"), Verdict::Allow(_)));
        assert_eq!(w("/work/repo/src/a.rs"), Verdict::Ask);
        assert_eq!(w("/work/lib/src/x.rs"), Verdict::Ask);
        assert!(matches!(w("/work/repo/.ai-tasks/policy.toml"), Verdict::Deny(_)));
        assert!(matches!(w("/work/repo/.ai-tasks/runs/T/x.md"), Verdict::Deny(_)));
        assert!(matches!(w("/work/repo/.git/config"), Verdict::Deny(_)));
        assert!(matches!(w("/etc/hosts"), Verdict::Deny(_)));
        assert_eq!("chat".parse::<Role>().unwrap(), Role::Chat);
        assert_eq!(Roles::default().worker(Role::Chat), Worker::claude());
    }

    #[test]
    fn implementer_writes_member_repos() {
        let p = Policy { extra_read: vec!["/work/lib".into(), "/work/docs".into()], extra_write: vec!["/work/lib".into()], ..Policy::default() };
        let w = |role, path: &str| eval(&p, role, "Edit", json!({ "file_path": path }));
        assert!(matches!(w(Role::Implementer, "/work/lib/src/x.rs"), Verdict::Allow(r) if r.contains("member")));
        assert!(matches!(w(Role::Implementer, "/work/lib/.git/config"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Implementer, "/work/docs/x.md"), Verdict::Deny(_)), "not a member: read-only");
        assert!(matches!(w(Role::Analyst, "/work/lib/src/x.rs"), Verdict::Deny(_)));
        assert!(matches!(w(Role::Reviewer, "/work/lib/src/x.rs"), Verdict::Deny(_)));
    }

    #[test]
    fn task_scope_covers_member_files() {
        let dir = std::env::temp_dir().join(format!("harnette-scope-member-{}", std::process::id()));
        let lib = std::env::temp_dir().join(format!("harnette-scope-lib-{}", std::process::id()));
        crate::tasks::write(&dir, "T", &format!("Fix `{}/src/a.rs` and `src/main.rs`.", lib.file_name().unwrap().to_string_lossy())).unwrap();
        let mut p = Policy::from_toml("[edits]\non_out_of_scope = \"block\"").unwrap();
        p.extra_write = vec![lib.clone()];
        let edit = |path: PathBuf| json!({ "file_path": path.display().to_string() });
        assert!(matches!(p.evaluate_task(Role::Implementer, "Edit", &edit(lib.join("src/a.rs")), &dir, "T"), Verdict::Allow(_)));
        assert!(matches!(p.evaluate_task(Role::Implementer, "Edit", &edit(lib.join("src/b.rs")), &dir, "T"), Verdict::Deny(_)));
    }

    #[test]
    fn related_repos_are_readable_not_writable() {
        let p = Policy { extra_read: vec![PathBuf::from("/work/lib")], ..Policy::default() };
        assert!(matches!(eval(&p, Role::Analyst, "Read", json!({ "file_path": "/work/lib/src/x.rs" })), Verdict::Allow(_)));
        assert!(matches!(eval(&p, Role::Analyst, "Grep", json!({ "pattern": "x", "path": "/work/lib" })), Verdict::Allow(_)));
        assert_eq!(eval(&p, Role::Analyst, "Read", json!({ "file_path": "/work/library/x" })), Verdict::Ask, "a sibling with the same prefix is not related");
        assert!(matches!(bash(&p, "git -C /work/lib log"), Verdict::Allow(_)));
        assert!(matches!(bash(&p, "git -C /work/lib push"), Verdict::Deny(_)));
        for role in [Role::Analyst, Role::Reviewer, Role::Implementer] {
            assert!(matches!(eval(&p, role, "Edit", json!({ "file_path": "/work/lib/src/x.rs" })), Verdict::Deny(_)), "{role:?}");
        }
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
    fn installed_plugin_ids() {
        let json = r#"{"version":2,"plugins":{"ponytail@ponytail":[{"scope":"user"}],"a@b":[]}}"#;
        assert_eq!(plugin_ids(json), ["a@b", "ponytail@ponytail"]);
        assert!(plugin_ids("not json").is_empty());
    }

    #[test]
    fn hook_response_shapes() {
        let decision = |input: &str| -> String {
            let out: Value = serde_json::from_str(&hook_response(Path::new("/nonexistent/repo"), Role::Analyst, None, &RunDirs::default(), input)).unwrap();
            out["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string()
        };
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#), "allow");
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"git push"}}"#), "deny");
        assert_eq!(decision(r#"{"tool_name":"Bash","tool_input":{"command":"mvn test"}}"#), "ask");
        assert_eq!(decision("not json"), "ask");
        assert_eq!(decision(r#"{"tool_input":{}}"#), "ask");
    }
}
