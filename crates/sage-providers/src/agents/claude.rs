//! Claude Code CLI (`claude -p ... --output-format stream-json`).

use super::{DEFAULT_AGENT_TIMEOUT_SECS, ProcSpec, Raw, find_exe, home_dir, make_outcome, run_process};
use anyhow::Context;
use async_trait::async_trait;
use regex::Regex;
use sage_core::Config;
use sage_core::llm::{AgentCli, AgentMode, AgentOutcome, AgentTask, DeltaSink, FailureKind, Usage};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;

static LIMIT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)usage limit|rate.?limit|limit reached|out of (?:extra )?usage|quota").expect("regex limite")
});
static AUTH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)invalid api key|not logged in|please run /login|authentication_error|oauth token")
        .expect("regex auth")
});
static PERM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)permission denied|not permitted|requires approval").expect("regex perm"));
static RESET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)resets?\s+(?:at\s+|in\s+)?(.+?)\.?$").expect("regex reset"));

pub struct ClaudeCli {
    exe: Option<PathBuf>,
    default_timeout: u64,
}

impl ClaudeCli {
    pub fn from_config(cfg: &Config) -> Self {
        let exe = cfg.claude_enabled.then(Self::locate).flatten();
        Self { exe, default_timeout: cfg.cli_timeout_seconds.unwrap_or(DEFAULT_AGENT_TIMEOUT_SECS) }
    }

    pub fn with_exe(exe: Option<PathBuf>, default_timeout: u64) -> Self {
        Self { exe, default_timeout }
    }

    fn locate() -> Option<PathBuf> {
        let mut cands = Vec::new();
        if let Some(h) = home_dir() {
            cands.push(h.join(".local").join("bin").join("claude.exe"));
        }
        find_exe("", &cands, &["claude", "claude.exe"])
    }
}

/// Argumentos (sem o executável). O prompt vai no stdin.
pub(crate) fn build_args(task: &AgentTask) -> Vec<String> {
    let mut a: Vec<String> = vec!["-p".into()];
    if !task.model.is_empty() {
        a.extend(["--model".into(), task.model.clone()]);
    }
    a.extend(
        ["--output-format", "stream-json", "--verbose", "--include-partial-messages"].map(String::from),
    );
    match &task.resume_session {
        Some(id) => a.extend(["--resume".into(), id.clone()]),
        None => a.push("--no-session-persistence".into()),
    }
    if let Some(e) = task.effort.as_deref().filter(|e| !e.is_empty()) {
        a.extend(["--effort".into(), e.to_string()]);
    }
    let perm = match task.mode {
        AgentMode::ReadOnly => "plan",
        AgentMode::Write => "acceptEdits",
    };
    a.extend(["--permission-mode".into(), perm.into()]);
    a
}

#[derive(Default)]
pub(crate) struct ClaudeParser {
    streamed: String,
    last_assistant: String,
    result: Option<String>,
    is_error: bool,
    usage: Usage,
    session_id: Option<String>,
    rate_status: Option<String>,
    resets_at: Option<i64>,
}

impl ClaudeParser {
    pub fn feed(&mut self, line: &str, sink: Option<&DeltaSink>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if let Some(id) = v.get("session_id").and_then(Value::as_str) {
            self.session_id = Some(id.to_string());
        }
        match v.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let delta = v.pointer("/event/delta");
                if let Some(t) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
                    self.streamed.push_str(t);
                    emit(sink, t, false);
                } else if let Some(t) = delta.and_then(|d| d.get("thinking")).and_then(Value::as_str) {
                    emit(sink, t, true);
                }
            }
            Some("assistant") => {
                let blocks = v.pointer("/message/content").and_then(Value::as_array);
                let mut text = String::new();
                for b in blocks.into_iter().flatten() {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
                        Some("tool_use") => {
                            let name = b.get("name").and_then(Value::as_str).unwrap_or("?");
                            emit(sink, &format!("\n[ferramenta] {name}\n"), true);
                        }
                        _ => {}
                    }
                }
                if !text.trim().is_empty() {
                    self.last_assistant = text;
                }
            }
            Some("rate_limit_event") => {
                let info = v.get("rate_limit_info").unwrap_or(&v);
                if let Some(s) = info.get("status").and_then(Value::as_str) {
                    self.rate_status = Some(s.to_string());
                }
                if let Some(t) = info.get("resetsAt").and_then(Value::as_i64) {
                    self.resets_at = Some(t);
                }
            }
            Some("result") => {
                self.result = v.get("result").and_then(Value::as_str).map(str::to_string);
                self.is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let u = v.get("usage");
                let n = |k: &str| u.and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
                self.usage = Usage {
                    input_tokens: n("input_tokens") + n("cache_creation_input_tokens"),
                    output_tokens: n("output_tokens"),
                    cache_read_tokens: n("cache_read_input_tokens"),
                    cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                };
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Raw {
        let text = self
            .result
            .filter(|r| !r.trim().is_empty())
            .or_else(|| Some(self.last_assistant).filter(|t| !t.trim().is_empty()))
            .unwrap_or(self.streamed);
        let rejected = self.rate_status.as_deref() == Some("rejected");
        let mut failure = None;
        let mut reset_hint = None;
        if self.is_error || rejected {
            if rejected || LIMIT_RE.is_match(&text) {
                failure = Some(FailureKind::RateLimited);
                reset_hint = self
                    .resets_at
                    .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                    .map(|d| d.to_rfc3339())
                    .or_else(|| RESET_RE.captures(text.trim()).map(|c| c[1].trim().to_string()));
            } else if AUTH_RE.is_match(&text) {
                failure = Some(FailureKind::AuthRequired);
            } else if PERM_RE.is_match(&text) {
                failure = Some(FailureKind::PermissionDenied);
            } else {
                failure = Some(FailureKind::Crashed);
            }
        }
        let error_text = if failure.is_some() { text.clone() } else { String::new() };
        Raw { text, usage: self.usage, session_id: self.session_id, failure, reset_hint, error_text }
    }
}

fn emit(sink: Option<&DeltaSink>, text: &str, thought: bool) {
    if let Some(s) = sink {
        s(text, thought);
    }
}

#[async_trait]
impl AgentCli for ClaudeCli {
    fn provider(&self) -> &'static str {
        "claude"
    }

    fn installed(&self) -> bool {
        self.exe.is_some()
    }

    async fn run(&self, task: AgentTask, sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome> {
        let exe = self.exe.as_ref().context("o CLI do Claude Code não foi encontrado")?;
        let args = build_args(&task);
        let timeout = if task.timeout_secs > 0 { task.timeout_secs } else { self.default_timeout };
        let started = Instant::now();
        let mut parser = ClaudeParser::default();
        let sink_ref = sink.as_ref();
        let run = run_process(
            ProcSpec {
                program: exe,
                args: &args,
                cwd: &task.cwd,
                // A assinatura do CLI deve valer: uma chave de API no ambiente mudaria a cobrança.
                env_remove: &["ANTHROPIC_API_KEY"],
                stdin: Some(&task.prompt),
            },
            timeout,
            &mut |l: &str| parser.feed(l, sink_ref),
        )
        .await?;
        Ok(make_outcome(parser.finish(), &run, started))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::WorkerStatus;

    fn task(mode: AgentMode) -> AgentTask {
        AgentTask {
            id: "t".into(),
            prompt: "faça".into(),
            model: "claude-haiku-5-5".into(),
            effort: Some("low".into()),
            mode,
            cwd: PathBuf::from("."),
            timeout_secs: 60,
            resume_session: None,
        }
    }

    #[test]
    fn argv_read_only_and_write() {
        let a = build_args(&task(AgentMode::ReadOnly));
        let s = a.join(" ");
        assert!(s.starts_with("-p --model claude-haiku-5-5 --output-format stream-json --verbose --include-partial-messages --no-session-persistence --effort low"));
        assert!(s.ends_with("--permission-mode plan"));
        assert!(build_args(&task(AgentMode::Write)).join(" ").ends_with("--permission-mode acceptEdits"));
        let mut t = task(AgentMode::Write);
        t.resume_session = Some("abc".into());
        let s = build_args(&t).join(" ");
        assert!(s.contains("--resume abc") && !s.contains("--no-session-persistence"));
    }

    #[test]
    fn parses_stream_success() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"sess-1"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hum"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Pronto"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{}}]}}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1900000000}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Pronto.\nSTATUS: DONE\nSUMMARY: ok","total_cost_usd":0.0123,"session_id":"sess-1","usage":{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":100,"output_tokens":20}}"#,
        ];
        let mut p = ClaudeParser::default();
        for l in lines {
            p.feed(l, None);
        }
        let raw = p.finish();
        assert_eq!(raw.session_id.as_deref(), Some("sess-1"));
        assert!(raw.failure.is_none());
        assert_eq!(raw.usage.input_tokens, 15);
        assert_eq!(raw.usage.cache_read_tokens, 100);
        assert_eq!(raw.usage.cost_usd, Some(0.0123));
        let run = super::super::RunInfo { timed_out: false, exit_ok: true, stderr: String::new() };
        let o = make_outcome(raw, &run, Instant::now());
        assert_eq!(o.status, WorkerStatus::Done);
        assert_eq!(o.summary, "ok");
    }

    #[test]
    fn rate_limit_rejected() {
        let lines = [
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1900000000}}"#,
            r#"{"type":"result","is_error":true,"result":"You've hit your limit"}"#,
        ];
        let mut p = ClaudeParser::default();
        for l in lines {
            p.feed(l, None);
        }
        let raw = p.finish();
        assert_eq!(raw.failure, Some(FailureKind::RateLimited));
        assert!(raw.reset_hint.as_deref().is_some_and(|h| h.starts_with("2030-")));
    }

    #[test]
    fn auth_error_and_garbage_lines() {
        let mut p = ClaudeParser::default();
        p.feed("não é json", None);
        p.feed(r#"{"type":"result","is_error":true,"result":"Invalid API key · Please run /login"}"#, None);
        assert_eq!(p.finish().failure, Some(FailureKind::AuthRequired));
    }

    #[test]
    fn falls_back_to_streamed_text() {
        let mut p = ClaudeParser::default();
        p.feed(r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"parcial"}}}"#, None);
        assert_eq!(p.finish().text, "parcial");
    }
}
