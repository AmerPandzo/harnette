//! Harness core: drives agent CLIs, enforces policy, records a journal.
//! No UI code lives here; the Tauri app and the spike CLI are thin shells over it.

pub mod build;
pub mod claude;
pub mod delivery;
pub mod event;
pub mod git;
pub mod journal;
pub mod ledger;
pub mod policy;
pub mod prompt;
pub mod review;
pub mod session;
pub mod tasks;
pub mod watch;
pub mod watchdog;

pub use claude::{BuildSummary, Mode, RunConfig, RunHandle, start_run};
pub use session::{LoopConfig, Session, start_loop, start_single};
pub use event::RunEvent;
