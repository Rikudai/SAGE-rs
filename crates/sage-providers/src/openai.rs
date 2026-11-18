//! Pool OpenAI-compatível: Groq, Cerebras, OpenRouter, Mistral, NVIDIA, SambaNova, Ollama.

use crate::sse::SseParser;
use crate::util::{http_error, timeout_from, transport_error};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures::StreamExt;
use sage_core::Config;
use sage_core::llm::{ChatRequest, ChatResponse, DeltaSink, Llm, LlmError, ModelRef, Role, Usage};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// Provedores atendidos por este cliente (ids de `cloud_providers`).
pub const COMPAT_PROVIDERS: &[&str] =
    &["groq", "cerebras", "openrouter", "mistral", "nvidia", "sambanova", "ollama"];

pub fn is_compat_provider(p: &str) -> bool {
    COMPAT_PROVIDERS.contains(&p)
}

pub fn default_base_url(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "groq" => "https://api.groq.com/openai/v1",
        "cerebras" => "https://api.cerebras.ai/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "mistral" => "https://api.mistral.ai/v1",
        "nvidia" => "https://integrate.api.nvidia.com/v1",
        "sambanova" => "https://api.sambanova.ai/v1",
        "ollama" => "http://localhost:11434/v1",
        _ => return None,
    })
}

pub struct OpenAiCompatClient {
    http: reqwest::Client,
    provider: String,
    base_url: String,
    key: Option<String>,
    model: ModelRef,
    timeout: Duration,
}

impl OpenAiCompatClient {
    pub fn new(
        http: reqwest::Client,
        provider: &str,
        base_url: &str,
        key: Option<String>,
        model: ModelRef,
        timeout: Duration,
    ) -> Self {
        Self {
            http,
            provider: provider.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
            model,
            timeout,
        }
    }

    /// Constrói a partir do Config; None se desligado, sem chave (exceto Ollama) ou sem URL.
    pub fn from_config(cfg: &Config, http: reqwest::Client, model: ModelRef) -> Option<Self> {
        let provider = model.provider.as_str();
        let block = cfg.cloud_providers.get(provider);
        if block.is_some_and(|b| !b.enabled) {
            return None;
        }
        let base = block
            .and_then(|b| b.base_url.clone())
            .filter(|u| !u.trim().is_empty())
            .or_else(|| default_base_url(provider).map(str::to_string))?;
        let key = cfg.api_key(provider);
        if key.is_none() && provider != "ollama" {
            return None;
        }
        let mut model = model;
        if model.model.is_empty() {
            model.model = block.map(|b| b.model.clone()).unwrap_or_default();
        }
        let provider = model.provider.clone();
        Some(Self::new(http, &provider, &base, key, model, timeout_from(cfg)))
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut rb = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .timeout(self.timeout)
            .json(body);
        if let Some(k) = &self.key {
            rb = rb.bearer_auth(k);
        }
        if self.provider == "openrouter" {
            rb = rb.header("HTTP-Referer", "https://github.com/sage").header("X-Title", "S.A.G.E.");
        }
        rb
    }

    async fn send(&self, body: &Value) -> Result<reqwest::Response, LlmError> {
        let resp = self.request(body).send().await.map_err(|e| transport_error(&self.provider, e, self.timeout))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let retry = resp.headers().get("retry-after").and_then(|h| h.to_str().ok()).map(str::to_string);
        let text = resp.text().await.unwrap_or_default();
        Err(http_error(&self.provider, status, retry.as_deref(), &text))
    }
}

/// Corpo `chat/completions`.
pub(crate) fn build_body(provider: &str, model: &ModelRef, req: &ChatRequest, stream: bool, json_mode: bool) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": req.system}));
    }
    for m in &req.messages {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        if m.images.is_empty() {
            messages.push(json!({"role": role, "content": m.text}));
        } else {
            let mut parts = vec![json!({"type": "text", "text": m.text})];
            for img in &m.images {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:image/png;base64,{}", B64.encode(img))}
                }));
            }
            messages.push(json!({"role": role, "content": parts}));
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(model.model));
    body.insert("messages".into(), Value::Array(messages));
    if let Some(t) = req.temperature {
        body.insert("temperature".into(), json!(t));
    }
    if let Some(n) = req.max_tokens {
        body.insert("max_tokens".into(), json!(n));
    }
    if json_mode {
        body.insert("response_format".into(), json!({"type": "json_object"}));
    }
    let effort = req.thinking.as_deref().or(model.effort.as_deref());
    let supports_effort = matches!(provider, "groq" | "cerebras") && model.model.contains("gpt-oss");
    if let Some(e) = effort.filter(|_| supports_effort) {
        body.insert("reasoning_effort".into(), json!(e));
    }
    if stream {
        body.insert("stream".into(), json!(true));
        if provider != "mistral" {
            body.insert("stream_options".into(), json!({"include_usage": true}));
        }
    }
    Value::Object(body)
}

/// `content` pode ser string ou lista de partes `{text}` (Mistral).
fn content_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let s: String = items.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
            Some(s)
        }
        _ => None,
    }
}

fn reasoning_text(obj: &Value) -> Option<String> {
    ["reasoning_content", "reasoning", "reasoning_text"]
        .iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .map(str::to_string)
}

fn parse_usage(u: &Value) -> Usage {
    let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: n(u, "prompt_tokens"),
        output_tokens: n(u, "completion_tokens"),
        cache_read_tokens: u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64).unwrap_or(0),
        cost_usd: None,
    }
}

#[derive(Default, Debug)]
pub(crate) struct OpenAiAcc {
    pub text: String,
    pub thoughts: String,
    pub usage: Usage,
}

impl OpenAiAcc {
    /// Uma resposta completa (`message`) ou um chunk de stream (`delta`).
    pub fn apply(&mut self, v: &Value, sink: Option<&DeltaSink>) {
        if let Some(choice) = v.pointer("/choices/0") {
            let node = choice.get("delta").or_else(|| choice.get("message"));
            if let Some(node) = node {
                if let Some(r) = reasoning_text(node) {
                    self.thoughts.push_str(&r);
                    if let Some(s) = sink {
                        s(&r, true);
                    }
                }
                if let Some(t) = node.get("content").and_then(content_text).filter(|t| !t.is_empty()) {
                    self.text.push_str(&t);
                    if let Some(s) = sink {
                        s(&t, false);
                    }
                }
            }
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = parse_usage(u);
        } else if let Some(u) = v.pointer("/x_groq/usage") {
            self.usage = parse_usage(u);
        }
    }
}

fn should_retry_without_json(e: &LlmError) -> bool {
    matches!(e, LlmError::Provider { detail, .. } if detail.starts_with("HTTP 400") || detail.starts_with("HTTP 422"))
}

#[async_trait]
impl Llm for OpenAiCompatClient {
    fn model(&self) -> ModelRef {
        self.model.clone()
    }

    async fn chat(&self, req: ChatRequest, sink: Option<DeltaSink>) -> Result<ChatResponse, LlmError> {
        let stream = sink.is_some();
        let mut json_mode = req.json;
        let resp = loop {
            let body = build_body(&self.provider, &self.model, &req, stream, json_mode);
            match self.send(&body).await {
                Ok(r) => break r,
                Err(e) if json_mode && should_retry_without_json(&e) => {
                    tracing::debug!("{}: 400/422 com response_format; repetindo sem", self.provider);
                    json_mode = false;
                }
                Err(e) => return Err(e),
            }
        };
        let mut acc = OpenAiAcc::default();
        match sink {
            None => {
                let v: Value = resp.json().await.map_err(|e| transport_error(&self.provider, e, self.timeout))?;
                acc.apply(&v, None);
            }
            Some(sink) => {
                let mut bytes = resp.bytes_stream();
                let mut sse = SseParser::new();
                'outer: while let Some(chunk) = bytes.next().await {
                    let chunk = chunk.map_err(|e| transport_error(&self.provider, e, self.timeout))?;
                    for data in sse.feed(&chunk) {
                        if data.trim() == "[DONE]" {
                            break 'outer;
                        }
                        match serde_json::from_str::<Value>(&data) {
                            Ok(v) => acc.apply(&v, Some(&sink)),
                            Err(e) => tracing::debug!("{}: chunk SSE inválido: {e}", self.provider),
                        }
                    }
                }
            }
        }
        if acc.text.trim().is_empty() {
            return Err(LlmError::Provider {
                provider: self.provider.clone(),
                detail: "resposta vazia".into(),
            });
        }
        Ok(ChatResponse { text: acc.text, thoughts: acc.thoughts, usage: acc.usage, model: self.model.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::Message;

    fn req() -> ChatRequest {
        ChatRequest {
            system: "sys".into(),
            messages: vec![Message::user("oi")],
            temperature: Some(0.5),
            max_tokens: Some(64),
            json: true,
            thinking: Some("low".into()),
        }
    }

    #[test]
    fn urls() {
        assert_eq!(default_base_url("groq"), Some("https://api.groq.com/openai/v1"));
        assert_eq!(default_base_url("ollama"), Some("http://localhost:11434/v1"));
        assert_eq!(default_base_url("xyz"), None);
        assert!(is_compat_provider("sambanova") && !is_compat_provider("gemini"));
    }

    #[test]
    fn body_json_stream_and_effort() {
        let m = ModelRef::new("groq", "openai/gpt-oss-120b");
        let b = build_body("groq", &m, &req(), true, true);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "oi");
        assert_eq!(b["response_format"]["type"], "json_object");
        assert_eq!(b["reasoning_effort"], "low");
        assert_eq!(b["stream"], true);
        assert_eq!(b["stream_options"]["include_usage"], true);
        let b2 = build_body("mistral", &ModelRef::new("mistral", "m"), &req(), true, false);
        assert!(b2.get("response_format").is_none() && b2.get("stream_options").is_none());
        assert!(b2.get("reasoning_effort").is_none());
    }

    #[test]
    fn body_with_image() {
        let mut r = req();
        r.messages[0].images.push(vec![1, 2, 3]);
        let b = build_body("openrouter", &ModelRef::new("openrouter", "x"), &r, false, false);
        assert_eq!(b["messages"][1]["content"][1]["image_url"]["url"], "data:image/png;base64,AQID");
    }

    #[test]
    fn parses_stream_with_reasoning_and_usage() {
        let chunks = [
            r#"{"choices":[{"delta":{"reasoning":"hmm "}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"ok"}}]}"#,
            r#"{"choices":[{"delta":{"content":"Olá"}}]}"#,
            r#"{"choices":[{"delta":{"content":"!"}}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":2}}}"#,
        ];
        let mut acc = OpenAiAcc::default();
        for c in chunks {
            acc.apply(&serde_json::from_str::<Value>(c).expect("fixture"), None);
        }
        assert_eq!(acc.text, "Olá!");
        assert_eq!(acc.thoughts, "hmm ok");
        assert_eq!(acc.usage, Usage { input_tokens: 7, output_tokens: 3, cache_read_tokens: 2, cost_usd: None });
    }

    #[test]
    fn parses_full_message_and_parts_content() {
        let mut acc = OpenAiAcc::default();
        acc.apply(&json!({"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}), None);
        assert_eq!(acc.text, "ab");
    }

    #[test]
    fn retry_rule_for_json_mode() {
        let e400 = LlmError::Provider { provider: "x".into(), detail: "HTTP 400: bad".into() };
        let e500 = LlmError::Provider { provider: "x".into(), detail: "HTTP 500: bad".into() };
        assert!(should_retry_without_json(&e400));
        assert!(!should_retry_without_json(&e500));
        assert!(!should_retry_without_json(&LlmError::Auth { provider: "x".into() }));
    }
}
