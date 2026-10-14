//! Watchdog: turns run events and the passage of time into alarms.
//! Pure state machine; the session applies the alarm's action (warn, pause, stop).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::event::RunEvent;
use crate::policy::{Budget, OnBreach};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Warn,
    Pause,
    Stop,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Pause => "pause",
            Self::Stop => "stop",
        }
    }
}

impl From<OnBreach> for Action {
    fn from(o: OnBreach) -> Self {
        match o {
            OnBreach::Warn => Self::Warn,
            OnBreach::Pause => Self::Pause,
            OnBreach::Stop => Self::Stop,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alarm {
    pub kind: String,
    pub message: String,
    pub action: Action,
}

/// Usage of this task before the session started (from the ledger).
#[derive(Debug, Clone, Copy, Default)]
pub struct PriorUsage {
    pub tokens: u64,
    pub usd: f64,
}

const REPEAT_LIMIT: u32 = 3;

pub struct Watchdog {
    budget: Budget,
    prior: PriorUsage,
    started: Instant,
    last_activity: Instant,
    seen_messages: HashSet<String>,
    tokens: u64,
    usd: f64,
    red_streak: u32,
    last_red_tail: Option<String>,
    denials: HashMap<String, u32>,
    fired: HashSet<String>,
    idle_fired: bool,
    paused: bool,
}

pub fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

impl Watchdog {
    pub fn new(budget: Budget, prior: PriorUsage, now: Instant) -> Self {
        Self {
            budget,
            prior,
            started: now,
            last_activity: now,
            seen_messages: HashSet::new(),
            tokens: 0,
            usd: 0.0,
            red_streak: 0,
            last_red_tail: None,
            denials: HashMap::new(),
            fired: HashSet::new(),
            idle_fired: false,
            paused: false,
        }
    }

    /// New tokens and USD spent in this session.
    pub fn session_usage(&self) -> (u64, f64) {
        (self.tokens, self.usd)
    }

    /// Dollars left for this task, if a USD budget is set (never below a cent, so the CLI accepts it).
    pub fn remaining_usd(&self) -> Option<f64> {
        self.budget.usd_per_task.map(|limit| (limit - self.prior.usd - self.usd).max(0.01))
    }

    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        self.paused = paused;
        if !paused {
            self.last_activity = now;
            self.idle_fired = false;
        }
    }

    fn breach(&self) -> Action {
        self.budget.on_breach.into()
    }

    /// Fires `key` once per session.
    fn once(&mut self, key: &str) -> bool {
        self.fired.insert(key.to_string())
    }

    fn threshold(&mut self, key: &str, what: &str, used: f64, limit: Option<f64>, show: impl Fn(f64) -> String) -> Vec<Alarm> {
        let Some(limit) = limit.filter(|l| *l > 0.0) else { return vec![] };
        if used >= limit && self.once(&format!("{key}:100")) {
            self.fired.insert(format!("{key}:80"));
            return vec![Alarm {
                kind: key.into(),
                message: format!("{what} budget reached: {} of {}.", show(used), show(limit)),
                action: self.breach(),
            }];
        }
        if used >= 0.8 * limit && self.once(&format!("{key}:80")) {
            return vec![Alarm {
                kind: key.into(),
                message: format!("{what} at {:.0}% of budget: {} of {}.", used / limit * 100.0, show(used), show(limit)),
                action: Action::Warn,
            }];
        }
        vec![]
    }

    pub fn observe(&mut self, event: &RunEvent, now: Instant) -> Vec<Alarm> {
        if !matches!(event, RunEvent::Quota { .. } | RunEvent::SessionEnded { .. }) {
            self.last_activity = now;
            self.idle_fired = false;
        }
        match event {
            RunEvent::Usage { message_id, new_tokens } => {
                if !self.seen_messages.insert(message_id.clone()) {
                    return vec![];
                }
                self.tokens += new_tokens;
                let used = (self.prior.tokens + self.tokens) as f64;
                let limit = self.budget.tokens_per_task.map(|t| t as f64);
                self.threshold("tokens", "Task token", used, limit, |v| fmt_tokens(v as u64))
            }
            RunEvent::Finished { cost_usd, .. } => {
                self.usd += cost_usd;
                let used = self.prior.usd + self.usd;
                self.threshold("usd", "Task cost", used, self.budget.usd_per_task, |v| format!("${v:.2}"))
            }
            RunEvent::Build { ok: false, tail, .. } => {
                if self.last_red_tail.as_deref() == Some(tail.as_str()) {
                    self.red_streak += 1;
                } else {
                    self.red_streak = 1;
                    self.last_red_tail = Some(tail.clone());
                }
                if self.red_streak >= REPEAT_LIMIT && self.once("repeated_build_failure") {
                    return vec![Alarm {
                        kind: "repeated_build_failure".into(),
                        message: format!("The build failed the same way {REPEAT_LIMIT} times in a row."),
                        action: self.breach(),
                    }];
                }
                vec![]
            }
            RunEvent::Build { ok: true, .. } => {
                self.red_streak = 0;
                self.last_red_tail = None;
                vec![]
            }
            RunEvent::PermissionAuto { allowed: false, tool, summary, .. } => {
                let key = format!("{tool}: {summary}");
                let n = {
                    let count = self.denials.entry(key.clone()).or_default();
                    *count += 1;
                    *count
                };
                if n >= REPEAT_LIMIT && self.once(&format!("denial:{key}")) {
                    return vec![Alarm {
                        kind: "repeated_denial".into(),
                        message: format!("The agent keeps trying a denied action ({n}×): {key}"),
                        action: self.breach(),
                    }];
                }
                vec![]
            }
            RunEvent::Quota { status, five_hour, seven_day } => {
                if status != "allowed" && !status.is_empty() && self.once("quota_blocked") {
                    return vec![Alarm {
                        kind: "quota".into(),
                        message: format!("Claude usage limit status: {status}. The worker may stop; reassign or wait for the reset."),
                        action: Action::Warn,
                    }];
                }
                let high = five_hour.max(*seven_day);
                if high >= 0.9 && self.once("quota_high") {
                    return vec![Alarm {
                        kind: "quota".into(),
                        message: format!("Claude usage at {:.0}% (5 h {:.0}%, 7 d {:.0}%).", high * 100.0, five_hour * 100.0, seven_day * 100.0),
                        action: Action::Warn,
                    }];
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// Time-based checks; call every few seconds. `agent_running` is false while Harnette builds or waits.
    pub fn tick(&mut self, now: Instant, agent_running: bool) -> Vec<Alarm> {
        let minutes = now.duration_since(self.started).as_secs_f64() / 60.0;
        let mut alarms = self.threshold(
            "minutes",
            "Run time",
            minutes,
            self.budget.minutes_per_run.map(|m| m as f64),
            |v| format!("{v:.0} min"),
        );
        if let Some(idle) = self.budget.idle_minutes.filter(|m| *m > 0) {
            let quiet = now.duration_since(self.last_activity);
            if agent_running && !self.paused && !self.idle_fired && quiet >= Duration::from_secs(idle * 60) {
                self.idle_fired = true;
                alarms.push(Alarm {
                    kind: "idle".into(),
                    message: format!("No agent activity for {idle} min; it may be stuck."),
                    action: self.breach(),
                });
            }
        }
        alarms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(f: impl FnOnce(&mut Budget)) -> Budget {
        let mut b = Budget::default();
        f(&mut b);
        b
    }

    fn usage(id: &str, n: u64) -> RunEvent {
        RunEvent::Usage { message_id: id.into(), new_tokens: n }
    }

    #[test]
    fn token_budget_dedupes_and_fires_once_per_threshold() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(budget(|b| b.tokens_per_task = Some(1000)), PriorUsage { tokens: 500, usd: 0.0 }, t0);
        assert!(w.observe(&usage("m1", 200), t0).is_empty());
        assert!(w.observe(&usage("m1", 200), t0).is_empty(), "same message counted once");
        let warn = w.observe(&usage("m2", 150), t0);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].action, Action::Warn);
        assert!(w.observe(&usage("m3", 10), t0).is_empty(), "80% fires once");
        let hit = w.observe(&usage("m4", 200), t0);
        assert_eq!(hit[0].action, Action::Pause, "default on_breach is pause");
        assert!(w.observe(&usage("m5", 200), t0).is_empty(), "100% fires once");
        assert_eq!(w.session_usage().0, 760);
    }

    #[test]
    fn usd_budget_and_remaining() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(
            budget(|b| {
                b.usd_per_task = Some(1.0);
                b.on_breach = OnBreach::Stop;
            }),
            PriorUsage { tokens: 0, usd: 0.5 },
            t0,
        );
        assert!((w.remaining_usd().unwrap() - 0.5).abs() < 1e-9);
        let fin = |usd| RunEvent::Finished {
            ok: true,
            result: String::new(),
            cost_usd: usd,
            input_tokens: 0,
            output_tokens: 0,
            new_tokens: 0,
            duration_ms: 0,
        };
        assert_eq!(w.observe(&fin(0.6), t0)[0].action, Action::Stop);
        assert_eq!(w.remaining_usd(), Some(0.01));
    }

    #[test]
    fn minutes_and_idle() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(
            budget(|b| {
                b.minutes_per_run = Some(10);
                b.idle_minutes = Some(2);
            }),
            PriorUsage::default(),
            t0,
        );
        let at = |m: u64| t0 + Duration::from_secs(m * 60);
        assert!(w.tick(at(1), true).is_empty());
        let idle = w.tick(at(3), true);
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0].kind, "idle");
        assert!(w.tick(at(4), true).is_empty(), "idle fires once per quiet spell");
        w.observe(&RunEvent::Text { text: "hi".into() }, at(4));
        w.set_paused(true, at(4));
        assert!(w.tick(at(7), true).iter().all(|a| a.kind != "idle"), "no idle alarm while paused");
        w.set_paused(false, at(7));
        assert!(w.tick(at(7), false).iter().all(|a| a.kind != "idle"), "no idle alarm while Harnette builds");
        let late = w.tick(at(8), true);
        assert!(late.iter().any(|a| a.kind == "minutes" && a.action == Action::Warn));
        let over = w.tick(at(10), true);
        assert!(over.iter().any(|a| a.kind == "minutes" && a.action == Action::Pause));
    }

    #[test]
    fn repeated_failures_and_denials() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(Budget::default(), PriorUsage::default(), t0);
        let red = |tail: &str| RunEvent::Build { ok: false, command: "npm test".into(), duration_ms: 1, tail: tail.into(), log: String::new() };
        assert!(w.observe(&red("E1"), t0).is_empty());
        assert!(w.observe(&red("E2"), t0).is_empty(), "a different failure restarts the streak");
        assert!(w.observe(&red("E2"), t0).is_empty());
        assert_eq!(w.observe(&red("E2"), t0)[0].kind, "repeated_build_failure");

        let deny = || RunEvent::PermissionAuto {
            request_id: "r".into(),
            tool: "Bash".into(),
            summary: "git push".into(),
            allowed: false,
            rule: "x".into(),
        };
        assert!(w.observe(&deny(), t0).is_empty());
        assert!(w.observe(&deny(), t0).is_empty());
        assert_eq!(w.observe(&deny(), t0)[0].kind, "repeated_denial");
        assert!(w.observe(&deny(), t0).is_empty());
    }

    #[test]
    fn quota_alarms() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(Budget::default(), PriorUsage::default(), t0);
        let q = |status: &str, five: f64| RunEvent::Quota { status: status.into(), five_hour: five, seven_day: 0.1 };
        assert!(w.observe(&q("allowed", 0.5), t0).is_empty());
        assert_eq!(w.observe(&q("allowed", 0.95), t0)[0].kind, "quota");
        assert_eq!(w.observe(&q("rejected", 0.95), t0)[0].action, Action::Warn);
    }
}
