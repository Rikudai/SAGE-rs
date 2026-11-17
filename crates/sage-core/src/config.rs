//! `config/config.json`: the same file the Python S.A.G.E. used, so API keys, model
//! choices and voice settings carry over unchanged. Typed fields cover what the Rust
//! port reads; every other key is kept in `extra` and written back untouched.
//!
//! Never log a whole Config: it holds API keys. Use [`Config::redacted`].

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

fn t() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelRef {
    /// Provider id: "gemini", "groq", "cerebras", "openrouter", "mistral", "nvidia",
    /// "sambanova", "ollama", or a CLI agent: "claude", "codex", "agy".
    pub provider: String,
    pub model: String,
    /// Optional effort / thinking level ("low", "medium", "high").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), model: model.into(), effort: None }
    }
    pub fn label(&self) -> String {
        match &self.effort {
            Some(e) => format!("{}:{}@{}", self.provider, self.model, e),
            None => format!("{}:{}", self.provider, self.model),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelChain {
    #[serde(default)]
    pub brain: ModelRef,
    #[serde(default)]
    pub fallbacks: Vec<ModelRef>,
    #[serde(default)]
    pub fast: ModelRef,
}

/// One OpenAI-compatible cloud provider block from `cloud_providers`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CloudProvider {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "t")]
    pub enabled: bool,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // ---- identity
    #[serde(default)]
    pub user_name: Option<String>,

    // ---- voice
    #[serde(default = "t")]
    pub voice_enabled: bool,
    #[serde(default = "t")]
    pub speak_greetings: bool,
    #[serde(default)]
    pub pause_media_while_speaking: bool,
    #[serde(default)]
    pub tts_provider: String,
    #[serde(default)]
    pub fish_audio_api_key: String,
    #[serde(default)]
    pub fish_audio_reference_id: String,
    #[serde(default)]
    pub fish_audio_model: String,
    #[serde(default)]
    pub voice_name: String,
    #[serde(default)]
    pub voice_rate: String,
    #[serde(default)]
    pub voice_volume: String,
    #[serde(default)]
    pub voice_pitch: String,
    #[serde(default)]
    pub voice_profile: String,
    #[serde(default)]
    pub hotkey_listen: String,
    #[serde(default)]
    pub hotkey_panel: String,
    #[serde(default)]
    pub wake_words: Vec<String>,
    #[serde(default = "t")]
    pub play_audio_cues: bool,
    #[serde(default)]
    pub max_speech_sentences: Option<u32>,
    #[serde(default)]
    pub transcribe_models: Vec<String>,

    // ---- models
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default)]
    pub gemini_model: String,
    #[serde(default)]
    pub vision_model: String,
    #[serde(default)]
    pub code_model: String,
    #[serde(default)]
    pub balanced_model: String,
    #[serde(default)]
    pub brain_thinking: String,
    #[serde(default)]
    pub model_config: ModelChain,
    #[serde(default)]
    pub cloud_providers: BTreeMap<String, CloudProvider>,
    #[serde(default)]
    pub model_timeout_seconds: Option<f64>,

    // ---- CLI agents
    #[serde(default = "t")]
    pub claude_enabled: bool,
    #[serde(default)]
    pub claude_model: String,
    #[serde(default)]
    pub claude_agent_model: String,
    #[serde(default)]
    pub claude_brain_model: String,
    #[serde(default)]
    pub claude_agent_dirs: Vec<String>,
    #[serde(default = "t")]
    pub antigravity_enabled: bool,
    #[serde(default)]
    pub agy_path: String,
    #[serde(default)]
    pub antigravity_model: String,
    #[serde(default)]
    pub cli_timeout_seconds: Option<u64>,

    // ---- PC control
    #[serde(default = "t")]
    pub enable_actions: bool,
    #[serde(default = "t")]
    pub enable_shell_tools: bool,
    #[serde(default)]
    pub shell_timeout_seconds: Option<u64>,

    // ---- pet
    #[serde(default = "t")]
    pub enable_desktop_pet: bool,
    #[serde(default)]
    pub pet_form: String,
    #[serde(default = "t")]
    pub enable_pet_walking: bool,
    #[serde(default)]
    pub pet_leash_radius: Option<f32>,
    #[serde(default)]
    pub pet_scale: Option<f32>,
    #[serde(default)]
    pub pet_opacity: Option<f32>,
    #[serde(default)]
    pub pet_sleep_after_seconds: Option<u64>,
    #[serde(default)]
    pub pet_personality: String,
    #[serde(default)]
    pub pet_activity: Option<f32>,
    #[serde(default = "t")]
    pub pet_free_will: bool,
    #[serde(default = "t")]
    pub enable_sfx: bool,

    // ---- brain (new in the Rust port)
    /// Active profile of config/playbooks.yaml: "base", "savings" or "aggressive".
    #[serde(default)]
    pub brain_profile: Option<String>,
    /// Hard caps for one orchestration; None = playbook defaults.
    #[serde(default)]
    pub brain_max_workers: Option<u32>,
    #[serde(default)]
    pub brain_max_cost_usd: Option<f64>,

    /// Every other key of the old config.json, preserved on save.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        serde_json::from_str("{}").expect("empty config deserializes")
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(raw.trim_start_matches('\u{feff}'))?)
    }

    pub fn load_default() -> anyhow::Result<Self> {
        Self::load(&crate::paths::config_file())
    }

    /// Atomic save: write a temp file, keep a `.bak`, then rename.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        if path.exists() {
            let _ = std::fs::copy(path, path.with_extension("json.bak"));
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// API key for a provider id ("gemini" or a `cloud_providers` entry), or env var.
    pub fn api_key(&self, provider: &str) -> Option<String> {
        let from_cfg = match provider {
            "gemini" if !self.gemini_api_key.is_empty() => Some(self.gemini_api_key.clone()),
            p => self.cloud_providers.get(p).map(|c| c.api_key.clone()).filter(|k| !k.is_empty()),
        };
        from_cfg.or_else(|| {
            let env = match provider {
                "gemini" => "GEMINI_API_KEY",
                "groq" => "GROQ_API_KEY",
                "openrouter" => "OPENROUTER_API_KEY",
                "cerebras" => "CEREBRAS_API_KEY",
                "mistral" => "MISTRAL_API_KEY",
                "nvidia" => "NVIDIA_API_KEY",
                "sambanova" => "SAMBANOVA_API_KEY",
                _ => return None,
            };
            std::env::var(env).ok().filter(|k| !k.is_empty())
        })
    }

    /// JSON copy with every secret replaced, safe for logs and worker briefs.
    pub fn redacted(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        redact(&mut v);
        v
    }
}

fn redact(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let lk = k.to_lowercase();
                if (lk.contains("key") || lk.contains("token") || lk.contains("secret"))
                    && !lk.starts_with("hotkey")
                    && val.is_string()
                {
                    *val = Value::String("<redacted>".into());
                } else {
                    redact(val);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_unknown_keys() {
        let raw = r#"{"gemini_api_key":"x","swarm_max_tasks":12,"cloud_providers":{"groq":{"api_key":"g","model":"m","enabled":true,"weird":1}}}"#;
        let c: Config = serde_json::from_str(raw).unwrap();
        assert_eq!(c.api_key("groq").as_deref(), Some("g"));
        let back = serde_json::to_value(&c).unwrap();
        assert_eq!(back["swarm_max_tasks"], 12);
        assert_eq!(back["cloud_providers"]["groq"]["weird"], 1);
        assert_eq!(c.redacted()["gemini_api_key"], "<redacted>");
    }
}

#[cfg(test)]
mod real_file {
    #[test]
    fn parses_the_real_config_when_present() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/config.json");
        if p.exists() {
            let c = super::Config::load(&p).expect("config.json parses");
            assert!(!c.model_config.brain.model.is_empty());
        }
    }
}
