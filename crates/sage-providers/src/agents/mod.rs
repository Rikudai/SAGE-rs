//! Agentes CLI (`claude`, `codex`, `agy`): execução de processo, achar o executável,
//! rodapé `STATUS:`/`SUMMARY:` e montagem do `AgentOutcome`.

mod agy;
mod claude;
mod codex;

pub use agy::AgyCli;
pub use claude::ClaudeCli;
pub use codex::CodexCli;

use crate::util::truncate_chars;
use anyhow::Context;
use sage_core::llm::{AgentOutcome, FailureKind, Usage, WorkerStatus};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Timeout padrão de um agente quando nem a tarefa nem o config definem um (15 min).
pub const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 900;

// ------------------------------------------------------------------ executáveis

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    env_dir("USERPROFILE").or_else(|| env_dir("HOME"))
}

pub(crate) fn local_app_data() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA")
}

/// Caminho configurado → candidatos conhecidos → PATH.
pub(crate) fn find_exe(configured: &str, candidates: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = candidates.iter().find(|p| p.is_file()) {
        return Some(p.clone());
    }
    names.iter().find_map(|n| which::which(n).ok())
}

// ------------------------------------------------------------------ rodapé STATUS/SUMMARY

fn strip_marks(s: &str) -> &str {
    s.trim().trim_matches(|c| matches!(c, '*' | '`' | '_' | '#' | '>')).trim()
}

fn parse_status_word(s: &str) -> Option<WorkerStatus> {
    let word = strip_marks(s).split_whitespace().next()?;
    match word.trim_matches(|c| matches!(c, '*' | '`' | '_' | '.')).to_uppercase().replace('-', "_").as_str() {
        "DONE" => Some(WorkerStatus::Done),
        "DONE_WITH_CONCERNS" => Some(WorkerStatus::DoneWithConcerns),
        "BLOCKED" => Some(WorkerStatus::Blocked),
        "NEEDS_CONTEXT" => Some(WorkerStatus::NeedsContext),
        _ => None,
    }
}

/// Procura (de baixo para cima) as linhas `STATUS: ...` e `SUMMARY: ...`.
pub fn parse_footer(text: &str) -> (Option<WorkerStatus>, Option<String>) {
    let mut status = None;
    let mut summary = None;
    for line in text.lines().rev() {
        let l = strip_marks(line);
        let upper = l.to_uppercase();
        if status.is_none() && upper.starts_with("STATUS:") {
            status = l.get(7..).and_then(parse_status_word);
        } else if summary.is_none() && upper.starts_with("SUMMARY:") {
            summary = l.get(8..).map(|s| strip_marks(s).to_string()).filter(|s| !s.is_empty());
        }
        if status.is_some() && summary.is_some() {
            break;
        }
    }
    (status, summary)
}

fn fallback_summary(text: &str) -> String {
    let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    truncate_chars(last, 240)
}

// ------------------------------------------------------------------ processo

pub(crate) struct ProcSpec<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    pub env_remove: &'a [&'a str],
    pub stdin: Option<&'a str>,
}

pub(crate) struct RunInfo {
    pub timed_out: bool,
    pub exit_ok: bool,
    pub stderr: String,
}

/// Roda o processo lendo o stdout linha a linha. O filho é morto no timeout (e ao soltar,
/// por `kill_on_drop`).
pub(crate) async fn run_process<F: FnMut(&str)>(
    spec: ProcSpec<'_>,
    timeout_secs: u64,
    on_line: &mut F,
) -> anyhow::Result<RunInfo> {
    let mut cmd = Command::new(spec.program);
    cmd.args(spec.args)
        .current_dir(spec.cwd)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if spec.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    for k in spec.env_remove {
        cmd.env_remove(k);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = cmd
        .spawn()
        .with_context(|| format!("não consegui iniciar {}", spec.program.display()))?;

    let stdin_task = match (spec.stdin, child.stdin.take()) {
        (Some(text), Some(mut si)) => {
            let data = text.to_owned();
            Some(tokio::spawn(async move {
                let _ = si.write_all(data.as_bytes()).await;
                let _ = si.shutdown().await;
            }))
        }
        _ => None,
    };
    let stdout = child.stdout.take().context("stdout do filho indisponível")?;
    let stderr = child.stderr.take().context("stderr do filho indisponível")?;
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = BufReader::new(stderr).read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let reading = async {
        let mut rd = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if rd.read_until(b'\n', &mut buf).await? == 0 {
                break;
            }
            on_line(String::from_utf8_lossy(&buf).trim_end());
        }
        child.wait().await
    };
    let waited = tokio::time::timeout(Duration::from_secs(timeout_secs), reading).await;

    let (timed_out, exit_ok) = match waited {
        Ok(status) => (false, status?.success()),
        Err(_) => {
            let _ = child.kill().await;
            (true, false)
        }
    };
    if let Some(t) = stdin_task {
        t.abort();
    }
    let stderr = match tokio::time::timeout(Duration::from_secs(2), err_task).await {
        Ok(Ok(s)) => s,
        _ => String::new(),
    };
    Ok(RunInfo { timed_out, exit_ok, stderr })
}

// ------------------------------------------------------------------ resultado

/// O que cada parser de CLI extrai do fluxo de saída.
#[derive(Default)]
pub(crate) struct Raw {
    pub text: String,
    pub usage: Usage,
    pub session_id: Option<String>,
    pub failure: Option<FailureKind>,
    pub reset_hint: Option<String>,
    /// Mensagens de erro do próprio CLI (para o resumo de falha).
    pub error_text: String,
}

pub(crate) fn make_outcome(raw: Raw, run: &RunInfo, started: Instant) -> AgentOutcome {
    let duration_secs = started.elapsed().as_secs_f64();
    let mut failure = raw.failure.clone();
    if run.timed_out {
        failure = Some(FailureKind::Timeout);
    } else if failure.is_none() && !run.exit_ok && raw.text.trim().is_empty() {
        failure = Some(FailureKind::Crashed);
    }
    let (status, summary) = match &failure {
        Some(f) => (WorkerStatus::Failed, failure_summary(f, &raw, run)),
        None => {
            let (st, sm) = parse_footer(&raw.text);
            let default = if run.exit_ok { WorkerStatus::Done } else { WorkerStatus::DoneWithConcerns };
            (st.unwrap_or(default), sm.unwrap_or_else(|| fallback_summary(&raw.text)))
        }
    };
    AgentOutcome {
        status,
        summary,
        text: raw.text,
        usage: raw.usage,
        session_id: raw.session_id,
        failure,
        reset_hint: raw.reset_hint,
        duration_secs,
    }
}

fn failure_summary(f: &FailureKind, raw: &Raw, run: &RunInfo) -> String {
    match f {
        FailureKind::Timeout => "Tempo esgotado; o processo foi encerrado.".to_string(),
        FailureKind::RateLimited => match &raw.reset_hint {
            Some(h) => format!("Limite de uso atingido (volta: {h})."),
            None => "Limite de uso atingido.".to_string(),
        },
        FailureKind::AuthRequired => "O CLI precisa de login.".to_string(),
        FailureKind::PermissionDenied => "Permissão negada ao agente.".to_string(),
        FailureKind::Crashed => {
            let detail = if !raw.error_text.trim().is_empty() { raw.error_text.trim() } else { run.stderr.trim() };
            if detail.is_empty() {
                "O processo terminou com erro sem mensagem.".to_string()
            } else {
                truncate_chars(detail, 300)
            }
        }
    }
}

/// Converte a dica de reinício ("em 1h20m", RFC 3339, "3:45 PM") em segundos a esperar.
pub fn hint_to_secs(hint: &str) -> Option<u64> {
    use chrono::{DateTime, Local, NaiveTime};
    let hint = hint.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(hint) {
        let secs = (dt.timestamp() - chrono::Utc::now().timestamp()).max(0);
        return Some(secs as u64);
    }
    let dur = hint.trim_start_matches("em ").trim_start_matches("in ").trim();
    let looks_like_duration = dur.chars().any(|c| c.is_ascii_digit())
        && dur.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '.' || "dhms".contains(c));
    if looks_like_duration {
        let n = crate::util::parse_duration_secs(dur);
        if n.is_some() {
            return n;
        }
    }
    let clock = hint.to_uppercase();
    let t = ["%I:%M %p", "%I:%M%p", "%I %p", "%H:%M"]
        .iter()
        .find_map(|f| NaiveTime::parse_from_str(clock.trim(), f).ok())?;
    let now = Local::now();
    let mut target = now.date_naive().and_time(t).and_local_timezone(Local).single()?;
    if target <= now {
        target += chrono::Duration::days(1);
    }
    Some((target - now).num_seconds().max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footer_parsing() {
        let t = "Feito.\n\n**STATUS: DONE_WITH_CONCERNS**\nSUMMARY: Implementei tudo, mas falta rodar no Windows.";
        let (st, sm) = parse_footer(t);
        assert_eq!(st, Some(WorkerStatus::DoneWithConcerns));
        assert_eq!(sm.as_deref(), Some("Implementei tudo, mas falta rodar no Windows."));
        assert_eq!(parse_footer("status: needs-context").0, Some(WorkerStatus::NeedsContext));
        assert_eq!(parse_footer("STATUS: BLOCKED").0, Some(WorkerStatus::Blocked));
        assert_eq!(parse_footer("sem rodape"), (None, None));
        assert_eq!(parse_footer("STATUS: talvez").0, None);
    }

    #[test]
    fn outcome_defaults_and_failures() {
        let ok = RunInfo { timed_out: false, exit_ok: true, stderr: String::new() };
        let raw = Raw { text: "linha 1\nresultado final".into(), ..Default::default() };
        let o = make_outcome(raw, &ok, Instant::now());
        assert_eq!(o.status, WorkerStatus::Done);
        assert_eq!(o.summary, "resultado final");

        let timed = RunInfo { timed_out: true, exit_ok: false, stderr: String::new() };
        let o = make_outcome(Raw::default(), &timed, Instant::now());
        assert_eq!(o.failure, Some(FailureKind::Timeout));
        assert_eq!(o.status, WorkerStatus::Failed);

        let bad = RunInfo { timed_out: false, exit_ok: false, stderr: "boom".into() };
        let o = make_outcome(Raw::default(), &bad, Instant::now());
        assert_eq!(o.failure, Some(FailureKind::Crashed));
        assert_eq!(o.summary, "boom");
    }

    #[test]
    fn hints() {
        assert_eq!(hint_to_secs("em 1h20m"), Some(4800));
        let future = (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339();
        assert!(hint_to_secs(&future).is_some_and(|s| (590..=600).contains(&s)));
        assert!(hint_to_secs("3:45 pm").is_some());
        assert_eq!(hint_to_secs("quando der"), None);
    }

    #[test]
    fn find_exe_prefers_configured_then_candidates() {
        let dir = std::env::temp_dir().join(format!("sage-prov-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let f = dir.join("fake-agent.exe");
        std::fs::write(&f, b"x").expect("tmp file");
        let cfg = f.to_string_lossy().to_string();
        assert_eq!(find_exe(&cfg, &[], &[]), Some(f.clone()));
        assert_eq!(find_exe("", std::slice::from_ref(&f), &[]), Some(f.clone()));
        assert_eq!(find_exe("", &[], &["definitivamente-nao-existe-xyz"]), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
