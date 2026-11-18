//! Thread dedicada de reprodução de áudio via `rodio 0.23` (`OutputStreamBuilder`, `Player`, `Decoder`)
//! e fila assíncrona de síntese TTS.
//!
//! Como `rodio::OutputStream` é `!Send`, todo o hardware de saída de som permanece
//! confinado na thread `"sage-audio"`, que recebe comandos por canal.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink as OutputStream, Player};
use sage_core::{AppStatus, Config, EventBus};

use crate::media::MediaFocus;
use crate::tts::synthesize_with_fallback;

enum AudioCommand {
    EnqueueSpeech {
        mp3_bytes: Vec<u8>,
        generation: u64,
    },
    StopSpeech,
    CheckIdle,
    PlayCue {
        path: PathBuf,
        volume: f32,
    },
    ReloadSfx,
}

struct TtsJob {
    text: String,
    cfg: Config,
    generation: u64,
}

/// Motor de áudio não-bloqueante (`Send + Sync`) que coordena a fila de TTS e a thread do `rodio`.
#[derive(Clone)]
pub struct AudioEngine {
    audio_tx: mpsc::Sender<AudioCommand>,
    tts_tx: mpsc::Sender<TtsJob>,
    speech_generation: Arc<AtomicU64>,
    pending_tts_jobs: Arc<AtomicUsize>,
    speaking_flag: Arc<AtomicBool>,
}

impl AudioEngine {
    pub fn new(bus: EventBus, media_focus: MediaFocus) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel::<AudioCommand>();
        let (tts_tx, tts_rx) = mpsc::channel::<TtsJob>();
        let speech_generation = Arc::new(AtomicU64::new(1));
        let pending_tts_jobs = Arc::new(AtomicUsize::new(0));
        let speaking_flag = Arc::new(AtomicBool::new(false));

        // Thread dedicada de saída de áudio (rodio::OutputStream é !Send).
        {
            let gen_ref = Arc::clone(&speech_generation);
            let pending_ref = Arc::clone(&pending_tts_jobs);
            let speaking_ref = Arc::clone(&speaking_flag);
            let bus_ref = bus.clone();
            let media_ref = media_focus.clone();

            let _ = std::thread::Builder::new()
                .name("sage-audio".into())
                .spawn(move || {
                    run_audio_thread(
                        audio_rx,
                        gen_ref,
                        pending_ref,
                        speaking_ref,
                        bus_ref,
                        media_ref,
                    );
                });
        }

        // Thread dedicada para requisições HTTP / WebSocket de síntese TTS em ordem FIFO.
        {
            let audio_tx_tts = audio_tx.clone();
            let gen_ref = Arc::clone(&speech_generation);
            let pending_ref = Arc::clone(&pending_tts_jobs);

            let _ = std::thread::Builder::new()
                .name("sage-tts".into())
                .spawn(move || {
                    run_tts_worker_thread(tts_rx, audio_tx_tts, gen_ref, pending_ref);
                });
        }

        Self {
            audio_tx,
            tts_tx,
            speech_generation,
            pending_tts_jobs,
            speaking_flag,
        }
    }

    /// Enfileira texto já higienizado para síntese TTS e reprodução não-bloqueante.
    pub fn speak_prepared(&self, text: String, cfg: Config) {
        if text.trim().is_empty() {
            return;
        }
        let generation = self.speech_generation.load(Ordering::SeqCst);
        self.pending_tts_jobs.fetch_add(1, Ordering::SeqCst);
        if self
            .tts_tx
            .send(TtsJob {
                text,
                cfg,
                generation,
            })
            .is_err()
        {
            self.pending_tts_jobs.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Interrompe imediatamente a fala atual e descarta quaisquer frases na fila.
    pub fn stop_speaking(&self) {
        self.speech_generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.audio_tx.send(AudioCommand::StopSpeech);
    }

    /// Retorna `true` se o assistente está reproduzindo fala no momento.
    pub fn is_speaking(&self) -> bool {
        self.speaking_flag.load(Ordering::SeqCst)
    }

    /// Reproduz um efeito sonoro de evento de forma não-bloqueante.
    pub fn play_cue(&self, path: PathBuf, volume: f32) {
        let _ = self.audio_tx.send(AudioCommand::PlayCue { path, volume });
    }

    /// Limpa o cache em memória de arquivos de efeito sonoro.
    pub fn reload_sfx(&self) {
        let _ = self.audio_tx.send(AudioCommand::ReloadSfx);
    }
}

fn run_tts_worker_thread(
    rx: mpsc::Receiver<TtsJob>,
    audio_tx: mpsc::Sender<AudioCommand>,
    speech_generation: Arc<AtomicU64>,
    pending_tts_jobs: Arc<AtomicUsize>,
) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        tracing::error!("[Voz] Não foi possível inicializar o runtime Tokio para TTS");
        return;
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    while let Ok(job) = rx.recv() {
        if job.generation != speech_generation.load(Ordering::SeqCst) {
            pending_tts_jobs.fetch_sub(1, Ordering::SeqCst);
            let _ = audio_tx.send(AudioCommand::CheckIdle);
            continue;
        }

        let synth_res = rt.block_on(synthesize_with_fallback(&client, &job.text, &job.cfg));
        match synth_res {
            Ok(mp3_bytes) => {
                if job.generation == speech_generation.load(Ordering::SeqCst) {
                    let _ = audio_tx.send(AudioCommand::EnqueueSpeech {
                        mp3_bytes,
                        generation: job.generation,
                    });
                }
            }
            Err(e) => {
                tracing::warn!("[Voz] Falha na síntese de fala (Fish Audio + Edge TTS): {e}");
            }
        }

        pending_tts_jobs.fetch_sub(1, Ordering::SeqCst);
        let _ = audio_tx.send(AudioCommand::CheckIdle);
    }
}

fn ensure_output_stream<'a>(
    stream_slot: &'a mut Option<OutputStream>,
    speech_player_slot: &mut Option<Player>,
) -> Option<&'a OutputStream> {
    if stream_slot.is_none() {
        match DeviceSinkBuilder::open_default_sink() {
            Ok(stream) => {
                let player = Player::connect_new(stream.mixer());
                *speech_player_slot = Some(player);
                *stream_slot = Some(stream);
            }
            Err(e) => {
                tracing::warn!("[Áudio] Não foi possível abrir dispositivo de saída padrão: {e}");
                return None;
            }
        }
    }
    stream_slot.as_ref()
}

fn run_audio_thread(
    rx: mpsc::Receiver<AudioCommand>,
    speech_generation: Arc<AtomicU64>,
    pending_tts_jobs: Arc<AtomicUsize>,
    speaking_flag: Arc<AtomicBool>,
    bus: EventBus,
    media_focus: MediaFocus,
) {
    let mut stream_slot: Option<OutputStream> = None;
    let mut speech_player: Option<Player> = None;
    let mut cue_players: Vec<Player> = Vec::new();
    let mut sfx_cache: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let mut speaking_active = false;

    loop {
        let cmd = if speaking_active || !cue_players.is_empty() {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => break,
            }
        };

        if let Some(command) = cmd {
            match command {
                AudioCommand::EnqueueSpeech {
                    mp3_bytes,
                    generation,
                } => {
                    if generation != speech_generation.load(Ordering::SeqCst) {
                        continue;
                    }
                    if ensure_output_stream(&mut stream_slot, &mut speech_player).is_none() {
                        continue;
                    }
                    let Some(player) = speech_player.as_ref() else {
                        continue;
                    };
                    match Decoder::try_from(Cursor::new(mp3_bytes)) {
                        Ok(decoder) => {
                            if !speaking_active {
                                speaking_active = true;
                                speaking_flag.store(true, Ordering::SeqCst);
                                media_focus.hold("speak");
                                bus.status(AppStatus::Speaking, "Falando...");
                            }
                            player.append(decoder);
                            player.play();
                        }
                        Err(e) => {
                            tracing::warn!("[Áudio] Falha ao decodificar MP3 de fala: {e}");
                        }
                    }
                }
                AudioCommand::StopSpeech => {
                    if let Some(player) = speech_player.take() {
                        player.stop();
                        player.clear();
                    }
                    if let Some(stream) = stream_slot.as_ref() {
                        speech_player = Some(Player::connect_new(stream.mixer()));
                    }
                    if speaking_active {
                        speaking_active = false;
                        speaking_flag.store(false, Ordering::SeqCst);
                        media_focus.release("speak");
                        bus.status(AppStatus::Idle, "");
                    }
                }
                AudioCommand::CheckIdle => {
                    // Tratado na verificação ao final do laço.
                }
                AudioCommand::PlayCue { path, volume } => {
                    cue_players.retain(|p| !p.empty());
                    let Some(stream) = ensure_output_stream(&mut stream_slot, &mut speech_player)
                    else {
                        continue;
                    };
                    let bytes = if let Some(cached) = sfx_cache.get(&path) {
                        cached.clone()
                    } else {
                        match std::fs::read(&path) {
                            Ok(b) => {
                                sfx_cache.insert(path.clone(), b.clone());
                                b
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[SFX] Não foi possível ler arquivo de som {}: {e}",
                                    path.display()
                                );
                                continue;
                            }
                        }
                    };

                    match Decoder::try_from(Cursor::new(bytes)) {
                        Ok(decoder) => {
                            let player = Player::connect_new(stream.mixer());
                            player.set_volume(volume.clamp(0.0, 1.0));
                            player.append(decoder);
                            cue_players.push(player);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[SFX] Falha ao decodificar arquivo de som {}: {e}",
                                path.display()
                            );
                        }
                    }
                }
                AudioCommand::ReloadSfx => {
                    sfx_cache.clear();
                }
            }
        }

        // Limpa reprodutores de efeitos sonoros que já terminaram.
        cue_players.retain(|p| !p.empty());

        // Se a fala terminou e não há outras sínteses pendentes na fila, volta para Idle.
        if speaking_active {
            let player_empty = match speech_player.as_ref() {
                Some(p) => p.empty(),
                None => true,
            };
            let no_pending_tts = pending_tts_jobs.load(Ordering::SeqCst) == 0;
            if player_empty && no_pending_tts {
                speaking_active = false;
                speaking_flag.store(false, Ordering::SeqCst);
                media_focus.release("speak");
                bus.status(AppStatus::Idle, "");
            }
        }
    }
}
