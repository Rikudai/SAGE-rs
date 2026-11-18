//! Antigravity CLI (`agy --output-format json`).

use super::{
    DEFAULT_AGENT_TIMEOUT_SECS, ProcSpec, Raw, find_exe, local_app_data, make_outcome, run_process,
};
use anyhow::Context;
use async_trait::async_trait;
use regex::Regex;
use sage_core::Config;
use sage_core::llm::{AgentCli, AgentMode, AgentOutcome, AgentTask, DeltaSink, FailureKind, Usage};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;

/// Limite de caracteres do prompt (argumento de linha de comando no Windows).
pub const MAX_PROMPT_CHARS: usize = 18_000;

static QUOTA_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)quota|rate.?limit|resource.?exhausted|usage limit|exhausted your capacity")
        .expect("regex cota agy")
});
static RESET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)resets?\s+in\s+((?:\d+\s*[dhms]\s*)+)").expect("regex reset agy")
});
static AUTH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)not logged in|please (?:log ?in|sign in)|unauthenticated").expect("regex auth agy"));

pub struct AgyCli {
    exe: Option<PathBuf>,
    default_timeout: u64,
    full_access: bool,
}

impl AgyCli {
    pub fn from_config(cfg: &Config) -> Self {
        let exe = cfg.antigravity_enabled.then(|| Self::locate(&cfg.agy_path)).flatten();
        Self {
            exe,
            default_timeout: cfg.cli_timeout_seconds.unwrap_or(DEFAULT_AGENT_TIMEOUT_SECS),
            full_access: cfg.full_access,
        }
    }

    pub fn with_exe(exe: Option<PathBuf>, default_timeout: u64) -> Self {
        Self { exe, default_timeout, full_access: false }
    }

    fn locate(configured: &str) -> Option<PathBuf> {
        let mut cands = Vec::new();
        if let Some(l) = local_app_data() {
            cands.push(l.join("agy").join("bin").join("agy.exe"));
            cands.push(l.join("Programs").join("Antigravity").join("bin").join("antigravity.cmd"));
        }
        find_exe(configured, &cands, &["agy", "agy.exe"])
    }
}

pub(crate) fn cap_prompt(prompt: &str) -> String {
    if prompt.chars().count() <= MAX_PROMPT_CHARS {
        return prompt.to_string();
    }
    const NOTE: &str = "\n\n[prompt truncado]";
    let keep = MAX_PROMPT_CHARS - NOTE.chars().count();
    let mut s: String = prompt.chars().take(keep).collect();
    s.push_str(NOTE);
    s
}

/// Argumentos (sem o executável). Aqui o prompt vai no argumento `--print=`.
pub(crate) fn build_args(task: &AgentTask, full_access: bool) -> Vec<String> {
    let mode = match task.mode {
        AgentMode::ReadOnly => "plan",
        AgentMode::Write => "accept-edits",
    };
    let mut a: Vec<String> = vec!["--output-format".into(), "json".into(), "--mode".into(), mode.into()];
    if full_access && task.mode == AgentMode::Write {
        a.push("--dangerously-skip-permissions".into());
    }
    if !task.model.is_empty() {
        a.extend(["--model".into(), task.model.clone()]);
    }
    a.push(format!("--print={}", cap_prompt(&task.prompt)));
    a
}

/// Pega o objeto JSON da saída (pode vir com ruído antes/depois).
pub(crate) fn extract_json(stdout: &str) -> Option<Value> {
    let s = stdout.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return Some(v);
    }
    let (a, b) = (s.find('{')?, s.rfind('}')?);
    if a >= b {
        return None;
    }
    serde_json::from_str(&s[a..=b]).ok()
}

/// "resets in 1h 20m 3s" → "em 1h20m3s".
pub(crate) fn parse_reset(text: &str) -> Option<String> {
    let c = RESET_RE.captures(text)?;
    let compact: String = c[1].split_whitespace().collect();
    Some(format!("em {compact}"))
}

#[derive(Default)]
pub(crate) struct AgyParser {
    stdout: String,
}

impl AgyParser {
    pub fn feed(&mut self, line: &str) {
        self.stdout.push_str(line);
        self.stdout.push('\n');
    }

    pub fn finish(self, stderr: &str, sink: Option<&DeltaSink>) -> Raw {
        let json = extract_json(&self.stdout);
        let (text, session_id, usage, errored) = match &json {
            Some(v) => {
                let text = ["response", "result", "text", "output"]
                    .iter()
                    .find_map(|k| v.get(*k).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                let sid = ["conversation_id", "conversationId", "session_id"]
                    .iter()
                    .find_map(|k| v.get(*k).and_then(Value::as_str))
                    .map(str::to_string);
                let errored = v.get("status").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("error"))
                    || v.get("error").is_some_and(|e| !e.is_null());
                (text, sid, parse_usage(v.get("usage")), errored)
            }
            None => (self.stdout.trim().to_string(), None, Usage::default(), false),
        };
        if let (Some(s), false) = (sink, text.is_empty()) {
            s(&text, false);
        }
        let diag = format!("{}\n{stderr}", if json.is_some() { json_error_text(json.as_ref()) } else { self.stdout.clone() });
        let mut failure = None;
        let mut reset_hint = None;
        if text.trim().is_empty() || errored {
            if QUOTA_RE.is_match(&diag) {
                failure = Some(FailureKind::RateLimited);
                reset_hint = parse_reset(&diag);
            } else if AUTH_RE.is_match(&diag) {
                failure = Some(FailureKind::AuthRequired);
            } else if errored {
                failure = Some(FailureKind::Crashed);
            }
        }
        let error_text = if failure.is_some() { diag.trim().to_string() } else { String::new() };
        Raw { text, usage, session_id, failure, reset_hint, error_text }
    }
}

fn json_error_text(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    let mut out = String::new();
    for k in ["error", "message", "status", "response"] {
        match v.get(k) {
            Some(Value::String(s)) => {
                out.push_str(s);
                out.push('\n');
            }
            Some(o @ Value::Object(_)) => {
                out.push_str(&o.to_string());
                out.push('\n');
            }
            _ => {}
        }
    }
    out
}

fn parse_usage(u: Option<&Value>) -> Usage {
    let Some(u) = u else { return Usage::default() };
    let n = |keys: &[&str]| keys.iter().find_map(|k| u.get(*k).and_then(Value::as_u64)).unwrap_or(0);
    let mut usage = Usage {
        input_tokens: n(&["input_tokens", "prompt_tokens"]),
        output_tokens: n(&["output_tokens", "completion_tokens"]),
        cache_read_tokens: n(&["cached_tokens", "cache_read_tokens"]),
        cost_usd: None,
    };
    let total = n(&["total_tokens"]);
    if usage.total() == 0 && total > 0 {
        // Só o total foi informado: guarda como saída para o orçamento enxergar.
        usage.output_tokens = total;
    }
    usage
}

#[async_trait]
impl AgentCli for AgyCli {
    fn provider(&self) -> &'static str {
        "agy"
    }

    fn installed(&self) -> bool {
        self.exe.is_some()
    }

    async fn run(&self, task: AgentTask, sink: Option<DeltaSink>) -> anyhow::Result<AgentOutcome> {
        let exe = self.exe.as_ref().context("o CLI do Antigravity (agy) não foi encontrado")?;
        let args = build_args(&task, self.full_access);
        let timeout = if task.timeout_secs > 0 { task.timeout_secs } else { self.default_timeout };
        let started = Instant::now();
        let mut parser = AgyParser::default();
        let run = run_process(
            ProcSpec { program: exe, args: &args, cwd: &task.cwd, env_remove: &[], stdin: None },
            timeout,
            &mut |l: &str| parser.feed(l),
        )
        .await?;
        let raw = parser.finish(&run.stderr, sink.as_ref());
        Ok(make_outcome(raw, &run, started))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(mode: AgentMode, prompt: &str) -> AgentTask {
        AgentTask {
            id: "t".into(),
            prompt: prompt.into(),
            model: "gemini-3.8-flash-high".into(),
            effort: None,
            mode,
            cwd: PathBuf::from("."),
            timeout_secs: 60,
            resume_session: None,
        }
    }

    #[test]
    fn argv_and_cap() {
        let a = build_args(&task(AgentMode::ReadOnly, "olá"), false);
        assert_eq!(
            a,
            ["--output-format", "json", "--mode", "plan", "--model", "gemini-3.8-flash-high", "--print=olá"]
        );
        assert_eq!(build_args(&task(AgentMode::Write, "x"), false)[3], "accept-edits");
        let long = "á".repeat(20_000);
        let capped = cap_prompt(&long);
        assert_eq!(capped.chars().count(), MAX_PROMPT_CHARS);
        assert!(capped.ends_with("[prompt truncado]"));
        assert_eq!(cap_prompt("curto"), "curto");
    }

    #[test]
    fn parses_json_with_noise() {
        let out = "carregando...\n{\"response\":\"Pronto.\\nSTATUS: DONE\\nSUMMARY: ok\",\"status\":\"success\",\"conversation_id\":\"c1\",\"usage\":{\"total_tokens\":321}}\n";
        let mut p = AgyParser::default();
        for l in out.lines() {
            p.feed(l);
        }
        let raw = p.finish("", None);
        assert!(raw.text.starts_with("Pronto."));
        assert_eq!(raw.session_id.as_deref(), Some("c1"));
        assert_eq!(raw.usage.total(), 321);
        assert!(raw.failure.is_none());
    }

    #[test]
    fn quota_with_reset() {
        let mut p = AgyParser::default();
        p.feed(r#"{"status":"error","error":"Quota exceeded. Your quota resets in 1h 20m 3s."}"#);
        let raw = p.finish("", None);
        assert_eq!(raw.failure, Some(FailureKind::RateLimited));
        assert_eq!(raw.reset_hint.as_deref(), Some("em 1h20m3s"));
        assert_eq!(crate::agents::hint_to_secs("em 1h20m3s"), Some(4803));
    }

    #[test]
    fn plain_text_fallback_and_quota_on_stderr() {
        let mut p = AgyParser::default();
        p.feed("resposta simples");
        assert_eq!(p.finish("", None).text, "resposta simples");
        let p = AgyParser::default();
        let raw = p.finish("RESOURCE_EXHAUSTED: resets in 2d3h", None);
        assert_eq!(raw.failure, Some(FailureKind::RateLimited));
        assert_eq!(raw.reset_hint.as_deref(), Some("em 2d3h"));
    }
}
