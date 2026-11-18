//! `QuotaLedger` (`data/quota.json`): provider usage observed from rate-limit errors and
//! CLI outcomes, the thresholds that move work to the next provider at the same tier.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local, NaiveTime, TimeDelta, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use sage_core::llm::{AgentOutcome, FailureKind, LlmError, ModelRef};

use crate::jsonx::clip;
use crate::lock;
use crate::playbooks::QuotaPolicy;

/// Cool-down after an API 429 without a reset hint.
const DEFAULT_API_COOLDOWN_SECS: i64 = 15 * 60;
/// Cool-down after a CLI usage limit without a reset hint.
const DEFAULT_CLI_COOLDOWN_SECS: i64 = 60 * 60;
/// Longest cool-down we accept from a hint.
const MAX_COOLDOWN_SECS: i64 = 30 * 86_400;
/// Usage records without a reset time are forgotten after this long.
const STALE_AFTER_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowUsage {
    pub used_pct: f64,
    #[serde(default)]
    pub resets_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
}

impl WindowUsage {
    fn active(&self, now: DateTime<Utc>) -> bool {
        match self.resets_at {
            Some(r) => r > now,
            None => now.signed_duration_since(self.observed_at) < TimeDelta::hours(STALE_AFTER_HOURS),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProviderQuota {
    /// Usage per window ("5h", "day", "week", or "limit" after a rate limit).
    #[serde(default)]
    pub windows: BTreeMap<String, WindowUsage>,
    #[serde(default)]
    pub rate_limits: u32,
    #[serde(default)]
    pub last_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaState {
    Healthy,
    /// At or above `criticalThreshold`: move work elsewhere when possible.
    Critical,
    /// At or above `stopAt`: do not use.
    Blocked,
}

/// Which candidate to use, and who was passed over for quota.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaPick {
    pub chosen: usize,
    /// Providers before the chosen one that were skipped, with their usage %.
    pub passed_over: Vec<(String, f64)>,
    /// Every candidate is at least critical; the chosen one is the least bad.
    pub all_low: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct QuotaLedger {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderQuota>,
}

impl QuotaLedger {
    /// Reads the ledger; a missing or unreadable file starts empty.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                tracing::warn!("quota.json ilegível, começando do zero: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Atomic save (temp file + rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Records an observed usage percentage for one window of a provider.
    pub fn record_usage(
        &mut self,
        provider: &str,
        window: &str,
        used_pct: f64,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) {
        let entry = self.providers.entry(provider.to_string()).or_default();
        entry
            .windows
            .insert(window.to_string(), WindowUsage { used_pct: used_pct.clamp(0.0, 100.0), resets_at, observed_at: now });
    }

    fn mark_limited(&mut self, provider: &str, reason: &str, resets_at: DateTime<Utc>, now: DateTime<Utc>) {
        self.record_usage(provider, "limit", 100.0, Some(resets_at), now);
        let entry = self.providers.entry(provider.to_string()).or_default();
        entry.rate_limits = entry.rate_limits.saturating_add(1);
        entry.last_reason = Some(clip(reason, 200));
    }

    /// Notes a `RateLimited` error; returns the provider it blocked.
    pub fn observe_llm_error(&mut self, err: &LlmError, now: DateTime<Utc>) -> Option<String> {
        let LlmError::RateLimited { provider, detail, retry_after_secs } = err else {
            return None;
        };
        let resets = retry_after_secs
            .map(|s| now + seconds(i64::try_from(s).unwrap_or(MAX_COOLDOWN_SECS)))
            .or_else(|| parse_reset_hint(detail, now))
            .unwrap_or_else(|| now + seconds(DEFAULT_API_COOLDOWN_SECS));
        self.mark_limited(provider, detail, resets, now);
        Some(provider.clone())
    }

    /// Notes a CLI outcome that failed on a usage limit; true when it did.
    pub fn observe_outcome(&mut self, provider: &str, out: &AgentOutcome, now: DateTime<Utc>) -> bool {
        if out.failure != Some(FailureKind::RateLimited) {
            return false;
        }
        let hint = out.reset_hint.clone().unwrap_or_default();
        let resets = parse_reset_hint(&hint, now)
            .or_else(|| parse_reset_hint(&out.text, now))
            .or_else(|| parse_reset_hint(&out.summary, now))
            .unwrap_or_else(|| now + seconds(DEFAULT_CLI_COOLDOWN_SECS));
        let reason = if hint.trim().is_empty() { out.summary.as_str() } else { hint.as_str() };
        self.mark_limited(provider, reason, resets, now);
        true
    }

    /// Highest usage % among the provider's windows still in effect.
    pub fn usage_pct(&self, provider: &str, now: DateTime<Utc>) -> f64 {
        self.providers
            .get(provider)
            .map(|p| p.windows.values().filter(|w| w.active(now)).map(|w| w.used_pct).fold(0.0, f64::max))
            .unwrap_or(0.0)
    }

    pub fn state(&self, provider: &str, policy: &QuotaPolicy, now: DateTime<Utc>) -> QuotaState {
        let pct = self.usage_pct(provider, now);
        if pct >= policy.stop_at {
            QuotaState::Blocked
        } else if pct >= policy.critical_threshold {
            QuotaState::Critical
        } else {
            QuotaState::Healthy
        }
    }

    /// Usage % of every known provider.
    pub fn snapshot(&self, now: DateTime<Utc>) -> BTreeMap<String, f64> {
        self.providers.keys().map(|p| (p.clone(), self.usage_pct(p, now))).collect()
    }

    /// First healthy candidate; when none is healthy, the first critical one.
    /// `None` when every candidate is blocked.
    pub fn pick(&self, candidates: &[ModelRef], policy: &QuotaPolicy, now: DateTime<Utc>) -> Option<QuotaPick> {
        let states: Vec<QuotaState> = candidates.iter().map(|m| self.state(&m.provider, policy, now)).collect();
        let (chosen, all_low) = match states.iter().position(|s| *s == QuotaState::Healthy) {
            Some(i) => (i, false),
            None => (states.iter().position(|s| *s == QuotaState::Critical)?, true),
        };
        let chosen_provider = &candidates[chosen].provider;
        let mut passed_over: Vec<(String, f64)> = Vec::new();
        for m in &candidates[..chosen] {
            if m.provider != *chosen_provider && !passed_over.iter().any(|(p, _)| *p == m.provider) {
                passed_over.push((m.provider.clone(), self.usage_pct(&m.provider, now)));
            }
        }
        Some(QuotaPick { chosen, passed_over, all_low })
    }
}

fn seconds(s: i64) -> TimeDelta {
    TimeDelta::seconds(s.clamp(0, MAX_COOLDOWN_SECS))
}

/// Reset time from texts like "resets in 2h 30m", "try again in 45 minutes",
/// "retry after 30s" or "try again at 3:05 PM".
pub fn parse_reset_hint(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let lower = text.to_lowercase();
    for marker in ["resets in", "reset in", "try again in", "retry in", "retry after", "tente novamente em", "again in"] {
        if let Some(i) = lower.find(marker)
            && let Some(secs) = parse_duration(&lower[i + marker.len()..])
        {
            return Some(now + seconds(secs));
        }
    }
    for marker in ["try again at", "resets at", "reset at", "available at", "tente novamente às"] {
        if let Some(i) = lower.find(marker)
            && let Some(at) = parse_clock(&lower[i + marker.len()..], now)
        {
            return Some(at);
        }
    }
    None
}

/// "2h 30m", "45 minutes", "1 day and 3 hours" -> seconds.
fn parse_duration(s: &str) -> Option<i64> {
    let mut total: i64 = 0;
    let mut found = false;
    let mut rest = s.trim_start();
    loop {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        let n: i64 = digits.parse().ok()?;
        rest = rest[digits.len()..].trim_start();
        let unit: String = rest.chars().take_while(|c| c.is_alphabetic()).collect();
        let mult: i64 = match unit.as_str() {
            "d" | "day" | "days" | "dia" | "dias" => 86_400,
            "h" | "hr" | "hrs" | "hour" | "hours" | "hora" | "horas" => 3_600,
            "m" | "min" | "mins" | "minute" | "minutes" | "minuto" | "minutos" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" | "segundo" | "segundos" => 1,
            _ => break,
        };
        total = total.saturating_add(n.saturating_mul(mult));
        found = true;
        rest = rest[unit.len()..].trim_start_matches([' ', ',']);
        rest = rest.strip_prefix("and ").or_else(|| rest.strip_prefix("e ")).unwrap_or(rest);
    }
    if found && total > 0 { Some(total.min(MAX_COOLDOWN_SECS)) } else { None }
}

/// "3:05 pm" or "15:05" as the next such local time.
fn parse_clock(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let s = s.trim_start();
    let hh: String = s.chars().take_while(char::is_ascii_digit).collect();
    if hh.is_empty() {
        return None;
    }
    let mut hour: u32 = hh.parse().ok()?;
    let rest = &s[hh.len()..];
    let (minute, rest): (u32, &str) = match rest.strip_prefix(':') {
        Some(r) => {
            let mm: String = r.chars().take_while(char::is_ascii_digit).collect();
            let m: u32 = mm.parse().ok()?;
            (m, &r[mm.len()..])
        }
        None => (0, rest),
    };
    let rest = rest.trim_start();
    if rest.starts_with("pm") && hour < 12 {
        hour += 12;
    } else if rest.starts_with("am") && hour == 12 {
        hour = 0;
    }
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let local_now = now.with_timezone(&Local);
    let naive = local_now.date_naive().and_time(time);
    let mut at = Local.from_local_datetime(&naive).earliest()?.with_timezone(&Utc);
    if at <= now {
        at += TimeDelta::days(1);
    }
    Some(at)
}

/// The ledger shared by the brain and its workers; every observation is saved.
#[derive(Clone)]
pub struct SharedQuota {
    ledger: Arc<Mutex<QuotaLedger>>,
    path: Option<PathBuf>,
}

impl SharedQuota {
    pub fn new(ledger: QuotaLedger, path: Option<PathBuf>) -> Self {
        Self { ledger: Arc::new(Mutex::new(ledger)), path }
    }

    /// Loads `path` (empty when missing) and saves back to it.
    pub fn load(path: PathBuf) -> Self {
        Self::new(QuotaLedger::load(&path), Some(path))
    }

    pub fn read<R>(&self, f: impl FnOnce(&QuotaLedger) -> R) -> R {
        f(&lock(&self.ledger))
    }

    pub fn observe_error(&self, err: &LlmError) -> Option<String> {
        let hit = lock(&self.ledger).observe_llm_error(err, Utc::now());
        if hit.is_some() {
            self.persist();
        }
        hit
    }

    pub fn observe_outcome(&self, provider: &str, out: &AgentOutcome) -> bool {
        let hit = lock(&self.ledger).observe_outcome(provider, out, Utc::now());
        if hit {
            self.persist();
        }
        hit
    }

    pub fn record_usage(&self, provider: &str, window: &str, used_pct: f64, resets_at: Option<DateTime<Utc>>) {
        lock(&self.ledger).record_usage(provider, window, used_pct, resets_at, Utc::now());
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = &self.path else { return };
        let snapshot = lock(&self.ledger).clone();
        if let Err(e) = snapshot.save(path) {
            tracing::warn!("não consegui salvar {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::{Usage, WorkerStatus};

    fn policy() -> QuotaPolicy {
        QuotaPolicy::default()
    }

    fn refs(ps: &[&str]) -> Vec<ModelRef> {
        ps.iter().map(|p| ModelRef::new(*p, "m")).collect()
    }

    #[test]
    fn rate_limit_blocks_and_pick_moves_to_next_provider() {
        let now = Utc::now();
        let mut l = QuotaLedger::default();
        let err = LlmError::RateLimited { provider: "gemini".into(), detail: "429".into(), retry_after_secs: Some(600) };
        assert_eq!(l.observe_llm_error(&err, now).as_deref(), Some("gemini"));
        assert_eq!(l.state("gemini", &policy(), now), QuotaState::Blocked);
        assert_eq!(l.state("groq", &policy(), now), QuotaState::Healthy);
        let pick = l.pick(&refs(&["gemini", "groq"]), &policy(), now).unwrap();
        assert_eq!(pick.chosen, 1);
        assert_eq!(pick.passed_over, vec![("gemini".to_string(), 100.0)]);
        assert!(!pick.all_low);
        // after the reset time the provider is healthy again
        let later = now + TimeDelta::seconds(601);
        assert_eq!(l.state("gemini", &policy(), later), QuotaState::Healthy);
        // other errors are not quota
        assert!(l.observe_llm_error(&LlmError::Timeout(3), now).is_none());
    }

    #[test]
    fn critical_threshold_and_all_low() {
        let now = Utc::now();
        let mut l = QuotaLedger::default();
        l.record_usage("gemini", "day", 86.0, None, now);
        l.record_usage("groq", "day", 90.0, None, now);
        assert_eq!(l.state("gemini", &policy(), now), QuotaState::Critical);
        let pick = l.pick(&refs(&["gemini", "groq"]), &policy(), now).unwrap();
        assert_eq!(pick.chosen, 0);
        assert!(pick.all_low);
        l.record_usage("groq", "day", 10.0, None, now);
        let pick = l.pick(&refs(&["gemini", "groq"]), &policy(), now).unwrap();
        assert_eq!((pick.chosen, pick.all_low), (1, false));
        l.record_usage("gemini", "day", 99.0, None, now);
        l.record_usage("groq", "day", 99.0, None, now);
        assert!(l.pick(&refs(&["gemini", "groq"]), &policy(), now).is_none());
        // stale records without a reset time stop counting
        assert_eq!(l.usage_pct("gemini", now + TimeDelta::hours(25)), 0.0);
    }

    #[test]
    fn cli_outcome_with_usage_limit() {
        let now = Utc::now();
        let mut l = QuotaLedger::default();
        let mut out = AgentOutcome {
            status: WorkerStatus::Failed,
            summary: "usage limit".into(),
            text: String::new(),
            usage: Usage::default(),
            session_id: None,
            failure: Some(FailureKind::RateLimited),
            reset_hint: Some("resets in 2h".into()),
            duration_secs: 1.0,
        };
        assert!(l.observe_outcome("claude", &out, now));
        let w = &l.providers["claude"].windows["limit"];
        assert_eq!(w.resets_at, Some(now + TimeDelta::hours(2)));
        out.failure = Some(FailureKind::Timeout);
        assert!(!l.observe_outcome("codex", &out, now));
        assert!(!l.providers.contains_key("codex"));
    }

    #[test]
    fn reset_hints() {
        let now = Utc::now();
        assert_eq!(parse_reset_hint("Limit hit; resets in 2h 30m.", now), Some(now + TimeDelta::seconds(9000)));
        assert_eq!(parse_reset_hint("Please try again in 45 minutes", now), Some(now + TimeDelta::seconds(2700)));
        assert_eq!(parse_reset_hint("retry after 30s", now), Some(now + TimeDelta::seconds(30)));
        assert_eq!(parse_reset_hint("resets in 1 day and 3 hours", now), Some(now + TimeDelta::seconds(97_200)));
        let at = parse_reset_hint("You've hit your usage limit. Try again at 3:05 PM.", now).unwrap();
        assert!(at > now && at <= now + TimeDelta::hours(25));
        assert!(parse_reset_hint("nada útil", now).is_none());
        assert!(parse_reset_hint("resets in soon", now).is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("sage-quota-{}", uuid::Uuid::new_v4()));
        let path = dir.join("quota.json");
        let shared = SharedQuota::load(path.clone());
        shared.record_usage("groq", "day", 42.0, None);
        let back = QuotaLedger::load(&path);
        assert_eq!(back.providers["groq"].windows["day"].used_pct, 42.0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
