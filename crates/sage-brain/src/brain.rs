//! `Brain`: the command loop, triage (chat / one action / task) and the orchestrated run:
//! pick a playbook, plan, run waves of workers, gate them, synthesize, report.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::future::join_all;
use serde_json::Value;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use sage_core::config::Config;
use sage_core::events::{AppStatus, Command, EventBus, SageEvent};
use sage_core::llm::{
    AgentCli, AgentMode, ChatRequest, ChatResponse, Llm, LlmError, Message, ModelHub, ModelRef, Tier, Usage,
    WorkerStatus,
};
use sage_core::tools::{MemoryHook, ToolResult, Toolbox};

use crate::budget::{Budget, BudgetLimits, BudgetState, WhenLow};
use crate::confirm::Confirmer;
use crate::gate::{self, Attempts, GateReply, NextStep, Verdict};
use crate::jsonx::{self, clip};
use crate::lock;
use crate::plan::{self, Plan, PlaybookChoice, UnitKind, UnitSpec, ValidatedPlan};
use crate::playbooks::{
    Playbook, Playbooks, QuotaPolicy, RuleContext, is_cli_provider, match_rule, tier_down, tier_name,
};
use crate::prompts::{CorePrompts, Prompts};
use crate::quota::{QuotaState, SharedQuota};
use crate::report::{GateRecord, RunReport, RunStatus, WaveReport, WorkerReport, status_label};
use crate::templates;
use crate::worker::{self, ApiJob, CliJob, WorkerEnv, WorkerRun};

/// Unit id / phase id used when the lead does the request itself.
const LEAD_UNIT: &str = "lead";
const LEAD_PHASE: &str = "lead";
/// `text::complexity` from which a "chat" triage is treated as a task.
const TASK_COMPLEXITY: f32 = 0.55;
const TURN_QUEUE: usize = 16;
/// Tarefas orquestradas ao mesmo tempo em segundo plano (as outras esperam a vez).
const MAX_BACKGROUND_TASKS: usize = 3;
const DEFAULT_MAX_UNITS: usize = 12;
const STOPPED_NOTE: &str = "Parei a execução, como você pediu.";

/// Tunables of the brain.
#[derive(Debug, Clone)]
pub struct BrainSettings {
    /// Conversation turns sent to triage and chat.
    pub context_turns: usize,
    /// Model calls one API worker may make.
    pub worker_max_steps: u32,
    /// Wall time of one API worker.
    pub worker_max_secs: u64,
    /// How long a `ConfirmRequest` waits for an answer.
    pub confirm_timeout_secs: u64,
}

impl Default for BrainSettings {
    fn default() -> Self {
        Self { context_turns: 8, worker_max_steps: 12, worker_max_secs: 300, confirm_timeout_secs: 120 }
    }
}

/// Everything [`Brain::with_options`] needs besides the shared services.
pub struct BrainOptions {
    pub playbooks: Playbooks,
    /// Re-read on `Command::ReloadConfig` when set.
    pub playbooks_path: Option<PathBuf>,
    pub prompts: Arc<dyn Prompts>,
    /// Holds `quota.json` and `runs/`.
    pub data_dir: PathBuf,
    pub settings: BrainSettings,
    pub memory: Option<Arc<dyn MemoryHook>>,
}

impl BrainOptions {
    pub fn new(playbooks: Playbooks, data_dir: PathBuf) -> Self {
        Self {
            playbooks,
            playbooks_path: None,
            prompts: Arc::new(CorePrompts),
            data_dir,
            settings: BrainSettings::default(),
            memory: None,
        }
    }

    pub fn with_prompts(mut self, prompts: Arc<dyn Prompts>) -> Self {
        self.prompts = prompts;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    Chat,
    Action,
    Task,
}

/// What one request produced (also emitted as `SageEvent::Reply`).
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub kind: TurnKind,
    pub spoken: String,
    pub full: String,
    pub emotion: Option<String>,
    pub report: Option<RunReport>,
    pub report_path: Option<PathBuf>,
    /// A tarefa foi para o segundo plano: o resultado chega depois, numa `Reply` própria.
    pub background: bool,
}

impl TurnOutcome {
    fn text(kind: TurnKind, text: impl Into<String>, emotion: Option<String>) -> Self {
        let full = text.into();
        Self { kind, spoken: full.clone(), full, emotion, report: None, report_path: None, background: false }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Triage {
    kind: TurnKind,
    reply: String,
    action: Option<String>,
    params: Value,
    emotion: Option<String>,
}

fn parse_triage(text: &str) -> Option<Triage> {
    let v = jsonx::extract_object(text)?;
    let kind = match jsonx::str_field(&v, "kind").trim().to_lowercase().as_str() {
        "chat" => TurnKind::Chat,
        "action" => TurnKind::Action,
        "task" => TurnKind::Task,
        _ => return None,
    };
    let params = match v.get("action_params") {
        Some(p @ Value::Object(_)) => p.clone(),
        _ => Value::Object(serde_json::Map::new()),
    };
    Some(Triage {
        kind,
        reply: jsonx::str_field(&v, "reply").trim().to_string(),
        action: jsonx::opt_str(&v, "action").filter(|a| !a.eq_ignore_ascii_case("none")),
        params,
        emotion: jsonx::opt_str(&v, "emotion"),
    })
}

/// The parts of `Config` the brain reads, copied out of the lock.
#[derive(Debug, Clone)]
struct CfgView {
    user_name: Option<String>,
    profile: Option<String>,
    max_workers: Option<u32>,
    max_cost: Option<f64>,
    cli_timeout: u64,
    agent_dirs: Vec<String>,
    /// `model_config.brain` + `fallbacks`, a fallback lead chain.
    brain_chain: Vec<ModelRef>,
    fast: Option<ModelRef>,
    full_access: bool,
}

fn usable_ref(m: &ModelRef) -> bool {
    !m.provider.trim().is_empty() && !m.model.trim().is_empty()
}

fn cfg_view(cfg: &Config) -> CfgView {
    let mut brain_chain = vec![cfg.model_config.brain.clone()];
    brain_chain.extend(cfg.model_config.fallbacks.iter().cloned());
    brain_chain.retain(usable_ref);
    CfgView {
        user_name: cfg.user_name.clone().filter(|n| !n.trim().is_empty()),
        profile: cfg.brain_profile.clone(),
        max_workers: cfg.brain_max_workers,
        max_cost: cfg.brain_max_cost_usd,
        cli_timeout: cfg.cli_timeout_seconds.unwrap_or(900),
        agent_dirs: cfg.claude_agent_dirs.clone(),
        brain_chain,
        fast: Some(cfg.model_config.fast.clone()).filter(usable_ref),
        full_access: cfg.full_access,
    }
}

fn dedup(chain: Vec<ModelRef>) -> Vec<ModelRef> {
    let mut out: Vec<ModelRef> = Vec::new();
    for m in chain {
        if !out.iter().any(|o| o.provider == m.provider && o.model == m.model) {
            out.push(m);
        }
    }
    out
}

fn new_run_id() -> String {
    let short: String = uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect();
    format!("{}-{short}", Utc::now().format("%Y%m%d-%H%M%S"))
}

/// One orchestrated run.
struct RunCtx {
    request: String,
    pb: Arc<Playbooks>,
    profile: String,
    policy: QuotaPolicy,
    cfg: CfgView,
    env: WorkerEnv,
    lead_chain: Vec<ModelRef>,
    /// O que a memória lembrou quando a tarefa começou (o chat segue mudando o `recall`).
    recall: String,
    /// The user's answer to "every provider is low, continue?", asked once per run.
    low_quota_ok: tokio::sync::Mutex<Option<bool>>,
}

/// A lead call: chain, prompts, where to stream.
struct Call<'a> {
    chain: &'a [ModelRef],
    system: String,
    user: String,
    stream: Option<&'a str>,
    max_tokens: Option<u32>,
}

enum Selection {
    Api(Vec<(ModelRef, Arc<dyn Llm>)>),
    Cli(Vec<(ModelRef, Arc<dyn AgentCli>)>),
}

/// What a unit accumulated over its attempts.
struct Tally {
    started: Instant,
    attempts: u32,
    usage: Usage,
    tool_calls: u32,
    gates: Vec<GateRecord>,
}

fn worker_report(
    unit: &UnitSpec,
    run: Option<&WorkerRun>,
    tier: Tier,
    tally: Tally,
    used: bool,
    note: Option<String>,
) -> WorkerReport {
    let (provider, model, status, summary, output) = match run {
        Some(r) => (
            r.model.provider.clone(),
            r.model.label(),
            r.outcome.status,
            r.outcome.summary.trim().to_string(),
            clip(r.outcome.text.trim(), 8000),
        ),
        None => (String::new(), String::new(), WorkerStatus::Failed, String::new(), String::new()),
    };
    let summary = match note {
        Some(n) if summary.is_empty() => n,
        Some(n) => format!("{summary} ({n})"),
        None => summary,
    };
    WorkerReport {
        id: unit.id.clone(),
        phase: unit.phase.clone(),
        role: unit.role.clone(),
        kind: unit.kind.as_str().to_string(),
        provider,
        model,
        tier,
        duration_secs: tally.started.elapsed().as_secs_f64(),
        usage: tally.usage,
        status,
        summary,
        output,
        attempts: tally.attempts,
        tool_calls: tally.tool_calls,
        gates: tally.gates,
        used,
    }
}

/// The unit the lead runs itself when the plan has no waves (playbook `direct`).
fn lead_unit(request: &str, playbook: &Playbook) -> UnitSpec {
    let phase = playbook.phases.iter().find(|p| p.is_lead() && !p.read_only);
    UnitSpec {
        id: LEAD_UNIT.to_string(),
        phase: LEAD_PHASE.to_string(),
        role: "lead".to_string(),
        brief: request.trim().to_string(),
        kind: UnitKind::Pc,
        owned_paths: Vec::new(),
        tier: Tier::Strong,
        tags: Vec::new(),
        read_only: false,
        isolate: false,
        does: phase
            .map(|p| p.does.clone())
            .unwrap_or_else(|| "Resolva o pedido você mesmo, com as ferramentas.".to_string()),
        returns: Some("O resultado com evidências.".to_string()),
        gate: None,
        cwd: None,
        provider: None,
        effort: None,
        rules: Vec::new(),
    }
}

fn unit_cwd(ctx: &RunCtx, unit: &UnitSpec) -> PathBuf {
    unit.cwd
        .iter()
        .chain(ctx.cfg.agent_dirs.iter())
        .map(PathBuf::from)
        .find(|p| p.is_dir())
        .unwrap_or_else(|| sage_core::paths::root().to_path_buf())
}

fn fallback_synthesis(waves: &[WaveReport], missing: &[String], note: Option<&str>) -> (String, String, Option<String>) {
    let mut full = String::new();
    if let Some(n) = note {
        full.push_str(n);
        full.push_str("\n\n");
    }
    for r in waves.iter().flat_map(|w| w.workers.iter()) {
        full.push_str(&format!("- **{}** ({}): {}\n", r.id, status_label(r.status), r.summary.trim()));
    }
    if !missing.is_empty() {
        full.push_str(&format!("\nFicou faltando: {}\n", missing.join(", ")));
    }
    let spoken = note.map_or_else(|| "Terminei. Os detalhes estão no chat.".to_string(), str::to_string);
    (spoken, full.trim().to_string(), None)
}

struct Inner {
    cfg: Arc<RwLock<Config>>,
    hub: Arc<dyn ModelHub>,
    tools: Arc<dyn Toolbox>,
    bus: EventBus,
    prompts: Arc<dyn Prompts>,
    playbooks: RwLock<Arc<Playbooks>>,
    playbooks_path: Option<PathBuf>,
    quota: SharedQuota,
    runs_dir: PathBuf,
    history: Mutex<VecDeque<(String, String)>>,
    confirmer: Confirmer,
    /// Cancellation of the request in progress.
    current: Mutex<Option<CancellationToken>>,
    settings: BrainSettings,
    /// Memória de longo prazo (opcional) e o que ela lembrou para o pedido atual.
    memory: Option<Arc<dyn MemoryHook>>,
    recall: Mutex<String>,
    /// Tarefas longas rodam em segundo plano e o chat segue livre (ligado pelo app).
    background: AtomicBool,
    /// Tarefas em segundo plano: chave -> (título, cancelamento).
    bg_runs: Mutex<BTreeMap<String, (String, CancellationToken)>>,
    bg_slots: Arc<Semaphore>,
    /// Imagens do pedido atual (vão na última mensagem do usuário).
    images: Mutex<Vec<Vec<u8>>>,
}

/// The orchestrator brain. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Brain {
    inner: Arc<Inner>,
}

impl Brain {
    /// Loads `config/playbooks.yaml` and keeps quota and runs under `data/`.
    pub fn new(
        cfg: Arc<RwLock<Config>>,
        hub: Arc<dyn ModelHub>,
        tools: Arc<dyn Toolbox>,
        bus: EventBus,
    ) -> anyhow::Result<Self> {
        Self::new_with_memory(cfg, hub, tools, bus, None)
    }

    /// Igual a `new`, com memória de longo prazo (lembra antes de responder e registra cada turno).
    pub fn new_with_memory(
        cfg: Arc<RwLock<Config>>,
        hub: Arc<dyn ModelHub>,
        tools: Arc<dyn Toolbox>,
        bus: EventBus,
        memory: Option<Arc<dyn MemoryHook>>,
    ) -> anyhow::Result<Self> {
        let path = sage_core::paths::playbooks_file();
        let playbooks = Playbooks::load(&path)?;
        let mut opts = BrainOptions::new(playbooks, sage_core::paths::data());
        opts.playbooks_path = Some(path);
        opts.memory = memory;
        Ok(Self::with_options(cfg, hub, tools, bus, opts))
    }

    pub fn with_options(
        cfg: Arc<RwLock<Config>>,
        hub: Arc<dyn ModelHub>,
        tools: Arc<dyn Toolbox>,
        bus: EventBus,
        opts: BrainOptions,
    ) -> Self {
        let quota = SharedQuota::load(opts.data_dir.join("quota.json"));
        let confirmer = Confirmer::new(bus.clone(), Duration::from_secs(opts.settings.confirm_timeout_secs));
        let runs_dir = opts.data_dir.join("runs");
        Self {
            inner: Arc::new(Inner {
                cfg,
                hub,
                tools,
                bus,
                prompts: opts.prompts,
                playbooks: RwLock::new(Arc::new(opts.playbooks)),
                playbooks_path: opts.playbooks_path,
                quota,
                runs_dir,
                history: Mutex::new(VecDeque::new()),
                confirmer,
                current: Mutex::new(None),
                settings: opts.settings,
                memory: opts.memory,
                recall: Mutex::new(String::new()),
                background: AtomicBool::new(false),
                bg_runs: Mutex::new(BTreeMap::new()),
                bg_slots: Arc::new(Semaphore::new(MAX_BACKGROUND_TASKS)),
                images: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn bus(&self) -> &EventBus {
        &self.inner.bus
    }

    pub fn quota(&self) -> &SharedQuota {
        &self.inner.quota
    }

    pub fn playbooks(&self) -> Arc<Playbooks> {
        self.inner.playbooks.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// A request is being handled.
    pub fn is_busy(&self) -> bool {
        lock(&self.inner.current).is_some()
    }

    /// Tarefas orquestradas passam a rodar em segundo plano: o S.A.G.E. responde na hora
    /// e continua conversando (e aceitando novas tarefas) enquanto elas rodam.
    pub fn enable_background_tasks(&self) {
        self.inner.background.store(true, Ordering::Relaxed);
    }

    /// Títulos das tarefas rodando em segundo plano.
    pub fn background_tasks(&self) -> Vec<String> {
        lock(&self.inner.bg_runs).values().map(|(t, _)| t.clone()).collect()
    }

    /// Cancels the request in progress and every background task (`Command::StopRun`);
    /// false when idle.
    pub fn stop(&self) -> bool {
        let mut any = false;
        if let Some(t) = lock(&self.inner.current).as_ref() {
            t.cancel();
            any = true;
        }
        for (_, t) in lock(&self.inner.bg_runs).values() {
            t.cancel();
            any = true;
        }
        any
    }

    /// Answers a `ConfirmRequest` (`Command::Confirm`).
    pub fn confirm(&self, id: &str, yes: bool) -> bool {
        self.inner.confirmer.answer(id, yes)
    }

    /// Re-reads `playbooks.yaml` when the brain was built from a file.
    pub fn reload_playbooks(&self) -> anyhow::Result<()> {
        let Some(path) = &self.inner.playbooks_path else {
            return Ok(());
        };
        let fresh = Playbooks::load(path)?;
        *self.inner.playbooks.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(fresh);
        Ok(())
    }

    /// Main loop: consumes commands until the channel closes. Requests run one at a time
    /// on their own task, so `Confirm` and `StopRun` are handled while one is running.
    pub async fn run(self, mut rx: mpsc::Receiver<Command>) {
        let (turn_tx, mut turn_rx) = mpsc::channel::<(String, bool, Vec<String>)>(TURN_QUEUE);
        let brain = self.clone();
        let turns = tokio::spawn(async move {
            while let Some((text, from_voice, images)) = turn_rx.recv().await {
                brain.ask_with_images(&text, from_voice, &images).await;
            }
        });
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Command::Ask { text, from_voice, images } => {
                    if self.is_busy() {
                        self.notice("Anotado; respondo assim que terminar o pedido atual.");
                    }
                    match turn_tx.try_send((text, from_voice, images)) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            self.notice("Tem pedidos demais na fila; espere eu terminar.");
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
                Command::Confirm { id, yes } => {
                    if !self.confirm(&id, yes) {
                        tracing::debug!("confirmação {id} sem pergunta pendente");
                    }
                }
                Command::StopRun => {
                    self.stop();
                }
                Command::ReloadConfig => {
                    if let Err(e) = self.reload_playbooks() {
                        self.notice(&format!("Não consegui recarregar o playbooks.yaml: {e}"));
                    }
                }
            }
        }
        drop(turn_tx);
        self.stop();
        if let Err(e) = turns.await {
            tracing::warn!("a fila de pedidos terminou com erro: {e}");
        }
    }

    /// Handles one request end to end and emits its `Reply`.
    pub async fn ask(&self, text: &str, from_voice: bool) -> TurnOutcome {
        self.ask_with_images(text, from_voice, &[]).await
    }

    /// Igual a `ask`, com imagens coladas no chat (caminhos de PNG): vão para o modelo
    /// junto com a mensagem, e os caminhos entram no pedido para os workers abrirem.
    pub async fn ask_with_images(&self, text: &str, from_voice: bool, images: &[String]) -> TurnOutcome {
        let bytes: Vec<Vec<u8>> = images.iter().filter_map(|p| std::fs::read(p).ok()).collect();
        *lock(&self.inner.images) = bytes;
        let with_paths;
        let text = if images.is_empty() {
            text
        } else {
            let base = if text.trim().is_empty() { "O que você vê nesta imagem?" } else { text.trim() };
            with_paths = format!("{base}

[Imagens coladas pelo usuário: {}]", images.join(", "));
            with_paths.as_str()
        };
        let out = self.ask_inner(text, from_voice).await;
        lock(&self.inner.images).clear();
        out
    }

    async fn ask_inner(&self, text: &str, from_voice: bool) -> TurnOutcome {
        let text = text.trim();
        let cancel = CancellationToken::new();
        *lock(&self.inner.current) = Some(cancel.clone());
        self.inner.bus.emit(SageEvent::UserText { text: text.to_string(), from_voice });
        self.inner.bus.status(AppStatus::Thinking, "Pensando...");
        if let Some(m) = &self.inner.memory {
            let m = m.clone();
            let q = text.to_string();
            let block = tokio::task::spawn_blocking(move || m.recall(&q)).await.unwrap_or_default();
            *lock(&self.inner.recall) = block;
        }
        let out = if text.is_empty() {
            TurnOutcome::text(TurnKind::Chat, "Não entendi; pode repetir?", None)
        } else {
            self.turn(text, &cancel).await
        };
        if let Some(m) = &self.inner.memory
            && !text.is_empty()
        {
            let m = m.clone();
            let (u, r) = (text.to_string(), out.full.clone());
            tokio::task::spawn_blocking(move || m.observe(&u, &r));
        }
        *lock(&self.inner.current) = None;
        if out.background {
            // A confirmação já foi dita; o resultado vem numa resposta própria.
            self.push_history(text, "(tarefa iniciada em segundo plano; o resultado chega quando terminar)");
            self.idle_if_free();
            return out;
        }
        if !text.is_empty() {
            self.push_history(text, &out.full);
        }
        self.inner.bus.emit(SageEvent::Reply {
            spoken: out.spoken.clone(),
            full: out.full.clone(),
            emotion: out.emotion.clone(),
        });
        if let Some(e) = &out.emotion {
            self.inner.bus.emit(SageEvent::Emotion { name: e.clone() });
        }
        self.idle_if_free();
        out
    }

    /// Volta o status para ocioso, ou para "N tarefas em segundo plano".
    fn idle_if_free(&self) {
        if self.is_busy() {
            return;
        }
        let n = lock(&self.inner.bg_runs).len();
        if n == 0 {
            self.inner.bus.status(AppStatus::Idle, "");
        } else {
            let s = if n == 1 { "1 tarefa em segundo plano".to_string() } else { format!("{n} tarefas em segundo plano") };
            self.inner.bus.status(AppStatus::Idle, s);
        }
    }

    /// Orquestra a tarefa: em segundo plano quando ligado (responde na hora e libera o chat),
    /// senão aqui mesmo.
    async fn start_task(&self, text: &str, cfg: &CfgView, cancel: &CancellationToken) -> TurnOutcome {
        let recall = lock(&self.inner.recall).clone();
        if !self.inner.background.load(Ordering::Relaxed) {
            return self.orchestrate(text, cfg, cancel, recall).await;
        }
        let key = new_run_id();
        let token = CancellationToken::new();
        let title = short_title(text);
        lock(&self.inner.bg_runs).insert(key.clone(), (title, token.clone()));
        let (me, req, cfg) = (self.clone(), text.to_string(), cfg.clone());
        tokio::spawn(async move {
            let permit = me.inner.bg_slots.clone().acquire_owned().await.ok();
            let out = me.orchestrate(&req, &cfg, &token, recall).await;
            drop(permit);
            lock(&me.inner.bg_runs).remove(&key);
            if let Some(m) = &me.inner.memory {
                let m = m.clone();
                let (u, r) = (req.clone(), out.full.clone());
                tokio::task::spawn_blocking(move || m.observe(&u, &r));
            }
            me.push_history(&format!("(resultado da tarefa) {}", clip(&req, 300)), &out.full);
            me.inner.bus.emit(SageEvent::Reply { spoken: out.spoken.clone(), full: out.full.clone(), emotion: out.emotion.clone() });
            if let Some(e) = &out.emotion {
                me.inner.bus.emit(SageEvent::Emotion { name: e.clone() });
            }
            me.idle_if_free();
        });
        TurnOutcome { background: true, ..TurnOutcome::text(TurnKind::Task, "", None) }
    }

    // ------------------------------------------------------------------ helpers

    /// Acrescenta ao prompt de sistema o que a memória lembrou para este pedido.
    fn with_memory(&self, system: String) -> String {
        let recall = lock(&self.inner.recall).clone();
        let mut system = self.with_recall(system, &recall);
        let bg = self.background_tasks();
        if !bg.is_empty() {
            system.push_str(&format!(
                "\n\nTAREFAS RODANDO EM SEGUNDO PLANO AGORA (outras instâncias suas cuidam delas; o resultado chega sozinho): {}. \
Se o usuário perguntar delas, diga que estão em andamento. Você segue livre para conversar e aceitar novas tarefas.",
                bg.join("; ")
            ));
        }
        system
    }

    fn with_recall(&self, system: String, recall: &str) -> String {
        let system = self.with_access(system);
        if recall.trim().is_empty() { system } else { format!("{system}\n\n{recall}") }
    }

    /// Acrescenta o lembrete de acesso total quando ele está ligado.
    fn with_access(&self, system: String) -> String {
        if self.cfg_view().full_access {
            format!("{system}\n\n{}", sage_core::persona::access_rules())
        } else {
            system
        }
    }

    fn notice(&self, text: &str) {
        self.inner.bus.emit(SageEvent::Notice { text: text.to_string() });
    }

    fn cfg_view(&self) -> CfgView {
        cfg_view(&self.inner.cfg.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn current_policy(&self, cfg: &CfgView) -> QuotaPolicy {
        let pb = self.playbooks();
        pb.quota_policy(&pb.active_profile(cfg.profile.as_deref()))
    }

    fn lead_chain(&self, pb: &Playbooks, cfg: &CfgView) -> Vec<ModelRef> {
        dedup(pb.lead.iter().chain(cfg.brain_chain.iter()).cloned().collect())
    }

    fn triage_chain(&self, pb: &Playbooks, cfg: &CfgView) -> Vec<ModelRef> {
        let mut chain: Vec<ModelRef> = pb.triage.clone();
        chain.extend(cfg.fast.iter().cloned());
        chain.extend(self.lead_chain(pb, cfg));
        dedup(chain)
    }

    /// A mensagem do usuário com as imagens coladas neste pedido.
    fn user_message(&self, text: &str) -> Message {
        let mut m = Message::user(text);
        m.images = lock(&self.inner.images).clone();
        m
    }

    fn history_messages(&self) -> Vec<Message> {
        lock(&self.inner.history)
            .iter()
            .flat_map(|(u, a)| [Message::user(u.clone()), Message::assistant(a.clone())])
            .collect()
    }

    fn push_history(&self, user: &str, reply: &str) {
        let mut h = lock(&self.inner.history);
        h.push_back((user.to_string(), clip(reply, 2000)));
        while h.len() > self.inner.settings.context_turns {
            h.pop_front();
        }
    }

    fn note_switch(&self, run: Option<&RunCtx>, from: &str, to: &str) {
        if from == to {
            return;
        }
        let reason = worker::switch_reason(from, to, 100.0);
        match run {
            Some(r) => r.env.switch_provider(from, to, reason),
            None => self.inner.bus.emit(SageEvent::ProviderSwitch { from: from.to_string(), to: to.to_string(), reason }),
        }
    }

    /// Tries each model of `chain` that is available and not blocked; moves on on
    /// transient errors. Returns the reply and the model that answered.
    async fn call_chain(
        &self,
        chain: &[ModelRef],
        req: &ChatRequest,
        stream: Option<&str>,
        run: Option<&RunCtx>,
        cancel: &CancellationToken,
    ) -> Result<(ChatResponse, ModelRef), LlmError> {
        let available = self.inner.hub.available();
        let policy = match run {
            Some(r) => r.policy.clone(),
            None => self.current_policy(&self.cfg_view()),
        };
        let now = Utc::now();
        let usable: Vec<&ModelRef> = chain
            .iter()
            .filter(|m| available.contains(&m.provider))
            .filter(|m| {
                // Modelos de API descansam um a um; o bloqueio do provedor inteiro vale só para as CLIs.
                if crate::quota::is_api_provider(&m.provider) {
                    !self.inner.quota.model_resting(m)
                } else {
                    self.inner.quota.read(|q| q.state(&m.provider, &policy, now)) != QuotaState::Blocked
                }
            })
            .collect();
        let mut last_err: Option<LlmError> = None;
        for (i, m) in usable.iter().enumerate() {
            let Some(llm) = self.inner.hub.llm(m) else { continue };
            let mut this_req = req.clone();
            if m.effort.is_some() {
                this_req.thinking.clone_from(&m.effort);
            }
            let sink = stream.map(|s| worker::stream_sink(&self.inner.bus, s));
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(LlmError::Other(anyhow::anyhow!("execução cancelada"))),
                r = llm.chat(this_req, sink) => r,
            };
            match res {
                Ok(resp) => {
                    if let Some(run) = run {
                        lock(&run.env.budget).record(&resp.usage);
                    }
                    return Ok((resp, ModelRef::clone(m)));
                }
                Err(e) => {
                    let limited = if crate::quota::is_api_provider(&m.provider) {
                        self.inner.quota.observe_model_error(m, &e)
                    } else {
                        self.inner.quota.observe_error(&e).is_some()
                    };
                    if !e.is_transient() {
                        return Err(e);
                    }
                    if limited && let Some(next) = usable.get(i + 1)
                        && next.provider != m.provider
                    {
                        self.note_switch(run, &m.provider, &next.provider);
                    }
                    tracing::warn!("modelo {} falhou: {e}", m.label());
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| LlmError::Provider {
            provider: "nenhum".to_string(),
            detail: "nenhum modelo disponível; confira as chaves e os provedores em Configurações".to_string(),
        }))
    }

    /// A JSON call to the lead; on a reply `parse` rejects, asks once more with the error.
    async fn lead_json<T, F>(&self, call: Call<'_>, run: Option<&RunCtx>, cancel: &CancellationToken, parse: F) -> Result<T, String>
    where
        F: Fn(&str) -> Result<T, String> + Send + Sync,
        T: Send,
    {
        let mut messages = vec![Message::user(call.user)];
        let mut last_err = String::from("sem resposta do lead");
        for attempt in 0..2 {
            let req = ChatRequest {
                system: call.system.clone(),
                messages: messages.clone(),
                temperature: Some(0.3),
                max_tokens: call.max_tokens,
                json: true,
                thinking: None,
            };
            let (resp, _) =
                self.call_chain(call.chain, &req, call.stream, run, cancel).await.map_err(|e| e.to_string())?;
            match parse(&resp.text) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if attempt == 0 {
                        messages.push(Message::assistant(resp.text));
                        messages.push(Message::user(format!(
                            "Sua resposta não passou na validação: {e}. Devolva só o objeto JSON corrigido."
                        )));
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    // ------------------------------------------------------------------ triage

    async fn turn(&self, text: &str, cancel: &CancellationToken) -> TurnOutcome {
        let cfg = self.cfg_view();
        let triage = self.triage(text, &cfg, cancel).await;
        if cancel.is_cancelled() {
            return TurnOutcome::text(TurnKind::Chat, "Tudo bem, parei.", None);
        }
        let prompts = &self.inner.prompts;
        let heuristic_task = prompts.looks_like_task(text) && prompts.complexity(text) >= TASK_COMPLEXITY;
        match triage {
            Some(t) if t.kind == TurnKind::Task || (t.kind == TurnKind::Chat && heuristic_task) => {
                self.acknowledge(text, &t.reply);
                self.start_task(text, &cfg, cancel).await
            }
            Some(t) if t.kind == TurnKind::Action => match t.action {
                Some(action) => self.single_action(text, &cfg, &action, t.params, &t.reply, t.emotion, cancel).await,
                None if heuristic_task => {
                    self.acknowledge(text, &t.reply);
                    self.start_task(text, &cfg, cancel).await
                }
                None => self.chat_reply(text, &cfg, cancel).await,
            },
            Some(t) if !t.reply.is_empty() => TurnOutcome::text(TurnKind::Chat, t.reply, t.emotion),
            Some(_) => self.chat_reply(text, &cfg, cancel).await,
            None if heuristic_task => {
                self.acknowledge(text, "");
                self.start_task(text, &cfg, cancel).await
            }
            None => self.chat_reply(text, &cfg, cancel).await,
        }
    }

    /// Responde na hora que vai fazer a tarefa (falado e no chat) enquanto a
    /// orquestração começa; o resultado chega depois, numa segunda resposta.
    fn acknowledge(&self, request: &str, triage_reply: &str) {
        let reply = triage_reply.trim();
        let text = if !reply.is_empty() && reply.chars().count() <= 220 {
            reply.to_string()
        } else {
            ack_text(request).to_string()
        };
        self.inner.bus.emit(SageEvent::Reply { spoken: text.clone(), full: text, emotion: Some("determinado".into()) });
        self.inner.bus.status(AppStatus::Orchestrating, "Planejando...");
    }

    async fn triage(&self, text: &str, cfg: &CfgView, cancel: &CancellationToken) -> Option<Triage> {
        let pb = self.playbooks();
        let specs = self.inner.tools.specs();
        let system = format!(
            "{}\n\n{}",
            self.with_memory(self.inner.prompts.chat(cfg.user_name.as_deref(), false)),
            templates::triage_instructions(&specs, &self.inner.prompts.emotions())
        );
        let mut messages = self.history_messages();
        messages.push(self.user_message(text));
        let req =
            ChatRequest { system, messages, temperature: Some(0.2), max_tokens: Some(1024), json: true, thinking: None };
        let chain = self.triage_chain(&pb, cfg);
        let (resp, _) = self.call_chain(&chain, &req, None, None, cancel).await.ok()?;
        parse_triage(&resp.text)
    }

    async fn chat_reply(&self, text: &str, cfg: &CfgView, cancel: &CancellationToken) -> TurnOutcome {
        let pb = self.playbooks();
        let mut messages = self.history_messages();
        messages.push(self.user_message(text));
        let req = ChatRequest {
            system: self.with_memory(self.inner.prompts.chat(cfg.user_name.as_deref(), true)),
            messages,
            temperature: Some(0.7),
            max_tokens: Some(2048),
            json: false,
            thinking: None,
        };
        let chain = self.triage_chain(&pb, cfg);
        match self.call_chain(&chain, &req, Some("chat"), None, cancel).await {
            Ok((resp, _)) if !resp.text.trim().is_empty() => TurnOutcome::text(TurnKind::Chat, resp.text.trim(), None),
            Ok(_) => TurnOutcome::text(TurnKind::Chat, "Fiquei sem resposta agora; pode repetir?", None),
            Err(e) => TurnOutcome::text(
                TurnKind::Chat,
                format!(
                    "Não consegui falar com nenhum modelo agora ({}). Confira as chaves em Configurações.",
                    clip(&e.to_string(), 160)
                ),
                None,
            ),
        }
    }

    /// One Toolbox call chosen by triage, then a short sentence.
    #[allow(clippy::too_many_arguments)]
    async fn single_action(
        &self,
        request: &str,
        cfg: &CfgView,
        name: &str,
        params: Value,
        reply: &str,
        emotion: Option<String>,
        cancel: &CancellationToken,
    ) -> TurnOutcome {
        let specs = self.inner.tools.specs();
        let Some(spec) = specs.iter().find(|s| s.name == name) else {
            return TurnOutcome::text(TurnKind::Action, format!("Não conheço a ação '{name}'."), None);
        };
        self.inner.bus.status(AppStatus::Thinking, format!("Executando {name}..."));
        if spec.needs_confirmation {
            let question =
                format!("Posso executar '{name}'? {} ({})", spec.description, clip(&params.to_string(), 160));
            if !self.inner.confirmer.ask(&question, cancel).await {
                return TurnOutcome::text(TurnKind::Action, "Tudo bem, não fiz nada.", None);
            }
        }
        let args = clip(&params.to_string(), 300);
        let result = tokio::select! {
            _ = cancel.cancelled() => ToolResult::err("Ação cancelada."),
            r = self.inner.tools.call(name, params) => r,
        };
        self.inner.bus.emit(SageEvent::WorkerTool {
            worker: "sage".to_string(),
            tool: name.to_string(),
            args,
            ok: result.ok,
        });
        // Ferramenta de consulta: o resultado volta ao modelo, que responde com o dado
        // de fato (em vez de "vou checar..." dito antes de saber a resposta).
        let summary = if result.ok && !spec.mutates && !result.details.trim().is_empty() {
            self.summarize_result(request, name, &result.details, cfg, cancel).await
        } else {
            None
        };
        let spoken = if let Some(s) = summary.clone() {
            s
        } else if result.ok && !reply.trim().is_empty() {
            reply.trim().to_string()
        } else if !result.spoken.trim().is_empty() {
            result.spoken.trim().to_string()
        } else if result.ok {
            "Feito.".to_string()
        } else {
            "Não consegui fazer isso.".to_string()
        };
        let mut full = spoken.clone();
        let details = result.details.trim();
        if !details.is_empty() && details != spoken && summary.is_none() {
            full.push_str(&format!("\n\n```\n{}\n```", clip(details, 4000)));
        }
        TurnOutcome {
            kind: TurnKind::Action,
            spoken,
            full,
            emotion: if result.ok { emotion } else { None },
            report: None,
            report_path: None,
            background: false,
        }
    }

    /// Resposta curta, na voz do S.A.G.E., a partir do resultado de uma ferramenta de consulta.
    async fn summarize_result(
        &self,
        request: &str,
        tool: &str,
        details: &str,
        cfg: &CfgView,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let pb = self.playbooks();
        let system = format!(
            "{}

Você acabou de executar a ferramenta '{tool}' para atender o pedido. Responda ao usuário              com o resultado concreto, em 1 ou 2 frases curtas para serem faladas, sem markdown, sem dizer              que vai checar (já checou) e sem repetir a pergunta.",
            self.with_memory(self.inner.prompts.chat(cfg.user_name.as_deref(), false))
        );
        let req = ChatRequest {
            system,
            messages: vec![Message::user(format!("Pedido: {request}

Resultado da ferramenta:
{}", clip(details, 3000)))],
            temperature: Some(0.3),
            max_tokens: Some(300),
            json: false,
            thinking: None,
        };
        let chain = self.triage_chain(&pb, cfg);
        match self.call_chain(&chain, &req, None, None, cancel).await {
            Ok((resp, _)) if !resp.text.trim().is_empty() => Some(resp.text.trim().to_string()),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ orchestration

    async fn orchestrate(&self, request: &str, cfg: &CfgView, cancel: &CancellationToken, recall: String) -> TurnOutcome {
        let started = Instant::now();
        let started_at = Utc::now();
        let pb = self.playbooks();
        let profile = pb.active_profile(cfg.profile.as_deref());
        let policy = pb.quota_policy(&profile);
        let run_id = new_run_id();
        let env = WorkerEnv {
            run_id: run_id.clone(),
            bus: self.inner.bus.clone(),
            tools: Arc::clone(&self.inner.tools),
            quota: self.inner.quota.clone(),
            budget: Arc::new(Mutex::new(Budget::from_policy(&pb.budget, None, cfg.max_cost))),
            switches: Arc::new(Mutex::new(Vec::new())),
            confirmer: self.inner.confirmer.clone(),
            cancel: cancel.clone(),
            max_steps: self.inner.settings.worker_max_steps,
            max_secs: self.inner.settings.worker_max_secs,
        };
        let ctx = RunCtx {
            request: request.to_string(),
            pb: Arc::clone(&pb),
            profile,
            policy,
            cfg: cfg.clone(),
            env,
            lead_chain: self.lead_chain(&pb, cfg),
            recall,
            low_quota_ok: tokio::sync::Mutex::new(None),
        };
        let bus = &self.inner.bus;
        bus.status(AppStatus::Orchestrating, "Escolhendo o playbook...");

        let choice = self.pick_playbook(&ctx).await;
        let playbook = pb.playbook(&choice.playbook).cloned().unwrap_or_default();
        bus.status(AppStatus::Orchestrating, format!("Planejando ({})...", choice.playbook));
        let plan =
            if cancel.is_cancelled() { ValidatedPlan::default() } else { self.make_plan(&ctx, &choice, &playbook).await };
        lock(&ctx.env.budget).limits = BudgetLimits::merge(plan.budget.as_ref(), &pb.budget, cfg.max_cost);

        let lead_alone = plan.waves.is_empty();
        let units = if lead_alone { vec![LEAD_UNIT.to_string()] } else { plan.unit_ids() };
        bus.emit(SageEvent::PlanReady {
            run_id: run_id.clone(),
            title: if choice.title.trim().is_empty() { short_title(request) } else { clip(choice.title.trim(), 60) },
            playbook: choice.playbook.clone(),
            why: choice.why.clone(),
            units,
        });

        let mut waves: Vec<WaveReport> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut prior = String::new();
        let mut note: Option<String> = None;
        let mut downgrade = false;

        if lead_alone && !cancel.is_cancelled() {
            let unit = lead_unit(request, &playbook);
            bus.status(AppStatus::Orchestrating, "Resolvendo eu mesmo...");
            let rep = self.run_unit(&ctx, &unit, "", false).await;
            waves.push(WaveReport { phase: unit.phase.clone(), workers: vec![rep] });
        }
        for (wi, wave) in plan.waves.iter().enumerate() {
            if cancel.is_cancelled() {
                note = Some(STOPPED_NOTE.to_string());
            } else {
                let (state, warned, pct, when_low) = {
                    let mut b = lock(&ctx.env.budget);
                    let warned = b.take_warning();
                    (b.state(), warned, b.percent(), b.when_low)
                };
                if warned {
                    self.notice(&format!("Orçamento da execução em {pct:.0}%."));
                }
                match state {
                    BudgetState::Exhausted => {
                        note = Some("O orçamento da execução acabou; parei antes das próximas fases.".to_string());
                    }
                    BudgetState::Low => match when_low {
                        WhenLow::Stop => note = Some("Orçamento baixo; parei antes das próximas fases.".to_string()),
                        WhenLow::FinishAndReport if wi > 0 => {
                            note = Some("Orçamento baixo; terminei o que estava em andamento e parei.".to_string());
                        }
                        WhenLow::FinishAndReport => {}
                        WhenLow::DowngradeTier => downgrade = true,
                    },
                    BudgetState::Healthy => {}
                }
            }
            if note.is_some() {
                for w in &plan.waves[wi..] {
                    missing.extend(w.units.iter().map(|u| format!("{} ({})", u.id, w.phase)));
                }
                break;
            }
            bus.status(AppStatus::Orchestrating, format!("Fase {}: {} worker(s)", wave.phase, wave.units.len()));
            let sem = Semaphore::new(ctx.pb.max_concurrent());
            let reports = {
                let (ctx_ref, sem_ref, prior_ref) = (&ctx, &sem, prior.as_str());
                join_all(wave.units.iter().map(|u| async move {
                    let _permit = sem_ref.acquire().await.ok();
                    self.run_unit(ctx_ref, u, prior_ref, downgrade).await
                }))
                .await
            };
            for r in &reports {
                prior.push_str(&format!(
                    "- [{}] {} ({}): {}\n",
                    r.phase,
                    r.id,
                    status_label(r.status),
                    clip(&r.summary, 300)
                ));
            }
            waves.push(WaveReport { phase: wave.phase.clone(), workers: reports });
        }

        let stopped = cancel.is_cancelled();
        if stopped {
            note = Some(STOPPED_NOTE.to_string());
        }
        let synth = if stopped { None } else { self.synthesize(&ctx, &choice, &waves, &missing).await };
        let (spoken, full, emotion) = match synth {
            Some((spoken, full, emotion)) => {
                let full = match &note {
                    Some(n) => format!("{full}\n\n_{n}_"),
                    None => full,
                };
                (spoken, full, emotion)
            }
            None => fallback_synthesis(&waves, &missing, note.as_deref()),
        };

        let (usage, calls) = {
            let b = lock(&ctx.env.budget);
            (b.usage.clone(), b.calls)
        };
        let switches = lock(&ctx.env.switches).clone();
        let report = RunReport {
            run_id: run_id.clone(),
            request: request.to_string(),
            playbook: choice.playbook.clone(),
            why: choice.why.clone(),
            profile: ctx.profile.clone(),
            status: RunStatus::decide(&waves, &missing, stopped),
            started_at,
            finished_at: Utc::now(),
            duration_secs: started.elapsed().as_secs_f64(),
            waves,
            switches,
            missing,
            calls,
            usage: usage.clone(),
            spoken: spoken.clone(),
            full: full.clone(),
        };
        let path = match report.write(&self.inner.runs_dir) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("não consegui salvar o relatório {run_id}: {e}");
                self.notice(&format!("Não consegui salvar o relatório da execução: {e}"));
                None
            }
        };
        bus.emit(SageEvent::RunReport {
            run_id,
            summary: report.summary_line(),
            usage,
            path: path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        });
        TurnOutcome { kind: TurnKind::Task, spoken, full, emotion, report: Some(report), report_path: path, background: false }
    }

    fn lead_system(&self, ctx: &RunCtx) -> String {
        templates::lead_system(&self.with_recall(self.inner.prompts.lead(ctx.cfg.user_name.as_deref()), &ctx.recall), &ctx.pb.principles)
    }

    async fn pick_playbook(&self, ctx: &RunCtx) -> PlaybookChoice {
        let pb = &ctx.pb;
        let fallback = || PlaybookChoice {
            playbook: pb.default_playbook().unwrap_or_default(),
            why: "Escolha padrão: o lead não respondeu com um playbook válido.".to_string(),
            title: String::new(),
        };
        if pb.playbooks.is_empty() {
            return fallback();
        }
        let ids = pb.playbooks.keys().map(String::as_str).collect::<Vec<_>>().join(", ");
        let call = Call {
            chain: &ctx.lead_chain,
            system: self.lead_system(ctx),
            user: templates::pick_prompt(&ctx.request, pb),
            stream: Some("lead"),
            max_tokens: Some(512),
        };
        let res = self
            .lead_json(call, Some(ctx), &ctx.env.cancel, |text| {
                let c: PlaybookChoice = jsonx::parse(text)?;
                let id = c.playbook.trim().to_string();
                if pb.playbook(&id).is_some() {
                    Ok(PlaybookChoice { playbook: id, why: c.why.trim().to_string(), title: c.title.trim().to_string() })
                } else {
                    Err(format!("o playbook '{id}' não existe; use um destes: {ids}"))
                }
            })
            .await;
        res.unwrap_or_else(|e| {
            tracing::warn!("escolha do playbook falhou: {e}");
            fallback()
        })
    }

    async fn make_plan(&self, ctx: &RunCtx, choice: &PlaybookChoice, playbook: &Playbook) -> ValidatedPlan {
        if !playbook.phases.iter().any(|p| p.is_wave()) {
            return ValidatedPlan::default();
        }
        let max_units = ctx.cfg.max_workers.map_or(DEFAULT_MAX_UNITS, |n| n as usize).clamp(1, 32);
        let call = Call {
            chain: &ctx.lead_chain,
            system: self.lead_system(ctx),
            user: templates::plan_prompt(&ctx.request, &choice.playbook, playbook, max_units),
            stream: Some("lead"),
            max_tokens: Some(4096),
        };
        let res = self
            .lead_json(call, Some(ctx), &ctx.env.cancel, |text| {
                let p: Plan = jsonx::parse(text)?;
                plan::validate(&p, playbook, max_units).map_err(|errs| errs.join("; "))
            })
            .await;
        match res {
            Ok(p) => p,
            Err(e) => {
                if !ctx.env.cancel.is_cancelled() {
                    self.notice(&format!("O plano do lead veio inválido ({}); vou resolver eu mesmo.", clip(&e, 200)));
                }
                ValidatedPlan::default()
            }
        }
    }

    async fn synthesize(
        &self,
        ctx: &RunCtx,
        choice: &PlaybookChoice,
        waves: &[WaveReport],
        missing: &[String],
    ) -> Option<(String, String, Option<String>)> {
        let emotions = self.inner.prompts.emotions();
        let call = Call {
            chain: &ctx.lead_chain,
            system: self.lead_system(ctx),
            user: templates::synthesis_prompt(&ctx.request, &choice.playbook, waves, missing, &emotions),
            stream: Some("lead"),
            max_tokens: Some(4096),
        };
        let res = self
            .lead_json(call, Some(ctx), &ctx.env.cancel, |text| {
                let v = jsonx::extract_object(text).ok_or("nenhum objeto JSON na resposta")?;
                let spoken = jsonx::str_field(&v, "spoken").trim().to_string();
                let full = jsonx::str_field(&v, "full").trim().to_string();
                if spoken.is_empty() && full.is_empty() {
                    return Err("faltam \"spoken\" e \"full\"".to_string());
                }
                Ok((spoken, full, jsonx::opt_str(&v, "emotion")))
            })
            .await;
        match res {
            Ok((spoken, full, emotion)) => {
                let spoken = if spoken.is_empty() { jsonx::first_sentence(&full) } else { spoken };
                let full = if full.is_empty() { spoken.clone() } else { full };
                Some((spoken, full, emotion))
            }
            Err(e) => {
                tracing::warn!("síntese falhou: {e}");
                None
            }
        }
    }

    /// Runs a unit with its gate: retry once with feedback, escalate one tier once.
    async fn run_unit(&self, ctx: &RunCtx, unit: &UnitSpec, prior: &str, downgrade: bool) -> WorkerReport {
        let mut tier = if downgrade { tier_down(unit.tier) } else { unit.tier };
        if unit.is_risky() && tier == Tier::Cheap {
            tier = Tier::Standard;
        }
        let mut tally = Tally { started: Instant::now(), attempts: 0, usage: Usage::default(), tool_calls: 0, gates: Vec::new() };
        let mut tries = Attempts::default();
        let mut feedback: Option<String> = None;
        let mut resume: Option<String> = None;
        // Depois de uma falha do provedor, a próxima tentativa troca de via (API <-> CLI)
        // quando há alternativa, em vez de bater de novo no mesmo modelo sobrecarregado.
        let mut flip = false;
        loop {
            tally.attempts += 1;
            let brief = worker::build_brief(unit, prior, feedback.as_deref());
            let (run, used_tier) = match self.execute(ctx, unit, tier, brief, resume.take(), flip).await {
                Ok(r) => r,
                Err(msg) => {
                    self.notice(&format!("{}: {msg}", unit.id));
                    return worker_report(unit, None, tier, tally, false, Some(msg));
                }
            };
            tier = used_tier;
            tally.usage.add(&run.outcome.usage);
            tally.tool_calls += run.tool_calls;
            self.inner.bus.emit(SageEvent::WorkerEnd {
                run_id: ctx.env.run_id.clone(),
                worker: unit.id.clone(),
                status: run.outcome.status,
                summary: run.outcome.summary.clone(),
                usage: run.outcome.usage.clone(),
            });
            if run.cancelled || ctx.env.cancel.is_cancelled() {
                return worker_report(unit, Some(&run), tier, tally, false, Some("cancelado".to_string()));
            }
            let budget_left = lock(&ctx.env.budget).state() != BudgetState::Exhausted;
            let infra = run.outcome.status == WorkerStatus::Failed && run.outcome.failure.is_some();
            let step = if infra {
                feedback = Some(format!(
                    "A tentativa anterior falhou por infraestrutura ({}); comece do zero.",
                    run.outcome.summary
                ));
                // Falha do provedor (503, cota): até 2 novas tentativas, com espera crescente,
                // para dar tempo ao provedor ou ao rodízio da cota escolher outro.
                if tries.retries < 2 && budget_left {
                    let wait = Duration::from_secs(4 * (u64::from(tries.retries) + 1));
                    tokio::select! {
                        _ = ctx.env.cancel.cancelled() => {}
                        _ = tokio::time::sleep(wait) => {}
                    }
                    NextStep::Retry
                } else {
                    NextStep::GiveUp
                }
            } else if let Some(criterion) = unit.gate.as_deref()
                && budget_left
            {
                let (reply, reviewer) = self.check_gate(ctx, unit, criterion, &run).await;
                self.inner.bus.emit(SageEvent::Gate {
                    run_id: ctx.env.run_id.clone(),
                    worker: unit.id.clone(),
                    verdict: reply.verdict.as_str().to_string(),
                    note: reply.note.clone(),
                });
                tally.gates.push(GateRecord {
                    verdict: reply.verdict.as_str().to_string(),
                    note: reply.note.clone(),
                    reviewer,
                });
                let step = gate::decide(reply.verdict, tries, tier, true);
                if step != NextStep::Accept {
                    feedback = Some(reply.note);
                }
                step
            } else {
                NextStep::Accept
            };
            match step {
                NextStep::Accept => {
                    let used = matches!(run.outcome.status, WorkerStatus::Done | WorkerStatus::DoneWithConcerns);
                    return worker_report(unit, Some(&run), tier, tally, used, None);
                }
                NextStep::Retry => {
                    tries.retries += 1;
                    if infra {
                        flip = !flip;
                    }
                    if !infra {
                        resume.clone_from(&run.outcome.session_id);
                    }
                }
                NextStep::Escalate(up) => {
                    tries.escalations += 1;
                    tier = up;
                }
                NextStep::GiveUp => {
                    let why = if infra { "falha de infraestrutura" } else { "não passou no gate" };
                    return worker_report(unit, Some(&run), tier, tally, false, Some(why.to_string()));
                }
            }
        }
    }

    async fn execute(
        &self,
        ctx: &RunCtx,
        unit: &UnitSpec,
        tier: Tier,
        brief: String,
        resume: Option<String>,
        flip: bool,
    ) -> Result<(WorkerRun, Tier), String> {
        let (selection, tier) = self.select_models(ctx, unit, tier, flip).await?;
        let persona = self.with_access(self.inner.prompts.worker(&worker::role_text(unit)));
        let run = match selection {
            Selection::Api(models) => {
                let first = models.first().map(|(m, _)| m.label()).unwrap_or_default();
                self.emit_start(ctx, unit, &first, tier);
                let extra = if unit.kind == UnitKind::Pc { self.inner.prompts.windows_rules() } else { String::new() };
                let specs = worker::allowed_specs(self.inner.tools.specs(), unit.read_only);
                let system = worker::api_worker_system(&persona, &extra, &specs, unit.read_only);
                let job = ApiJob { worker_id: unit.id.clone(), system, brief, models, read_only: unit.read_only };
                worker::run_api_worker(&ctx.env, job).await
            }
            Selection::Cli(agents) => {
                let first = agents.first().map(|(m, _)| m.label()).unwrap_or_default();
                self.emit_start(ctx, unit, &first, tier);
                let job = CliJob {
                    worker_id: unit.id.clone(),
                    prompt: worker::cli_worker_prompt(&persona, &brief),
                    agents,
                    mode: if unit.read_only { AgentMode::ReadOnly } else { AgentMode::Write },
                    cwd: unit_cwd(ctx, unit),
                    timeout_secs: ctx.cfg.cli_timeout,
                    resume_session: resume,
                };
                worker::run_cli_worker(&ctx.env, job).await
            }
        };
        Ok((run, tier))
    }

    fn emit_start(&self, ctx: &RunCtx, unit: &UnitSpec, model: &str, tier: Tier) {
        self.inner.bus.emit(SageEvent::WorkerStart {
            run_id: ctx.env.run_id.clone(),
            worker: unit.id.clone(),
            role: unit.role.clone(),
            model: model.to_string(),
            tier,
            brief: clip(&unit.brief, 400),
        });
    }

    /// Tier -> models: phase rules, preferred provider, profile overrides, availability
    /// and quota (first healthy provider at the same tier). Returns the tier used.
    async fn select_models(&self, ctx: &RunCtx, unit: &UnitSpec, tier: Tier, flip: bool) -> Result<(Selection, Tier), String> {
        let hub = &self.inner.hub;
        let available = hub.available();
        if unit.phase == LEAD_PHASE {
            let models: Vec<(ModelRef, Arc<dyn Llm>)> = ctx
                .lead_chain
                .iter()
                .filter(|m| available.contains(&m.provider))
                .filter_map(|m| hub.llm(m).map(|l| (m.clone(), l)))
                .collect();
            if models.is_empty() {
                return Err("Nenhum modelo do lead está disponível.".to_string());
            }
            return Ok((Selection::Api(models), tier));
        }

        let now = Utc::now();
        let quota_pct = self.inner.quota.read(|q| q.snapshot(now));
        let budget_low = lock(&ctx.env.budget).state() != BudgetState::Healthy;
        let rule = match_rule(
            &unit.rules,
            &RuleContext { tags: unit.tags.clone(), budget_low, quota_pct, critical: ctx.policy.critical_threshold },
        );
        let mut tier = tier;
        let mut front = unit.provider.clone();
        if let Some(r) = &rule {
            if let Some(t) = r.apply.tier {
                tier = t;
            }
            if r.apply.provider.is_some() {
                front.clone_from(&r.apply.provider);
            }
        }
        if unit.is_risky() && tier == Tier::Cheap {
            tier = Tier::Standard;
        }
        let mut order: Vec<String> = ctx.pb.provider_order().into_iter().filter(|p| available.contains(p)).collect();
        if let Some(p) = front
            && let Some(pos) = order.iter().position(|x| *x == p)
        {
            let item = order.remove(pos);
            order.insert(0, item);
        }
        let adjust = |mut m: ModelRef| -> ModelRef {
            if unit.effort.is_some() {
                m.effort.clone_from(&unit.effort);
            }
            if let Some(r) = &rule {
                let hinted = r.provider_hint.as_deref() == Some(m.provider.as_str());
                if hinted && let Some(model) = &r.apply.model {
                    m.model.clone_from(model);
                }
                if (hinted || r.provider_hint.is_none()) && r.apply.effort.is_some() {
                    m.effort.clone_from(&r.apply.effort);
                }
            }
            m
        };
        let profile = ctx.profile.as_str();
        let cli: Vec<(ModelRef, Arc<dyn AgentCli>)> = order
            .iter()
            .filter(|p| is_cli_provider(p))
            .filter_map(|p| {
                let agent = hub.agent(p)?;
                if !agent.installed() {
                    return None;
                }
                Some((adjust(ctx.pb.resolve(tier, p, profile)?), agent))
            })
            .collect();
        // Modelo do tier pedido em cada provedor e, como reserva, os dos tiers vizinhos:
        // um 503 ("high demand") num modelo passa para outro em vez de derrubar o worker.
        let mut tiers = vec![tier];
        tiers.extend(tier.up());
        if tier_down(tier) != tier {
            tiers.push(tier_down(tier));
        }
        let mut api: Vec<(ModelRef, Arc<dyn Llm>)> = Vec::new();
        for t in &tiers {
            for p in order.iter().filter(|p| !is_cli_provider(p)) {
                let Some(m) = ctx.pb.resolve(*t, p, profile).map(&adjust) else { continue };
                if api.iter().any(|(x, _)| x.provider == m.provider && x.model == m.model) {
                    continue;
                }
                if let Some(llm) = hub.llm(&m) {
                    api.push((m, llm));
                }
            }
        }
        let (cli, cli_low) = self.order_by_quota(ctx, cli);
        let (api, api_low) = self.order_by_quota(ctx, api);
        if cli.is_empty() && api.is_empty() {
            return Err(format!(
                "Nenhum modelo disponível para o tier {} (provedores ativos: {}).",
                tier_name(tier),
                available.join(", ")
            ));
        }
        let mut use_cli = (unit.kind == UnitKind::Code && !cli.is_empty()) || api.is_empty();
        if flip && (if use_cli { !api.is_empty() } else { !cli.is_empty() }) {
            use_cli = !use_cli;
        }
        let low = if use_cli { cli_low } else { api_low };
        if low && ctx.policy.on_both_critical == "ask" && !self.confirm_low_quota(ctx).await {
            return Err("A cota está baixa em todos os provedores e você preferiu não continuar.".to_string());
        }
        let selection = if use_cli { Selection::Cli(cli) } else { Selection::Api(api) };
        Ok((selection, tier))
    }

    /// Chosen candidate first, then the healthy ones after it, then the critical ones
    /// before it; blocked ones are dropped. Emits `ProviderSwitch` for passed-over ones.
    fn order_by_quota<T>(&self, ctx: &RunCtx, list: Vec<(ModelRef, T)>) -> (Vec<(ModelRef, T)>, bool) {
        if list.is_empty() {
            return (list, false);
        }
        let refs: Vec<ModelRef> = list.iter().map(|(m, _)| m.clone()).collect();
        let now = Utc::now();
        let (pick, blocked) = self.inner.quota.read(|q| {
            let blocked: Vec<bool> =
                refs.iter().map(|m| q.state(&m.provider, &ctx.policy, now) == QuotaState::Blocked).collect();
            (q.pick(&refs, &ctx.policy, now), blocked)
        });
        let Some(pick) = pick else {
            return (Vec::new(), false);
        };
        let chosen_provider = refs[pick.chosen].provider.clone();
        for (from, pct) in &pick.passed_over {
            ctx.env.switch_provider(from, &chosen_provider, worker::switch_reason(from, &chosen_provider, *pct));
        }
        let mut first = Vec::with_capacity(list.len());
        let mut after = Vec::new();
        let mut before = Vec::new();
        for (i, item) in list.into_iter().enumerate() {
            if i == pick.chosen {
                first.push(item);
            } else if !blocked[i] {
                if i > pick.chosen {
                    after.push(item);
                } else {
                    before.push(item);
                }
            }
        }
        first.extend(after);
        first.extend(before);
        (first, pick.all_low)
    }

    async fn confirm_low_quota(&self, ctx: &RunCtx) -> bool {
        let mut answer = ctx.low_quota_ok.lock().await;
        if let Some(a) = *answer {
            return a;
        }
        let question = format!(
            "Todos os provedores estão com a cota acima de {:.0}%. Continuar mesmo assim?",
            ctx.policy.critical_threshold
        );
        let yes = self.inner.confirmer.ask(&question, &ctx.env.cancel).await;
        *answer = Some(yes);
        yes
    }

    /// The phase gate, judged by a standard-tier model of another provider (then the lead).
    async fn check_gate(&self, ctx: &RunCtx, unit: &UnitSpec, criterion: &str, run: &WorkerRun) -> (GateReply, String) {
        let available = self.inner.hub.available();
        let worker_provider = run.model.provider.as_str();
        let mut chain: Vec<ModelRef> = ctx
            .pb
            .provider_order()
            .iter()
            .filter(|p| !is_cli_provider(p) && p.as_str() != worker_provider && available.contains(p))
            .filter_map(|p| ctx.pb.resolve(Tier::Standard, p, &ctx.profile))
            .collect();
        chain.extend(ctx.lead_chain.iter().cloned());
        let chain = dedup(chain);
        let req = ChatRequest {
            system: gate::gate_system(),
            messages: vec![Message::user(gate::gate_user(criterion, &unit.brief, &run.outcome))],
            temperature: Some(0.1),
            max_tokens: Some(512),
            json: true,
            thinking: None,
        };
        match self.call_chain(&chain, &req, None, Some(ctx), &ctx.env.cancel).await {
            Ok((resp, model)) => match gate::parse_gate(&resp.text) {
                Some(reply) => (reply, model.label()),
                None => (
                    GateReply { verdict: Verdict::Ok, note: "Veredito ilegível; aceito sem revisão.".to_string() },
                    model.label(),
                ),
            },
            Err(e) => (
                GateReply {
                    verdict: Verdict::Ok,
                    note: format!("Revisor indisponível ({}); aceito sem revisão.", clip(&e.to_string(), 120)),
                },
                String::new(),
            ),
        }
    }
}

/// Resposta imediata para uma tarefa (variada, mas estável para o mesmo pedido).
fn ack_text(request: &str) -> &'static str {
    const ACKS: &[&str] = &[
        "Entendido. Já comecei: vou dividir o trabalho em partes e te aviso quando terminar.",
        "Pode deixar, já estou cuidando disso. Te conto o resultado assim que ficar pronto.",
        "Certo! Vou distribuir isso entre os meus clones e já volto com o resultado.",
        "Combinado. Estou planejando agora e começo a executar em seguida; te aviso no fim.",
    ];
    let h = request.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    ACKS[h as usize % ACKS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_json() {
        let t = parse_triage(r#"{"kind":"Action","reply":"Abrindo.","action":"open_app","action_params":{"target":"spotify"},"emotion":"feliz"}"#).unwrap();
        assert_eq!(t.kind, TurnKind::Action);
        assert_eq!(t.action.as_deref(), Some("open_app"));
        assert_eq!(t.params["target"], "spotify");
        assert_eq!(t.emotion.as_deref(), Some("feliz"));
        let t = parse_triage(r#"{"kind":"chat","reply":"Oi!","action":"none","action_params":null}"#).unwrap();
        assert_eq!((t.kind, t.action), (TurnKind::Chat, None));
        assert!(parse_triage(r#"{"kind":"outro"}"#).is_none());
    }

    #[test]
    fn chains_dedup_and_cfg_view() {
        let mut cfg = Config::default();
        cfg.model_config.brain = ModelRef::new("gemini", "gemini-4-hydrogen");
        cfg.model_config.fallbacks = vec![ModelRef::new("groq", "x"), ModelRef::new("", "")];
        let v = cfg_view(&cfg);
        assert_eq!(v.brain_chain.len(), 2);
        assert!(v.fast.is_none());
        let d = dedup(vec![ModelRef::new("a", "m"), ModelRef::new("a", "m"), ModelRef::new("b", "m")]);
        assert_eq!(d.len(), 2);
        assert!(new_run_id().len() > 16);
    }
}

/// Título curto de uma tarefa quando o lead não deu um: as primeiras palavras do pedido,
/// sem "eu preciso que você", "por favor" e afins.
pub fn short_title(request: &str) -> String {
    const FILLER: &[&str] = &[
        "eu", "preciso", "quero", "que", "você", "voce", "tu", "por", "favor", "faz", "fazendo", "olha", "só",
        "seiji", "sage", "ai", "aí", "então", "agora", "pra", "mim", "me", "na", "verdade", "cara", "e",
    ];
    let words: Vec<&str> = request
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?' | ';' | ':'))
        .filter(|w| !w.is_empty())
        .collect();
    let start = words.iter().position(|w| !FILLER.contains(&w.to_lowercase().as_str())).unwrap_or(0);
    let picked: Vec<&str> = words.iter().skip(start).take(6).copied().collect();
    let mut t = picked.join(" ");
    if let Some(c) = t.chars().next() {
        t = c.to_uppercase().collect::<String>() + &t[c.len_utf8()..];
    }
    if t.is_empty() { "Tarefa".to_string() } else { clip(&t, 60) }
}

#[cfg(test)]
mod title_tests {
    use super::short_title;

    #[test]
    fn short_title_drops_filler_and_keeps_it_short() {
        assert_eq!(short_title("eu preciso que tu instale o freecad para mim faz o download"), "Instale o freecad para mim faz");
        assert_eq!(short_title("   "), "Tarefa");
    }
}
