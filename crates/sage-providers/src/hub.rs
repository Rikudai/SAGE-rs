//! `Hub: ModelHub` — fábrica de clientes a partir do `Config`, com cooldown de cota.

use crate::agents::{AgyCli, ClaudeCli, CodexCli, hint_to_secs};
use crate::cooldown::Cooldowns;
use crate::gemini::{GeminiClient, GeminiTranscriber};
use crate::openai::{OpenAiCompatClient, COMPAT_PROVIDERS};
use async_trait::async_trait;
use sage_core::Config;
use sage_core::llm::{
    AgentCli, AgentOutcome, AgentTask, ChatRequest, ChatResponse, DeltaSink, FailureKind, Llm, LlmError,
    ModelHub, ModelRef, Transcriber, WorkerStatus,
};
use std::sync::Arc;

/// Descanso depois de falha de autenticação (30 min, como no SAGE antigo).
const AUTH_COOLDOWN_SECS: u64 = 30 * 60;
/// Descanso de um agente CLI quando a dica de reinício não pôde ser lida.
const AGENT_DEFAULT_COOLDOWN_SECS: u64 = 30 * 60;

pub struct Hub {
    // Contém chaves de API: nunca logar.
    cfg: Config,
    http: reqwest::Client,
    cooldowns: Arc<Cooldowns>,
    claude: Arc<dyn AgentCli>,
    codex: Arc<dyn AgentCli>,
    agy: Arc<dyn AgentCli>,
}

impl Hub {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        Self::with_cooldowns(cfg, Arc::new(Cooldowns::new()))
    }

    /// Reaproveita o mapa de cooldown ao reconstruir o hub depois de mudar as configurações.
    pub fn with_cooldowns(cfg: &Config, cooldowns: Arc<Cooldowns>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().build()?;
        Ok(Self {
            cfg: cfg.clone(),
            http,
            cooldowns,
            claude: Arc::new(ClaudeCli::from_config(cfg)),
            codex: Arc::new(CodexCli::from_config(cfg)),
            agy: Arc::new(AgyCli::from_config(cfg)),
        })
    }

    pub fn cooldowns(&self) -> Arc<Cooldowns> {
        self.cooldowns.clone()
    }

    fn raw_agent(&self, provider: &str) -> Option<&Arc<dyn AgentCli>> {
        match provider {
            "claude" => Some(&self.claude),
            "codex" => Some(&self.codex),
            "agy" | "antigravity" => Some(&self.agy),
            _ => None,
        }
    }

    fn build_llm(&self, m: &ModelRef) -> Option<Arc<dyn Llm>> {
        if m.provider == "gemini" {
            let c = GeminiClient::from_config(&self.cfg, self.http.clone(), m.clone())?;
            return Some(Arc::new(c));
        }
        let c = OpenAiCompatClient::from_config(&self.cfg, self.http.clone(), m.clone())?;
        Some(Arc::new(c))
    }

    /// Provedores OpenAI-compatíveis conhecidos mais os extras com `base_url` própria.
    fn compat_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = COMPAT_PROVIDERS.iter().map(|s| s.to_string()).collect();
        for (name, block) in &self.cfg.cloud_providers {
            if name != "gemini" && block.base_url.is_some() && !ids.contains(name) {
                ids.push(name.clone());
            }
        }
        ids
    }
}

fn cooldown_key(provider: &str, model: &str) -> String {
    // A cota do Gemini é por modelo; nos demais, por provedor.
    if provider == "gemini" { format!("gemini:{model}") } else { provider.to_string() }
}

impl ModelHub for Hub {
    fn llm(&self, m: &ModelRef) -> Option<Arc<dyn Llm>> {
        let mut m = m.clone();
        if m.model.is_empty() {
            m.model = match m.provider.as_str() {
                "gemini" => self.cfg.gemini_model.clone(),
                p => self.cfg.cloud_providers.get(p).map(|b| b.model.clone()).unwrap_or_default(),
            };
        }
        let inner = self.build_llm(&m)?;
        Some(Arc::new(GuardedLlm {
            key: cooldown_key(&m.provider, &m.model),
            inner,
            cooldowns: self.cooldowns.clone(),
        }))
    }

    fn agent(&self, provider: &str) -> Option<Arc<dyn AgentCli>> {
        let inner = self.raw_agent(provider)?;
        if !inner.installed() {
            return None;
        }
        Some(Arc::new(GuardedAgent { inner: inner.clone(), cooldowns: self.cooldowns.clone() }))
    }

    fn available(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.cfg.api_key("gemini").is_some() {
            out.push("gemini".to_string());
        }
        for id in self.compat_ids() {
            let usable = OpenAiCompatClient::from_config(&self.cfg, self.http.clone(), ModelRef::new(&id, ""))
                .is_some();
            if usable && !self.cooldowns.is_cooling(&id) {
                out.push(id);
            }
        }
        for id in ["claude", "codex", "agy"] {
            let installed = self.raw_agent(id).is_some_and(|a| a.installed());
            if installed && !self.cooldowns.is_cooling(id) {
                out.push(id.to_string());
            }
        }
        out
    }

    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        let t = GeminiTranscriber::from_config(&self.cfg, self.http.clone())?;
        Some(Arc::new(t))
    }
}

// ------------------------------------------------------------------ guardas de cooldown

struct GuardedLlm {
    key: String,
    inner: Arc<dyn Llm>,
    cooldowns: Arc<Cooldowns>,
}

#[async_trait]
impl Llm for GuardedLlm {
    fn model(&self) -> ModelRef {
        self.inner.model()
    }

    async fn chat(&self, req: ChatRequest, sink: Option<DeltaSink>) -> Result<ChatResponse, LlmError> {
        if let Some(rem) = self.cooldowns.remaining(&self.key) {
            return Err(LlmError::RateLimited {
                provider: self.inner.model().provider,
                detail: "provedor em descanso por limite de cota".into(),
                retry_after_secs: Some(rem),
            });
        }
        let r = self.inner.chat(req, sink).await;
        match &r {
            Err(LlmError::RateLimited { retry_after_secs, .. }) => self.cooldowns.set(&self.key, *retry_after_secs),
            Err(LlmError::Auth { .. }) => self.cooldowns.set(&self.key, Some(AUTH_COOLDOWN_SECS)),
            _ => {}
        }
        r
    }
}

struct GuardedAgent {
    inner: Arc<dyn AgentCli>,
    cooldowns: Arc<Cooldowns>,
}

#[async_trait]
impl AgentCli for GuardedAgent {
    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    fn installed(&self) -> bool {
        self.inner.installed()
    }

    async fn run(&self, task: AgentTask, sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome> {
        let key = self.inner.provider();
        if let Some(rem) = self.cooldowns.remaining(key) {
            return Ok(AgentOutcome {
                status: WorkerStatus::Failed,
                summary: format!("{key} em descanso por limite de uso (mais ~{rem}s)."),
                text: String::new(),
                usage: Default::default(),
                session_id: None,
                failure: Some(FailureKind::RateLimited),
                reset_hint: Some(format!("em {rem}s")),
                duration_secs: 0.0,
            });
        }
        let out = self.inner.run(task, sink).await?;
        match out.failure {
            Some(FailureKind::RateLimited) => {
                let secs = out.reset_hint.as_deref().and_then(hint_to_secs).unwrap_or(AGENT_DEFAULT_COOLDOWN_SECS);
                self.cooldowns.set(key, Some(secs));
            }
            Some(FailureKind::AuthRequired) => self.cooldowns.set(key, Some(AUTH_COOLDOWN_SECS)),
            _ => {}
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        serde_json::from_str(
            r#"{
                "gemini_api_key": "chave-de-teste",
                "gemini_model": "gemini-3.1-flash-lite",
                "transcribe_models": ["gemini-3.5-transcribe"],
                "claude_enabled": false,
                "antigravity_enabled": false,
                "cloud_providers": {
                    "groq": {"api_key": "k", "model": "openai/gpt-oss-120b", "enabled": true},
                    "mistral": {"api_key": "k", "model": "m", "enabled": false},
                    "ollama": {"model": "qwen", "enabled": true}
                }
            }"#,
        )
        .expect("config de teste")
    }

    #[test]
    fn builds_clients_by_provider() {
        let hub = Hub::new(&cfg()).expect("hub");
        assert!(hub.llm(&ModelRef::new("gemini", "gemini-3.6-flash")).is_some());
        let g = hub.llm(&ModelRef::new("groq", "")).expect("groq");
        assert_eq!(g.model().model, "openai/gpt-oss-120b");
        assert!(hub.llm(&ModelRef::new("mistral", "m")).is_none(), "desligado");
        assert!(hub.llm(&ModelRef::new("apinex", "m")).is_none(), "sem URL conhecida");
        assert!(hub.llm(&ModelRef::new("ollama", "qwen")).is_some(), "ollama não precisa de chave");
        assert!(hub.llm(&ModelRef::new("claude", "x")).is_none());
        assert!(hub.transcriber().is_some());
        assert!(hub.agent("claude").is_none(), "desligado no config");
        assert!(hub.agent("nada").is_none());
    }

    #[test]
    fn available_respects_flags_and_cooldown() {
        let hub = Hub::new(&cfg()).expect("hub");
        let a = hub.available();
        assert!(a.contains(&"gemini".to_string()) && a.contains(&"groq".to_string()));
        assert!(a.contains(&"ollama".to_string()));
        assert!(!a.contains(&"mistral".to_string()) && !a.contains(&"claude".to_string()));
        hub.cooldowns().set("groq", Some(60));
        assert!(!hub.available().contains(&"groq".to_string()));
    }

    #[tokio::test]
    async fn cooled_down_llm_fails_fast() {
        let hub = Hub::new(&cfg()).expect("hub");
        let groq = hub.llm(&ModelRef::new("groq", "")).expect("groq");
        hub.cooldowns().set("groq", Some(60));
        let err = groq.chat(ChatRequest::default(), None).await.expect_err("em descanso");
        match err {
            LlmError::RateLimited { retry_after_secs, .. } => assert!(retry_after_secs.is_some_and(|s| s > 0)),
            other => panic!("esperava RateLimited, veio {other:?}"),
        }
    }
}
