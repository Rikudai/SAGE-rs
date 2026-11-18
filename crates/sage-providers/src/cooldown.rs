//! Mapa em memória de "provedor em descanso": quem devolveu RateLimited fica de fora até o horário.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Espera padrão quando o provedor não diz quando a cota volta (30s, como no SAGE antigo).
pub const DEFAULT_COOLDOWN_SECS: u64 = 30;
/// Teto: nunca bloqueia por mais de 24h.
pub const MAX_COOLDOWN_SECS: u64 = 24 * 3600;

#[derive(Default)]
pub struct Cooldowns {
    map: Mutex<HashMap<String, Instant>>,
}

impl Cooldowns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bloqueia `key` por `secs` (None = padrão de 30s).
    pub fn set(&self, key: &str, secs: Option<u64>) {
        let secs = secs.unwrap_or(DEFAULT_COOLDOWN_SECS).min(MAX_COOLDOWN_SECS);
        let until = Instant::now() + Duration::from_secs(secs);
        if let Ok(mut m) = self.map.lock() {
            m.insert(key.to_string(), until);
        }
    }

    /// Segundos que faltam, ou None se liberado.
    pub fn remaining(&self, key: &str) -> Option<u64> {
        let mut m = self.map.lock().ok()?;
        let until = *m.get(key)?;
        let now = Instant::now();
        if until <= now {
            m.remove(key);
            return None;
        }
        Some((until - now).as_secs_f64().ceil() as u64)
    }

    pub fn is_cooling(&self, key: &str) -> bool {
        self.remaining(key).is_some()
    }

    pub fn clear(&self, key: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_until_time_passes() {
        let c = Cooldowns::new();
        assert!(!c.is_cooling("groq"));
        c.set("groq", Some(60));
        assert!(c.is_cooling("groq"));
        assert!(c.remaining("groq").is_some_and(|s| s > 0 && s <= 60));
        assert!(!c.is_cooling("cerebras"));
        c.set("groq", Some(0));
        assert!(!c.is_cooling("groq"));
    }

    #[test]
    fn default_and_cap() {
        let c = Cooldowns::new();
        c.set("a", None);
        assert!(c.remaining("a").is_some_and(|s| s <= DEFAULT_COOLDOWN_SECS));
        c.set("b", Some(u64::MAX / 4));
        assert!(c.remaining("b").is_some_and(|s| s <= MAX_COOLDOWN_SECS));
        c.clear("b");
        assert!(!c.is_cooling("b"));
    }
}
