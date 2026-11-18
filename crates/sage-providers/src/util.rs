//! Utilidades comuns: duração/retry-after, mapeamento de erro HTTP, truncamento seguro.

use regex::Regex;
use sage_core::llm::LlmError;
use std::sync::LazyLock;
use std::time::Duration;

static DUR_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*(ms|d|h|m|s)").expect("regex de duração"));
static RETRY_DELAY_JSON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)"retryDelay"\s*:\s*"([^"]+)""#).expect("regex retryDelay"));
static RETRY_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:retry|try again)\s+in\s+((?:\d+(?:\.\d+)?\s*(?:ms|d|h|m|s)\s*)+)")
        .expect("regex retry in")
});

/// Daily-quota default used when the provider does not say when it resets (6h, as in the old SAGE).
pub const DAILY_QUOTA_SECS: u64 = 6 * 3600;

/// "30", "30s", "7m12.96s", "1h2m3s", "250ms" -> seconds (rounded up).
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return (n.is_finite() && n >= 0.0).then(|| n.ceil() as u64);
    }
    let mut total = 0.0_f64;
    let mut any = false;
    for c in DUR_UNIT.captures_iter(s) {
        let n: f64 = c[1].parse().ok()?;
        let mult = match &c[2] {
            "d" => 86_400.0,
            "h" => 3_600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 0.001,
            _ => continue,
        };
        total += n * mult;
        any = true;
    }
    any.then(|| total.ceil() as u64)
}

/// Segundos até poder tentar de novo, a partir do header `Retry-After` e/ou do corpo do erro.
pub fn retry_after_secs(header: Option<&str>, body: &str) -> Option<u64> {
    if let Some(n) = header.and_then(parse_duration_secs) {
        return Some(n);
    }
    for re in [&*RETRY_DELAY_JSON, &*RETRY_IN_TEXT] {
        let found = re.captures(body).and_then(|c| parse_duration_secs(&c[1]));
        if found.is_some() {
            return found;
        }
    }
    let lower = body.to_lowercase();
    if lower.contains("perday") || lower.contains("per day") || lower.contains("daily") {
        return Some(DAILY_QUOTA_SECS);
    }
    None
}

/// Corta em `max` caracteres (respeitando limites UTF-8).
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Traduz status HTTP + corpo em `LlmError` (429/cota → RateLimited, 401/403 → Auth).
pub fn http_error(provider: &str, status: u16, retry_header: Option<&str>, body: &str) -> LlmError {
    let api_key_invalid = {
        let l = body.to_lowercase();
        l.contains("api key not valid") || l.contains("api_key_invalid")
    };
    if matches!(status, 401 | 403) || (status == 400 && api_key_invalid) {
        return LlmError::Auth { provider: provider.to_string() };
    }
    if matches!(status, 429 | 402) || body.contains("RESOURCE_EXHAUSTED") {
        return LlmError::RateLimited {
            provider: provider.to_string(),
            detail: format!("HTTP {status}: {}", truncate_chars(body.trim(), 300)),
            retry_after_secs: retry_after_secs(retry_header, body),
        };
    }
    LlmError::Provider {
        provider: provider.to_string(),
        detail: format!("HTTP {status}: {}", truncate_chars(body.trim(), 300)),
    }
}

/// Erro de transporte do reqwest → `LlmError`.
pub fn transport_error(provider: &str, e: reqwest::Error, timeout: Duration) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout(timeout.as_secs())
    } else {
        // `without_url` evita vazar a URL (pode ter parâmetros) em logs.
        LlmError::Provider { provider: provider.to_string(), detail: e.without_url().to_string() }
    }
}

pub fn timeout_from(cfg: &sage_core::Config) -> Duration {
    let s = cfg.model_timeout_seconds.filter(|s| *s > 0.0 && s.is_finite()).unwrap_or(120.0);
    Duration::from_secs_f64(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_secs("30"), Some(30));
        assert_eq!(parse_duration_secs("34s"), Some(34));
        assert_eq!(parse_duration_secs("7m12.96s"), Some(433));
        assert_eq!(parse_duration_secs("1h2m3s"), Some(3723));
        assert_eq!(parse_duration_secs("1d"), Some(86_400));
        assert_eq!(parse_duration_secs("abc"), None);
    }

    #[test]
    fn retry_sources() {
        assert_eq!(retry_after_secs(Some("12"), ""), Some(12));
        let gem = r#"{"error":{"details":[{"retryDelay":"34s"}]}}"#;
        assert_eq!(retry_after_secs(None, gem), Some(34));
        assert_eq!(retry_after_secs(None, "Rate limit. Please try again in 7m12.96s."), Some(433));
        assert_eq!(retry_after_secs(None, "tokens per day (TPD) exceeded"), Some(DAILY_QUOTA_SECS));
        assert_eq!(retry_after_secs(None, "boom"), None);
    }

    #[test]
    fn maps_http_errors() {
        assert!(matches!(http_error("groq", 401, None, ""), LlmError::Auth { .. }));
        assert!(matches!(http_error("gemini", 403, None, ""), LlmError::Auth { .. }));
        assert!(matches!(
            http_error("gemini", 400, None, "API key not valid. Please pass a valid API key."),
            LlmError::Auth { .. }
        ));
        match http_error("groq", 429, Some("5"), "slow down") {
            LlmError::RateLimited { retry_after_secs, .. } => assert_eq!(retry_after_secs, Some(5)),
            other => panic!("esperava RateLimited, veio {other:?}"),
        }
        assert!(matches!(http_error("x", 500, None, "oops"), LlmError::Provider { .. }));
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_chars("áéíóú", 3), "áéí…");
        assert_eq!(truncate_chars("ab", 3), "ab");
    }
}
