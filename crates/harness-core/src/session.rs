//! A session is what the UI controls: one agent run (Analyze/Refine) or a whole
//! implement → build → review loop. Asks, notes and stop always go to the agent running now.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use tokio::sync::Notify;

use crate::build::{self, BuildOutcome};
use crate::agent::{RunHandle, start_run};
use crate::claude::{BuildSummary, Mode, RunConfig};
use crate::event::RunEvent;
use crate::git;
use crate::journal::{Journal, new_run_id};
use crate::policy::{Agent, Budget, OnQuota, Policy, Worker};
use crate::tasks::{self, StatusKind};
use crate::watchdog::{Action, Alarm, PriorUsage, Watchdog};

type Sink = Arc<dyn Fn(RunEvent) + Send + Sync>;

#[derive(Default)]
struct SessionInner {
    current: Mutex<Option<RunHandle>>,
    cancelled: AtomicBool,
    finished: AtomicBool,
    cancel: Notify,
    paused: AtomicBool,
    resumed: Notify,
    /// Unwatched sink, for events the session itself emits (pause, resume, alarms).
    sink: Mutex<Option<Sink>>,
    watchdog: Mutex<Option<Watchdog>>,
    /// Agents that ran out of quota in this session; their roles go to Claude for the rest of it.
    exhausted: Mutex<Vec<Agent>>,
}

#[derive(Clone, Default)]
pub struct Session {
    inner: Arc<SessionInner>,
}

impl Session {
    pub fn answer(&self, request_id: &str, allow: bool) -> bool {
        self.current().is_some_and(|h| h.answer(request_id, allow))
    }

    pub async fn note(&self, text: &str) -> anyhow::Result<()> {
        self.current().context("No agent is running right now (Harness may be building).")?.note(text).await
    }

    pub fn stop(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.cancel.notify_waiters();
        self.inner.resumed.notify_waiters();
        if let Some(h) = self.current() {
            h.stop();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// Freeze the running agent; a loop also holds before its next phase.
    pub fn pause(&self, reason: &str) {
        if self.inner.paused.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(h) = self.current()
            && let Err(e) = h.pause()
        {
            self.emit(RunEvent::Error { message: format!("{e:#}") });
        }
        if let Some(w) = self.inner.watchdog.lock().unwrap().as_mut() {
            w.set_paused(true, Instant::now());
        }
        self.emit(RunEvent::Paused { reason: reason.to_string() });
    }

    pub fn resume(&self) {
        if !self.inner.paused.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(h) = self.current() {
            let _ = h.resume();
        }
        if let Some(w) = self.inner.watchdog.lock().unwrap().as_mut() {
            w.set_paused(false, Instant::now());
        }
        self.inner.resumed.notify_waiters();
        self.emit(RunEvent::Resumed);
    }

    async fn wait_if_paused(&self) {
        while self.is_paused() && !self.cancelled() {
            let resumed = self.inner.resumed.notified();
            tokio::pin!(resumed);
            resumed.as_mut().enable();
            if !self.is_paused() || self.cancelled() {
                break;
            }
            resumed.await;
        }
    }

    fn emit(&self, e: RunEvent) {
        let sink = self.inner.sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink(e);
        }
    }

    fn remaining_usd(&self) -> Option<f64> {
        self.inner.watchdog.lock().unwrap().as_ref().and_then(Watchdog::remaining_usd)
    }

    fn apply(&self, alarms: Vec<Alarm>) {
        for a in alarms {
            self.emit(RunEvent::Alarm { alarm: a.kind.clone(), message: a.message.clone(), action: a.action.as_str().into() });
            match a.action {
                Action::Warn => {}
                Action::Pause => self.pause(&a.message),
                Action::Stop => {
                    self.emit(RunEvent::Error { message: format!("Stopped by the watchdog: {}", a.message) });
                    self.stop();
                }
            }
        }
    }

    /// Route events through the watchdog and start its clock. Returns the sink agents should use.
    fn watch(&self, sink: Sink, budget: Budget, prior: PriorUsage) -> Sink {
        self.inner.sink.lock().unwrap().replace(sink.clone());
        self.inner.watchdog.lock().unwrap().replace(Watchdog::new(budget, prior, Instant::now()));
        let s = self.clone();
        tokio::spawn(async move {
            while !s.is_finished() {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let running = s.current().is_some();
                let alarms = s.inner.watchdog.lock().unwrap().as_mut().map(|w| w.tick(Instant::now(), running)).unwrap_or_default();
                if !s.is_finished() {
                    s.apply(alarms);
                }
            }
        });
        let s = self.clone();
        Arc::new(move |e: RunEvent| {
            let alarms = s.inner.watchdog.lock().unwrap().as_mut().map(|w| w.observe(&e, Instant::now())).unwrap_or_default();
            sink(e);
            s.apply(alarms);
        })
    }

    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::SeqCst)
    }

    /// Fill in the worker for the run's role from `[roles]`. `claude_model` (picked in the UI)
    /// applies to Claude roles that name no model.
    fn assign_worker(&self, policy: &Policy, cfg: &mut RunConfig, claude_model: Option<String>) {
        let mut worker = policy.roles.worker(cfg.mode.role());
        if self.inner.exhausted.lock().unwrap().contains(&worker.agent) {
            worker = Worker::claude();
        }
        cfg.model = worker.model.clone().or(if worker.agent == Agent::Claude { claude_model } else { None });
        cfg.worker = worker;
    }

    /// Wait for a run; if its worker ran out of quota, apply `roles.on_quota_exhausted` and,
    /// unless that says stop, run the same step again on Claude.
    async fn wait_with_fallback(
        &self,
        mut handle: RunHandle,
        mut cfg: RunConfig,
        claude_model: Option<String>,
        sink: &Sink,
    ) -> anyhow::Result<bool> {
        loop {
            self.inner.current.lock().unwrap().replace(handle.clone());
            let ok = handle.wait().await;
            self.inner.current.lock().unwrap().take();
            if ok || !handle.quota_exhausted() || self.cancelled() || cfg.worker.agent == Agent::Claude {
                return Ok(ok);
            }
            let who = cfg.worker.label();
            let role = cfg.mode.role().as_str();
            self.inner.exhausted.lock().unwrap().push(cfg.worker.agent);
            let on_quota = Policy::load(&cfg.repo).map(|p| p.roles.on_quota_exhausted).unwrap_or(OnQuota::Ask);
            match on_quota {
                OnQuota::Stop => {
                    let message = format!("{who} is out of quota; stopping (roles.on_quota_exhausted = \"stop\").");
                    sink(RunEvent::Alarm { alarm: "quota".into(), message: message.clone(), action: "stop".into() });
                    bail!(message);
                }
                OnQuota::Ask => {
                    let message = format!("{who} is out of quota. Resume continues the {role} step on Claude; Stop ends the run.");
                    sink(RunEvent::Alarm { alarm: "quota".into(), message: message.clone(), action: "pause".into() });
                    self.pause(&message);
                    self.wait_if_paused().await;
                    if self.cancelled() {
                        return Ok(false);
                    }
                }
                OnQuota::Reassign => {}
            }
            sink(RunEvent::Info { message: format!("{role} reassigned from {who} to Claude (out of quota).") });
            cfg.worker = Worker::claude();
            cfg.model = claude_model.clone();
            let s2 = sink.clone();
            handle = start_run(cfg.clone(), move |e| s2(e)).await?;
        }
    }

    fn current(&self) -> Option<RunHandle> {
        self.inner.current.lock().unwrap().clone()
    }

    fn cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    fn end(&self, sink: &Sink, ok: bool, summary: String) {
        self.inner.finished.store(true, Ordering::SeqCst);
        sink(RunEvent::SessionEnded { ok, summary });
    }
}

/// One agent run (Analyze or Refine) as a session.
pub async fn start_single<F>(cfg: RunConfig, sink: F) -> anyhow::Result<Session>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    let session = Session::default();
    let policy = Policy::load(&cfg.repo)?;
    let sink = session.watch(Arc::new(sink), policy.budget.clone(), cfg.prior);
    let mut cfg = cfg;
    let claude_model = cfg.model.clone();
    session.assign_worker(&policy, &mut cfg, claude_model.clone());
    if cfg.max_budget_usd.is_none() {
        cfg.max_budget_usd = session.remaining_usd();
    }
    let label = match cfg.mode {
        Mode::Analyze { .. } => "Analysis",
        Mode::Refine => "Refine",
        Mode::Implement { .. } => "Implementation",
        Mode::Review { .. } => "Review",
    };
    let s2 = sink.clone();
    let handle = match start_run(cfg.clone(), move |e| s2(e)).await {
        Ok(h) => h,
        Err(e) => {
            session.inner.finished.store(true, Ordering::SeqCst);
            return Err(e);
        }
    };
    session.inner.current.lock().unwrap().replace(handle.clone());
    let s = session.clone();
    tokio::spawn(async move {
        let (ok, summary) = match s.wait_with_fallback(handle, cfg, claude_model, &sink).await {
            Ok(true) => (true, format!("{label} finished")),
            Ok(false) => (false, format!("{label} did not finish")),
            Err(e) => (false, format!("{e:#}")),
        };
        s.end(&sink, ok, summary);
    });
    Ok(session)
}

pub struct LoopConfig {
    pub repo: PathBuf,
    pub slug: String,
    pub model: Option<String>,
    pub claude_bin: String,
    pub copilot_bin: String,
    pub guard_bin: Option<PathBuf>,
    /// Usage of this task before the loop (from the ledger), for budgets.
    pub prior: PriorUsage,
}

impl LoopConfig {
    pub fn new(repo: impl Into<PathBuf>, slug: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            slug: slug.into(),
            model: None,
            claude_bin: "claude".into(),
            copilot_bin: "copilot".into(),
            guard_bin: None,
            prior: PriorUsage::default(),
        }
    }

    fn agent(&self, session: &Session, policy: &Policy, repo: &std::path::Path, mode: Mode, max_budget_usd: Option<f64>) -> RunConfig {
        let mut cfg = RunConfig::new(repo, &self.slug, mode);
        session.assign_worker(policy, &mut cfg, self.model.clone());
        cfg.claude_bin = self.claude_bin.clone();
        cfg.copilot_bin = self.copilot_bin.clone();
        cfg.guard_bin = self.guard_bin.clone();
        cfg.max_budget_usd = max_budget_usd;
        cfg
    }
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Implement,
    Review,
    Stop(String),
}

/// What the loop does next, given the doc's Status.
pub fn next_step(status: StatusKind) -> Step {
    match status {
        StatusKind::Ready | StatusKind::ChangesRequested => Step::Implement,
        StatusKind::InReview => Step::Review,
        StatusKind::Done => Step::Stop("Task is Done.".into()),
        StatusKind::Analysis => Step::Stop("Task is still in Analysis: answer the open questions and Refine first.".into()),
        StatusKind::Unknown => Step::Stop("Task has no recognised Status line.".into()),
    }
}

/// The status after a review, enforcing the build gate. Returns the status and why Harness overrode it.
pub fn gate_status(reviewer: StatusKind, build_ok: Option<bool>) -> (StatusKind, Option<String>) {
    match reviewer {
        StatusKind::Done if build_ok == Some(false) => {
            (StatusKind::ChangesRequested, Some("The build is red; Harness does not accept Done.".into()))
        }
        StatusKind::Done | StatusKind::ChangesRequested => (reviewer, None),
        _ => (StatusKind::ChangesRequested, Some("The reviewer did not set Done or Changes requested.".into())),
    }
}

fn status_of(repo: &std::path::Path, slug: &str) -> anyhow::Result<StatusKind> {
    Ok(tasks::parse(slug, &tasks::read(repo, slug)?).status_kind)
}

/// Implement → build → review until Done, max rounds, failure or stop.
pub async fn start_loop<F>(cfg: LoopConfig, sink: F) -> anyhow::Result<Session>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    let repo = cfg.repo.canonicalize().with_context(|| format!("not a directory: {}", cfg.repo.display()))?;
    if !tasks::valid_slug(&cfg.slug) {
        bail!("invalid slug {:?}", cfg.slug);
    }
    if !git::is_repo(&repo) {
        bail!("{} is not a git repository; the run loop needs git for checkpoints.", repo.display());
    }
    if let Step::Stop(reason) = next_step(status_of(&repo, &cfg.slug)?) {
        bail!(reason);
    }
    let policy = Policy::load(&repo)?;
    build::check_requires(&policy.build.requires)?;

    let loop_id = format!("loop-{}", new_run_id().trim_start_matches("run-"));
    let journal = Arc::new(Mutex::new(Journal::open(&repo, &cfg.slug, &loop_id)?));
    let sink: Sink = {
        let journal = journal.clone();
        Arc::new(move |e: RunEvent| {
            // Loop-level events go to the loop journal; agent runs keep their own.
            if matches!(
                e,
                RunEvent::Phase { .. }
                    | RunEvent::Checkpoint { .. }
                    | RunEvent::Build { .. }
                    | RunEvent::StatusChanged { .. }
                    | RunEvent::SessionEnded { .. }
                    | RunEvent::Error { .. }
                    | RunEvent::Alarm { .. }
                    | RunEvent::Paused { .. }
                    | RunEvent::Resumed
                    | RunEvent::Info { .. }
                    | RunEvent::DeliveryReady { .. }
                    | RunEvent::Delivered { .. }
            ) {
                journal.lock().unwrap().event(&e);
            }
            sink(e)
        })
    };
    let session = Session::default();
    let sink = session.watch(sink, policy.budget.clone(), cfg.prior);
    let s = session.clone();
    let run_dir = journal.lock().unwrap().dir().to_path_buf();
    tokio::spawn(async move {
        let (ok, summary) = match run_loop(&s, &cfg, &repo, &policy, &run_dir, &sink).await {
            Ok(summary) => {
                if !s.cancelled() && status_of(&repo, &cfg.slug).ok() == Some(StatusKind::Done) {
                    offer_delivery(&repo, &cfg.slug, &sink);
                }
                (true, summary)
            }
            Err(e) => {
                let msg = format!("{e:#}");
                sink(RunEvent::Error { message: msg.clone() });
                (false, msg)
            }
        };
        s.inner.current.lock().unwrap().take();
        s.end(&sink, ok, summary);
    });
    Ok(session)
}

/// After Done: deliver automatically, or offer the plan for approval, as `[delivery]` says.
fn offer_delivery(repo: &std::path::Path, slug: &str, sink: &Sink) {
    use crate::policy::{Approval, DeliveryMode};
    // Re-read the policy: the human may have changed delivery settings during the loop.
    let policy = match Policy::load(repo) {
        Ok(p) => p,
        Err(e) => return sink(RunEvent::Error { message: format!("{e:#}") }),
    };
    if policy.delivery.mode == DeliveryMode::None {
        return;
    }
    match crate::delivery::plan(repo, slug, &policy) {
        Err(e) => sink(RunEvent::Info { message: format!("Not delivered: {e:#}") }),
        Ok(plan) if policy.delivery.approval == Approval::Auto => match crate::delivery::execute(repo, &plan) {
            Ok(o) => sink(RunEvent::Delivered { branch: o.branch, commit: o.commit, pushed: o.pushed, pr_url: o.pr_url }),
            Err(e) => sink(RunEvent::Error { message: format!("Delivery failed: {e:#}") }),
        },
        Ok(plan) => sink(RunEvent::DeliveryReady { mode: plan.mode, branch: plan.branch, files: plan.files, commands: plan.commands }),
    }
}

/// Move fully verified earlier reviews into `<slug>.history.md` (workflow rule).
fn archive_reviews(repo: &std::path::Path, slug: &str, sink: &Sink) -> anyhow::Result<()> {
    let md = tasks::read(repo, slug)?;
    if let Some((doc, moved)) = crate::review::archive_verified(&md) {
        let history = tasks::tasks_dir(repo).join(format!("{slug}.history.md"));
        let mut text = std::fs::read_to_string(&history).unwrap_or_else(|_| format!("# {slug} — review history\n\n"));
        text.push_str(&moved);
        std::fs::write(&history, text)?;
        tasks::write(repo, slug, &doc)?;
        sink(RunEvent::Info { message: format!("Moved fully verified reviews to {slug}.history.md") });
    }
    Ok(())
}

async fn run_agent(session: &Session, cfg: RunConfig, claude_model: Option<String>, sink: &Sink) -> anyhow::Result<bool> {
    let s2 = sink.clone();
    let handle = start_run(cfg.clone(), move |e| s2(e)).await?;
    session.wait_with_fallback(handle, cfg, claude_model, sink).await
}

async fn run_loop(
    session: &Session,
    cfg: &LoopConfig,
    repo: &std::path::Path,
    policy: &Policy,
    run_dir: &std::path::Path,
    sink: &Sink,
) -> anyhow::Result<String> {
    let slug = cfg.slug.as_str();
    let max_rounds = policy.run.max_rounds.max(1);
    // Rounds are numbered per task, across loops, so a new loop never overwrites an old checkpoint.
    let first = git::list_checkpoints(repo, slug)?.last().map(|c| c.round).unwrap_or(0);
    let mut rounds_done = 0u32;
    let mut round = first;
    let mut last_build: Option<BuildOutcome> = None;

    loop {
        session.wait_if_paused().await;
        if session.cancelled() {
            return Ok("Stopped by the human.".into());
        }
        match next_step(status_of(repo, slug)?) {
            Step::Stop(reason) => return Ok(reason),
            Step::Implement => {
                if rounds_done >= max_rounds {
                    return Ok(format!("Stopped after {max_rounds} round(s) (run.max_rounds); the task needs another look."));
                }
                rounds_done += 1;
                round = first + rounds_done;
                sink(RunEvent::Phase { round, role: "implementer".into() });
                let git_ref = git::checkpoint(repo, slug, round)?;
                sink(RunEvent::Checkpoint { round, git_ref });

                let head_before = git::head(repo)?;
                let mode = Mode::Implement { round, build_command: policy.build.command.clone() };
                let ok = run_agent(session, cfg.agent(session, policy, repo, mode, session.remaining_usd()), cfg.model.clone(), sink).await?;
                if session.cancelled() {
                    return Ok("Stopped by the human.".into());
                }
                if git::head(repo)? != head_before {
                    bail!("HEAD moved during the implement round (a commit, checkout or reset happened). Stopped; checkpoint {round} holds the state before the round.");
                }
                if !ok {
                    bail!("The implementer did not finish round {round}.");
                }

                session.wait_if_paused().await;
                if session.cancelled() {
                    return Ok("Stopped by the human.".into());
                }
                last_build = match &policy.build.command {
                    Some(command) => {
                        build::check_requires(&policy.build.requires)?;
                        let log = run_dir.join(format!("build-{round}.log"));
                        let outcome = build::run_build(repo, command, &log, &session.inner.cancel).await?;
                        let tree = git::tree_of(repo, &git::snapshot(repo)?)?;
                        let ts_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        crate::delivery::record_build(
                            repo,
                            slug,
                            &crate::delivery::LastBuild { ok: outcome.ok, command: command.clone(), tree, ts_ms },
                        )?;
                        sink(RunEvent::Build {
                            ok: outcome.ok,
                            command: command.clone(),
                            duration_ms: outcome.duration_ms,
                            tail: outcome.tail.clone(),
                            log: outcome.log.display().to_string(),
                        });
                        if session.cancelled() {
                            return Ok("Stopped by the human.".into());
                        }
                        Some(outcome)
                    }
                    None => None,
                };
                tasks::set_status(repo, slug, StatusKind::InReview)?;
                sink(RunEvent::StatusChanged { to: StatusKind::InReview.label().into(), by: "harness".into(), reason: None });
            }
            Step::Review => {
                let review_round = round.max(1);
                sink(RunEvent::Phase { round: review_round, role: "reviewer".into() });
                // Everything since the first checkpoint of this task, or since HEAD when none exists.
                let base = git::list_checkpoints(repo, slug)?
                    .first()
                    .map(|c| c.git_ref.clone())
                    .or(git::head(repo)?)
                    .context("nothing to diff against: no checkpoint and no commits")?;
                let patch = git::diff(repo, &base)?;
                let diff_file = run_dir.join(format!("diff-{review_round}.patch"));
                std::fs::write(&diff_file, &patch)?;
                let diff_path = diff_file.strip_prefix(repo).unwrap_or(&diff_file).display().to_string();

                let build = match (&policy.build.command, &last_build) {
                    (Some(command), Some(b)) => Some(BuildSummary { ok: b.ok, command: command.clone(), tail: b.tail.clone() }),
                    _ => None,
                };
                let reviewed = git::reviewed_ref(slug);
                let changed_since_last_review = match git::resolve(repo, &reviewed) {
                    Some(_) => Some(git::changed_since(repo, &reviewed)?),
                    None => None,
                };
                let mode = Mode::Review { round: review_round, diff_path, build, changed_since_last_review };
                let ok = run_agent(session, cfg.agent(session, policy, repo, mode, session.remaining_usd()), cfg.model.clone(), sink).await?;
                if session.cancelled() {
                    return Ok("Stopped by the human.".into());
                }
                if !ok {
                    bail!("The reviewer did not finish round {review_round}.");
                }

                git::mark_reviewed(repo, slug)?;
                archive_reviews(repo, slug, sink)?;
                let reviewer_status = status_of(repo, slug)?;
                let (status, reason) = gate_status(reviewer_status, last_build.as_ref().map(|b| b.ok));
                if status != reviewer_status {
                    tasks::set_status(repo, slug, status)?;
                }
                let by = if reason.is_some() { "harness" } else { "reviewer" };
                sink(RunEvent::StatusChanged { to: status.label().into(), by: by.into(), reason });
                if status == StatusKind::ChangesRequested && rounds_done >= max_rounds {
                    return Ok(format!("Changes requested after {rounds_done} round(s); run.max_rounds reached."));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_step_table() {
        assert_eq!(next_step(StatusKind::Ready), Step::Implement);
        assert_eq!(next_step(StatusKind::ChangesRequested), Step::Implement);
        assert_eq!(next_step(StatusKind::InReview), Step::Review);
        assert!(matches!(next_step(StatusKind::Done), Step::Stop(_)));
        assert!(matches!(next_step(StatusKind::Analysis), Step::Stop(r) if r.contains("Refine")));
        assert!(matches!(next_step(StatusKind::Unknown), Step::Stop(_)));
    }

    #[test]
    fn build_gate_overrides_done() {
        assert_eq!(gate_status(StatusKind::Done, Some(true)), (StatusKind::Done, None));
        assert_eq!(gate_status(StatusKind::Done, None), (StatusKind::Done, None));
        assert!(matches!(gate_status(StatusKind::Done, Some(false)), (StatusKind::ChangesRequested, Some(_))));
        assert_eq!(gate_status(StatusKind::ChangesRequested, Some(true)), (StatusKind::ChangesRequested, None));
        assert!(matches!(gate_status(StatusKind::InReview, Some(true)), (StatusKind::ChangesRequested, Some(_))));
    }
}
