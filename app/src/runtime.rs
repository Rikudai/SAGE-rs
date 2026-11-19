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
            // Auto-edição: muda o próprio código numa cópia, testa, aplica e reinicia.
            Arc::new(sage_tools::selfedit::SelfEdit::new(
                sage_core::paths::root().to_path_buf(),
                swap_hub.clone() as Arc<dyn ModelHub>,
                bus.clone(),
            )),
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
        // Tarefas longas vão para o segundo plano; o chat segue respondendo.
        brain.enable_background_tasks();
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
        announce_updates(&rt, bus.clone());
        if snapshot.tts_provider.eq_ignore_ascii_case("local") {
            // Já carrega o modelo de voz local, para a primeira fala não demorar.
            sage_voice::tts::start_local_server();
        }

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
        self.ask_with_images(text, from_voice, Vec::new());
    }

    /// Pedido com imagens coladas no chat (caminhos de PNG).
    pub fn ask_with_images(&self, text: String, from_voice: bool, images: Vec<String>) {
        let tx = self.cmd.clone();
        self.rt.spawn(async move {
            let _ = tx.send(Command::Ask { text, from_voice, images }).await;
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
                    let _ = tx.send(Command::Ask { text, from_voice: true, images: vec![] }).await;
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

/// Avisa o resultado do `scripts/reiniciar-atualizado.ps1` ("Estou atualizado..."): olha o
/// `data/resultado_atualizacao.json` ao abrir e de tempos em tempos (o script também grava
/// ali quando a compilação falha com o S.A.G.E. ainda aberto).
fn announce_updates(rt: &tokio::runtime::Runtime, bus: EventBus) {
    let path = sage_core::paths::data().join("resultado_atualizacao.json");
    rt.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        loop {
            if let Some(text) = take_update_result(&path) {
                sage_tools::update::log_line(sage_core::paths::root(), &format!("[anúncio] {text}"));
                bus.emit(SageEvent::Reply { spoken: text.clone(), full: text, emotion: Some("feliz".into()) });
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

/// Lê o resultado ainda não anunciado e marca como anunciado.
pub fn take_update_result(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut v: serde_json::Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    if v.get("anunciado").and_then(serde_json::Value::as_bool).unwrap_or(true) {
        return None;
    }
    v["anunciado"] = serde_json::Value::Bool(true);
    let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    let get = |k: &str| v.get(k).and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string();
    let (status, resumo, testes, commit) = (get("status"), get("resumo"), get("testes"), get("commit"));
    let mut extra = String::new();
    if !resumo.is_empty() {
        extra.push_str(&format!(" {}.", resumo.trim_end_matches('.')));
    }
    if !testes.is_empty() {
        extra.push_str(&format!(" Testes: {testes}."));
    }
    let versao = if commit.is_empty() { String::new() } else { format!(" ({commit})") };
    Some(match status.as_str() {
        "atualizado" => format!("Estou atualizado e rodando a versão nova{versao}.{extra}"),
        "revertido" => format!("A versão nova não abriu direito, então voltei para a anterior.{extra} Os detalhes estão em logs/atualizacao.log."),
        "falhou_build" => format!("Não me atualizei: a compilação falhou e continuo na versão atual.{extra}"),
        _ => format!("A atualização não terminou bem ({status}).{extra} Veja logs/atualizacao.log."),
    })
}

/// O S.A.G.E. abriu de verdade: o script de reinício espera por este arquivo (com o pid).
pub fn write_health() {
    let v = serde_json::json!({ "pid": std::process::id(), "at": chrono::Local::now().to_rfc3339() });
    let _ = std::fs::write(sage_core::paths::data().join("saude.json"), v.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_each_update_result_once() {
        let dir = std::env::temp_dir().join(format!("sage-anuncio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("resultado_atualizacao.json");
        let body = "\u{feff}{\"status\":\"atualizado\",\"resumo\":\"Voz local\",\"testes\":\"200 testes passaram\",\"commit\":\"abc1234\",\"anunciado\":false}";
        std::fs::write(&p, body).unwrap();
        let t = take_update_result(&p).expect("anúncio");
        assert!(t.starts_with("Estou atualizado") && t.contains("Voz local") && t.contains("200 testes passaram"), "{t}");
        assert!(take_update_result(&p).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
