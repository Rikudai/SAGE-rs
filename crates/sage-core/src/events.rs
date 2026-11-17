//! Event bus: the brain, voice and tools publish; the chat UI and the pet subscribe.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::llm::{Tier, Usage, WorkerStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    Idle,
    Listening,
    Transcribing,
    Thinking,
    /// Lead planning / running waves of workers.
    Orchestrating,
    Speaking,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SageEvent {
    Status { status: AppStatus, detail: String },
    /// Text the user sent (typed or transcribed).
    UserText { text: String, from_voice: bool },
    /// Final answer of a turn. `spoken` is short, `full` goes to the chat.
    Reply { spoken: String, full: String, emotion: Option<String> },
    /// Live tokens of the lead or a worker (`source` = "lead" or worker id).
    Stream { source: String, text: String, thought: bool },
    /// The lead picked a playbook and wrote a plan.
    PlanReady { run_id: String, playbook: String, why: String, units: Vec<String> },
    WorkerStart { run_id: String, worker: String, role: String, model: String, tier: Tier, brief: String },
    WorkerTool { worker: String, tool: String, args: String, ok: bool },
    WorkerEnd { run_id: String, worker: String, status: WorkerStatus, summary: String, usage: Usage },
    /// A gate or review verdict on a unit.
    Gate { run_id: String, worker: String, verdict: String, note: String },
    /// Provider moved because of quota/limits (the user is told in one line).
    ProviderSwitch { from: String, to: String, reason: String },
    RunReport { run_id: String, summary: String, usage: Usage, path: String },
    Emotion { name: String },
    Form { name: String },
    Pointer { x: i32, y: i32, label: String },
    /// Ask the user to confirm something risky; UI answers via `Command::Confirm`.
    ConfirmRequest { id: String, question: String },
    Notice { text: String },
}

/// Commands flowing into the brain (from UI, hotkeys, voice).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Ask { text: String, from_voice: bool },
    Confirm { id: String, yes: bool },
    StopRun,
    ReloadConfig,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<SageEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self { tx }
    }
    pub fn emit(&self, e: SageEvent) {
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<SageEvent> {
        self.tx.subscribe()
    }
    pub fn status(&self, status: AppStatus, detail: impl Into<String>) {
        self.emit(SageEvent::Status { status, detail: detail.into() });
    }
}
