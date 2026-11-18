//! Transcrição rápida pelo Whisper da Groq (`whisper-large-v3-turbo`, API compatível
//! com a da OpenAI). Costuma responder em menos de 1 s; o Gemini fica de reserva.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use sage_core::{Config, Transcriber};
use serde_json::Value;

const DEFAULT_BASE: &str = "https://api.groq.com/openai/v1";
pub const WHISPER_MODEL: &str = "whisper-large-v3-turbo";

/// Frases que o Whisper "inventa" quando o áudio é só silêncio ou ruído.
const HALLUCINATIONS: &[&str] = &[
    "legendas pela comunidade amara.org",
    "obrigado por assistir",
    "obrigada por assistir",
    "inscreva-se no canal",
    "legenda adriana zanotto",
    "transcrição e legendas",
];

pub struct WhisperTranscriber {
    http: reqwest::Client,
    key: String,
    base: String,
    model: String,
    timeout: Duration,
}

impl WhisperTranscriber {
    /// Só existe quando a Groq está ligada e tem chave.
    pub fn from_config(cfg: &Config, http: reqwest::Client) -> Option<Self> {
        let p = cfg.cloud_providers.get("groq");
        if p.is_some_and(|p| !p.enabled) {
            return None;
        }
        let key = cfg.api_key("groq")?;
        let base = p.and_then(|p| p.base_url.clone()).unwrap_or_else(|| DEFAULT_BASE.to_string());
        Some(Self { http, key, base: base.trim_end_matches('/').to_string(), model: WHISPER_MODEL.into(), timeout: Duration::from_secs(20) })
    }
}

/// Corpo multipart/form-data montado à mão (o reqwest do workspace não tem `multipart`).
pub(crate) fn multipart_body(boundary: &str, wav: &[u8], model: &str, language: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(wav.len() + 512);
    let field = |b: &mut Vec<u8>, name: &str, value: &str| {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    };
    field(&mut b, "model", model);
    field(&mut b, "language", language);
    field(&mut b, "response_format", "json");
    field(&mut b, "temperature", "0");
    b.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fala.wav\"\r\nContent-Type: audio/wav\r\n\r\n")
            .as_bytes(),
    );
    b.extend_from_slice(wav);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

pub(crate) fn clean(text: &str) -> String {
    let t = text.trim();
    let low = t.to_lowercase();
    if HALLUCINATIONS.iter().any(|h| low.contains(h)) && t.chars().count() < 80 {
        return String::new();
    }
    t.to_string()
}

#[async_trait]
impl Transcriber for WhisperTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, language: &str) -> anyhow::Result<String> {
        let lang = language.split(['-', '_']).next().unwrap_or("pt").to_lowercase();
        let boundary = format!("sage{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(7));
        let body = multipart_body(&boundary, &wav, &self.model, &lang);
        let resp = self
            .http
            .post(format!("{}/audio/transcriptions", self.base))
            .bearer_auth(&self.key)
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .timeout(self.timeout)
            .body(body)
            .send()
            .await
            .context("Groq Whisper: falha de conexão")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("Groq Whisper HTTP {status}: {}", text.chars().take(200).collect::<String>()));
        }
        let v: Value = serde_json::from_str(&text).context("Groq Whisper: resposta inválida")?;
        Ok(clean(v.get("text").and_then(Value::as_str).unwrap_or("")))
    }
}

/// Tenta cada transcritor em ordem; um vazio ou com erro passa para o próximo.
pub struct ChainTranscriber(pub Vec<Arc<dyn Transcriber>>);

#[async_trait]
impl Transcriber for ChainTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, language: &str) -> anyhow::Result<String> {
        let mut last = None;
        for t in &self.0 {
            match t.transcribe(wav.clone(), language).await {
                Ok(s) if !s.trim().is_empty() => return Ok(s),
                Ok(_) => last = Some(anyhow!("transcrição vazia")),
                Err(e) => {
                    tracing::warn!("transcritor falhou, tentando o próximo: {e:#}");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("nenhum transcritor configurado")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_has_fields_and_file() {
        let b = multipart_body("XX", b"RIFF", "whisper-large-v3-turbo", "pt");
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("name=\"model\"\r\n\r\nwhisper-large-v3-turbo"));
        assert!(s.contains("name=\"language\"\r\n\r\npt"));
        assert!(s.contains("filename=\"fala.wav\""));
        assert!(s.ends_with("--XX--\r\n"));
    }

    #[test]
    fn drops_silence_hallucinations() {
        assert_eq!(clean(" Legendas pela comunidade Amara.org "), "");
        assert_eq!(clean("Abra o Spotify"), "Abra o Spotify");
    }
}
