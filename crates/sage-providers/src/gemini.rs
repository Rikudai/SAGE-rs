//! Gemini AI Studio (REST v1beta): chat com SSE, imagens, thinking e transcrição de áudio.

use crate::sse::SseParser;
use crate::util::{http_error, timeout_from, transport_error, truncate_chars};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use futures::StreamExt;
use sage_core::Config;
use sage_core::llm::{
    ChatRequest, ChatResponse, DeltaSink, Llm, LlmError, ModelRef, Role, Transcriber, Usage,
};
use serde_json::{Map, Value, json};
use std::time::Duration;

pub const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const PROVIDER: &str = "gemini";

fn is_gemma(model: &str) -> bool {
    model.to_lowercase().trim_start_matches("models/").starts_with("gemma")
}

/// Versão maior do modelo ("gemini-3.6-flash" → 3.6). None se não for `gemini-<n>`.
fn gemini_version(model: &str) -> Option<f32> {
    let m = model.to_lowercase();
    let rest = m.trim_start_matches("models/").strip_prefix("gemini-")?;
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    num.trim_end_matches('.').parse().ok()
}

/// `thinkingConfig` por geração: `thinkingLevel` (3+/4) ou `thinkingBudget` (2.5).
pub(crate) fn thinking_config(model: &str, level: Option<&str>) -> Option<Value> {
    let level = level?.trim().to_lowercase();
    if level.is_empty() {
        return None;
    }
    let v = gemini_version(model)?;
    if v >= 3.0 {
        return Some(json!({"thinkingLevel": level, "includeThoughts": true}));
    }
    if v >= 2.5 {
        let budget = match level.as_str() {
            "minimal" => 512,
            "low" => 1024,
            "medium" => 8192,
            "high" => 24576,
            _ => return None,
        };
        return Some(json!({"thinkingBudget": budget, "includeThoughts": true}));
    }
    None
}

fn text_part(t: &str) -> Value {
    json!({ "text": t })
}

/// Corpo de `generateContent` / `streamGenerateContent`.
pub(crate) fn build_request(model: &str, req: &ChatRequest, effort: Option<&str>) -> Value {
    let gemma = is_gemma(model);
    let mut system = req.system.trim().to_string();
    let mut contents: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for m in &req.messages {
        if m.role == Role::System {
            if !m.text.trim().is_empty() {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(m.text.trim());
            }
            continue;
        }
        let mut parts = Vec::new();
        if !m.text.is_empty() {
            parts.push(text_part(&m.text));
        }
        for img in &m.images {
            parts.push(json!({"inline_data": {"mime_type": "image/png", "data": B64.encode(img)}}));
        }
        if parts.is_empty() {
            continue;
        }
        let role = if m.role == Role::Assistant { "model" } else { "user" };
        contents.push((role, parts));
    }

    if gemma && !system.is_empty() {
        // Gemma não tem systemInstruction: o sistema vai antes do primeiro texto do usuário.
        match contents.iter_mut().find(|(r, _)| *r == "user") {
            Some((_, parts)) => match parts.iter_mut().find(|p| p.get("text").is_some()) {
                Some(p) => {
                    let old = p["text"].as_str().unwrap_or_default().to_string();
                    p["text"] = Value::String(format!("{system}\n\n{old}"));
                }
                None => parts.insert(0, text_part(&system)),
            },
            None => contents.insert(0, ("user", vec![text_part(&system)])),
        }
    }

    let mut gen_cfg = Map::new();
    if let Some(t) = req.temperature {
        gen_cfg.insert("temperature".into(), json!(t));
    }
    if let Some(n) = req.max_tokens {
        gen_cfg.insert("maxOutputTokens".into(), json!(n));
    }
    if req.json && !gemma {
        gen_cfg.insert("responseMimeType".into(), json!("application/json"));
    }
    if let Some(tc) = thinking_config(model, req.thinking.as_deref().or(effort)) {
        gen_cfg.insert("thinkingConfig".into(), tc);
    }

    let mut body = Map::new();
    body.insert(
        "contents".into(),
        Value::Array(contents.into_iter().map(|(r, p)| json!({"role": r, "parts": p})).collect()),
    );
    if !system.is_empty() && !gemma {
        body.insert("systemInstruction".into(), json!({"parts": [text_part(&system)]}));
    }
    if !gen_cfg.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen_cfg));
    }
    Value::Object(body)
}

/// Acumula respostas (uma só no modo normal, várias chunks no SSE).
#[derive(Default, Debug)]
pub(crate) struct GeminiAcc {
    pub text: String,
    pub thoughts: String,
    pub usage: Usage,
    pub block_reason: Option<String>,
}

impl GeminiAcc {
    pub fn apply(&mut self, v: &Value, sink: Option<&DeltaSink>) {
        if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(Value::as_array) {
            for p in parts {
                let Some(t) = p.get("text").and_then(Value::as_str) else { continue };
                if t.is_empty() {
                    continue;
                }
                let thought = p.get("thought").and_then(Value::as_bool).unwrap_or(false);
                if thought {
                    self.thoughts.push_str(t);
                } else {
                    self.text.push_str(t);
                }
                if let Some(s) = sink {
                    s(t, thought);
                }
            }
        }
        if let Some(r) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
            self.block_reason = Some(r.to_string());
        }
        if let Some(u) = v.get("usageMetadata") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            self.usage = Usage {
                input_tokens: n("promptTokenCount"),
                output_tokens: n("candidatesTokenCount") + n("thoughtsTokenCount"),
                cache_read_tokens: n("cachedContentTokenCount"),
                cost_usd: None,
            };
        }
    }

    fn into_response(self, model: ModelRef) -> Result<ChatResponse, LlmError> {
        if self.text.trim().is_empty() {
            let detail = match self.block_reason {
                Some(r) => format!("pedido bloqueado pelo Gemini ({r})"),
                None => "resposta vazia".to_string(),
            };
            return Err(LlmError::Provider { provider: PROVIDER.into(), detail });
        }
        Ok(ChatResponse { text: self.text, thoughts: self.thoughts, usage: self.usage, model })
    }
}

pub struct GeminiClient {
    http: reqwest::Client,
    key: String,
    model: ModelRef,
    base: String,
    timeout: Duration,
}

impl GeminiClient {
    pub fn new(http: reqwest::Client, key: String, model: ModelRef, timeout: Duration) -> Self {
        Self { http, key, model, base: BASE_URL.to_string(), timeout }
    }

    /// Constrói a partir do Config (None se não houver chave).
    pub fn from_config(cfg: &Config, http: reqwest::Client, model: ModelRef) -> Option<Self> {
        let key = cfg.api_key(PROVIDER).or_else(|| {
            std::env::var("GOOGLE_API_KEY").ok().filter(|k| !k.is_empty())
        })?;
        Some(Self::new(http, key, model, timeout_from(cfg)))
    }

    fn url(&self, stream: bool) -> String {
        let m = self.model.model.trim_start_matches("models/");
        if stream {
            format!("{}/models/{m}:streamGenerateContent?alt=sse", self.base)
        } else {
            format!("{}/models/{m}:generateContent", self.base)
        }
    }
}

async fn post(
    http: &reqwest::Client,
    url: &str,
    key: &str,
    body: &Value,
    timeout: Duration,
) -> Result<reqwest::Response, LlmError> {
    let resp = http
        .post(url)
        .header("x-goog-api-key", key)
        .timeout(timeout)
        .json(body)
        .send()
        .await
        .map_err(|e| transport_error(PROVIDER, e, timeout))?;
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let retry = resp
        .headers()
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let text = resp.text().await.unwrap_or_default();
    Err(http_error(PROVIDER, status, retry.as_deref(), &text))
}

#[async_trait]
impl Llm for GeminiClient {
    fn model(&self) -> ModelRef {
        self.model.clone()
    }

    async fn chat(&self, req: ChatRequest, sink: Option<DeltaSink>) -> Result<ChatResponse, LlmError> {
        let body = build_request(&self.model.model, &req, self.model.effort.as_deref());
        let mut acc = GeminiAcc::default();
        match sink {
            None => {
                let resp = post(&self.http, &self.url(false), &self.key, &body, self.timeout).await?;
                let v: Value = resp
                    .json()
                    .await
                    .map_err(|e| transport_error(PROVIDER, e, self.timeout))?;
                acc.apply(&v, None);
            }
            Some(sink) => {
                let resp = post(&self.http, &self.url(true), &self.key, &body, self.timeout).await?;
                let mut stream = resp.bytes_stream();
                let mut sse = SseParser::new();
                while let Some(chunk) = stream.next().await {
                    let bytes = chunk.map_err(|e| transport_error(PROVIDER, e, self.timeout))?;
                    for data in sse.feed(&bytes) {
                        match serde_json::from_str::<Value>(&data) {
                            Ok(v) => acc.apply(&v, Some(&sink)),
                            Err(e) => tracing::debug!("gemini: chunk SSE inválido: {e}"),
                        }
                    }
                }
                for data in sse.finish() {
                    if let Ok(v) = serde_json::from_str::<Value>(&data) {
                        acc.apply(&v, Some(&sink));
                    }
                }
            }
        }
        acc.into_response(self.model.clone())
    }
}

// ------------------------------------------------------------------ transcrição

pub struct GeminiTranscriber {
    http: reqwest::Client,
    key: String,
    models: Vec<String>,
    timeout: Duration,
}

impl GeminiTranscriber {
    pub fn new(http: reqwest::Client, key: String, models: Vec<String>, timeout: Duration) -> Self {
        Self { http, key, models, timeout }
    }

    /// Usa `transcribe_models` na ordem; sem lista, cai em `gemini_model`.
    pub fn from_config(cfg: &Config, http: reqwest::Client) -> Option<Self> {
        let key = cfg.api_key(PROVIDER)?;
        let mut models: Vec<String> =
            cfg.transcribe_models.iter().filter(|m| !m.trim().is_empty()).cloned().collect();
        if models.is_empty() && !cfg.gemini_model.is_empty() {
            models.push(cfg.gemini_model.clone());
        }
        if models.is_empty() {
            return None;
        }
        Some(Self::new(http, key, models, timeout_from(cfg)))
    }
}

pub(crate) fn transcribe_instruction(language: &str) -> String {
    let lang = if language.trim().is_empty() { "pt-BR" } else { language.trim() };
    format!(
        "Transcreva fielmente o áudio a seguir (idioma: {lang}). Responda somente com o texto \
         falado, sem comentários, aspas ou marcações. Se não houver fala, responda vazio."
    )
}

pub(crate) fn build_transcribe_request(wav: &[u8], language: &str) -> Value {
    json!({
        "contents": [{
            "role": "user",
            "parts": [
                {"text": transcribe_instruction(language)},
                {"inline_data": {"mime_type": "audio/wav", "data": B64.encode(wav)}}
            ]
        }],
        "generationConfig": {"temperature": 0.0}
    })
}

#[async_trait]
impl Transcriber for GeminiTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, language: &str) -> anyhow::Result<String> {
        let body = build_transcribe_request(&wav, language);
        let mut last: Option<anyhow::Error> = None;
        for model in &self.models {
            let url = format!("{BASE_URL}/models/{}:generateContent", model.trim_start_matches("models/"));
            let attempt: Result<String, LlmError> = async {
                let resp = post(&self.http, &url, &self.key, &body, self.timeout).await?;
                let v: Value = resp.json().await.map_err(|e| transport_error(PROVIDER, e, self.timeout))?;
                let mut acc = GeminiAcc::default();
                acc.apply(&v, None);
                Ok(acc.text.trim().to_string())
            }
            .await;
            match attempt {
                Ok(t) => return Ok(t),
                Err(e) => {
                    tracing::warn!("transcrição com {model} falhou: {}", truncate_chars(&e.to_string(), 200));
                    last = Some(anyhow!(e));
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("nenhum modelo de transcrição configurado")))
            .context("falha na transcrição de áudio")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::Message;
    use std::sync::{Arc, Mutex};

    fn req(system: &str, text: &str) -> ChatRequest {
        ChatRequest { system: system.into(), messages: vec![Message::user(text)], ..Default::default() }
    }

    #[test]
    fn thinking_by_version() {
        assert_eq!(
            thinking_config("gemini-3.6-flash", Some("medium")),
            Some(json!({"thinkingLevel": "medium", "includeThoughts": true}))
        );
        assert!(thinking_config("gemini-4-hydrogen", Some("low")).is_some());
        assert_eq!(
            thinking_config("gemini-2.5-flash", Some("low")),
            Some(json!({"thinkingBudget": 1024, "includeThoughts": true}))
        );
        assert!(thinking_config("gemini-2.0-flash", Some("low")).is_none());
        assert!(thinking_config("gemma-4-31b-it", Some("low")).is_none());
        assert!(thinking_config("gemini-3.6-flash", None).is_none());
    }

    #[test]
    fn request_with_system_json_and_image() {
        let mut r = req("Seja breve.", "oi");
        r.json = true;
        r.temperature = Some(0.2);
        r.max_tokens = Some(100);
        r.messages[0].images.push(vec![1, 2, 3]);
        r.messages.push(Message::assistant("olá"));
        let b = build_request("gemini-3.6-flash", &r, Some("high"));
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "Seja breve.");
        assert_eq!(b["contents"][0]["role"], "user");
        assert_eq!(b["contents"][0]["parts"][1]["inline_data"]["mime_type"], "image/png");
        assert_eq!(b["contents"][0]["parts"][1]["inline_data"]["data"], "AQID");
        assert_eq!(b["contents"][1]["role"], "model");
        assert_eq!(b["generationConfig"]["responseMimeType"], "application/json");
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 100);
        assert_eq!(b["generationConfig"]["thinkingConfig"]["thinkingLevel"], "high");
    }

    #[test]
    fn gemma_prepends_system() {
        let mut r = req("Sistema", "pergunta");
        r.json = true;
        let b = build_request("gemma-4-31b-it", &r, None);
        assert!(b.get("systemInstruction").is_none());
        assert_eq!(b["contents"][0]["parts"][0]["text"], "Sistema\n\npergunta");
        assert!(b.get("generationConfig").is_none());
    }

    #[test]
    fn parses_stream_chunks_and_usage() {
        let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
        let s2 = seen.clone();
        let sink: DeltaSink = Arc::new(move |t, th| {
            if let Ok(mut g) = s2.lock() {
                g.push((t.to_string(), th));
            }
        });
        let mut acc = GeminiAcc::default();
        let c1 = r#"{"candidates":[{"content":{"parts":[{"text":"pensando","thought":true},{"text":"Olá"}]}}]}"#;
        let c2 = r#"{"candidates":[{"content":{"parts":[{"text":", mundo"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":4,"thoughtsTokenCount":6,"cachedContentTokenCount":2}}"#;
        for c in [c1, c2] {
            acc.apply(&serde_json::from_str::<Value>(c).expect("fixture"), Some(&sink));
        }
        let r = acc.into_response(ModelRef::new("gemini", "x")).expect("ok");
        assert_eq!(r.text, "Olá, mundo");
        assert_eq!(r.thoughts, "pensando");
        assert_eq!(r.usage, Usage { input_tokens: 10, output_tokens: 10, cache_read_tokens: 2, cost_usd: None });
        let g = seen.lock().expect("lock");
        assert_eq!(g[0], ("pensando".to_string(), true));
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn blocked_prompt_is_error() {
        let mut acc = GeminiAcc::default();
        acc.apply(&json!({"promptFeedback": {"blockReason": "SAFETY"}}), None);
        let e = acc.into_response(ModelRef::default()).expect_err("vazio");
        assert!(e.to_string().contains("SAFETY"));
    }

    #[test]
    fn transcribe_request_has_wav() {
        let b = build_transcribe_request(&[0, 1], "pt-BR");
        assert_eq!(b["contents"][0]["parts"][1]["inline_data"]["mime_type"], "audio/wav");
        assert!(b["contents"][0]["parts"][0]["text"].as_str().is_some_and(|t| t.contains("pt-BR")));
    }
}
