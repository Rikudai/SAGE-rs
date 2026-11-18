//! Síntese de fala (TTS): higienização de texto, cliente Fish Audio e fallback Edge TTS (`msedge-tts`).

use std::time::Duration;

use sage_core::Config;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::profiles::{DEFAULT_FISH_MODEL, DEFAULT_FISH_REFERENCE_ID, ResolvedVoiceSettings};

/// Endpoint oficial de TTS da Fish Audio.
pub const FISH_AUDIO_TTS_URL: &str = "https://api.fish.audio/v1/tts";

/// Número máximo padrão de frases faladas quando `max_speech_sentences` não está definido.
pub const DEFAULT_MAX_SPEECH_SENTENCES: u32 = 4;

/// Formato de saída MP3 usado no Edge TTS.
pub const EDGE_TTS_AUDIO_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// Corpo da requisição JSON enviada para `POST https://api.fish.audio/v1/tts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FishTtsRequest {
    pub text: String,
    pub reference_id: String,
    pub format: String,
}

impl FishTtsRequest {
    pub fn new(text: impl Into<String>, reference_id: impl Into<String>) -> Self {
        let reference = reference_id.into();
        let ref_trimmed = reference.trim();
        Self {
            text: text.into(),
            reference_id: if ref_trimmed.is_empty() {
                DEFAULT_FISH_REFERENCE_ID.to_string()
            } else {
                ref_trimmed.to_string()
            },
            format: "mp3".to_string(),
        }
    }
}

/// Constrói o corpo JSON `{text, reference_id, format: "mp3"}` para a API da Fish Audio.
pub fn build_fish_request_body(text: &str, reference_id: &str) -> Value {
    let req = FishTtsRequest::new(text, reference_id);
    serde_json::json!({
        "text": req.text,
        "reference_id": req.reference_id,
        "format": req.format,
    })
}

/// Prepara o texto para leitura em voz alta usando `sage_core::text::sanitize_for_speech`
/// e `sage_core::text::pronounce_name`.
pub fn prepare_speech_text(raw: &str, max_sentences: Option<u32>) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    let limit = max_sentences.unwrap_or(DEFAULT_MAX_SPEECH_SENTENCES).max(1) as usize;
    let pronounced = sage_core::text::pronounce_name(raw);
    let sanitized = sage_core::text::sanitize_for_speech(&pronounced, limit);
    sage_core::text::pronounce_name(&sanitized)
}

/// Converte parâmetros de voz no formato Edge (`"+6%"`, `"-2Hz"`, `"+0%"`) para `i32`.
pub fn parse_edge_param(raw: &str) -> i32 {
    let cleaned = raw
        .trim()
        .trim_end_matches('%')
        .trim_end_matches("Hz")
        .trim_end_matches("hz")
        .trim_end_matches("HZ")
        .trim()
        .trim_start_matches('+');
    cleaned.parse::<i32>().unwrap_or(0)
}

/// Obtém a chave da Fish Audio a partir do `Config` ou da variável de ambiente `FISH_AUDIO_API_KEY`.
pub fn resolve_fish_api_key(cfg: &Config) -> Option<String> {
    let from_cfg = cfg.fish_audio_api_key.trim();
    if !from_cfg.is_empty() {
        return Some(from_cfg.to_string());
    }
    std::env::var("FISH_AUDIO_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn post_fish_tts(
    client: &reqwest::Client,
    text: &str,
    api_key: &str,
    reference_id: &str,
    model: &str,
) -> Result<Vec<u8>, (Option<u16>, anyhow::Error)> {
    let body = build_fish_request_body(text, reference_id);
    let model_header = if model.trim().is_empty() {
        DEFAULT_FISH_MODEL
    } else {
        model.trim()
    };

    let resp = client
        .post(FISH_AUDIO_TTS_URL)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .header("model", model_header)
        .header("Content-Type", "application/json")
        .header("User-Agent", "S.A.G.E./2.0")
        .timeout(Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| (None, anyhow::anyhow!("Erro de rede na Fish Audio: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let detail = resp.text().await.unwrap_or_default();
        return Err((
            Some(code),
            anyhow::anyhow!("Fish Audio retornou HTTP {code}: {detail}"),
        ));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| (None, anyhow::anyhow!("Erro ao ler resposta da Fish Audio: {e}")))?;

    if bytes.is_empty() {
        return Err((None, anyhow::anyhow!("Fish Audio retornou áudio vazio")));
    }

    Ok(bytes.to_vec())
}

/// Sintetiza fala em MP3 via Fish Audio (`POST https://api.fish.audio/v1/tts`),
/// com tentativa automática no modelo gratuito (`s2.1-pro-free`) em caso de HTTP 402.
pub async fn synthesize_fish_audio(
    client: &reqwest::Client,
    text: &str,
    api_key: &str,
    reference_id: &str,
    model: &str,
) -> anyhow::Result<Vec<u8>> {
    if api_key.trim().is_empty() {
        anyhow::bail!("Chave da Fish Audio não configurada");
    }
    match post_fish_tts(client, text, api_key, reference_id, model).await {
        Ok(bytes) => Ok(bytes),
        Err((Some(402), _)) if model.trim() != DEFAULT_FISH_MODEL => {
            post_fish_tts(client, text, api_key, reference_id, DEFAULT_FISH_MODEL)
                .await
                .map_err(|(_, e)| e)
        }
        Err((_, e)) => Err(e),
    }
}

/// Sintetiza fala em MP3 usando o fallback Edge TTS (`msedge-tts`).
pub async fn synthesize_edge_tts(
    text: &str,
    voice_name: &str,
    rate: &str,
    pitch: &str,
    volume: &str,
) -> anyhow::Result<Vec<u8>> {
    let text_owned = text.to_string();
    let voice_owned = if voice_name.trim().is_empty() {
        "pt-BR-ThalitaMultilingualNeural".to_string()
    } else {
        voice_name.trim().to_string()
    };
    let rate_num = parse_edge_param(rate);
    let pitch_num = parse_edge_param(pitch);
    let volume_num = parse_edge_param(volume);

    tokio::task::spawn_blocking(move || {
        let config = msedge_tts::tts::SpeechConfig {
            voice_name: voice_owned,
            audio_format: EDGE_TTS_AUDIO_FORMAT.to_string(),
            pitch: pitch_num,
            rate: rate_num,
            volume: volume_num,
        };
        let mut tts = msedge_tts::tts::client::connect()
            .map_err(|e| anyhow::anyhow!("Falha ao conectar ao Edge TTS: {e:?}"))?;
        let audio = tts
            .synthesize(&text_owned, &config)
            .map_err(|e| anyhow::anyhow!("Falha ao sintetizar via Edge TTS: {e:?}"))?;
        if audio.audio_bytes.is_empty() {
            anyhow::bail!("Edge TTS retornou áudio vazio");
        }
        Ok(audio.audio_bytes)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Tarefa do Edge TTS falhou: {e}"))?
}

/// Sintetiza `text` (já preparado) tentando Fish Audio primeiro (quando configurado)
/// e caindo para Edge TTS em caso de falha.
pub async fn synthesize_with_fallback(
    client: &reqwest::Client,
    text: &str,
    cfg: &Config,
) -> anyhow::Result<Vec<u8>> {
    let resolved = ResolvedVoiceSettings::from_config(cfg);
    let provider = resolved.tts_provider.to_ascii_lowercase();
    let wants_fish = provider.is_empty() || provider == "fish_audio" || provider == "fish";
    let fish_key = if wants_fish {
        resolve_fish_api_key(cfg)
    } else {
        None
    };

    if let Some(api_key) = fish_key {
        match synthesize_fish_audio(
            client,
            text,
            &api_key,
            &resolved.fish_reference_id,
            &resolved.fish_model,
        )
        .await
        {
            Ok(mp3) => return Ok(mp3),
            Err(e) => {
                tracing::warn!(
                    "[Voz] Aviso na Fish Audio TTS (usando fallback Edge TTS): {e}"
                );
            }
        }
    }

    synthesize_edge_tts(
        text,
        &resolved.edge_voice,
        &resolved.rate,
        &resolved.pitch,
        &resolved.volume,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fish_request_body_builder_formats_exact_json_fields() {
        let body = build_fish_request_body(
            "Olá, eu sou o Seije.",
            "9dfb8ae2b936473d892167cc3f3a7233",
        );
        assert_eq!(
            body,
            serde_json::json!({
                "text": "Olá, eu sou o Seije.",
                "reference_id": "9dfb8ae2b936473d892167cc3f3a7233",
                "format": "mp3"
            })
        );
    }

    #[test]
    fn fish_request_body_builder_uses_default_reference_when_empty() {
        let body = build_fish_request_body("Teste", "   ");
        assert_eq!(body["text"], "Teste");
        assert_eq!(body["reference_id"], DEFAULT_FISH_REFERENCE_ID);
        assert_eq!(body["format"], "mp3");
    }

    #[test]
    fn prepare_speech_text_sanitizes_markdown_and_pronounces_sage_name() {
        let raw = "Olá! Eu sou o **S.A.G.E.**\n```rust\nfn main() {}\n```\nVeja https://github.com/user/repo agora.";
        let prepared = prepare_speech_text(raw, Some(4));
        assert!(prepared.contains("Seije"));
        assert!(!prepared.contains("S.A.G.E."));
        assert!(!prepared.contains("```"));
        assert!(prepared.contains("GitHub"));
    }

    #[test]
    fn parse_edge_param_extracts_signed_integers() {
        assert_eq!(parse_edge_param("+6%"), 6);
        assert_eq!(parse_edge_param("-2Hz"), -2);
        assert_eq!(parse_edge_param("+10hz"), 10);
        assert_eq!(parse_edge_param("+0%"), 0);
        assert_eq!(parse_edge_param(""), 0);
        assert_eq!(parse_edge_param("invalido"), 0);
    }
}
