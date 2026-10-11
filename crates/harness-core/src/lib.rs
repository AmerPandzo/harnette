//! Harness core: drives agent CLIs, enforces policy, records a journal.
//! No UI code lives here; the Tauri app and the spike CLI are thin shells over it.

pub mod claude;
pub mod event;
pub mod journal;
pub mod policy;
pub mod prompt;
pub mod tasks;
pub mod watch;

pub use claude::{Mode, RunConfig, RunHandle, start_run};
pub use event::RunEvent;
