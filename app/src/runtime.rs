//! Liga os crates: hub de modelos, ferramentas, cérebro e voz num runtime tokio.

use anyhow::Result;
use async_trait::async_trait;
use sage_core::config::ModelRef;
use sage_core::events::Command;
use sage_core::llm::{AgentCli, Llm, Transcriber};
use sage_core::tools::{ToolResult, ToolSpec};
use sage_core::{Config, EventBus, ModelHub, SageEvent, Toolbox};
use serde_json::Value;
use std::sync::{Arc, PoisonError, RwLock};
use tokio::sync::mpsc;

/// Hub de modelos trocável: o cérebro guarda este `Arc`, e salvar as configurações
/// troca o hub de dentro (chaves, provedores e modelos novos valem no próximo pedido).
struct SwapHub(RwLock<Arc<dyn ModelHub>>);

impl SwapHub {
    fn get(&self) -> Arc<dyn ModelHub> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ModelHub for SwapHub {
    fn llm(&self, m: &ModelRef) -> Option<Arc<dyn Llm>> {
        self.get().llm(m)
    }
    fn agent(&self, provider: &str) -> Option<Arc<dyn AgentCli>> {
        self.get().agent(provider)
    }
    fn available(&self) -> Vec<String> {
        self.get().available()
    }
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        self.get().transcriber()
    }
}

/// Ferramentas do PC + memória + skills, numa caixa só (cada nome vai para a dona).
struct SageTools {
    pc: Arc<dyn Toolbox>,
    extra: Vec<Arc<dyn Toolbox>>,
}

#[async_trait]
impl Toolbox for SageTools {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut v = self.pc.specs();
        for t in &self.extra {
            v.extend(t.specs());
        }
        v
    }
    async fn call(&self, name: &str, args: Value) -> ToolResult {
        for t in &self.extra {
            if t.specs().iter().any(|s| s.name == name) {
                return t.call(name, args).await;
            }
        }
        self.pc.call(name, args).await
    }
}

/// Ferramentas trocáveis (liga/desliga ações no PC e PowerShell na hora).
struct SwapTools(RwLock<Arc<dyn Toolbox>>);

#[async_trait]
impl Toolbox for SwapTools {
    fn specs(&self) -> Vec<ToolSpec> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).specs()
    }
    async fn call(&self, name: &str, args: Value) -> ToolResult {
        let tools = self.0.read().unwrap_or_else(PoisonError::into_inner).clone();
        tools.call(name, args).await
    }
}

pub struct Runtime {
    pub rt: tokio::runtime::Runtime,
    pub bus: EventBus,
    pub cmd: mpsc::Sender<Command>,
    pub voice: Arc<sage_voice::Voice>,
    hub: Arc<SwapHub>,
    tools: Arc<SwapTools>,
    pub memory: Arc<sage_tools::memory::MemoryStore>,
    extra: Vec<Arc<dyn Toolbox>>,
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

        let swap_hub = Arc::new(SwapHub(RwLock::new(Arc::new(sage_providers::Hub::new(&snapshot)?))));
        let memory = sage_tools::memory::MemoryStore::open(sage_core::paths::data().join("memory"));
        let skills = Arc::new(sage_tools::skills::SkillCatalog::new(sage_core::paths::root()));
        let extra: Vec<Arc<dyn Toolbox>> = vec![
            Arc::new(sage_tools::memory::MemoryTools(memory.clone())),
            Arc::new(sage_tools::skills::SkillTools(skills)),
            Arc::new(sage_tools::update::UpdateTools { root: sage_core::paths::root().to_path_buf() }),
        ];
        let pc: Arc<dyn Toolbox> = Arc::new(sage_tools::PcToolbox::new(&snapshot));
        let swap_tools = Arc::new(SwapTools(RwLock::new(Arc::new(SageTools { pc, extra: extra.clone() }))));
        let hub: Arc<dyn ModelHub> = swap_hub.clone();
        let tools: Arc<dyn Toolbox> = swap_tools.clone();
        let voice = Arc::new(sage_voice::Voice::new(&snapshot, hub.transcriber(), bus.clone()));

        let (tx, rx) = mpsc::channel::<Command>(64);
        let brain = {
            let _g = rt.enter();
            let mem: Arc<dyn sage_core::MemoryHook> = memory.clone();
            sage_brain::Brain::new_with_memory(cfg.clone(), hub, tools, bus.clone(), Some(mem))?
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

        startup_update(&rt, &snapshot, bus.clone());

        Ok(Self { rt, bus, cmd: tx, voice, hub: swap_hub, tools: swap_tools, memory, extra })
    }

    /// Aplica uma config nova sem reiniciar: modelos/chaves, ferramentas, voz e playbooks.
    pub fn reload(&self, cfg: &Config) {
        match sage_providers::Hub::new(cfg) {
            Ok(h) => {
                let h: Arc<dyn ModelHub> = Arc::new(h);
                self.voice.set_transcriber(h.transcriber());
                *self.hub.0.write().unwrap_or_else(PoisonError::into_inner) = h;
            }
            Err(e) => tracing::warn!("config nova: não consegui montar os provedores ({e:#}); mantive os antigos"),
        }
        let pc: Arc<dyn Toolbox> = Arc::new(sage_tools::PcToolbox::new(cfg));
        *self.tools.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(SageTools { pc, extra: self.extra.clone() });
        self.voice.update_config(cfg);
        self.send(Command::ReloadConfig);
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

/// Ao abrir: procura a versão nova no GitHub (porte do `startup_check` do SAGE antigo).
/// "auto" baixa, compila e instala (vale no próximo reinício); "check" só avisa; "off" nada.
/// Roda em segundo plano, depois de a janela abrir, e nunca atrapalha a abertura.
fn startup_update(rt: &tokio::runtime::Runtime, cfg: &Config, bus: EventBus) {
    use sage_tools::update;
    let mode = cfg.update_on_start.trim().to_lowercase();
    let skip = std::env::var("SAGE_SKIP_UPDATE").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "sim"));
    let root = sage_core::paths::root().to_path_buf();
    if mode == "off" || skip || !root.join(".git").exists() {
        return;
    }
    rt.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let st = match update::status(&root, 30).await {
            Ok(s) => s,
            Err(e) => {
                update::log_line(&root, &format!("[ao abrir] não consegui verificar: {e}"));
                return;
            }
        };
        if st.incoming.is_empty() {
            update::log_line(&root, "[ao abrir] já estou na versão mais nova");
            return;
        }
        let n = st.incoming.len();
        let titles = st.incoming.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
        if mode == "check" || st.ahead > 0 {
            let why = if st.ahead > 0 { " (tenho commits locais que não estão lá, então não apliquei)" } else { "" };
            let text = format!("Tem {n} atualização(ões) no GitHub{why}: {titles}. Diga \"se atualize\" para eu instalar.");
            update::log_line(&root, &format!("[ao abrir] {text}"));
            bus.emit(SageEvent::Notice { text });
            return;
        }
        bus.emit(SageEvent::Notice { text: format!("Baixando {n} novidade(s) do GitHub: {titles}...") });
        let r = update::apply(&root).await;
        update::log_line(&root, &format!("[ao abrir] {}", r.message));
        let text = if r.ok && r.changed {
            format!("{} Diga \"reinicie\" quando quiser.", r.message)
        } else {
            r.message
        };
        bus.emit(SageEvent::Notice { text });
    });
}
