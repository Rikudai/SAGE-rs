//! The lead's JSON: playbook choice and plan, plus validation against the playbook.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use sage_core::llm::Tier;

use crate::budget::BudgetLimits;
use crate::playbooks::{Playbook, Rule};

/// Hard cap of units in one wave.
pub const MAX_UNITS_PER_WAVE: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaybookChoice {
    pub playbook: String,
    #[serde(default)]
    pub why: String,
    /// Título curto da tarefa para o painel (2 a 5 palavras).
    #[serde(default)]
    pub title: String,
}

/// Which executor a unit needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UnitKind {
    /// PC control through the Toolbox (API worker).
    #[default]
    Pc,
    /// Code in a project folder (CLI agent).
    Code,
    /// Reading / research (API worker).
    Read,
    /// Writing text (API worker).
    WriteText,
}

impl UnitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pc => "pc",
            Self::Code => "code",
            Self::Read => "read",
            Self::WriteText => "write-text",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanUnit {
    pub id: String,
    #[serde(default)]
    pub role: String,
    pub brief: String,
    #[serde(default)]
    pub kind: UnitKind,
    #[serde(default, alias = "ownedPaths")]
    pub owned_paths: Vec<String>,
    #[serde(default)]
    pub tier: Option<Tier>,
    /// "light", "deep", "risky" (rules and safety).
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanWave {
    pub phase: String,
    #[serde(default)]
    pub units: Vec<PlanUnit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Plan {
    #[serde(default)]
    pub summary: String,
    /// Project folder for code units.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub waves: Vec<PlanWave>,
    #[serde(default)]
    pub budget: Option<BudgetLimits>,
}

/// A unit ready to run: plan data plus what its phase says.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitSpec {
    pub id: String,
    pub phase: String,
    pub role: String,
    pub brief: String,
    pub kind: UnitKind,
    pub owned_paths: Vec<String>,
    pub tier: Tier,
    pub tags: Vec<String>,
    pub read_only: bool,
    pub isolate: bool,
    pub does: String,
    pub returns: Option<String>,
    pub gate: Option<String>,
    pub cwd: Option<String>,
    /// Phase's preferred provider.
    pub provider: Option<String>,
    /// Phase's effort override.
    pub effort: Option<String>,
    pub rules: Vec<Rule>,
}

impl UnitSpec {
    pub fn is_risky(&self) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case("risky"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WaveSpec {
    pub phase: String,
    pub units: Vec<UnitSpec>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ValidatedPlan {
    pub summary: String,
    /// Empty: the lead does the request itself.
    pub waves: Vec<WaveSpec>,
    pub budget: Option<BudgetLimits>,
}

impl ValidatedPlan {
    pub fn unit_ids(&self) -> Vec<String> {
        self.waves.iter().flat_map(|w| w.units.iter().map(|u| u.id.clone())).collect()
    }
}

fn norm_path(p: &str) -> String {
    p.trim()
        .replace('\\', "/")
        .trim_end_matches("/**")
        .trim_end_matches("/*")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Same path, or one inside the other.
pub fn paths_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (norm_path(a), norm_path(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || b.starts_with(&format!("{a}/")) || a.starts_with(&format!("{b}/"))
}

fn overlapping_paths(units: &[UnitSpec]) -> Option<String> {
    for (i, a) in units.iter().enumerate() {
        for b in &units[i + 1..] {
            for pa in &a.owned_paths {
                if b.owned_paths.iter().any(|pb| paths_overlap(pa, pb)) {
                    return Some(format!(
                        "as unidades '{}' e '{}' disputam o caminho '{pa}'; dê caminhos disjuntos",
                        a.id, b.id
                    ));
                }
            }
        }
    }
    None
}

/// Checks the plan against the playbook. Errors are pt-BR sentences sent back to the
/// lead for its one retry.
pub fn validate(plan: &Plan, playbook: &Playbook, max_units: usize) -> Result<ValidatedPlan, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total = 0usize;
    let mut waves: Vec<(usize, WaveSpec)> = Vec::new();

    for (wi, wave) in plan.waves.iter().enumerate() {
        let Some((pidx, phase)) = playbook.phases.iter().enumerate().find(|(_, p)| p.id == wave.phase) else {
            errors.push(format!("onda {}: a fase '{}' não existe neste playbook", wi + 1, wave.phase));
            continue;
        };
        if phase.is_lead() {
            errors.push(format!("a fase '{}' é do lead; não crie unidades para ela", phase.id));
            continue;
        }
        if phase.is_follow_up() {
            errors.push(format!(
                "a fase '{}' é de acompanhamento e só roda quando um gate pede; não a planeje",
                phase.id
            ));
            continue;
        }
        if wave.units.is_empty() {
            errors.push(format!("a onda da fase '{}' não tem unidades", phase.id));
            continue;
        }
        if wave.units.len() > MAX_UNITS_PER_WAVE {
            errors.push(format!(
                "a onda da fase '{}' tem {} unidades; o máximo por onda é {MAX_UNITS_PER_WAVE}",
                phase.id,
                wave.units.len()
            ));
        }
        let mut units = Vec::with_capacity(wave.units.len());
        for u in &wave.units {
            total += 1;
            let id = u.id.trim();
            if id.is_empty() || id.contains(char::is_whitespace) {
                errors.push(format!("fase '{}': cada unidade precisa de um id curto, sem espaços", phase.id));
                continue;
            }
            if !seen.insert(id.to_string()) {
                errors.push(format!("id de unidade repetido: '{id}'"));
            }
            if u.brief.trim().is_empty() {
                errors.push(format!("a unidade '{id}' está sem brief"));
            }
            let tags: Vec<String> =
                u.tags.iter().map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).collect();
            let mut tier = u.tier.or(phase.tier).unwrap_or(Tier::Standard);
            if tier == Tier::Cheap && tags.iter().any(|t| t.eq_ignore_ascii_case("risky")) {
                tier = Tier::Standard;
            }
            let role = if u.role.trim().is_empty() { phase.role.clone() } else { u.role.trim().to_string() };
            units.push(UnitSpec {
                id: id.to_string(),
                phase: phase.id.clone(),
                role,
                brief: u.brief.trim().to_string(),
                kind: u.kind,
                owned_paths: u.owned_paths.clone(),
                tier,
                tags,
                read_only: phase.read_only,
                isolate: phase.isolate,
                does: phase.does.clone(),
                returns: phase.returns.clone(),
                gate: phase.gate.clone(),
                cwd: u.cwd.clone().or_else(|| plan.cwd.clone()).filter(|c| !c.trim().is_empty()),
                provider: phase.provider.clone(),
                effort: phase.effort.clone(),
                rules: phase.rules.clone(),
            });
        }
        if !phase.read_only
            && let Some(msg) = overlapping_paths(&units)
        {
            errors.push(msg);
        }
        waves.push((pidx, WaveSpec { phase: phase.id.clone(), units }));
    }
    if total > max_units {
        errors.push(format!("o plano tem {total} unidades; o máximo é {max_units}"));
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    waves.sort_by_key(|(i, _)| *i);
    Ok(ValidatedPlan {
        summary: plan.summary.clone(),
        waves: waves.into_iter().map(|(_, w)| w).collect(),
        budget: plan.budget.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonx;
    use crate::playbooks::Playbooks;

    fn playbooks() -> Playbooks {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/playbooks.yaml");
        Playbooks::load(&p).unwrap()
    }

    fn unit(id: &str, paths: &[&str]) -> PlanUnit {
        PlanUnit {
            id: id.into(),
            role: String::new(),
            brief: format!("faça {id}"),
            kind: UnitKind::Code,
            owned_paths: paths.iter().map(|p| (*p).to_string()).collect(),
            tier: None,
            tags: vec![],
            cwd: None,
        }
    }

    #[test]
    fn valid_plan_takes_phase_defaults() {
        let pb = playbooks();
        let raw = r#"```json
        {"summary": "dois módulos", "cwd": "C:/proj", "waves": [{"phase": "investigate", "units": [
          {"id": "a", "brief": "leia a", "kind": "read", "tier": null},
          {"id": "b", "role": "leitor", "brief": "leia b", "kind": "code", "tier": "cheap", "tags": ["Risky"]}
        ]}], "budget": {"maxCalls": 20}}
        ```"#;
        let plan: Plan = jsonx::parse(raw).unwrap();
        let v = validate(&plan, pb.playbook("plan-big-execute-small").unwrap(), 12).unwrap();
        assert_eq!(v.unit_ids(), ["a", "b"]);
        let a = &v.waves[0].units[0];
        assert_eq!((a.tier, a.read_only, a.role.as_str()), (Tier::Standard, true, "worker"));
        assert!(a.gate.is_some());
        assert_eq!(a.cwd.as_deref(), Some("C:/proj"));
        // risky never runs on cheap
        assert_eq!(v.waves[0].units[1].tier, Tier::Standard);
        assert_eq!(v.waves[0].units[1].role, "leitor");
        assert_eq!(v.budget.as_ref().and_then(|b| b.max_calls), Some(20));
    }

    #[test]
    fn rejects_unknown_lead_and_follow_up_phases() {
        let pb = playbooks();
        let complete = pb.playbook("complete").unwrap();
        for phase in ["nope", "plan", "fix"] {
            let plan = Plan { waves: vec![PlanWave { phase: phase.into(), units: vec![unit("x", &[])] }], ..Default::default() };
            let errs = validate(&plan, complete, 10).unwrap_err();
            assert_eq!(errs.len(), 1, "{phase}: {errs:?}");
        }
    }

    #[test]
    fn rejects_duplicates_overlaps_and_too_many_units() {
        let pb = playbooks();
        let pw = pb.playbook("parallel-worktrees").unwrap();
        let plan = Plan {
            waves: vec![PlanWave {
                phase: "build".into(),
                units: vec![unit("a", &["crates/x/**"]), unit("b", &["crates\\x\\src\\lib.rs"]), unit("a", &["docs"])],
            }],
            ..Default::default()
        };
        let errs = validate(&plan, pw, 10).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("repetido")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("disputam")), "{errs:?}");
        let errs = validate(&plan, pw, 2).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("máximo é 2")), "{errs:?}");

        let ok = Plan {
            waves: vec![PlanWave { phase: "build".into(), units: vec![unit("a", &["crates/x"]), unit("b", &["crates/xy"])] }],
            ..Default::default()
        };
        assert!(validate(&ok, pw, 10).is_ok());
    }

    #[test]
    fn empty_plan_means_lead_does_it_and_waves_follow_phase_order() {
        let pb = playbooks();
        let stb = pb.playbook("scout-then-build").unwrap();
        assert!(validate(&Plan::default(), stb, 5).unwrap().waves.is_empty());
        let plan = Plan {
            waves: vec![
                PlanWave { phase: "build".into(), units: vec![unit("build-1", &[])] },
                PlanWave { phase: "scout".into(), units: vec![unit("scout-1", &[])] },
            ],
            ..Default::default()
        };
        let v = validate(&plan, stb, 5).unwrap();
        assert_eq!(v.waves[0].phase, "scout");
        assert_eq!(v.waves[0].units[0].tier, Tier::Cheap);
        assert_eq!(v.waves[1].phase, "build");
    }

    #[test]
    fn overlap_rules() {
        assert!(paths_overlap("src", "SRC/lib.rs"));
        assert!(paths_overlap("a/b/", "a/b"));
        assert!(!paths_overlap("a/b", "a/bc"));
        assert!(!paths_overlap("", "a"));
    }
}
