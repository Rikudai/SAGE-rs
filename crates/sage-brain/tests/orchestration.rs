//! End-to-end runs of the brain with fake models, a fake CLI agent and a fake Toolbox.
//! No network, no real hardware, no persona texts (prompts are injected).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

use sage_brain::gate::GATE_MARKER;
use sage_brain::playbooks::is_cli_provider;
use sage_brain::report::RunStatus;
use sage_brain::templates::{PICK_MARKER, PLAN_MARKER, SYNTH_MARKER, TRIAGE_MARKER};
use sage_brain::worker::PROTOCOL_MARKER;
use sage_brain::{Brain, BrainOptions, Playbooks, Prompts, RunReport, TurnKind};
use sage_core::config::Config;
use sage_core::events::{AppStatus, Command, EventBus, SageEvent};
use sage_core::llm::{
    AgentCli, AgentMode, AgentOutcome, AgentTask, ChatRequest, ChatResponse, DeltaSink, Llm, LlmError, ModelHub,
    ModelRef, Tier, Transcriber, Usage, WorkerStatus,
};
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};

type Script = dyn Fn(&ModelRef, &ChatRequest) -> Result<String, LlmError> + Send + Sync;
type GateFn = Arc<dyn Fn() -> String + Send + Sync>;

const TRIAGE_TASK: &str = r#"{"kind":"task","reply":"","action":"none","action_params":{},"emotion":"pensando"}"#;
const GATE_OK: &str = r#"{"verdict":"ok","note":"Evidência suficiente."}"#;
const GATE_ESCALATE: &str = r#"{"verdict":"escalate","note":"Faltou a evidência de linha."}"#;
const PLAN_TWO: &str = r#"{"summary":"Mapear o projeto","waves":[{"phase":"investigate","units":[
  {"id":"ler-main","role":"leitor","brief":"Ache a função main e cite arquivo:linha.","kind":"read","tier":"standard","tags":["light"]},
  {"id":"mapear-lib","role":"mapeador","brief":"Mapeie src/lib.rs.","kind":"code","tier":"standard"}]}]}"#;
const PLAN_ONE: &str =
    r#"{"waves":[{"phase":"investigate","units":[{"id":"ler-main","brief":"Ache a main.","kind":"read"}]}]}"#;

// ---------------------------------------------------------------- fakes

struct FakeLlm {
    m: ModelRef,
    script: Arc<Script>,
}

#[async_trait]
impl Llm for FakeLlm {
    fn model(&self) -> ModelRef {
        self.m.clone()
    }
    async fn chat(&self, req: ChatRequest, sink: Option<DeltaSink>) -> Result<ChatResponse, LlmError> {
        let text = (self.script)(&self.m, &req)?;
        if let Some(s) = &sink {
            s(&text, false);
        }
        Ok(ChatResponse {
            text,
            thoughts: String::new(),
            usage: Usage { input_tokens: 100, output_tokens: 20, cache_read_tokens: 0, cost_usd: None },
            model: self.m.clone(),
        })
    }
}

#[derive(Default)]
struct FakeAgent {
    tasks: Mutex<Vec<AgentTask>>,
}

#[async_trait]
impl AgentCli for FakeAgent {
    fn provider(&self) -> &'static str {
        "claude"
    }
    fn installed(&self) -> bool {
        true
    }
    async fn run(&self, task: AgentTask, _sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome> {
        self.tasks.lock().unwrap().push(task);
        Ok(AgentOutcome {
            status: WorkerStatus::Done,
            summary: String::new(),
            text: "A entrada fica em src/lib.rs:10.\nSTATUS: DONE\nSUMMARY: Mapeei o módulo.".into(),
            usage: Usage { input_tokens: 500, output_tokens: 50, cache_read_tokens: 0, cost_usd: Some(0.01) },
            session_id: Some("sessao-1".into()),
            failure: None,
            reset_hint: None,
            duration_secs: 0.1,
        })
    }
}

struct FakeHub {
    providers: Vec<String>,
    script: Arc<Script>,
    agent: Arc<FakeAgent>,
}

impl ModelHub for FakeHub {
    fn llm(&self, m: &ModelRef) -> Option<Arc<dyn Llm>> {
        (self.providers.contains(&m.provider) && !is_cli_provider(&m.provider))
            .then(|| Arc::new(FakeLlm { m: m.clone(), script: Arc::clone(&self.script) }) as Arc<dyn Llm>)
    }
    fn agent(&self, provider: &str) -> Option<Arc<dyn AgentCli>> {
        (provider == "claude" && self.providers.iter().any(|p| p == "claude"))
            .then(|| Arc::clone(&self.agent) as Arc<dyn AgentCli>)
    }
    fn available(&self) -> Vec<String> {
        self.providers.clone()
    }
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        None
    }
}

#[derive(Default)]
struct FakeTools {
    calls: Mutex<Vec<String>>,
}

fn spec(name: &str, mutates: bool, needs_confirmation: bool) -> ToolSpec {
    ToolSpec { name: name.into(), description: format!("ferramenta {name}"), args: json!({}), mutates, needs_confirmation }
}

#[async_trait]
impl Toolbox for FakeTools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![spec("file_read", false, false), spec("file_write", true, false), spec("power", true, true), spec("slow", false, false)]
    }
    async fn call(&self, name: &str, _args: Value) -> ToolResult {
        self.calls.lock().unwrap().push(name.to_string());
        if name == "slow" {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        ToolResult::ok("Feito.", format!("saída de {name}: fn main() {{}}"))
    }
}

struct FakePrompts;

impl Prompts for FakePrompts {
    fn lead(&self, _: Option<&str>) -> String {
        "Você é o lead de teste.".into()
    }
    fn chat(&self, _: Option<&str>, _: bool) -> String {
        "Você é o S.A.G.E. de teste.".into()
    }
    fn worker(&self, role: &str) -> String {
        format!("Worker de teste: {role}")
    }
    fn windows_rules(&self) -> String {
        String::new()
    }
    fn emotions(&self) -> Vec<String> {
        vec!["feliz".into(), "pensando".into()]
    }
    fn looks_like_task(&self, _: &str) -> bool {
        false
    }
    fn complexity(&self, _: &str) -> f32 {
        0.0
    }
}

/// Routes each request to a canned reply by the markers in its prompts.
/// `limited`: worker calls on that provider fail with a rate limit.
fn script(
    triage: &'static str,
    playbook: &'static str,
    plan: &'static str,
    gate: GateFn,
    limited: Option<&'static str>,
) -> Arc<Script> {
    Arc::new(move |m: &ModelRef, req: &ChatRequest| -> Result<String, LlmError> {
        let first = req.messages.first().map(|x| x.text.as_str()).unwrap_or("");
        let last = req.messages.last().map(|x| x.text.as_str()).unwrap_or("");
        if req.system.contains(TRIAGE_MARKER) {
            return Ok(triage.to_string());
        }
        if req.system.contains(GATE_MARKER) {
            return Ok(gate());
        }
        if req.system.contains(PROTOCOL_MARKER) {
            if limited == Some(m.provider.as_str()) {
                return Err(LlmError::RateLimited {
                    provider: m.provider.clone(),
                    detail: "429 quota exceeded".into(),
                    retry_after_secs: Some(600),
                });
            }
            if last.starts_with("RESULTADO de") {
                return Ok(r#"{"thought":"tenho a evidência","status":"done","action":"none","action_params":{},"reply":"A main fica em src/main.rs:1.\nSTATUS: DONE\nSUMMARY: Achei a main."}"#.to_string());
            }
            let tool = if first.contains("DEVAGAR") { "slow" } else { "file_read" };
            return Ok(format!(
                r#"{{"thought":"vou ler","status":"continue","action":"{tool}","action_params":{{"path":"src/main.rs"}},"reply":""}}"#
            ));
        }
        if first.contains(PICK_MARKER) {
            return Ok(format!(r#"{{"playbook":"{playbook}","why":"Leitura de partes independentes."}}"#));
        }
        if first.contains(PLAN_MARKER) {
            return Ok(plan.to_string());
        }
        if first.contains(SYNTH_MARKER) {
            return Ok(r###"{"spoken":"Pronto, mapeei o projeto.","full":"## Resultado\n- main em src/main.rs:1","emotion":"feliz"}"###.to_string());
        }
        Ok("{}".to_string())
    })
}

fn ok_gate() -> GateFn {
    Arc::new(|| GATE_OK.to_string())
}

fn playbooks() -> Playbooks {
    Playbooks::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/playbooks.yaml")).expect("playbooks")
}

struct Harness {
    brain: Brain,
    bus: EventBus,
    tools: Arc<FakeTools>,
    agent: Arc<FakeAgent>,
    data: PathBuf,
}

fn harness(providers: &[&str], script: Arc<Script>) -> Harness {
    let bus = EventBus::new();
    let tools = Arc::new(FakeTools::default());
    let agent = Arc::new(FakeAgent::default());
    let hub = Arc::new(FakeHub {
        providers: providers.iter().map(|p| (*p).to_string()).collect(),
        script,
        agent: Arc::clone(&agent),
    });
    let data = std::env::temp_dir().join(format!("sage-brain-test-{}", uuid::Uuid::new_v4()));
    let mut opts = BrainOptions::new(playbooks(), data.clone()).with_prompts(Arc::new(FakePrompts));
    opts.settings.confirm_timeout_secs = 5;
    let brain = Brain::with_options(Arc::new(RwLock::new(Config::default())), hub, tools.clone(), bus.clone(), opts);
    Harness { brain, bus, tools, agent, data }
}

fn drain(rx: &mut broadcast::Receiver<SageEvent>) -> Vec<SageEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn count(events: &[SageEvent], pred: fn(&SageEvent) -> bool) -> usize {
    events.iter().filter(|e| pred(e)).count()
}

// ---------------------------------------------------------------- tests

#[tokio::test]
async fn full_run_with_api_and_cli_workers_writes_a_report() {
    let h = harness(&["gemini", "groq", "claude"], script(TRIAGE_TASK, "plan-big-execute-small", PLAN_TWO, ok_gate(), None));
    let mut rx = h.bus.subscribe();
    let out = h.brain.ask("Mapeie este projeto e me diga onde fica a main", false).await;
    let events = drain(&mut rx);

    assert_eq!(out.kind, TurnKind::Task);
    assert_eq!(out.spoken, "Pronto, mapeei o projeto.");
    assert_eq!(out.emotion.as_deref(), Some("feliz"));
    let report = out.report.clone().expect("report");
    assert_eq!(report.playbook, "plan-big-execute-small");
    assert_eq!(report.status, RunStatus::Completed);
    assert!(report.calls >= 8, "calls {}", report.calls);

    let workers: Vec<_> = report.workers().collect();
    assert_eq!(workers.len(), 2);
    let api = workers.iter().find(|w| w.id == "ler-main").expect("api worker");
    assert_eq!(api.model, "gemini:gemini-3.6-flash@medium");
    assert_eq!(api.tier, Tier::Standard);
    assert_eq!(api.tool_calls, 1);
    assert_eq!(api.summary, "Achei a main.");
    assert!(api.used);
    // the reviewer came from another provider
    assert_eq!(api.gates.len(), 1);
    assert!(api.gates[0].reviewer.starts_with("groq:"), "{}", api.gates[0].reviewer);

    let cli = workers.iter().find(|w| w.id == "mapear-lib").expect("cli worker");
    assert_eq!(cli.model, "claude:sonnet@medium");
    assert_eq!(cli.summary, "Mapeei o módulo.");
    assert_eq!(cli.usage.cost_usd, Some(0.01));
    assert!(cli.gates[0].reviewer.starts_with("gemini:"), "{}", cli.gates[0].reviewer);

    let path = out.report_path.expect("report path");
    assert!(path.exists());
    assert!(path.with_file_name("report.md").exists());
    assert_eq!(RunReport::load(&path).expect("report.json"), report);

    {
        let tasks = h.agent.tasks.lock().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, AgentMode::ReadOnly);
        assert_eq!(tasks[0].model, "sonnet");
        assert!(tasks[0].prompt.contains("CONTRATO DO WORKER"));
        assert!(tasks[0].prompt.contains("Mapeie src/lib.rs."));
    }
    assert_eq!(*h.tools.calls.lock().unwrap(), vec!["file_read".to_string()]);

    assert!(events.iter().any(
        |e| matches!(e, SageEvent::PlanReady { units, playbook, .. } if units == &["ler-main", "mapear-lib"] && playbook == "plan-big-execute-small")
    ));
    assert_eq!(count(&events, |e| matches!(e, SageEvent::WorkerStart { .. })), 2);
    assert_eq!(count(&events, |e| matches!(e, SageEvent::WorkerEnd { status: WorkerStatus::Done, .. })), 2);
    assert_eq!(count(&events, |e| matches!(e, SageEvent::Gate { verdict, .. } if verdict == "ok")), 2);
    assert_eq!(count(&events, |e| matches!(e, SageEvent::WorkerTool { ok: true, .. })), 1);
    assert_eq!(count(&events, |e| matches!(e, SageEvent::RunReport { .. })), 1);
    // Uma resposta imediata ("já comecei") e o resultado no fim.
    assert_eq!(count(&events, |e| matches!(e, SageEvent::Reply { .. })), 2);
    assert!(matches!(events.iter().find(|e| matches!(e, SageEvent::Reply { .. })), Some(SageEvent::Reply { emotion: Some(e), .. }) if e == "determinado"));
    assert_eq!(count(&events, |e| matches!(e, SageEvent::UserText { .. })), 1);
    assert!(count(&events, |e| matches!(e, SageEvent::Stream { source, .. } if source == "lead")) > 0);
    assert!(count(&events, |e| matches!(e, SageEvent::Stream { source, .. } if source == "ler-main")) > 0);
    assert!(count(&events, |e| matches!(e, SageEvent::Status { status: AppStatus::Orchestrating, .. })) > 0);
    assert!(count(&events, |e| matches!(e, SageEvent::Emotion { .. })) > 0);
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn rate_limit_moves_workers_to_the_next_provider() {
    let h = harness(&["gemini", "groq"], script(TRIAGE_TASK, "plan-big-execute-small", PLAN_ONE, ok_gate(), Some("gemini")));
    let mut rx = h.bus.subscribe();
    let out = h.brain.ask("Ache a main do projeto", false).await;
    let events = drain(&mut rx);

    let report = out.report.expect("report");
    let w = report.workers().next().expect("worker");
    assert!(w.model.starts_with("groq:"), "{}", w.model);
    assert!(w.used);
    assert!(events.iter().any(
        |e| matches!(e, SageEvent::ProviderSwitch { from, to, .. } if from == "gemini" && to == "groq")
    ));
    assert_eq!(report.switches.len(), 1);
    assert!(h.brain.quota().read(|q| q.usage_pct("gemini", chrono::Utc::now())) >= 100.0);
    assert!(h.data.join("quota.json").exists());
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn failed_gate_escalates_one_tier_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let gate: GateFn = {
        let c = Arc::clone(&calls);
        Arc::new(move || {
            if c.fetch_add(1, Ordering::SeqCst) == 0 { GATE_ESCALATE.to_string() } else { GATE_OK.to_string() }
        })
    };
    let h = harness(&["gemini", "groq"], script(TRIAGE_TASK, "plan-big-execute-small", PLAN_ONE, gate, None));
    let out = h.brain.ask("Ache a main", false).await;
    let report = out.report.expect("report");
    let w = report.workers().next().expect("worker");
    assert_eq!(w.attempts, 2);
    assert_eq!(w.tier, Tier::Strong);
    assert_eq!(w.model, "gemini:gemini-4-hydrogen@high");
    assert_eq!(w.gates.iter().map(|g| g.verdict.as_str()).collect::<Vec<_>>(), ["escalate", "ok"]);
    assert!(w.used);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn exhausted_budget_stops_before_the_waves() {
    const PLAN_TINY: &str = r#"{"waves":[{"phase":"investigate","units":[{"id":"ler-main","brief":"Ache a main.","kind":"read"}]}],"budget":{"maxCalls":2}}"#;
    let h = harness(&["gemini", "groq"], script(TRIAGE_TASK, "plan-big-execute-small", PLAN_TINY, ok_gate(), None));
    let mut rx = h.bus.subscribe();
    let out = h.brain.ask("Ache a main", false).await;
    let events = drain(&mut rx);
    let report = out.report.expect("report");
    assert_eq!(report.status, RunStatus::Partial);
    assert_eq!(report.missing, vec!["ler-main (investigate)".to_string()]);
    assert_eq!(report.workers().count(), 0);
    assert_eq!(count(&events, |e| matches!(e, SageEvent::WorkerStart { .. })), 0);
    assert!(out.full.contains("orçamento"), "{}", out.full);
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn chat_is_answered_directly() {
    const TRIAGE_CHAT: &str = r#"{"kind":"chat","reply":"Oi! Tudo bem por aqui.","action":"none","emotion":"feliz"}"#;
    let h = harness(&["groq", "gemini"], script(TRIAGE_CHAT, "direct", "{}", ok_gate(), None));
    let mut rx = h.bus.subscribe();
    let out = h.brain.ask("oi, tudo bem?", false).await;
    let events = drain(&mut rx);
    assert_eq!(out.kind, TurnKind::Chat);
    assert_eq!(out.spoken, "Oi! Tudo bem por aqui.");
    assert!(out.report.is_none());
    assert_eq!(count(&events, |e| matches!(e, SageEvent::PlanReady { .. })), 0);
    assert!(events.iter().any(|e| matches!(e, SageEvent::Emotion { name } if name == "feliz")));
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn run_loop_confirms_a_risky_action() {
    const TRIAGE_POWER: &str = r#"{"kind":"action","reply":"Desligando o PC.","action":"power","action_params":{"mode":"off"},"emotion":"feliz"}"#;
    let h = harness(&["groq", "gemini"], script(TRIAGE_POWER, "direct", "{}", ok_gate(), None));
    let mut rx = h.bus.subscribe();
    let (tx, cmd_rx) = mpsc::channel(8);
    let task = tokio::spawn(h.brain.clone().run(cmd_rx));
    tx.send(Command::Ask { text: "desliga o pc".into(), from_voice: true }).await.unwrap();
    let spoken = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(SageEvent::ConfirmRequest { id, .. }) => tx.send(Command::Confirm { id, yes: true }).await.unwrap(),
                Ok(SageEvent::Reply { spoken, .. }) => break spoken,
                Ok(_) => {}
                Err(e) => panic!("bus: {e}"),
            }
        }
    })
    .await
    .expect("reply in time");
    assert_eq!(spoken, "Desligando o PC.");
    assert_eq!(*h.tools.calls.lock().unwrap(), vec!["power".to_string()]);
    drop(tx);
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(&h.data);
}

#[tokio::test]
async fn stop_run_cancels_a_running_worker() {
    const PLAN_SLOW: &str =
        r#"{"waves":[{"phase":"investigate","units":[{"id":"devagar","brief":"DEVAGAR: leia tudo.","kind":"read"}]}]}"#;
    let h = harness(&["gemini", "groq"], script(TRIAGE_TASK, "plan-big-execute-small", PLAN_SLOW, ok_gate(), None));
    let mut rx = h.bus.subscribe();
    let (tx, cmd_rx) = mpsc::channel(8);
    let task = tokio::spawn(h.brain.clone().run(cmd_rx));
    tx.send(Command::Ask { text: "leia tudo".into(), from_voice: false }).await.unwrap();
    let (spoken, path) = tokio::time::timeout(Duration::from_secs(10), async {
        let mut path = String::new();
        loop {
            match rx.recv().await {
                Ok(SageEvent::WorkerStart { .. }) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    tx.send(Command::StopRun).await.unwrap();
                }
                Ok(SageEvent::RunReport { path: p, .. }) => path = p,
                // A primeira resposta é o aviso imediato; espera a do fim (depois do relatório).
                Ok(SageEvent::Reply { spoken, .. }) if !path.is_empty() => break (spoken, path),
                Ok(_) => {}
                Err(e) => panic!("bus: {e}"),
            }
        }
    })
    .await
    .expect("stopped well before the 30 s tool finished");
    assert_eq!(spoken, "Parei a execução, como você pediu.");
    let report = RunReport::load(Path::new(&path)).expect("report.json");
    assert_eq!(report.status, RunStatus::Stopped);
    assert!(!report.workers().any(|w| w.used));
    drop(tx);
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(&h.data);
}
