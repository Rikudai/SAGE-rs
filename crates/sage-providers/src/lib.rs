//! sage-providers — clientes de modelos (Gemini, pool OpenAI-compatível), agentes CLI
//! (`claude`, `codex`, `agy`) e o `Hub: ModelHub`. Ver docs/ARCHITECTURE.md.

pub mod agents;
pub mod cooldown;
pub mod gemini;
pub mod hub;
pub mod openai;
pub mod sse;
pub mod util;

pub use agents::{AgyCli, ClaudeCli, CodexCli};
pub use cooldown::Cooldowns;
pub use gemini::{GeminiClient, GeminiTranscriber};
pub use hub::Hub;
pub use openai::OpenAiCompatClient;
