//! sage-brain: the S.A.G.E. orchestrator brain ("plan big, execute small").
//!
//! Triage picks a direct reply, one tool call or an orchestrated run. For a run the
//! lead picks a playbook from `config/playbooks.yaml`, writes a plan, runs waves of
//! bounded workers, gates them and synthesizes the answer. See docs/ARCHITECTURE.md.

pub mod brain;
pub mod budget;
pub mod confirm;
pub mod gate;
pub mod jsonx;
pub mod plan;
pub mod playbooks;
pub mod prompts;
pub mod quota;
pub mod report;
pub mod templates;
pub mod worker;

pub use brain::{Brain, BrainOptions, BrainSettings, TurnKind, TurnOutcome};
pub use playbooks::Playbooks;
pub use prompts::{CorePrompts, Prompts};
pub use quota::{QuotaLedger, SharedQuota};
pub use report::RunReport;

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a std mutex, recovering the data if a panicking thread poisoned it.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
