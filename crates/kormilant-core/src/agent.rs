//! One agent run, whichever CLI does it. The session only talks to `RunHandle`.

use crate::claude::{self, RunConfig};
use crate::copilot;
use crate::event::RunEvent;
use crate::policy::Agent;

#[derive(Clone)]
pub enum RunHandle {
    Claude(claude::Handle),
    Copilot(copilot::Handle),
}

macro_rules! each {
    ($self:ident, $h:ident => $e:expr) => {
        match $self {
            RunHandle::Claude($h) => $e,
            RunHandle::Copilot($h) => $e,
        }
    };
}

impl RunHandle {
    pub fn run_id(&self) -> &str {
        each!(self, h => h.run_id())
    }

    /// Answer a pending permission request. Returns false if it was already answered or timed out.
    pub fn answer(&self, request_id: &str, allow: bool) -> bool {
        each!(self, h => h.answer(request_id, allow))
    }

    /// Steering note: Claude sees it after its current tool call, Copilot after its current turn.
    pub async fn note(&self, text: &str, images: &[String]) -> anyhow::Result<()> {
        each!(self, h => h.note(text, images).await)
    }

    pub fn is_finished(&self) -> bool {
        each!(self, h => h.is_finished())
    }

    /// Wait until the agent has exited; true if it finished its task successfully.
    pub async fn wait(&self) -> bool {
        each!(self, h => h.wait().await)
    }

    pub fn pause(&self) -> anyhow::Result<()> {
        each!(self, h => h.pause())
    }

    pub fn resume(&self) -> anyhow::Result<()> {
        each!(self, h => h.resume())
    }

    pub fn stop(&self) {
        each!(self, h => h.stop())
    }

    pub fn kill_now(&self) {
        each!(self, h => h.kill_now())
    }

    /// The worker ran out of plan quota (the run did not do the task).
    pub fn quota_exhausted(&self) -> bool {
        match self {
            RunHandle::Claude(_) => false,
            RunHandle::Copilot(h) => h.quota_exhausted(),
        }
    }
}

pub async fn start_run<F>(cfg: RunConfig, sink: F) -> anyhow::Result<RunHandle>
where
    F: Fn(RunEvent) + Send + Sync + 'static,
{
    Ok(match cfg.worker.agent {
        Agent::Claude => RunHandle::Claude(claude::start(cfg, sink).await?),
        Agent::Copilot => RunHandle::Copilot(copilot::start(cfg, sink).await?),
    })
}
