//! Bounded workers. API worker: a JSON action loop over the Toolbox (port of the old
//! `task_loop.py`). CLI worker: one `AgentCli` run. Both follow the worker contract.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use sage_core::events::{EventBus, SageEvent};
use sage_core::llm::{
    AgentCli, AgentMode, AgentOutcome, AgentTask, ChatRequest, DeltaSink, FailureKind, Llm, LlmError, Message,
    ModelRef, Role, Usage, WorkerStatus,
};
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};

use crate::budget::{Budget, BudgetState};
use crate::confirm::Confirmer;
use crate::jsonx::{self, clip};
use crate::lock;
use crate::plan::UnitSpec;
use crate::playbooks::display_name;
use crate::quota::SharedQuota;
use crate::report::SwitchRecord;

/// Rules every worker gets, after its persona.
pub const WORKER_CONTRACT: &str = "CONTRATO DO WORKER\n\
- Você é um worker com escopo limitado. Não delegue, não crie subagentes e não chame outros agentes.\n\
- Faça só o que o brief pede, dentro do escopo; não invente trabalho extra.\n\
- Devolva um resultado conciso com evidências (arquivo:linha, URL ou trecho de saída), nunca a transcrição inteira.\n\
- Diga o que ficou incerto ou faltando.\n\
- Termine com exatamente estas duas linhas:\n\
STATUS: DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT\n\
SUMMARY: <uma frase>";

/// Marker of the API worker's JSON protocol in its system prompt.
pub const PROTOCOL_MARKER: &str = "PROTOCOLO DE AÇÕES";

const PROTOCOL: &str = "Responda SEMPRE com um único objeto JSON, sem texto fora dele:\n\
{\"thought\": \"raciocínio curto sobre a evidência atual\", \"status\": \"continue\" | \"done\" | \"ask\", \
\"action\": \"<nome da ferramenta>\" | \"none\", \"action_params\": {...}, \"reply\": \"...\"}\n\
- continue: chame uma ferramenta por vez em action/action_params; o RESULTADO chega na próxima mensagem.\n\
- done: action \"none\"; reply traz o resultado com evidências e termina com as linhas STATUS: e SUMMARY:.\n\
- ask: só para confirmar algo arriscado ou irreversível; reply é a pergunta de sim ou não para o usuário.";

const INVALID_STEP_NUDGE: &str =
    "Resposta inválida: devolva apenas um objeto JSON no formato do PROTOCOLO DE AÇÕES.";
const NO_ACTION_NUDGE: &str =
    "Nenhuma ferramenta foi pedida. Chame uma ferramenta em action ou finalize com status \"done\".";

/// Live tokens to `SageEvent::Stream` under `source`.
pub fn stream_sink(bus: &EventBus, source: &str) -> DeltaSink {
    let bus = bus.clone();
    let source = source.to_string();
    Arc::new(move |text: &str, thought: bool| {
        if !text.is_empty() {
            bus.emit(SageEvent::Stream { source: source.clone(), text: text.to_string(), thought });
        }
    })
}

/// "Movendo os workers para o Groq: Gemini em 86%."
pub fn switch_reason(from: &str, to: &str, pct: f64) -> String {
    let (from, to) = (display_name(from), display_name(to));
    if pct >= 100.0 {
        format!("Movendo os workers para o {to}: {from} atingiu o limite de uso.")
    } else {
        format!("Movendo os workers para o {to}: {from} em {pct:.0}%.")
    }
}

/// What every worker of a run shares.
#[derive(Clone)]
pub struct WorkerEnv {
    pub run_id: String,
    pub bus: EventBus,
    pub tools: Arc<dyn Toolbox>,
    pub quota: SharedQuota,
    pub budget: Arc<Mutex<Budget>>,
    pub switches: Arc<Mutex<Vec<SwitchRecord>>>,
    pub confirmer: Confirmer,
    pub cancel: CancellationToken,
    /// Model calls an API worker may make.
    pub max_steps: u32,
    /// Wall time of one API worker.
    pub max_secs: u64,
}

impl WorkerEnv {
    /// Emits `ProviderSwitch` once per (from, to) pair in the run and records it.
    pub fn switch_provider(&self, from: &str, to: &str, reason: String) {
        if from == to {
            return;
        }
        {
            let mut list = lock(&self.switches);
            if list.iter().any(|s| s.from == from && s.to == to) {
                return;
            }
            list.push(SwitchRecord { from: from.to_string(), to: to.to_string(), reason: reason.clone(), at: Utc::now() });
        }
        self.bus.emit(SageEvent::ProviderSwitch { from: from.to_string(), to: to.to_string(), reason });
    }

    fn budget_exhausted(&self) -> bool {
        lock(&self.budget).state() == BudgetState::Exhausted
    }

    fn record_usage(&self, usage: &Usage) {
        lock(&self.budget).record(usage);
    }
}

/// Result of one worker attempt.
#[derive(Debug, Clone)]
pub struct WorkerRun {
    pub outcome: AgentOutcome,
    /// Model that produced the outcome.
    pub model: ModelRef,
    pub tool_calls: u32,
    pub cancelled: bool,
}

fn outcome(
    status: WorkerStatus,
    summary: impl Into<String>,
    text: impl Into<String>,
    usage: Usage,
    failure: Option<FailureKind>,
    started: Instant,
) -> AgentOutcome {
    AgentOutcome {
        status,
        summary: summary.into(),
        text: text.into(),
        usage,
        session_id: None,
        failure,
        reset_hint: None,
        duration_secs: started.elapsed().as_secs_f64(),
    }
}

fn failure_of(e: &LlmError) -> FailureKind {
    match e {
        LlmError::RateLimited { .. } => FailureKind::RateLimited,
        LlmError::Auth { .. } => FailureKind::AuthRequired,
        LlmError::Timeout(_) => FailureKind::Timeout,
        LlmError::Provider { .. } | LlmError::Other(_) => FailureKind::Crashed,
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) { s.get(prefix.len()..) } else { None }
}

pub fn parse_status_word(s: &str) -> Option<WorkerStatus> {
    let word: String =
        s.trim().chars().take_while(|c| c.is_ascii_alphabetic() || *c == '_' || *c == '-' || *c == ' ').collect();
    match word.trim().to_ascii_uppercase().replace(['-', ' '], "_").as_str() {
        "DONE" => Some(WorkerStatus::Done),
        "DONE_WITH_CONCERNS" => Some(WorkerStatus::DoneWithConcerns),
        "BLOCKED" => Some(WorkerStatus::Blocked),
        "NEEDS_CONTEXT" => Some(WorkerStatus::NeedsContext),
        "FAILED" => Some(WorkerStatus::Failed),
        _ => None,
    }
}

/// The last `STATUS:` and `SUMMARY:` lines of a worker's answer (markdown tolerated).
pub fn parse_status_lines(text: &str) -> (Option<WorkerStatus>, Option<String>) {
    let mut status = None;
    let mut summary = None;
    for line in text.lines().rev() {
        let clean: String = line.chars().filter(|c| *c != '*' && *c != '`').collect();
        let clean = clean.trim();
        if status.is_none()
            && let Some(rest) = strip_prefix_ci(clean, "STATUS:")
        {
            status = parse_status_word(rest);
        } else if summary.is_none()
            && let Some(rest) = strip_prefix_ci(clean, "SUMMARY:")
        {
            let rest = rest.trim();
            if !rest.is_empty() {
                summary = Some(rest.to_string());
            }
        }
        if status.is_some() && summary.is_some() {
            break;
        }
    }
    (status, summary)
}

/// Outcome from a final answer: its STATUS/SUMMARY lines, or Done and its first line.
pub fn final_outcome(reply: &str, usage: Usage, started: Instant) -> AgentOutcome {
    let (status, summary) = parse_status_lines(reply);
    let summary = summary.unwrap_or_else(|| clip(&jsonx::first_sentence(reply), 200));
    outcome(status.unwrap_or(WorkerStatus::Done), summary, reply.trim(), usage, None, started)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Continue,
    Done,
    Ask,
}

/// One step of the API worker's protocol.
#[derive(Debug, Clone, PartialEq)]
pub struct StepReply {
    pub thought: String,
    pub status: StepStatus,
    /// Tool to call (`None` for "none" or blank).
    pub action: Option<String>,
    pub params: Value,
    pub reply: String,
}

pub fn parse_step(text: &str) -> Option<StepReply> {
    let v = jsonx::extract_object(text)?;
    let status = match jsonx::str_field(&v, "status").trim().to_lowercase().as_str() {
        "done" | "final" => StepStatus::Done,
        "ask" => StepStatus::Ask,
        _ => StepStatus::Continue,
    };
    let action = jsonx::opt_str(&v, "action").filter(|a| !a.eq_ignore_ascii_case("none"));
    let params = match v.get("action_params") {
        Some(p @ Value::Object(_)) => p.clone(),
        _ => Value::Object(serde_json::Map::new()),
    };
    Some(StepReply {
        thought: jsonx::str_field(&v, "thought"),
        status,
        action,
        params,
        reply: jsonx::str_field(&v, "reply"),
    })
}

/// Tools a worker may see: read-only workers only get tools with `mutates == false`.
pub fn allowed_specs(specs: Vec<ToolSpec>, read_only: bool) -> Vec<ToolSpec> {
    specs.into_iter().filter(|s| !read_only || !s.mutates).collect()
}

/// Tool list for prompts.
pub fn tools_block(specs: &[ToolSpec]) -> String {
    if specs.is_empty() {
        return "(nenhuma ferramenta disponível)\n".to_string();
    }
    let mut s = String::new();
    for t in specs {
        let mut flags = Vec::new();
        if t.mutates {
            flags.push("altera");
        }
        if t.needs_confirmation {
            flags.push("pede confirmação");
        }
        let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
        s.push_str(&format!("- {}({}): {}{flags}\n", t.name, t.args, t.description));
    }
    s
}

/// "role: what the phase does", for `persona::worker`.
pub fn role_text(unit: &UnitSpec) -> String {
    if unit.does.trim().is_empty() { unit.role.clone() } else { format!("{}: {}", unit.role, unit.does.trim()) }
}

/// System prompt of an API worker.
pub fn api_worker_system(persona: &str, extra_rules: &str, specs: &[ToolSpec], read_only: bool) -> String {
    let mode = if read_only {
        "Você está em modo SOMENTE LEITURA: não altere arquivos, processos nem a entrada do usuário."
    } else {
        "Você pode alterar coisas, mas só dentro do escopo do brief."
    };
    let mut s = format!("{}\n\n{WORKER_CONTRACT}\n\n{mode}\n", persona.trim());
    if !extra_rules.trim().is_empty() {
        s.push('\n');
        s.push_str(extra_rules.trim());
        s.push('\n');
    }
    s.push_str(&format!("\n{PROTOCOL_MARKER}\n{PROTOCOL}\n\nFERRAMENTAS:\n{}", tools_block(specs)));
    s
}

/// Prompt of a CLI worker (one shot).
pub fn cli_worker_prompt(persona: &str, brief: &str) -> String {
    format!("{}\n\n{WORKER_CONTRACT}\n\n{}", persona.trim(), brief.trim())
}

/// The brief a worker receives: the lead's brief plus phase, scope and feedback.
pub fn build_brief(unit: &UnitSpec, prior: &str, feedback: Option<&str>) -> String {
    let mut s = format!("UNIDADE: {} (fase {}, papel {})\n", unit.id, unit.phase, unit.role);
    if !unit.does.trim().is_empty() {
        s.push_str(&format!("O QUE A FASE FAZ: {}\n", unit.does.trim()));
    }
    s.push_str(&format!("\nBRIEF:\n{}\n", unit.brief.trim()));
    if let Some(r) = unit.returns.as_deref().filter(|r| !r.trim().is_empty()) {
        s.push_str(&format!("\nRETORNO ESPERADO: {}\n", r.trim()));
    }
    if !unit.owned_paths.is_empty() {
        s.push_str(&format!("\nCAMINHOS SOB SUA RESPONSABILIDADE: {}\n", unit.owned_paths.join(", ")));
    }
    if unit.read_only {
        s.push_str("\nMODO: somente leitura.\n");
    }
    if !prior.trim().is_empty() {
        s.push_str(&format!("\nRELATÓRIOS DE ONDAS ANTERIORES:\n{}\n", prior.trim()));
    }
    if let Some(f) = feedback.filter(|f| !f.trim().is_empty()) {
        s.push_str(&format!("\nFEEDBACK DA TENTATIVA ANTERIOR (corrija isto):\n{}\n", f.trim()));
    }
    s
}

/// An API worker job: models in fallback order (same tier, next provider first).
pub struct ApiJob {
    pub worker_id: String,
    pub system: String,
    pub brief: String,
    pub models: Vec<(ModelRef, Arc<dyn Llm>)>,
    pub read_only: bool,
}

/// A CLI worker job: agents in fallback order.
pub struct CliJob {
    pub worker_id: String,
    pub prompt: String,
    pub agents: Vec<(ModelRef, Arc<dyn AgentCli>)>,
    pub mode: AgentMode,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
    pub resume_session: Option<String>,
}

fn ended(outcome: AgentOutcome, model: &ModelRef, tool_calls: u32) -> WorkerRun {
    WorkerRun { outcome, model: model.clone(), tool_calls, cancelled: false }
}

fn cancelled_run(model: &ModelRef, usage: Usage, tool_calls: u32, started: Instant) -> WorkerRun {
    WorkerRun {
        outcome: outcome(WorkerStatus::Blocked, "Execução cancelada pelo usuário.", "", usage, None, started),
        model: model.clone(),
        tool_calls,
        cancelled: true,
    }
}

async fn run_tool(env: &WorkerEnv, specs: &[ToolSpec], name: &str, params: Value, read_only: bool) -> ToolResult {
    let Some(spec) = specs.iter().find(|s| s.name == name) else {
        let suffix = if read_only { " (este worker é somente leitura)" } else { "" };
        return ToolResult::err(format!("A ferramenta '{name}' não está disponível{suffix}."));
    };
    if spec.needs_confirmation {
        let question = format!("Posso executar '{}'? {} ({})", spec.name, spec.description, clip(&params.to_string(), 160));
        if !env.confirmer.ask(&question, &env.cancel).await {
            return ToolResult::err("O usuário não confirmou a ação.");
        }
    }
    tokio::select! {
        _ = env.cancel.cancelled() => ToolResult::err("Execução cancelada."),
        r = env.tools.call(name, params) => r,
    }
}

/// JSON action loop: ask the model for a step, run its tool, feed the result back, until
/// `done`, the step/time limit, the budget or cancellation.
pub async fn run_api_worker(env: &WorkerEnv, job: ApiJob) -> WorkerRun {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(env.max_secs.max(1));
    let specs = allowed_specs(env.tools.specs(), job.read_only);
    let sink = stream_sink(&env.bus, &job.worker_id);
    let mut messages = vec![Message::user(job.brief.clone())];
    let mut usage = Usage::default();
    let mut idx = 0usize;
    let mut invalid = 0u32;
    let mut tool_calls = 0u32;
    let mut model = match job.models.first() {
        Some((m, _)) => m.clone(),
        None => {
            let o = outcome(WorkerStatus::Failed, "Nenhum modelo disponível para este worker.", "", usage, None, started);
            return ended(o, &ModelRef::default(), 0);
        }
    };
    let mut last_text = String::new();

    for _ in 0..env.max_steps.max(1) {
        if env.cancel.is_cancelled() {
            return cancelled_run(&model, usage, tool_calls, started);
        }
        if env.budget_exhausted() {
            let o = outcome(
                WorkerStatus::DoneWithConcerns,
                "Orçamento da execução esgotado antes de concluir.",
                last_text,
                usage,
                None,
                started,
            );
            return ended(o, &model, tool_calls);
        }
        let now = Instant::now();
        if now >= deadline {
            let o = outcome(
                WorkerStatus::Failed,
                format!("Tempo esgotado ({} s).", env.max_secs),
                last_text,
                usage,
                Some(FailureKind::Timeout),
                started,
            );
            return ended(o, &model, tool_calls);
        }
        let Some((mref, llm)) = job.models.get(idx) else { break };
        model = mref.clone();
        let req = ChatRequest {
            system: job.system.clone(),
            messages: messages.clone(),
            temperature: Some(0.2),
            max_tokens: Some(2048),
            json: true,
            thinking: mref.effort.clone(),
        };
        let call = tokio::time::timeout(deadline.saturating_duration_since(now), llm.chat(req, Some(sink.clone())));
        let res = tokio::select! {
            _ = env.cancel.cancelled() => return cancelled_run(&model, usage, tool_calls, started),
            r = call => r,
        };
        let resp = match res {
            Err(_) => {
                let o = outcome(
                    WorkerStatus::Failed,
                    format!("Tempo esgotado ({} s).", env.max_secs),
                    last_text,
                    usage,
                    Some(FailureKind::Timeout),
                    started,
                );
                return ended(o, &model, tool_calls);
            }
            Ok(Err(e)) => {
                let limited = if crate::quota::is_api_provider(&mref.provider) {
                    env.quota.observe_model_error(mref, &e)
                } else {
                    env.quota.observe_error(&e).is_some()
                };
                if e.is_transient()
                    && let Some((next, _)) = job.models.get(idx + 1)
                {
                    if limited {
                        env.switch_provider(&mref.provider, &next.provider, switch_reason(&mref.provider, &next.provider, 100.0));
                    }
                    idx += 1;
                    continue;
                }
                let o = outcome(
                    WorkerStatus::Failed,
                    format!("Falha no modelo {}: {}", mref.label(), clip(&e.to_string(), 200)),
                    last_text,
                    usage,
                    Some(failure_of(&e)),
                    started,
                );
                return ended(o, &model, tool_calls);
            }
            Ok(Ok(r)) => r,
        };
        usage.add(&resp.usage);
        env.record_usage(&resp.usage);

        let Some(step) = parse_step(&resp.text) else {
            invalid += 1;
            if invalid >= 3 {
                let text = resp.text.trim().to_string();
                let status = if text.is_empty() { WorkerStatus::Failed } else { WorkerStatus::DoneWithConcerns };
                let o = outcome(status, "O modelo não seguiu o protocolo JSON.", text, usage, None, started);
                return ended(o, &model, tool_calls);
            }
            messages.push(Message::assistant(resp.text));
            messages.push(Message::user(INVALID_STEP_NUDGE));
            continue;
        };
        invalid = 0;
        if !step.reply.trim().is_empty() {
            last_text = step.reply.clone();
        }
        messages.push(Message::assistant(resp.text));
        match step.status {
            StepStatus::Done => {
                return ended(final_outcome(&step.reply, usage, started), &model, tool_calls);
            }
            StepStatus::Ask => {
                let question = if step.reply.trim().is_empty() {
                    "Posso continuar com esta ação?".to_string()
                } else {
                    step.reply.trim().to_string()
                };
                let yes = env.confirmer.ask(&question, &env.cancel).await;
                messages.push(Message::user(if yes {
                    "RESPOSTA DO USUÁRIO: sim, pode continuar."
                } else {
                    "RESPOSTA DO USUÁRIO: não. Não faça isso; finalize explicando o que ficou pendente."
                }));
            }
            StepStatus::Continue => {
                let Some(name) = step.action else {
                    messages.push(Message::user(NO_ACTION_NUDGE));
                    continue;
                };
                let args = clip(&step.params.to_string(), 300);
                let result = run_tool(env, &specs, &name, step.params, job.read_only).await;
                tool_calls += 1;
                env.bus.emit(SageEvent::WorkerTool { worker: job.worker_id.clone(), tool: name.clone(), args, ok: result.ok });
                // keep only the newest screenshot in the conversation
                for m in &mut messages {
                    m.images.clear();
                }
                let body = if result.details.trim().is_empty() { result.spoken.clone() } else { result.details.clone() };
                messages.push(Message {
                    role: Role::User,
                    text: format!("RESULTADO de {name} (ok={}):\n{}", result.ok, clip(&body, 6000)),
                    images: result.image.into_iter().collect(),
                });
            }
        }
    }
    let o = outcome(
        WorkerStatus::Blocked,
        format!("Limite de {} passos atingido sem concluir.", env.max_steps),
        last_text,
        usage,
        None,
        started,
    );
    ended(o, &model, tool_calls)
}

/// One CLI agent run; on a usage limit, the same brief goes to the next agent.
pub async fn run_cli_worker(env: &WorkerEnv, job: CliJob) -> WorkerRun {
    let started = Instant::now();
    let sink = stream_sink(&env.bus, &job.worker_id);
    let mut last: Option<WorkerRun> = None;
    for (i, (mref, agent)) in job.agents.iter().enumerate() {
        if env.cancel.is_cancelled() {
            return cancelled_run(mref, Usage::default(), 0, started);
        }
        let task = AgentTask {
            id: format!("{}-{}", env.run_id, job.worker_id),
            prompt: job.prompt.clone(),
            model: mref.model.clone(),
            effort: mref.effort.clone(),
            mode: job.mode,
            cwd: job.cwd.clone(),
            timeout_secs: job.timeout_secs,
            resume_session: if i == 0 { job.resume_session.clone() } else { None },
        };
        let res = tokio::select! {
            _ = env.cancel.cancelled() => return cancelled_run(mref, Usage::default(), 0, started),
            r = agent.run(task, Some(sink.clone())) => r,
        };
        let next = job.agents.get(i + 1).map(|(m, _)| m.provider.clone());
        match res {
            Ok(mut out) => {
                env.record_usage(&out.usage);
                let limited = env.quota.observe_outcome(&mref.provider, &out);
                if out.summary.trim().is_empty() {
                    let (_, summary) = parse_status_lines(&out.text);
                    out.summary = summary.unwrap_or_else(|| clip(&jsonx::first_sentence(&out.text), 200));
                }
                let run = ended(out, mref, 0);
                if limited && let Some(to) = next {
                    env.switch_provider(&mref.provider, &to, switch_reason(&mref.provider, &to, 100.0));
                    last = Some(run);
                    continue;
                }
                return run;
            }
            Err(e) => {
                let o = outcome(
                    WorkerStatus::Failed,
                    format!("O agente {} falhou: {}", display_name(&mref.provider), clip(&e.to_string(), 200)),
                    "",
                    Usage::default(),
                    Some(FailureKind::Crashed),
                    started,
                );
                let run = ended(o, mref, 0);
                if next.is_some() {
                    last = Some(run);
                    continue;
                }
                return run;
            }
        }
    }
    last.unwrap_or_else(|| {
        let o = outcome(WorkerStatus::Failed, "Nenhum agente CLI disponível.", "", Usage::default(), None, started);
        ended(o, &ModelRef::default(), 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::Tier;

    fn spec(name: &str, mutates: bool) -> ToolSpec {
        ToolSpec { name: name.into(), description: "d".into(), args: serde_json::json!({}), mutates, needs_confirmation: false }
    }

    #[test]
    fn status_lines() {
        let t = "Achei em src/a.rs:3.\n\n**STATUS:** DONE_WITH_CONCERNS\n`SUMMARY:` falta um teste\n";
        assert_eq!(parse_status_lines(t), (Some(WorkerStatus::DoneWithConcerns), Some("falta um teste".into())));
        assert_eq!(parse_status_lines("status: blocked"), (Some(WorkerStatus::Blocked), None));
        assert_eq!(parse_status_lines("nada"), (None, None));
        assert_eq!(parse_status_word("needs-context"), Some(WorkerStatus::NeedsContext));
        let o = final_outcome("Feito. Arquivo salvo.", Usage::default(), Instant::now());
        assert_eq!((o.status, o.summary.as_str()), (WorkerStatus::Done, "Feito."));
    }

    #[test]
    fn steps() {
        let s = parse_step(r#"{"thought":"t","status":"continue","action":"file_read","action_params":{"path":"a"}}"#).unwrap();
        assert_eq!((s.status, s.action.as_deref()), (StepStatus::Continue, Some("file_read")));
        assert_eq!(s.params["path"], "a");
        let s = parse_step(r#"{"status":"DONE","action":"none","action_params":null,"reply":"ok"}"#).unwrap();
        assert_eq!((s.status, s.action, s.reply.as_str()), (StepStatus::Done, None, "ok"));
        assert!(s.params.is_object());
        assert_eq!(parse_step(r#"{"status":"ask","reply":"Apago?"}"#).unwrap().status, StepStatus::Ask);
        assert!(parse_step("não é json").is_none());
    }

    #[test]
    fn read_only_workers_only_see_non_mutating_tools() {
        let specs = vec![spec("file_read", false), spec("file_write", true)];
        let ro = allowed_specs(specs.clone(), true);
        assert_eq!(ro.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["file_read"]);
        assert_eq!(allowed_specs(specs, false).len(), 2);
        let sys = api_worker_system("persona", "", &ro, true);
        assert!(sys.contains(PROTOCOL_MARKER) && sys.contains("CONTRATO DO WORKER") && sys.contains("SOMENTE LEITURA"));
        assert!(sys.contains("file_read") && !sys.contains("file_write"));
    }

    #[test]
    fn brief_carries_scope_and_feedback() {
        let unit = UnitSpec {
            id: "u1".into(),
            phase: "build".into(),
            role: "builder".into(),
            brief: "Implemente X.".into(),
            kind: crate::plan::UnitKind::Code,
            owned_paths: vec!["src/x.rs".into()],
            tier: Tier::Standard,
            tags: vec![],
            read_only: false,
            isolate: false,
            does: "Implement one unit.".into(),
            returns: Some("Changed files.".into()),
            gate: None,
            cwd: None,
            provider: None,
            effort: None,
            rules: vec![],
        };
        let b = build_brief(&unit, "- [scout] s1: achei", Some("faltou o teste"));
        for part in ["Implemente X.", "src/x.rs", "Changed files.", "achei", "faltou o teste"] {
            assert!(b.contains(part), "{part}");
        }
        assert_eq!(role_text(&unit), "builder: Implement one unit.");
        assert!(switch_reason("gemini", "groq", 86.4).contains("Groq: Gemini em 86%"));
    }
}
