//! Run reports: `data/runs/<run_id>/report.json` + `report.md`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use sage_core::llm::{Tier, Usage, WorkerStatus};

use crate::playbooks::{display_name, tier_name};

pub fn status_label(s: WorkerStatus) -> &'static str {
    match s {
        WorkerStatus::Done => "concluído",
        WorkerStatus::DoneWithConcerns => "concluído com ressalvas",
        WorkerStatus::Blocked => "bloqueado",
        WorkerStatus::NeedsContext => "precisa de contexto",
        WorkerStatus::Failed => "falhou",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRecord {
    pub verdict: String,
    pub note: String,
    /// Model that judged ("" when no reviewer answered).
    pub reviewer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerReport {
    pub id: String,
    pub phase: String,
    pub role: String,
    pub kind: String,
    pub provider: String,
    /// Model label, e.g. "groq:openai/gpt-oss-120b@medium".
    pub model: String,
    pub tier: Tier,
    pub duration_secs: f64,
    /// Summed over every attempt.
    pub usage: Usage,
    pub status: WorkerStatus,
    pub summary: String,
    /// The worker's answer with evidence (clipped).
    pub output: String,
    pub attempts: u32,
    pub tool_calls: u32,
    pub gates: Vec<GateRecord>,
    /// Used by the synthesis, or discarded (failed check / failure).
    pub used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaveReport {
    pub phase: String,
    pub workers: Vec<WorkerReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SwitchRecord {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    /// Something was left out (budget, discarded units).
    Partial,
    /// Cancelled by the user.
    Stopped,
    /// No unit produced a usable result.
    Failed,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "concluída",
            Self::Partial => "parcial",
            Self::Stopped => "interrompida",
            Self::Failed => "falhou",
        }
    }

    pub fn decide(waves: &[WaveReport], missing: &[String], stopped: bool) -> Self {
        if stopped {
            return Self::Stopped;
        }
        let workers: Vec<&WorkerReport> = waves.iter().flat_map(|w| &w.workers).collect();
        if !workers.is_empty() && workers.iter().all(|w| !w.used) {
            return Self::Failed;
        }
        if !missing.is_empty() || workers.iter().any(|w| !w.used) {
            return Self::Partial;
        }
        Self::Completed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunReport {
    pub run_id: String,
    pub request: String,
    pub playbook: String,
    pub why: String,
    pub profile: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_secs: f64,
    pub waves: Vec<WaveReport>,
    pub switches: Vec<SwitchRecord>,
    /// Units planned but not run.
    pub missing: Vec<String>,
    /// Model calls of the whole run (lead, workers, reviewers).
    pub calls: u32,
    pub usage: Usage,
    pub spoken: String,
    pub full: String,
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\r', '\n'], " ")
}

fn cost(c: Option<f64>) -> String {
    c.map_or_else(|| "—".to_string(), |v| format!("${v:.4}"))
}

impl RunReport {
    pub fn workers(&self) -> impl Iterator<Item = &WorkerReport> {
        self.waves.iter().flat_map(|w| w.workers.iter())
    }

    /// One line for `SageEvent::RunReport` and the runs list.
    pub fn summary_line(&self) -> String {
        let n = self.workers().count();
        let price = match self.usage.cost_usd {
            Some(c) => format!(", ${c:.4}"),
            None => String::new(),
        };
        format!(
            "{} ({}): {n} worker(s), {} chamadas, {} tokens{price}, {:.0} s",
            self.playbook,
            self.status.label(),
            self.calls,
            self.usage.total(),
            self.duration_secs
        )
    }

    pub fn render_markdown(&self) -> String {
        let mut s = format!("# Execução {}\n\n", self.run_id);
        s.push_str(&format!("**Pedido:** {}\n\n", self.request.trim()));
        s.push_str(&format!("**Playbook:** {} — {}\n\n", self.playbook, self.why.trim()));
        s.push_str(&format!(
            "**Perfil:** {} · **Status:** {} · **Duração:** {:.1} s · **Chamadas:** {} · **Tokens:** {} (entrada {}, saída {}) · **Custo:** {}\n\n",
            self.profile,
            self.status.label(),
            self.duration_secs,
            self.calls,
            self.usage.total(),
            self.usage.input_tokens,
            self.usage.output_tokens,
            cost(self.usage.cost_usd)
        ));
        s.push_str("## Workers\n\n");
        s.push_str("| Worker | Fase | Papel | Modelo | Tier | Duração | Tokens | Custo | Status | Tentativas | Resultado |\n");
        s.push_str("|---|---|---|---|---|---|---|---|---|---|---|\n");
        for w in self.workers() {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.1} s | {} | {} | {} | {} | {} |\n",
                cell(&w.id),
                cell(&w.phase),
                cell(&w.role),
                cell(&w.model),
                tier_name(w.tier),
                w.duration_secs,
                w.usage.total(),
                cost(w.usage.cost_usd),
                status_label(w.status),
                w.attempts,
                if w.used { "usado" } else { "descartado" }
            ));
        }
        let gates: Vec<(&WorkerReport, &GateRecord)> =
            self.workers().flat_map(|w| w.gates.iter().map(move |g| (w, g))).collect();
        if !gates.is_empty() {
            s.push_str("\n## Gates\n\n");
            for (w, g) in gates {
                let by = if g.reviewer.is_empty() { String::new() } else { format!(" ({})", g.reviewer) };
                s.push_str(&format!("- **{}**: {}{by} — {}\n", w.id, g.verdict, g.note));
            }
        }
        if !self.switches.is_empty() {
            s.push_str("\n## Trocas de provedor\n\n");
            for sw in &self.switches {
                s.push_str(&format!("- {} → {}: {}\n", display_name(&sw.from), display_name(&sw.to), sw.reason));
            }
        }
        if !self.missing.is_empty() {
            s.push_str("\n## Ficou faltando\n\n");
            for m in &self.missing {
                s.push_str(&format!("- {m}\n"));
            }
        }
        s.push_str("\n## Resultados dos workers\n\n");
        for w in self.workers() {
            s.push_str(&format!("### {} ({})\n\n{}\n\n", w.id, status_label(w.status), w.summary.trim()));
        }
        s.push_str("## Resposta\n\n");
        s.push_str(self.full.trim());
        s.push('\n');
        s
    }

    /// Writes `runs_dir/<run_id>/report.json` and `report.md`; returns the JSON path.
    pub fn write(&self, runs_dir: &Path) -> anyhow::Result<PathBuf> {
        let dir = runs_dir.join(&self.run_id);
        std::fs::create_dir_all(&dir)?;
        let json = dir.join("report.json");
        std::fs::write(&json, serde_json::to_string_pretty(self)?)?;
        std::fs::write(dir.join("report.md"), self.render_markdown())?;
        Ok(json)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(id: &str, used: bool) -> WorkerReport {
        WorkerReport {
            id: id.into(),
            phase: "investigate".into(),
            role: "worker".into(),
            kind: "read".into(),
            provider: "groq".into(),
            model: "groq:openai/gpt-oss-120b@medium".into(),
            tier: Tier::Standard,
            duration_secs: 2.5,
            usage: Usage { input_tokens: 100, output_tokens: 20, cache_read_tokens: 0, cost_usd: Some(0.002) },
            status: if used { WorkerStatus::Done } else { WorkerStatus::Failed },
            summary: "achei | tudo".into(),
            output: "src/lib.rs:1".into(),
            attempts: 1,
            tool_calls: 2,
            gates: vec![GateRecord { verdict: "ok".into(), note: "boa evidência".into(), reviewer: "gemini:x".into() }],
            used,
        }
    }

    fn report(workers: Vec<WorkerReport>) -> RunReport {
        let now = Utc::now();
        RunReport {
            run_id: format!("teste-{}", uuid::Uuid::new_v4()),
            request: "mapeie o projeto".into(),
            playbook: "plan-big-execute-small".into(),
            why: "leitura".into(),
            profile: "base".into(),
            status: RunStatus::decide(&[WaveReport { phase: "investigate".into(), workers: workers.clone() }], &[], false),
            started_at: now,
            finished_at: now,
            duration_secs: 3.0,
            waves: vec![WaveReport { phase: "investigate".into(), workers }],
            switches: vec![SwitchRecord { from: "gemini".into(), to: "groq".into(), reason: "Gemini em 86%".into(), at: now }],
            missing: vec!["c (investigate)".into()],
            calls: 5,
            usage: Usage { input_tokens: 300, output_tokens: 60, cache_read_tokens: 0, cost_usd: None },
            spoken: "Pronto.".into(),
            full: "## Resultado".into(),
        }
    }

    #[test]
    fn markdown_has_a_row_per_worker_and_the_extras() {
        let r = report(vec![worker("a", true), worker("b", false)]);
        assert_eq!(r.status, RunStatus::Partial);
        let md = r.render_markdown();
        assert!(md.contains("| a | investigate | worker | groq:openai/gpt-oss-120b@medium | standard | 2.5 s | 120 | $0.0020 | concluído | 1 | usado |"), "{md}");
        assert!(md.contains("| b |") && md.contains("descartado"));
        assert!(md.contains("Gemini → Groq"));
        assert!(md.contains("Ficou faltando"));
        assert!(md.contains("achei | tudo"));
        assert!(r.summary_line().contains("2 worker(s)"));
    }

    #[test]
    fn write_and_load() {
        let dir = std::env::temp_dir().join(format!("sage-runs-{}", uuid::Uuid::new_v4()));
        let r = report(vec![worker("a", true)]);
        let path = r.write(&dir).unwrap();
        assert!(path.ends_with("report.json"));
        assert!(path.with_file_name("report.md").exists());
        assert_eq!(RunReport::load(&path).unwrap(), r);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn run_status_rules() {
        let w = |used| WaveReport { phase: "p".into(), workers: vec![worker("x", used)] };
        assert_eq!(RunStatus::decide(&[w(true)], &[], false), RunStatus::Completed);
        assert_eq!(RunStatus::decide(&[w(false)], &[], false), RunStatus::Failed);
        assert_eq!(RunStatus::decide(&[], &["x".to_string()], false), RunStatus::Partial);
        assert_eq!(RunStatus::decide(&[w(true)], &[], true), RunStatus::Stopped);
    }
}
