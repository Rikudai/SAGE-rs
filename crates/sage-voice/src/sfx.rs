//! Banco de efeitos sonoros (SFX) do S.A.G.E. (portado de `legacy/sound_bank.py` e `legacy/sound_config.json`).
//!
//! Mapeia eventos do assistente (`ready`/`start`, `listen_start`/`listen`, `listen_stop`,
//! `success`/`done`, `error`, `clone_spawn`, `clone_done`, etc.) para arquivos de áudio
//! em `assets/sfx/library/` ou `assets/sfx/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Eventos suportados pelo banco de som e suas descrições em português (pt-BR).
pub const EVENTS: &[(&str, &str)] = &[
    ("ready", "S.A.G.E. iniciou"),
    ("listen_start", "Começou a ouvir"),
    ("listen_stop", "Parou de ouvir"),
    ("queued", "Mensagem entrou na fila"),
    ("thinking", "Pensando / executando"),
    ("success", "Ação concluída"),
    ("error", "Erro"),
    ("notify", "Aviso / trabalho pronto"),
    ("question", "Pergunta para você"),
    ("transform", "Transformação"),
    ("teleport", "Teleporte"),
    ("clone_spawn", "Clone surgiu"),
    ("clone_done", "Clone entregou"),
    ("screenshot", "Print da tela"),
    ("click", "Clique"),
];

/// Mapeamento padrão `(evento, arquivo relativo em assets/sfx/library, volume)` de `legacy/sound_bank.py`.
pub const DEFAULTS: &[(&str, &str, f32)] = &[
    ("ready", "slime/sparkle_rise.wav", 0.7),
    ("listen_start", "kenney/drop_002.wav", 0.8),
    ("listen_stop", "kenney/drop_003.wav", 0.7),
    ("queued", "slime/bloop_small.wav", 0.6),
    ("thinking", "kenney/pluck_001.wav", 0.5),
    ("success", "kenney/confirmation_002.wav", 0.7),
    ("error", "kenney/error_004.wav", 0.7),
    ("notify", "kenney/glass_002.wav", 0.75),
    ("question", "kenney/question_001.wav", 0.75),
    ("transform", "slime/squish_morph.wav", 0.8),
    ("teleport", "kenney/maximize_006.wav", 0.7),
    ("clone_spawn", "slime/bloop_up.wav", 0.75),
    ("clone_done", "slime/bubbles_pop.wav", 0.7),
    ("screenshot", "kenney/click_003.wav", 0.7),
    ("click", "kenney/click_002.wav", 0.5),
];

/// Normaliza apelidos de eventos (`start`, `listen`, `done`, ...) para as chaves canônicas do `sound_bank`.
pub fn normalize_cue_name(name: &str) -> &str {
    match name.trim().to_ascii_lowercase().as_str() {
        "ready" | "start" | "boot" | "init" => "ready",
        "listen_start" | "listen" | "mic_on" | "record_start" => "listen_start",
        "listen_stop" | "stop" | "stop_listening" | "listen_end" | "mic_off" | "record_stop" => {
            "listen_stop"
        }
        "queued" | "queue" => "queued",
        "thinking" | "think" | "plan" => "thinking",
        "success" | "done" | "ok" | "complete" => "success",
        "error" | "err" | "fail" => "error",
        "notify" | "notice" | "warn" => "notify",
        "question" | "ask" | "confirm" => "question",
        "transform" | "morph" | "form" => "transform",
        "teleport" => "teleport",
        "clone_spawn" | "spawn" | "worker_start" => "clone_spawn",
        "clone_done" | "worker_done" | "clone_end" | "worker_end" => "clone_done",
        "screenshot" | "print" => "screenshot",
        "click" => "click",
        _ => name.trim(),
    }
}

/// Configuração individual de um evento sonoro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundEventEntry {
    pub file: String,
    pub volume: f32,
    pub enabled: bool,
}

/// Configuração completa do banco de som (`data/sound_config.json` / `legacy/sound_config.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundConfig {
    pub master: f32,
    pub enabled: bool,
    pub events: BTreeMap<String, SoundEventEntry>,
}

impl Default for SoundConfig {
    fn default() -> Self {
        let mut events = BTreeMap::new();
        for &(event, file, volume) in DEFAULTS {
            events.insert(
                event.to_string(),
                SoundEventEntry {
                    file: file.to_string(),
                    volume,
                    enabled: true,
                },
            );
        }
        Self {
            master: 0.8,
            enabled: true,
            events,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PartialSoundEvent {
    file: Option<String>,
    volume: Option<f32>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct PartialSoundConfig {
    master: Option<f32>,
    enabled: Option<bool>,
    events: Option<BTreeMap<String, PartialSoundEvent>>,
}

impl SoundConfig {
    /// Carrega um JSON parcial sobre os padrões de `DEFAULTS`, como `load_config()` em `legacy/sound_bank.py`.
    pub fn load_from_str(raw: &str) -> Self {
        let mut cfg = Self::default();
        let trimmed = raw.trim_start_matches('\u{feff}');
        let Ok(user) = serde_json::from_str::<PartialSoundConfig>(trimmed) else {
            return cfg;
        };
        if let Some(m) = user.master {
            cfg.master = m.clamp(0.0, 1.0);
        }
        if let Some(en) = user.enabled {
            cfg.enabled = en;
        }
        if let Some(user_events) = user.events {
            for (k, v) in user_events {
                if let Some(entry) = cfg.events.get_mut(&k) {
                    if let Some(file) = v.file {
                        entry.file = file;
                    }
                    if let Some(vol) = v.volume {
                        entry.volume = vol.clamp(0.0, 1.0);
                    }
                    if let Some(en) = v.enabled {
                        entry.enabled = en;
                    }
                }
            }
        }
        cfg
    }

    /// Carrega `data/sound_config.json` (ou `legacy/sound_config.json` como fallback) se existir.
    pub fn load() -> Self {
        let data_cfg = sage_core::paths::data().join("sound_config.json");
        if let Ok(raw) = std::fs::read_to_string(&data_cfg) {
            return Self::load_from_str(&raw);
        }
        let legacy_cfg = sage_core::paths::root()
            .join("legacy")
            .join("sound_config.json");
        if let Ok(raw) = std::fs::read_to_string(&legacy_cfg) {
            return Self::load_from_str(&raw);
        }
        Self::default()
    }

    /// Resolve o mapeamento lógico `(arquivo_relativo, volume_efetivo)` para um evento.
    /// Retorna `None` quando o som está desativado, sem arquivo ou contém tentativa de path traversal.
    pub fn resolve_mapping(&self, event: &str) -> Option<(String, f32)> {
        if !self.enabled {
            return None;
        }
        let canonical = normalize_cue_name(event);
        let entry = self.events.get(canonical)?;
        if !entry.enabled {
            return None;
        }
        let safe_rel = sanitize_relative_sfx_path(&entry.file)?;
        let effective_volume = (entry.volume * self.master).clamp(0.0, 1.0);
        Some((safe_rel, effective_volume))
    }

    /// Resolve o caminho absoluto em disco e o volume final `(PathBuf, f32)` dentro de `sfx_dir` (`assets/sfx`).
    pub fn resolve_path(&self, event: &str, sfx_dir: &Path) -> Option<(PathBuf, f32)> {
        let (rel, volume) = self.resolve_mapping(event)?;
        let canonical = normalize_cue_name(event);

        // 1. Prioriza `assets/sfx/library/<rel>` (como em `legacy/sound_bank.py`)
        let in_library = sfx_dir.join("library").join(&rel);
        if in_library.is_file() {
            return Some((in_library, volume));
        }

        // 2. Fallback direto em `assets/sfx/<rel>`
        let direct = sfx_dir.join(&rel);
        if direct.is_file() {
            return Some((direct, volume));
        }

        // 3. Fallback procedural em `assets/sfx/<canonical>.wav` (como em `legacy/sfx.py`)
        let fallback_wav = sfx_dir.join(format!("{canonical}.wav"));
        if fallback_wav.is_file() {
            return Some((fallback_wav, volume));
        }

        None
    }
}

/// Valida e normaliza um caminho relativo de SFX, bloqueando path traversal (`..`).
pub fn sanitize_relative_sfx_path(rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    let trimmed = normalized.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return None;
        }
        parts.push(part);
    }
    Some(parts.join("/"))
}

/// Atalho para resolver um evento usando a configuração carregada e a pasta `assets/sfx` padrão.
pub fn resolve(event: &str) -> Option<(PathBuf, f32)> {
    let cfg = SoundConfig::load();
    let sfx_dir = sage_core::paths::assets().join("sfx");
    cfg.resolve_path(event, &sfx_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_aliases_normalize_to_canonical_events() {
        assert_eq!(normalize_cue_name("start"), "ready");
        assert_eq!(normalize_cue_name("READY"), "ready");
        assert_eq!(normalize_cue_name("listen"), "listen_start");
        assert_eq!(normalize_cue_name("listen_start"), "listen_start");
        assert_eq!(normalize_cue_name("stop"), "listen_stop");
        assert_eq!(normalize_cue_name("listen_stop"), "listen_stop");
        assert_eq!(normalize_cue_name("done"), "success");
        assert_eq!(normalize_cue_name("success"), "success");
        assert_eq!(normalize_cue_name("error"), "error");
        assert_eq!(normalize_cue_name("clone_spawn"), "clone_spawn");
        assert_eq!(normalize_cue_name("clone_done"), "clone_done");
    }

    #[test]
    fn default_config_maps_all_known_cues_with_master_volume() {
        let cfg = SoundConfig::default();
        let (file, vol) = cfg.resolve_mapping("start").expect("start resolves");
        assert_eq!(file, "slime/sparkle_rise.wav");
        assert!((vol - 0.7 * 0.8).abs() < 1e-5);

        let (file, vol) = cfg.resolve_mapping("listen").expect("listen resolves");
        assert_eq!(file, "kenney/drop_002.wav");
        assert!((vol - 0.8 * 0.8).abs() < 1e-5);

        let (file, vol) = cfg.resolve_mapping("done").expect("done resolves");
        assert_eq!(file, "kenney/confirmation_002.wav");
        assert!((vol - 0.7 * 0.8).abs() < 1e-5);

        let (file, vol) = cfg.resolve_mapping("clone_spawn").expect("clone_spawn resolves");
        assert_eq!(file, "slime/bloop_up.wav");
        assert!((vol - 0.75 * 0.8).abs() < 1e-5);

        let (file, vol) = cfg.resolve_mapping("clone_done").expect("clone_done resolves");
        assert_eq!(file, "slime/bubbles_pop.wav");
        assert!((vol - 0.7 * 0.8).abs() < 1e-5);
    }

    #[test]
    fn user_json_merges_and_respects_disabled_flags() {
        let raw = r#"{
            "master": 0.5,
            "enabled": true,
            "events": {
                "ready": { "file": "slime/sparkle_rise.wav", "volume": 0.3, "enabled": true },
                "error": { "file": "kenney/error_004.wav", "volume": 0.7, "enabled": false }
            }
        }"#;
        let cfg = SoundConfig::load_from_str(raw);
        let (file, vol) = cfg.resolve_mapping("ready").expect("ready enabled");
        assert_eq!(file, "slime/sparkle_rise.wav");
        assert!((vol - 0.15).abs() < 1e-5);
        assert!(cfg.resolve_mapping("error").is_none());

        let muted = SoundConfig::load_from_str(r#"{"enabled": false}"#);
        assert!(muted.resolve_mapping("ready").is_none());
    }

    #[test]
    fn path_traversal_is_rejected() {
        assert!(sanitize_relative_sfx_path("../secret.wav").is_none());
        assert!(sanitize_relative_sfx_path("kenney/../../secret.wav").is_none());
        assert!(sanitize_relative_sfx_path("C:/Windows/Media/tada.wav").is_none());
        assert!(sanitize_relative_sfx_path("").is_none());
        assert_eq!(
            sanitize_relative_sfx_path(r"kenney\drop_002.wav").as_deref(),
            Some("kenney/drop_002.wav")
        );
    }

    #[test]
    fn resolves_real_sfx_files_in_repo_assets() {
        let sfx_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/sfx");
        if sfx_dir.is_dir() {
            let cfg = SoundConfig::default();
            for cue in ["start", "listen", "done", "error", "clone_spawn", "clone_done"] {
                let resolved = cfg.resolve_path(cue, &sfx_dir);
                assert!(resolved.is_some(), "esperava resolver arquivo real para o cue {cue}");
            }
        }
    }
}
