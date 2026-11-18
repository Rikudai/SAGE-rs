//! Liga os crates: hub de modelos, ferramentas, cérebro e voz num runtime tokio.

use anyhow::Result;
use sage_core::events::Command;
use sage_core::{Config, EventBus, ModelHub, SageEvent, Toolbox};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;

pub struct Runtime {
    pub rt: tokio::runtime::Runtime,
    pub bus: EventBus,
    pub cmd: mpsc::Sender<Command>,
    pub voice: Arc<sage_voice::Voice>,
}

impl Runtime {
    pub fn start(cfg: Arc<RwLock<Config>>) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("sage-rt")
            .build()?;
        let bus = EventBus::new();
        let snapshot = cfg.read().map_err(|_| anyhow::anyhow!("config travada"))?.clone();

        let hub: Arc<dyn ModelHub> = Arc::new(sage_providers::Hub::new(&snapshot)?);
        let tools: Arc<dyn Toolbox> = Arc::new(sage_tools::PcToolbox::new(&snapshot));
        let voice = Arc::new(sage_voice::Voice::new(&snapshot, hub.transcriber(), bus.clone()));

        let (tx, rx) = mpsc::channel::<Command>(64);
        let brain = {
            let _g = rt.enter();
            sage_brain::Brain::new(cfg.clone(), hub, tools, bus.clone())?
        };
        rt.spawn(brain.run(rx));

        // Fala as respostas e toca os sons de evento.
        {
            let voice = voice.clone();
            let cfg = cfg.clone();
            let mut sub = bus.subscribe();
            rt.spawn(async move {
                while let Ok(ev) = sub.recv().await {
                    let voice_on = cfg.read().map(|c| c.voice_enabled).unwrap_or(true);
                    match ev {
                        SageEvent::Reply { spoken, .. } if voice_on && !spoken.trim().is_empty() => {
                            voice.speak(&spoken);
                        }
                        SageEvent::WorkerStart { .. } => voice.cue("clone_spawn"),
                        SageEvent::WorkerEnd { .. } => voice.cue("clone_done"),
                        _ => {}
                    }
                }
            });
        }

        Ok(Self { rt, bus, cmd: tx, voice })
    }

    pub fn ask(&self, text: String, from_voice: bool) {
        let tx = self.cmd.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Command::Ask { text, from_voice }).await;
        });
    }

    pub fn send(&self, c: Command) {
        let tx = self.cmd.clone();
        self.rt.spawn(async move {
            let _ = tx.send(c).await;
        });
    }

    /// Segurar para falar: começa a gravar.
    pub fn ptt_down(&self) {
        self.voice.stop_speaking();
        self.voice.start_listening();
    }

    /// Soltou: transcreve e manda ao cérebro.
    pub fn ptt_up(&self) {
        let voice = self.voice.clone();
        let tx = self.cmd.clone();
        self.rt.spawn(async move {
            match voice.finish_listening().await {
                Ok(text) if !text.trim().is_empty() => {
                    let _ = tx.send(Command::Ask { text, from_voice: true }).await;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("voz: {e:#}"),
            }
        });
    }
}
