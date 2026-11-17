//! PC-control tool contract. `sage-tools` implements a Toolbox; API-model workers of
//! the brain call it through a JSON action loop (no native function calling needed).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    /// One line, Portuguese, shown to the model.
    pub description: String,
    /// JSON object describing arguments, e.g. {"x":"int","y":"int"}.
    pub args: Value,
    /// True when the tool changes something (files, input, processes). Read-only
    /// workers only get tools where this is false.
    pub mutates: bool,
    /// True when the tool needs a user confirmation before running (power, kill).
    pub needs_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolResult {
    pub ok: bool,
    /// Short sentence that can be spoken.
    pub spoken: String,
    /// Full text for the model / chat panel (stdout, file content...).
    pub details: String,
    #[serde(default)]
    pub data: Value,
    /// PNG bytes when the tool captured the screen.
    #[serde(skip)]
    pub image: Option<Vec<u8>>,
}

impl ToolResult {
    pub fn ok(spoken: impl Into<String>, details: impl Into<String>) -> Self {
        Self { ok: true, spoken: spoken.into(), details: details.into(), ..Default::default() }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self { ok: false, spoken: m.clone(), details: m, ..Default::default() }
    }
}

#[async_trait]
pub trait Toolbox: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    async fn call(&self, name: &str, args: Value) -> ToolResult;
}
