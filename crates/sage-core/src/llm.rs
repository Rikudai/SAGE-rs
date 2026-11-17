//! Model and agent contracts. `sage-providers` implements them; `sage-brain` uses them.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::config::ModelRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Cheap,
    Standard,
    Strong,
}

impl Tier {
    pub fn up(self) -> Option<Tier> {
        match self {
            Tier::Cheap => Some(Tier::Standard),
            Tier::Standard => Some(Tier::Strong),
            Tier::Strong => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// PNG images attached to this message (screenshots), raw bytes.
    #[serde(skip)]
    pub images: Vec<Vec<u8>>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, text: text.into(), images: vec![] }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, text: text.into(), images: vec![] }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Ask the provider for a JSON object reply (`response_format` / `responseMimeType`).
    pub json: bool,
    /// Thinking level for models that support it ("minimal".."high").
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// Only some providers (Claude CLI) report a dollar cost.
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_tokens += o.cache_read_tokens;
        self.cost_usd = match (self.cost_usd, o.cost_usd) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
    }
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub text: String,
    pub thoughts: String,
    pub usage: Usage,
    pub model: ModelRef,
}

/// Streaming callback: `(text_delta, is_thought)`.
pub type DeltaSink = Arc<dyn Fn(&str, bool) + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("rate limited or out of quota on {provider}: {detail}")]
    RateLimited { provider: String, detail: String, retry_after_secs: Option<u64> },
    #[error("auth failed on {provider}")]
    Auth { provider: String },
    #[error("timeout after {0}s")]
    Timeout(u64),
    #[error("provider {provider} error: {detail}")]
    Provider { provider: String, detail: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl LlmError {
    /// Worth trying the next model in a chain.
    pub fn is_transient(&self) -> bool {
        !matches!(self, LlmError::Other(_))
    }
}

/// A chat model reached over an HTTP API (Gemini, Groq, ...).
#[async_trait]
pub trait Llm: Send + Sync {
    fn model(&self) -> ModelRef;
    async fn chat(&self, req: ChatRequest, sink: Option<DeltaSink>) -> Result<ChatResponse, LlmError>;
}

/// Builds clients for model refs; the brain asks it for whatever a tier resolves to.
pub trait ModelHub: Send + Sync {
    fn llm(&self, m: &ModelRef) -> Option<Arc<dyn Llm>>;
    fn agent(&self, provider: &str) -> Option<Arc<dyn AgentCli>>;
    /// Provider ids usable right now (key present / CLI installed and enabled).
    fn available(&self) -> Vec<String>;
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>>;
}

// ---------------------------------------------------------------- CLI agents

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMode {
    ReadOnly,
    Write,
}

#[derive(Debug, Clone)]
pub struct AgentTask {
    pub id: String,
    pub prompt: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode: AgentMode,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    /// Resume an earlier session for a focused follow-up.
    pub resume_session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Done,
    DoneWithConcerns,
    Blocked,
    NeedsContext,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    RateLimited,
    AuthRequired,
    Timeout,
    PermissionDenied,
    Crashed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutcome {
    pub status: WorkerStatus,
    pub summary: String,
    pub text: String,
    pub usage: Usage,
    pub session_id: Option<String>,
    pub failure: Option<FailureKind>,
    pub reset_hint: Option<String>,
    pub duration_secs: f64,
}

/// A coding agent CLI (Claude Code, Codex, Antigravity) run as a bounded worker.
#[async_trait]
pub trait AgentCli: Send + Sync {
    fn provider(&self) -> &'static str;
    fn installed(&self) -> bool;
    async fn run(&self, task: AgentTask, sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome>;
}

/// Speech to text (WAV bytes in, transcript out).
#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, wav: Vec<u8>, language: &str) -> anyhow::Result<String>;
}
