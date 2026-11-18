//! Controle de foco de mídia do sistema (portado de `legacy/media_focus.py`).
//!
//! Pausa apenas as sessões de mídia que estavam tocando quando o S.A.G.E. começa
//! a ouvir ou falar, e retoma exclusivamente o que ele mesmo pausou após uma breve
//! folga (`grace`) quando o último detentor libera o foco.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

/// Folga padrão (1,2 s) antes de retomar a mídia após liberar o foco,
/// evitando piscar a música entre "terminou de ouvir" e "começou a responder".
pub const DEFAULT_GRACE_DURATION: Duration = Duration::from_millis(1200);

/// Contrato do backend de controle de mídia do sistema.
pub trait MediaBackend: Send + Sync {
    /// Pausa todas as sessões atualmente em reprodução (`Playing`) e devolve seus IDs (`SourceAppUserModelId`).
    fn pause_playing(&self) -> anyhow::Result<Vec<String>>;

    /// Retoma apenas as sessões cujos IDs estão na lista fornecida.
    fn resume(&self, sessions: &[String]) -> anyhow::Result<()>;
}

/// Backend nulo para plataformas sem SMTC ou fallback seguro.
#[derive(Debug, Default)]
pub struct NoopMediaBackend;

impl MediaBackend for NoopMediaBackend {
    fn pause_playing(&self) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn resume(&self, _sessions: &[String]) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
mod winrt {
    use super::MediaBackend;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    /// Backend Windows via `GlobalSystemMediaTransportControlsSessionManager` (SMTC).
    #[derive(Debug, Default)]
    pub struct WinRtMediaBackend;

    impl WinRtMediaBackend {
        async fn pause_playing_async() -> windows::core::Result<Vec<String>> {
            let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
            let sessions = manager.GetSessions()?;
            let mut paused = Vec::new();
            for session in sessions {
                let Ok(info) = session.GetPlaybackInfo() else {
                    continue;
                };
                let Ok(status) = info.PlaybackStatus() else {
                    continue;
                };
                if status != GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing {
                    continue;
                }
                let Ok(op) = session.TryPauseAsync() else {
                    continue;
                };
                if !op.await.unwrap_or(false) {
                    continue;
                }
                let Ok(id) = session.SourceAppUserModelId() else {
                    continue;
                };
                let id_str = id.to_string();
                if !id_str.is_empty() && !paused.contains(&id_str) {
                    paused.push(id_str);
                }
            }
            Ok(paused)
        }

        async fn resume_async(wanted: &[String]) -> windows::core::Result<()> {
            if wanted.is_empty() {
                return Ok(());
            }
            let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
            let sessions = manager.GetSessions()?;
            for session in sessions {
                let Ok(id) = session.SourceAppUserModelId() else {
                    continue;
                };
                let id_str = id.to_string();
                if !wanted.iter().any(|w| w == &id_str) {
                    continue;
                }
                if let Ok(op) = session.TryPlayAsync() {
                    let _ = op.await;
                }
            }
            Ok(())
        }
    }

    impl MediaBackend for WinRtMediaBackend {
        fn pause_playing(&self) -> anyhow::Result<Vec<String>> {
            futures::executor::block_on(Self::pause_playing_async())
                .map_err(|e| anyhow::anyhow!("Falha ao pausar mídia via SMTC: {e}"))
        }

        fn resume(&self, sessions: &[String]) -> anyhow::Result<()> {
            futures::executor::block_on(Self::resume_async(sessions))
                .map_err(|e| anyhow::anyhow!("Falha ao retomar mídia via SMTC: {e}"))
        }
    }
}

/// Cria o backend de mídia padrão do sistema operacional.
pub fn default_media_backend() -> Arc<dyn MediaBackend> {
    #[cfg(windows)]
    {
        Arc::new(winrt::WinRtMediaBackend)
    }
    #[cfg(not(windows))]
    {
        Arc::new(NoopMediaBackend)
    }
}

enum MediaJob {
    Pause,
    Resume,
    Sync(Arc<(Mutex<bool>, Condvar)>),
}

struct FocusState {
    holders: HashSet<String>,
    generation: u64,
}

/// Coordenador que controla quem segura o silêncio da mídia (`"listen"`, `"speak"`).
#[derive(Clone)]
pub struct MediaFocus {
    state: Arc<Mutex<FocusState>>,
    enabled: Arc<AtomicBool>,
    grace: Duration,
    tx: mpsc::Sender<MediaJob>,
}

impl MediaFocus {
    /// Cria um `MediaFocus` com o backend padrão do sistema.
    pub fn new(enabled: bool) -> Self {
        Self::with_backend(default_media_backend(), enabled, DEFAULT_GRACE_DURATION)
    }

    /// Cria um `MediaFocus` com backend e tempo de folga customizados (útil para testes).
    pub fn with_backend(
        backend: Arc<dyn MediaBackend>,
        enabled: bool,
        grace: Duration,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<MediaJob>();
        let state = Arc::new(Mutex::new(FocusState {
            holders: HashSet::new(),
            generation: 0,
        }));
        let enabled_flag = Arc::new(AtomicBool::new(enabled));

        let _ = std::thread::Builder::new()
            .name("sage-media-focus".into())
            .spawn(move || {
                let mut paused_sessions: Vec<String> = Vec::new();
                while let Ok(job) = rx.recv() {
                    match job {
                        MediaJob::Pause => {
                            if !paused_sessions.is_empty() {
                                continue;
                            }
                            match backend.pause_playing() {
                                Ok(paused) => {
                                    if !paused.is_empty() {
                                        tracing::debug!(
                                            "[Mídia] Pausei para falar/ouvir: {}",
                                            paused.join(", ")
                                        );
                                        paused_sessions = paused;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "[Mídia] Não consegui pausar a mídia do sistema: {e}"
                                    );
                                }
                            }
                        }
                        MediaJob::Resume => {
                            let sessions = std::mem::take(&mut paused_sessions);
                            if !sessions.is_empty() {
                                match backend.resume(&sessions) {
                                    Ok(()) => {
                                        tracing::debug!(
                                            "[Mídia] Retomei: {}",
                                            sessions.join(", ")
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "[Mídia] Não consegui retomar a mídia do sistema: {e}"
                                        );
                                    }
                                }
                            }
                        }
                        MediaJob::Sync(pair) => {
                            let (lock, cvar) = &*pair;
                            if let Ok(mut done) = lock.lock() {
                                *done = true;
                                cvar.notify_all();
                            }
                        }
                    }
                }
            });

        Self {
            state,
            enabled: enabled_flag,
            grace,
            tx,
        }
    }

    /// Atualiza se a pausa automática de mídia está habilitada (`pause_media_while_speaking`).
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Retorna se a pausa automática está habilitada.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Retorna `true` se há pelo menos um detentor ativo (`"listen"` ou `"speak"`).
    pub fn is_holding(&self) -> bool {
        self.state
            .lock()
            .map(|g| !g.holders.is_empty())
            .unwrap_or(false)
    }

    /// Solicita silêncio de mídia em nome de `who` (`"listen"` ou `"speak"`).
    /// Na primeira aquisição, envia comando para pausar o que estiver tocando.
    pub fn hold(&self, who: &str) {
        let first = {
            let Ok(mut guard) = self.state.lock() else {
                return;
            };
            guard.generation = guard.generation.wrapping_add(1);
            let was_empty = guard.holders.is_empty();
            guard.holders.insert(who.to_string());
            was_empty
        };

        if first && self.enabled.load(Ordering::SeqCst) {
            let _ = self.tx.send(MediaJob::Pause);
        }
    }

    /// Libera o foco de `who` usando a folga (`grace`) padrão.
    pub fn release(&self, who: &str) {
        self.release_with_grace(who, self.grace);
    }

    /// Libera o foco de `who`; quando ninguém mais segura, retoma a mídia após `grace`.
    pub fn release_with_grace(&self, who: &str, grace: Duration) {
        let epoch = {
            let Ok(mut guard) = self.state.lock() else {
                return;
            };
            if !guard.holders.remove(who) {
                return;
            }
            if !guard.holders.is_empty() {
                return;
            }
            guard.generation
        };

        if grace.is_zero() {
            let should_resume = self
                .state
                .lock()
                .map(|g| g.holders.is_empty() && g.generation == epoch)
                .unwrap_or(false);
            if should_resume {
                let _ = self.tx.send(MediaJob::Resume);
            }
            return;
        }

        let state = Arc::clone(&self.state);
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new()
            .name("sage-media-grace".into())
            .spawn(move || {
                std::thread::sleep(grace);
                let should_resume = state
                    .lock()
                    .map(|g| g.holders.is_empty() && g.generation == epoch)
                    .unwrap_or(false);
                if should_resume {
                    let _ = tx.send(MediaJob::Resume);
                }
            });
    }

    /// Aguarda o processamento de todas as pausas/retomadas enfileiradas até o momento.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        if self.tx.send(MediaJob::Sync(Arc::clone(&pair))).is_err() {
            return false;
        }
        let (lock, cvar) = &*pair;
        let Ok(guard) = lock.lock() else {
            return false;
        };
        cvar.wait_timeout_while(guard, timeout, |done| !*done)
            .map(|(_, res)| !res.timed_out())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockMediaBackend {
        playing: Mutex<Vec<String>>,
        pause_calls: Mutex<usize>,
        resumed: Mutex<Vec<Vec<String>>>,
    }

    impl MediaBackend for MockMediaBackend {
        fn pause_playing(&self) -> anyhow::Result<Vec<String>> {
            if let Ok(mut c) = self.pause_calls.lock() {
                *c += 1;
            }
            Ok(self
                .playing
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default())
        }

        fn resume(&self, sessions: &[String]) -> anyhow::Result<()> {
            if let Ok(mut r) = self.resumed.lock() {
                r.push(sessions.to_vec());
            }
            Ok(())
        }
    }

    #[test]
    fn pauses_only_once_across_overlapping_holders_and_resumes_what_it_paused() {
        let mock = Arc::new(MockMediaBackend {
            playing: Mutex::new(vec!["Spotify.exe".into(), "chrome.exe".into()]),
            ..Default::default()
        });
        let focus = MediaFocus::with_backend(mock.clone(), true, Duration::ZERO);

        focus.hold("listen");
        focus.hold("speak");
        assert!(focus.is_holding());
        focus.wait_idle(Duration::from_secs(2));
        assert_eq!(*mock.pause_calls.lock().unwrap(), 1);

        // Soltar "listen" enquanto "speak" ainda segura não deve retomar a mídia.
        focus.release_with_grace("listen", Duration::ZERO);
        assert!(focus.is_holding());
        focus.wait_idle(Duration::from_secs(2));
        assert!(mock.resumed.lock().unwrap().is_empty());

        // Soltar "speak" retoma exatamente as sessões que foram pausadas.
        focus.release_with_grace("speak", Duration::ZERO);
        assert!(!focus.is_holding());
        focus.wait_idle(Duration::from_secs(2));
        assert_eq!(
            *mock.resumed.lock().unwrap(),
            vec![vec!["Spotify.exe".to_string(), "chrome.exe".to_string()]]
        );
    }

    #[test]
    fn does_nothing_when_disabled_or_when_nothing_was_playing() {
        let mock = Arc::new(MockMediaBackend::default());
        let focus = MediaFocus::with_backend(mock.clone(), false, Duration::ZERO);

        focus.hold("speak");
        focus.release_with_grace("speak", Duration::ZERO);
        focus.wait_idle(Duration::from_secs(2));
        assert_eq!(*mock.pause_calls.lock().unwrap(), 0);
        assert!(mock.resumed.lock().unwrap().is_empty());

        // Mesmo habilitado, se nada estava tocando, nunca dispara resume.
        focus.set_enabled(true);
        focus.hold("speak");
        focus.release_with_grace("speak", Duration::ZERO);
        focus.wait_idle(Duration::from_secs(2));
        assert_eq!(*mock.pause_calls.lock().unwrap(), 1);
        assert!(mock.resumed.lock().unwrap().is_empty());
    }
}
