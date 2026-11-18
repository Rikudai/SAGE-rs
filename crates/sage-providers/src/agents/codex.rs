//! Codex CLI (`codex exec --json`).

use super::{DEFAULT_AGENT_TIMEOUT_SECS, ProcSpec, Raw, find_exe, make_outcome, run_process};
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
    Regex::new(r"(?is)usage limit.*try again (?:at|in)\s+(.+?)\.?\s*$").expect("regex limite codex")
});
static LIMIT_NO_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)usage limit|rate limit|quota exceeded").expect("regex limite codex 2"));
static AUTH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)not logged in|please log ?in|codex login|unauthorized|invalid api key").expect("regex auth codex")
});

pub struct CodexCli {
    exe: Option<PathBuf>,
    default_timeout: u64,
}

impl CodexCli {
    pub fn from_config(cfg: &Config) -> Self {
        let mut cands = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            cands.push(appdata.join("npm").join("codex.cmd"));
        }
        let exe = find_exe("", &cands, &["codex", "codex.exe", "codex.cmd"]);
        Self { exe, default_timeout: cfg.cli_timeout_seconds.unwrap_or(DEFAULT_AGENT_TIMEOUT_SECS) }
    }

    pub fn with_exe(exe: Option<PathBuf>, default_timeout: u64) -> Self {
        Self { exe, default_timeout }
    }
}

/// Argumentos (sem o executável). O prompt vai no stdin (`-`).
pub(crate) fn build_args(task: &AgentTask) -> Vec<String> {
    let sandbox = match task.mode {
        AgentMode::ReadOnly => "read-only",
        AgentMode::Write => "workspace-write",
    };
    let mut a: Vec<String> = vec!["exec".into()];
    if let Some(id) = &task.resume_session {
        a.extend(["resume".into(), id.clone()]);
    }
    a.push("--json".into());
    if !task.model.is_empty() {
        a.extend(["--model".into(), task.model.clone()]);
    }
    if let Some(e) = task.effort.as_deref().filter(|e| !e.is_empty()) {
        a.extend(["-c".into(), format!("model_reasoning_effort={e}")]);
    }
    if task.resume_session.is_some() {
        // `exec resume` não aceita --sandbox/-C: o sandbox vai por -c e a pasta por current_dir.
        a.extend(["-c".into(), format!("sandbox_mode=\"{sandbox}\"")]);
        a.push("--skip-git-repo-check".into());
    } else {
        a.extend(["--sandbox".into(), sandbox.into(), "--skip-git-repo-check".into()]);
        a.extend(["-C".into(), task.cwd.to_string_lossy().into_owned()]);
    }
    a.push("-".into());
    a
}

#[derive(Default)]
pub(crate) struct CodexParser {
    last_message: String,
    usage: Usage,
    thread_id: Option<String>,
    errors: Vec<String>,
    turn_failed: bool,
}

impl CodexParser {
    pub fn feed(&mut self, line: &str, sink: Option<&DeltaSink>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        match v.get("type").and_then(Value::as_str) {
            Some("thread.started") => {
                self.thread_id = v.get("thread_id").and_then(Value::as_str).map(str::to_string);
            }
            Some("item.completed") => {
                let item = v.get("item");
                let text = item.and_then(|i| i.get("text")).and_then(Value::as_str);
                match (item.and_then(|i| i.get("type")).and_then(Value::as_str), text) {
                    (Some("agent_message"), Some(t)) => {
                        if !self.last_message.is_empty() {
                            emit(sink, "\n", false);
                        }
                        self.last_message = t.to_string();
                        emit(sink, t, false);
                    }
                    (Some("reasoning"), Some(t)) => emit(sink, &format!("{t}\n"), true),
                    (Some("command_execution"), _) => {
                        let cmd = item.and_then(|i| i.get("command")).and_then(Value::as_str).unwrap_or("?");
                        emit(sink, &format!("[comando] {cmd}\n"), true);
                    }
                    (Some("error"), _) => {
                        if let Some(m) = item.and_then(|i| i.get("message")).and_then(Value::as_str) {
                            self.errors.push(m.to_string());
                        }
                    }
                    _ => {}
                }
            }
            Some("turn.completed") => {
                if let Some(u) = v.get("usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let cached = n("cached_input_tokens");
                    // `input_tokens` já inclui o cache; separa para não contar duas vezes.
                    self.usage.input_tokens += n("input_tokens").saturating_sub(cached);
                    self.usage.cache_read_tokens += cached;
                    self.usage.output_tokens += n("output_tokens");
                }
            }
            Some("turn.failed") => {
                self.turn_failed = true;
                if let Some(m) = v.pointer("/error/message").and_then(Value::as_str) {
                    self.errors.push(m.to_string());
                }
            }
            Some("error") => {
                if let Some(m) = v.get("message").and_then(Value::as_str) {
                    self.errors.push(m.to_string());
                }
            }
            _ => {}
        }
    }

    pub fn finish(self, stderr: &str) -> Raw {
        let error_text = self.errors.join("\n");
        let haystack = format!("{error_text}\n{stderr}");
        let mut failure = None;
        let mut reset_hint = None;
        if let Some(c) = LIMIT_RE.captures(haystack.trim()) {
            failure = Some(FailureKind::RateLimited);
            reset_hint = Some(c[1].trim().to_string());
        } else if self.last_message.is_empty() && LIMIT_NO_HINT_RE.is_match(&haystack) {
            failure = Some(FailureKind::RateLimited);
        } else if self.last_message.is_empty() && AUTH_RE.is_match(&haystack) {
            failure = Some(FailureKind::AuthRequired);
        } else if self.turn_failed && self.last_message.is_empty() {
            failure = Some(FailureKind::Crashed);
        }
        Raw {
            text: self.last_message,
            usage: self.usage,
            session_id: self.thread_id,
            failure,
            reset_hint,
            error_text,
        }
    }
}

fn emit(sink: Option<&DeltaSink>, text: &str, thought: bool) {
    if let Some(s) = sink {
        s(text, thought);
    }
}

#[async_trait]
impl AgentCli for CodexCli {
    fn provider(&self) -> &'static str {
        "codex"
    }

    fn installed(&self) -> bool {
        self.exe.is_some()
    }

    async fn run(&self, task: AgentTask, sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome> {
        let exe = self.exe.as_ref().context("o CLI do Codex não foi encontrado")?;
        let args = build_args(&task);
        let timeout = if task.timeout_secs > 0 { task.timeout_secs } else { self.default_timeout };
        let started = Instant::now();
        let mut parser = CodexParser::default();
        let sink_ref = sink.as_ref();
        let run = run_process(
            ProcSpec { program: exe, args: &args, cwd: &task.cwd, env_remove: &[], stdin: Some(&task.prompt) },
            timeout,
            &mut |l: &str| parser.feed(l, sink_ref),
        )
        .await?;
        let raw = parser.finish(&run.stderr);
        Ok(make_outcome(raw, &run, started))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(mode: AgentMode) -> AgentTask {
        AgentTask {
            id: "t".into(),
            prompt: "x".into(),
            model: "gpt-5.5".into(),
            effort: Some("high".into()),
            mode,
            cwd: PathBuf::from("C:/proj"),
            timeout_secs: 60,
            resume_session: None,
        }
    }

    #[test]
    fn argv() {
        let s = build_args(&task(AgentMode::ReadOnly)).join(" ");
        assert_eq!(
            s,
            "exec --json --model gpt-5.5 -c model_reasoning_effort=high --sandbox read-only --skip-git-repo-check -C C:/proj -"
        );
        assert!(build_args(&task(AgentMode::Write)).join(" ").contains("--sandbox workspace-write"));
        let mut t = task(AgentMode::Write);
        t.resume_session = Some("th-1".into());
        let s = build_args(&t).join(" ");
        assert!(s.starts_with("exec resume th-1 --json"));
        assert!(s.contains("sandbox_mode=\"workspace-write\"") && !s.contains("-C "));
    }

    #[test]
    fn parses_success() {
        let lines = [
            r#"{"type":"thread.started","thread_id":"th-9"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"i0","type":"reasoning","text":"pensando"}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"Feito.\nSTATUS: DONE\nSUMMARY: tudo certo"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":5}}"#,
        ];
        let mut p = CodexParser::default();
        for l in lines {
            p.feed(l, None);
        }
        let raw = p.finish("");
        assert_eq!(raw.session_id.as_deref(), Some("th-9"));
        assert!(raw.text.starts_with("Feito."));
        assert_eq!((raw.usage.input_tokens, raw.usage.cache_read_tokens, raw.usage.output_tokens), (40, 60, 20));
        assert!(raw.failure.is_none());
    }

    #[test]
    fn usage_limit_with_reset() {
        let lines = [
            r#"{"type":"error","message":"You've hit your usage limit. Upgrade to Pro or try again at 3:45 PM."}"#,
            r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit. Upgrade to Pro or try again at 3:45 PM."}}"#,
        ];
        let mut p = CodexParser::default();
        for l in lines {
            p.feed(l, None);
        }
        let raw = p.finish("");
        assert_eq!(raw.failure, Some(FailureKind::RateLimited));
        assert_eq!(raw.reset_hint.as_deref(), Some("3:45 PM"));
    }

    #[test]
    fn auth_and_crash() {
        let mut p = CodexParser::default();
        p.feed(r#"{"type":"error","message":"Not logged in"}"#, None);
        assert_eq!(p.finish("").failure, Some(FailureKind::AuthRequired));
        let mut p = CodexParser::default();
        p.feed(r#"{"type":"turn.failed","error":{"message":"boom"}}"#, None);
        assert_eq!(p.finish("").failure, Some(FailureKind::Crashed));
    }
}
