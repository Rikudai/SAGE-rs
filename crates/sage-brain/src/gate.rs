//! Gates: a reviewer checks a unit against its phase's `gate` text and returns
//! `{verdict: ok|retry|escalate, note}`; [`decide`] turns that into the next step.

use serde::{Deserialize, Serialize};

use sage_core::llm::{AgentOutcome, Tier};

use crate::jsonx;
use crate::report::status_label;

/// Marker at the top of the reviewer's system prompt.
pub const GATE_MARKER: &str = "REVISOR DE GATE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Retry,
    Escalate,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Retry => "retry",
            Self::Escalate => "escalate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "ok" | "pass" | "passed" | "aprovado" => Some(Self::Ok),
            "retry" | "fail" | "refazer" => Some(Self::Retry),
            "escalate" | "escalar" => Some(Self::Escalate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateReply {
    pub verdict: Verdict,
    pub note: String,
}

pub fn parse_gate(text: &str) -> Option<GateReply> {
    let v = jsonx::extract_object(text)?;
    let verdict = Verdict::parse(v.get("verdict")?.as_str()?)?;
    Some(GateReply { verdict, note: jsonx::str_field(&v, "note").trim().to_string() })
}

/// What already happened to a unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attempts {
    pub retries: u8,
    pub escalations: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    Accept,
    /// Same worker again, with the reviewer's feedback.
    Retry,
    /// One tier up, once per unit.
    Escalate(Tier),
    /// Out of retries and escalations: report it as not done.
    GiveUp,
}

/// Retry once, then escalate one tier once; `escalate` goes straight up when it can.
pub fn decide(verdict: Verdict, attempts: Attempts, tier: Tier, can_escalate: bool) -> NextStep {
    let up = if can_escalate && attempts.escalations == 0 { tier.up() } else { None };
    match verdict {
        Verdict::Ok => NextStep::Accept,
        Verdict::Retry if attempts.retries == 0 => NextStep::Retry,
        Verdict::Retry => up.map_or(NextStep::GiveUp, NextStep::Escalate),
        Verdict::Escalate => match up {
            Some(t) => NextStep::Escalate(t),
            None if attempts.retries == 0 => NextStep::Retry,
            None => NextStep::GiveUp,
        },
    }
}

pub fn gate_system() -> String {
    format!(
        "{GATE_MARKER} do S.A.G.E.\n\
Você confere se o trabalho de um worker cumpre o critério do gate. Julgue só pela evidência do \
relatório (arquivos e linhas, saídas de comando, URLs), nunca pela palavra do worker.\n\
Responda só com um objeto JSON: {{\"verdict\": \"ok\" | \"retry\" | \"escalate\", \"note\": \"uma ou duas frases\"}}\n\
- ok: o critério foi cumprido e há evidência.\n\
- retry: não foi cumprido, mas o mesmo worker consegue corrigir; diga na note exatamente o que falta.\n\
- escalate: o problema exige um modelo mais forte."
    )
}

pub fn gate_user(criterion: &str, brief: &str, out: &AgentOutcome) -> String {
    format!(
        "CRITÉRIO DO GATE: {}\n\nBRIEF DA UNIDADE:\n{}\n\nSTATUS DECLARADO PELO WORKER: {}\nRESUMO: {}\n\nRELATÓRIO:\n{}",
        criterion.trim(),
        brief.trim(),
        status_label(out.status),
        out.summary.trim(),
        jsonx::clip(out.text.trim(), 6000)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verdicts() {
        let r = parse_gate(r#"{"verdict": "RETRY", "note": " falta a linha "}"#).unwrap();
        assert_eq!(r, GateReply { verdict: Verdict::Retry, note: "falta a linha".into() });
        assert_eq!(parse_gate(r#"ok: {"verdict":"escalate"}"#).unwrap().verdict, Verdict::Escalate);
        assert!(parse_gate(r#"{"verdict": "talvez"}"#).is_none());
        assert!(parse_gate("sem json").is_none());
    }

    #[test]
    fn retry_once_then_escalate_once() {
        let a0 = Attempts::default();
        assert_eq!(decide(Verdict::Ok, a0, Tier::Cheap, true), NextStep::Accept);
        assert_eq!(decide(Verdict::Retry, a0, Tier::Cheap, true), NextStep::Retry);
        let a1 = Attempts { retries: 1, escalations: 0 };
        assert_eq!(decide(Verdict::Retry, a1, Tier::Cheap, true), NextStep::Escalate(Tier::Standard));
        let a2 = Attempts { retries: 1, escalations: 1 };
        assert_eq!(decide(Verdict::Retry, a2, Tier::Standard, true), NextStep::GiveUp);
        assert_eq!(decide(Verdict::Escalate, a0, Tier::Standard, true), NextStep::Escalate(Tier::Strong));
        // nothing above strong: one retry, then give up
        assert_eq!(decide(Verdict::Escalate, a0, Tier::Strong, true), NextStep::Retry);
        assert_eq!(decide(Verdict::Escalate, a1, Tier::Strong, true), NextStep::GiveUp);
        assert_eq!(decide(Verdict::Retry, a1, Tier::Cheap, false), NextStep::GiveUp);
    }
}
