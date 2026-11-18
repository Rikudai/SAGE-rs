//! Perfis de voz do S.A.G.E. (portado de `legacy/voice_profiles.py` e `legacy/fish_audio.py`).
//!
//! Cada perfil agrupa o modelo principal na nuvem (Fish Audio) com uma voz neural
//! de fallback no Edge TTS que mantém o mesmo timbre, garantindo degradação suave
//! quando a rede ou os créditos da API não estão disponíveis.

use sage_core::Config;
use serde::{Deserialize, Serialize};

/// Identificador padrão de referência na Fish Audio (Clássica / Jarvis UCM PT-BR).
pub const DEFAULT_FISH_REFERENCE_ID: &str = "a5b93aeddcc948c19ea04f0afe9d178c";

/// Modelo / tier padrão da Fish Audio.
pub const DEFAULT_FISH_MODEL: &str = "s2.1-pro-free";

/// Chave do perfil de voz padrão.
pub const DEFAULT_PROFILE_KEY: &str = "rimuru";

/// Perfil de voz combinando Fish Audio e fallback Edge TTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub provider: &'static str,
    pub fish_model_id: &'static str,
    pub fish_tier: &'static str,
    pub edge_voice: &'static str,
    pub rate: &'static str,
    pub pitch: &'static str,
    pub volume: &'static str,
}

/// Catálogo de perfis verificados do S.A.G.E.
pub const PROFILES: &[VoiceProfile] = &[
    VoiceProfile {
        key: "rimuru",
        label: "Rimuru",
        description: "Voz jovem, leve e animada (Rimuru Tempest PT-BR).",
        provider: "fish_audio",
        fish_model_id: "9dfb8ae2b936473d892167cc3f3a7233",
        fish_tier: DEFAULT_FISH_MODEL,
        edge_voice: "pt-BR-ThalitaMultilingualNeural",
        rate: "+6%",
        pitch: "+10Hz",
        volume: "+0%",
    },
    VoiceProfile {
        key: "sabio",
        label: "Grande Sábio",
        description: "Voz calma e precisa para relatórios e análises.",
        provider: "fish_audio",
        fish_model_id: "e4b076f01a4442eeb91d026393bc21ce",
        fish_tier: DEFAULT_FISH_MODEL,
        edge_voice: "pt-BR-FranciscaNeural",
        rate: "+3%",
        pitch: "+2Hz",
        volume: "+0%",
    },
    VoiceProfile {
        key: "classica",
        label: "Clássica (S.A.G.E.)",
        description: "Voz formal e grave, no estilo do S.A.G.E. original.",
        provider: "fish_audio",
        fish_model_id: DEFAULT_FISH_REFERENCE_ID,
        fish_tier: DEFAULT_FISH_MODEL,
        edge_voice: "pt-BR-AntonioNeural",
        rate: "+0%",
        pitch: "-2Hz",
        volume: "+0%",
    },
];

/// Modelos legados do catálogo Fish Audio (`legacy/fish_audio.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FishVoiceCatalogEntry {
    pub key: &'static str,
    pub id: &'static str,
    pub title: &'static str,
    pub language: &'static str,
    pub author: &'static str,
    pub description: &'static str,
}

pub const JARVIS_FISH_MODELS: &[FishVoiceCatalogEntry] = &[
    FishVoiceCatalogEntry {
        key: "jarvis_ucm_ptbr",
        id: "a5b93aeddcc948c19ea04f0afe9d178c",
        title: "Jarvis (UCM) - Português Brasileiro",
        language: "pt",
        author: "Rogerio Geraldo",
        description: "Voz principal do Jarvis (UCM) em Português Brasileiro (mais popular).",
    },
    FishVoiceCatalogEntry {
        key: "jarvis_homem_de_ferro_ptbr",
        id: "d9a100f6d45f43dea41bd0c160d7e578",
        title: "Jarvis PT-BR 2 (Jarvis do filme Homem de Ferro)",
        language: "pt",
        author: "tijay70487",
        description: "Jarvis do filme Homem de Ferro dublado em PT-BR.",
    },
    FishVoiceCatalogEntry {
        key: "jarvis_ptbr_v2",
        id: "82d13948027e4be69892dd3d0104e681",
        title: "Jarvis (PT-BR)",
        language: "pt",
        author: "Victor santos",
        description: "Voz alternativa do Jarvis em Português Brasileiro.",
    },
    FishVoiceCatalogEntry {
        key: "jarvis_portugues",
        id: "ec426c7ea3554caba8a5b077a4c701aa",
        title: "Jarvis português",
        language: "pt",
        author: "tijay70487",
        description: "Voz calma e profissional do Jarvis em Português.",
    },
    FishVoiceCatalogEntry {
        key: "jarvis_mcu_en",
        id: "612b878b113047d9a770c069c8b4fdfe",
        title: "Jarvis (MCU) J.A.R.V.I.S",
        language: "en",
        author: "Fish Audio Community",
        description: "Voz original em Inglês do J.A.R.V.I.S (Paul Bettany - MCU).",
    },
];

/// Retorna o perfil correspondente à chave informada ou o perfil padrão (`"rimuru"`).
pub fn get_profile(key: Option<&str>) -> &'static VoiceProfile {
    let normalized = key.unwrap_or("").trim().to_ascii_lowercase();
    PROFILES
        .iter()
        .find(|p| p.key == normalized)
        .unwrap_or(&PROFILES[0])
}

/// Lista as chaves de todos os perfis de voz disponíveis.
pub fn profile_keys() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.key).collect()
}

/// Parâmetros de voz resolvidos a partir do `Config` e do `VoiceProfile` ativo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVoiceSettings {
    pub profile: &'static VoiceProfile,
    pub tts_provider: String,
    pub fish_reference_id: String,
    pub fish_model: String,
    pub edge_voice: String,
    pub rate: String,
    pub pitch: String,
    pub volume: String,
}

impl ResolvedVoiceSettings {
    /// Combina as configurações explícitas do `Config` com os padrões do perfil ativo.
    pub fn from_config(cfg: &Config) -> Self {
        let profile = get_profile(Some(&cfg.voice_profile));

        let tts_provider = if cfg.tts_provider.trim().is_empty() {
            profile.provider.to_string()
        } else {
            cfg.tts_provider.trim().to_string()
        };

        // Um reference_id que é a voz de algum perfil embutido (o config antigo guardava o
        // da Clássica/Jarvis) não sobrepõe o perfil escolhido; só uma voz própria sobrepõe.
        let explicit = cfg.fish_audio_reference_id.trim();
        let builtin = explicit == DEFAULT_FISH_REFERENCE_ID || PROFILES.iter().any(|p| p.fish_model_id == explicit);
        let fish_reference_id = if explicit.is_empty() || builtin {
            profile.fish_model_id.to_string()
        } else {
            cfg.fish_audio_reference_id.trim().to_string()
        };

        let fish_model = if cfg.fish_audio_model.trim().is_empty() {
            profile.fish_tier.to_string()
        } else {
            cfg.fish_audio_model.trim().to_string()
        };

        let edge_voice = if cfg.voice_name.trim().is_empty() {
            profile.edge_voice.to_string()
        } else {
            cfg.voice_name.trim().to_string()
        };

        let rate = if cfg.voice_rate.trim().is_empty() {
            profile.rate.to_string()
        } else {
            cfg.voice_rate.trim().to_string()
        };

        let pitch = if cfg.voice_pitch.trim().is_empty() {
            profile.pitch.to_string()
        } else {
            cfg.voice_pitch.trim().to_string()
        };

        let volume = if cfg.voice_volume.trim().is_empty() {
            profile.volume.to_string()
        } else {
            cfg.voice_volume.trim().to_string()
        };

        Self {
            profile,
            tts_provider,
            fish_reference_id,
            fish_model,
            edge_voice,
            rate,
            pitch,
            volume,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_profile_returns_known_profiles_and_defaults_to_rimuru() {
        assert_eq!(get_profile(Some("rimuru")).key, "rimuru");
        assert_eq!(get_profile(Some("  SABIO ")).key, "sabio");
        assert_eq!(get_profile(Some("classica")).key, "classica");
        assert_eq!(get_profile(Some("desconhecido")).key, "rimuru");
        assert_eq!(get_profile(None).key, "rimuru");
        assert_eq!(profile_keys(), vec!["rimuru", "sabio", "classica"]);
    }

    #[test]
    fn resolved_settings_inherit_from_profile_when_config_is_empty() {
        let mut cfg = Config::default();
        cfg.voice_profile = "sabio".to_string();

        let resolved = ResolvedVoiceSettings::from_config(&cfg);
        assert_eq!(resolved.profile.key, "sabio");
        assert_eq!(resolved.fish_reference_id, "e4b076f01a4442eeb91d026393bc21ce");
        assert_eq!(resolved.fish_model, "s2.1-pro-free");
        assert_eq!(resolved.edge_voice, "pt-BR-FranciscaNeural");
        assert_eq!(resolved.rate, "+3%");
        assert_eq!(resolved.pitch, "+2Hz");
        assert_eq!(resolved.volume, "+0%");
    }

    #[test]
    fn builtin_reference_id_does_not_override_chosen_profile() {
        let mut cfg = Config::default();
        cfg.voice_profile = "rimuru".to_string();
        cfg.fish_audio_reference_id = DEFAULT_FISH_REFERENCE_ID.to_string();
        let resolved = ResolvedVoiceSettings::from_config(&cfg);
        assert_eq!(resolved.fish_reference_id, "9dfb8ae2b936473d892167cc3f3a7233");
    }

    #[test]
    fn resolved_settings_respect_explicit_config_overrides() {
        let mut cfg = Config::default();
        cfg.voice_profile = "classica".to_string();
        cfg.fish_audio_reference_id = "custom_ref_123".to_string();
        cfg.voice_name = "pt-BR-AntonioNeural".to_string();
        cfg.voice_rate = "+12%".to_string();

        let resolved = ResolvedVoiceSettings::from_config(&cfg);
        assert_eq!(resolved.profile.key, "classica");
        assert_eq!(resolved.fish_reference_id, "custom_ref_123");
        assert_eq!(resolved.edge_voice, "pt-BR-AntonioNeural");
        assert_eq!(resolved.rate, "+12%");
        assert_eq!(resolved.pitch, "-2Hz");
    }
}
