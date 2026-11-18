//! `sage-voice`: captura de microfone push-to-talk / modo toque (`cpal`),
//! transcrição STT (`Transcriber`), síntese de voz Fish Audio + fallback Edge TTS (`msedge-tts`),
//! reprodução não-bloqueante em thread própria (`rodio 0.23`), pausa inteligente de mídia
//! do sistema (`Windows.Media.Control` SMTC) e banco de efeitos sonoros (`assets/sfx`).

pub mod media;
pub mod player;
pub mod profiles;
pub mod recorder;
pub mod sfx;
pub mod tts;

use std::borrow::Borrow;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use sage_core::{AppStatus, Config, EventBus, Transcriber};

pub use media::{MediaBackend, MediaFocus};
pub use profiles::{PROFILES, ResolvedVoiceSettings, VoiceProfile, get_profile, profile_keys};
pub use recorder::{
    Recorder, SilenceDetector, TARGET_SAMPLE_RATE, encode_wav_16k_mono, resample_mono,
};
pub use sfx::{SoundConfig, SoundEventEntry, normalize_cue_name};
pub use tts::{FISH_AUDIO_TTS_URL, FishTtsRequest, build_fish_request_body, prepare_speech_text};

use player::AudioEngine;

/// Limpa a transcrição bruta aplicando `sage_core::text::fix_transcription`
/// e `sage_core::text::strip_wake_word`.
pub fn clean_transcription(raw: &str, wake_words: &[String]) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let fixed = sage_core::text::fix_transcription(trimmed);
    let stripped = sage_core::text::strip_wake_word(&fixed, wake_words);
    stripped.trim().to_string()
}

/// Subsistema principal de voz e áudio do S.A.G.E. (`Send + Sync`).
#[derive(Clone)]
pub struct Voice {
    cfg: Arc<RwLock<Config>>,
    transcriber: Arc<RwLock<Option<Arc<dyn Transcriber>>>>,
    bus: EventBus,
    recorder: Arc<Recorder>,
    engine: AudioEngine,
    media_focus: MediaFocus,
    sound_cfg: Arc<RwLock<SoundConfig>>,
    sfx_dir: PathBuf,
}

impl Voice {
    /// Cria uma nova instância de `Voice`.
    /// Aceita tanto `&Config` quanto `Config` (via `Borrow<Config>`).
    pub fn new(
        cfg: impl Borrow<Config>,
        transcriber: Option<Arc<dyn Transcriber>>,
        bus: EventBus,
    ) -> Self {
        let cfg_owned = cfg.borrow().clone();
        let media_focus = MediaFocus::new(cfg_owned.pause_media_while_speaking);
        let engine = AudioEngine::new(bus.clone(), media_focus.clone());
        let sound_cfg = SoundConfig::load();
        let sfx_dir = sage_core::paths::assets().join("sfx");

        Self {
            cfg: Arc::new(RwLock::new(cfg_owned)),
            transcriber: Arc::new(RwLock::new(transcriber)),
            bus,
            recorder: Arc::new(Recorder::new()),
            engine,
            media_focus,
            sound_cfg: Arc::new(RwLock::new(sound_cfg)),
            sfx_dir,
        }
    }

    /// Atualiza a configuração em tempo de execução.
    pub fn update_config(&self, cfg: &Config) {
        self.media_focus.set_enabled(cfg.pause_media_while_speaking);
        if let Ok(mut guard) = self.cfg.write() {
            *guard = cfg.clone();
        }
    }

    /// Atualiza o provedor de transcrição STT em tempo de execução.
    pub fn set_transcriber(&self, transcriber: Option<Arc<dyn Transcriber>>) {
        if let Ok(mut guard) = self.transcriber.write() {
            *guard = transcriber;
        }
    }

    /// Recarrega `data/sound_config.json` e limpa o cache de efeitos sonoros.
    pub fn reload_sfx(&self) {
        if let Ok(mut guard) = self.sound_cfg.write() {
            *guard = SoundConfig::load();
        }
        self.engine.reload_sfx();
    }

    /// Higieniza o texto (`sanitize_for_speech` + `pronounce_name`), sintetiza via Fish Audio
    /// (com fallback para Edge TTS) e reproduz o MP3 de forma não-bloqueante.
    pub fn speak(&self, text: &str) {
        let cfg_snapshot = match self.cfg.read() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        if !cfg_snapshot.voice_enabled {
            return;
        }
        let prepared = prepare_speech_text(text, cfg_snapshot.max_speech_sentences);
        if prepared.trim().is_empty() {
            return;
        }
        self.media_focus
            .set_enabled(cfg_snapshot.pause_media_while_speaking);
        self.engine.speak_prepared(prepared, cfg_snapshot);
    }

    /// Interrompe imediatamente qualquer fala em andamento ou na fila.
    pub fn stop_speaking(&self) {
        self.engine.stop_speaking();
    }

    /// Apelido para [`Voice::stop_speaking`].
    pub fn stop(&self) {
        self.stop_speaking();
    }

    /// Retorna `true` se o assistente está falando no momento.
    pub fn is_speaking(&self) -> bool {
        self.engine.is_speaking()
    }

    /// Toca um som de evento (`start`, `listen`, `done`, `error`, `clone_spawn`, `clone_done`...)
    /// de forma não-bloqueante.
    pub fn cue(&self, name: &str) {
        let sfx_enabled = self
            .cfg
            .read()
            .map(|c| c.enable_sfx && c.play_audio_cues)
            .unwrap_or(true);
        if !sfx_enabled {
            return;
        }
        let resolved = self
            .sound_cfg
            .read()
            .ok()
            .and_then(|sc| sc.resolve_path(name, &self.sfx_dir));
        if let Some((path, volume)) = resolved {
            self.engine.play_cue(path, volume);
        }
    }

    /// Inicia a captura de áudio push-to-talk no microfone padrão.
    pub fn start_listening(&self) {
        self.stop_speaking();

        let pause_media = self
            .cfg
            .read()
            .map(|c| c.pause_media_while_speaking)
            .unwrap_or(false);
        self.media_focus.set_enabled(pause_media);
        if pause_media {
            self.media_focus.hold("listen");
        }

        self.cue("listen_start");
        self.bus.status(AppStatus::Listening, "Ouvindo...");

        if let Err(e) = self.recorder.start_with_mode(false) {
            tracing::warn!("[Voz] Erro ao iniciar captura de microfone: {e}");
            self.media_focus.release("listen");
            self.cue("error");
            self.bus
                .status(AppStatus::Error, format!("Erro no microfone: {e}"));
        }
    }

    /// Encerra a captura iniciada por [`Voice::start_listening`], transcreve em `"pt-BR"`
    /// e devolve o texto limpo (`fix_transcription` + `strip_wake_word`).
    pub async fn finish_listening(&self) -> anyhow::Result<String> {
        let wav_res = self.recorder.stop_async().await;
        self.media_focus.release("listen");

        let wav = match wav_res {
            Ok(bytes) => {
                self.cue("listen_stop");
                bytes
            }
            Err(e) => {
                self.cue("error");
                self.bus.status(AppStatus::Idle, "");
                return Err(e);
            }
        };

        self.transcribe_and_clean(wav).await
    }

    /// Grava uma frase no modo toque (parada automática após 2,5 s de silêncio depois da fala
    /// ou no limite de 120 s), emite `Status::Listening`/`Status::Transcribing`, transcreve
    /// em `"pt-BR"` e devolve o texto limpo.
    pub async fn listen_once(&self) -> anyhow::Result<String> {
        self.stop_speaking();

        let pause_media = self
            .cfg
            .read()
            .map(|c| c.pause_media_while_speaking)
            .unwrap_or(false);
        self.media_focus.set_enabled(pause_media);
        if pause_media {
            self.media_focus.hold("listen");
        }

        self.cue("listen_start");
        self.bus.status(AppStatus::Listening, "Ouvindo...");

        if let Err(e) = self.recorder.start_with_mode(true) {
            self.media_focus.release("listen");
            self.cue("error");
            self.bus.status(AppStatus::Idle, "");
            return Err(e);
        }

        let wav_res = self.recorder.wait_for_completion().await;
        self.media_focus.release("listen");

        let wav = match wav_res {
            Ok(bytes) => {
                self.cue("listen_stop");
                bytes
            }
            Err(e) => {
                self.cue("error");
                self.bus.status(AppStatus::Idle, "");
                return Err(e);
            }
        };

        self.transcribe_and_clean(wav).await
    }

    async fn transcribe_and_clean(&self, wav: Vec<u8>) -> anyhow::Result<String> {
        self.bus
            .status(AppStatus::Transcribing, "Transcrevendo...");

        let transcriber = self.transcriber.read().ok().and_then(|g| g.clone());
        let Some(transcriber) = transcriber else {
            self.bus.status(AppStatus::Idle, "");
            anyhow::bail!("Nenhum transcritor de voz configurado");
        };

        let raw = match transcriber.transcribe(wav, "pt-BR").await {
            Ok(t) => t,
            Err(e) => {
                self.cue("error");
                self.bus.status(AppStatus::Idle, "");
                return Err(e);
            }
        };

        let wake_words = self
            .cfg
            .read()
            .map(|c| c.wake_words.clone())
            .unwrap_or_default();
        let cleaned = clean_transcription(&raw, &wake_words);
        self.bus.status(AppStatus::Idle, "");
        Ok(cleaned)
    }

    /// Acesso ao gravador subjacente (`start()` / `stop()`).
    pub fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    /// Acesso ao coordenador de foco de mídia.
    pub fn media_focus(&self) -> &MediaFocus {
        &self.media_focus
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn voice_is_send_and_sync_and_accepts_borrowed_or_owned_config() {
        assert_send_sync::<Voice>();

        let cfg = Config::default();
        let bus = EventBus::new();
        let _v1 = Voice::new(&cfg, None, bus.clone());
        let _v2 = Voice::new(cfg, None, bus);
    }

    #[test]
    fn clean_transcription_fixes_terms_and_strips_wake_words() {
        let wake = vec!["Seije".to_string(), "Nexo".to_string()];
        let out = clean_transcription("CG, abri o Beach Hub e o Power Shell", &wake);
        assert_eq!(out, "abri o GitHub e o PowerShell");

        let out2 = clean_transcription("Nexo, verifique o vs code", &wake);
        assert_eq!(out2, "verifique o VS Code");
    }
}
