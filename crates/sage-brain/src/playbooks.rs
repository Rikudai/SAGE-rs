//! `config/playbooks.yaml`: principles, providers, lead/triage chains, tiers, profiles,
//! quota, budget, pick order and the playbooks with their phases (alethe-orchestrate format).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use sage_core::llm::{ModelRef, Tier};

/// Providers that are coding agent CLIs; every other provider is an HTTP chat model.
pub const CLI_PROVIDERS: &[&str] = &["claude", "codex", "agy"];

pub fn is_cli_provider(provider: &str) -> bool {
    CLI_PROVIDERS.contains(&provider)
}

/// Provider name as shown to the user ("groq" -> "Groq").
pub fn display_name(provider: &str) -> String {
    match provider {
        "agy" => "Antigravity".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        "sambanova" => "SambaNova".to_string(),
        "nvidia" => "NVIDIA".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

pub fn tier_name(t: Tier) -> &'static str {
    match t {
        Tier::Cheap => "cheap",
        Tier::Standard => "standard",
        Tier::Strong => "strong",
    }
}

/// One step down; the cheap tier stays cheap.
pub fn tier_down(t: Tier) -> Tier {
    match t {
        Tier::Strong => Tier::Standard,
        Tier::Standard | Tier::Cheap => Tier::Cheap,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TierModel {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// `tiers[tier][provider]`.
pub type TierTable = BTreeMap<Tier, BTreeMap<String, TierModel>>;

fn d_critical() -> f64 {
    80.0
}
fn d_stop() -> f64 {
    95.0
}
fn d_warn() -> f64 {
    80.0
}
fn d_quota_when_low() -> String {
    "switch-provider".to_string()
}
fn d_ask() -> String {
    "ask".to_string()
}
fn d_budget_when_low() -> String {
    "finish-and-report".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaPolicy {
    /// Usage % from which a provider counts as low and work moves away from it.
    #[serde(default = "d_critical")]
    pub critical_threshold: f64,
    /// Usage % from which a provider is not used at all.
    #[serde(default = "d_stop")]
    pub stop_at: f64,
    #[serde(default = "d_quota_when_low")]
    pub when_low: String,
    /// "ask": confirm with the user when every candidate is low.
    #[serde(default = "d_ask")]
    pub on_both_critical: String,
    #[serde(default)]
    pub when_spare: String,
    #[serde(default)]
    pub spare_within_hours: Option<f64>,
    #[serde(default)]
    pub spare_min_left: Option<f64>,
    #[serde(default)]
    pub plan_ends: BTreeMap<String, Value>,
}

impl Default for QuotaPolicy {
    fn default() -> Self {
        Self {
            critical_threshold: d_critical(),
            stop_at: d_stop(),
            when_low: d_quota_when_low(),
            on_both_critical: d_ask(),
            when_spare: String::new(),
            spare_within_hours: None,
            spare_min_left: None,
            plan_ends: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaOverride {
    #[serde(default)]
    pub critical_threshold: Option<f64>,
    #[serde(default)]
    pub stop_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Passive {
    #[serde(default)]
    pub check_every_minutes: Option<u32>,
    #[serde(default)]
    pub spend_step_usd: Option<f64>,
    #[serde(default)]
    pub prefer: Vec<String>,
    #[serde(default)]
    pub ask_above_workers: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetPolicy {
    /// Percent of any limit at which `when_low` kicks in.
    #[serde(default = "d_warn")]
    pub warn_at: f64,
    /// "finish-and-report" | "downgrade-tier" | "stop".
    #[serde(default = "d_budget_when_low")]
    pub when_low: String,
    #[serde(default)]
    pub max_calls: Option<u32>,
    #[serde(default)]
    pub max_wall_seconds: Option<u64>,
    #[serde(default)]
    pub max_reported_tokens: Option<u64>,
    #[serde(default)]
    pub max_reported_cost_usd: Option<f64>,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        Self {
            warn_at: d_warn(),
            when_low: d_budget_when_low(),
            max_calls: None,
            max_wall_seconds: None,
            max_reported_tokens: None,
            max_reported_cost_usd: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub quota: Option<QuotaOverride>,
    /// Per-tier, per-provider overrides; an entry replaces model and effort together.
    #[serde(default)]
    pub tiers: TierTable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PickRule {
    pub when: String,
    pub playbook: String,
}

/// `if:` of a rule, one condition or a list that must all hold.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn items(&self) -> Vec<&str> {
        match self {
            Self::One(s) => vec![s.as_str()],
            Self::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RuleUse {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub provider: Option<String>,
}

/// A phase rule: when every condition holds, use these model settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    #[serde(rename = "if")]
    pub when: OneOrMany,
    #[serde(rename = "use")]
    pub apply: RuleUse,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub id: String,
    #[serde(default)]
    pub role: String,
    /// "lead" when the lead does this phase itself.
    #[serde(default)]
    pub who: Option<String>,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub workers: Option<u32>,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub isolate: bool,
    #[serde(default)]
    pub does: String,
    #[serde(default)]
    pub returns: Option<String>,
    #[serde(default)]
    pub gate: Option<String>,
    /// Follow-up phase: runs only after this phase id, when its condition holds.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default, rename = "if")]
    pub condition: Option<String>,
    #[serde(default)]
    pub then: Option<String>,
    #[serde(default)]
    pub limit: Option<String>,
    #[serde(default)]
    pub with: Option<String>,
    /// Preferred provider for this phase (e.g. a reviewer on another provider).
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Phase {
    pub fn is_lead(&self) -> bool {
        self.who.as_deref() == Some("lead")
    }
    pub fn is_follow_up(&self) -> bool {
        self.after.is_some()
    }
    /// Phases the plan turns into waves of workers.
    pub fn is_wave(&self) -> bool {
        !self.is_lead() && !self.is_follow_up()
    }

    /// One line describing the phase for the lead's planning prompt.
    pub fn describe(&self) -> String {
        let tier = self.tier.map(tier_name).unwrap_or("standard");
        let mode = if self.read_only { "somente leitura" } else { "pode alterar" };
        let mut s = format!("- `{}`: papel {}, tier {tier}, {mode}", self.id, self.role);
        if let Some(n) = self.workers {
            s.push_str(&format!(", até {n} worker(s)"));
        }
        s.push_str(&format!(". Faz: {}", self.does.trim()));
        if let Some(r) = &self.returns {
            s.push_str(&format!(" Retorna: {}", r.trim()));
        }
        if let Some(g) = &self.gate {
            s.push_str(&format!(" Gate: {}", g.trim()));
        }
        if let Some(c) = &self.condition {
            s.push_str(&format!(" Só quando: {}", c.trim()));
        }
        s
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playbook {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub pick_when: String,
    #[serde(default)]
    pub saves: String,
    #[serde(default)]
    pub avoid_when: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub phases: Vec<Phase>,
}

/// The whole `config/playbooks.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playbooks {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub principles: Vec<String>,
    /// Provider preference order.
    #[serde(default)]
    pub providers: Vec<String>,
    /// Lead models tried in order.
    #[serde(default)]
    pub lead: Vec<ModelRef>,
    /// Triage models tried in order.
    #[serde(default)]
    pub triage: Vec<ModelRef>,
    #[serde(default)]
    pub tiers: TierTable,
    #[serde(default)]
    pub quota: QuotaPolicy,
    #[serde(default)]
    pub passive: Passive,
    #[serde(default)]
    pub budget: BudgetPolicy,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub pick_order: Vec<PickRule>,
    #[serde(default)]
    pub playbooks: BTreeMap<String, Playbook>,
    #[serde(default)]
    pub sources: Vec<String>,
    /// Workers running at the same time in one wave (default 4).
    #[serde(default)]
    pub max_concurrent: Option<usize>,
}

impl Playbooks {
    pub fn parse(yaml: &str) -> anyhow::Result<Self> {
        let pb: Playbooks = serde_yaml::from_str(yaml).context("playbooks.yaml inválido")?;
        pb.check()?;
        Ok(pb)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| format!("lendo {}", path.display()))?;
        Self::parse(raw.trim_start_matches('\u{feff}'))
    }

    fn check(&self) -> anyhow::Result<()> {
        for (id, pb) in &self.playbooks {
            let mut seen = BTreeSet::new();
            for ph in &pb.phases {
                if ph.id.trim().is_empty() {
                    bail!("playbook {id}: fase sem id");
                }
                if !seen.insert(ph.id.as_str()) {
                    bail!("playbook {id}: fase repetida '{}'", ph.id);
                }
            }
            for ph in &pb.phases {
                if let Some(after) = &ph.after
                    && !seen.contains(after.as_str())
                {
                    bail!("playbook {id}: a fase '{}' vem depois de '{after}', que não existe", ph.id);
                }
            }
        }
        for rule in &self.pick_order {
            if !self.playbooks.contains_key(&rule.playbook) {
                bail!("pickOrder cita o playbook '{}', que não existe", rule.playbook);
            }
        }
        Ok(())
    }

    /// The profile in use: the requested one if it exists, else the file's, else "base".
    pub fn active_profile(&self, requested: Option<&str>) -> String {
        [requested, self.profile.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|p| !p.is_empty() && (*p == "base" || self.profiles.contains_key(*p)))
            .unwrap_or("base")
            .to_string()
    }

    /// `tiers[tier][provider]` with the profile's override when it has one.
    pub fn resolve(&self, tier: Tier, provider: &str, profile: &str) -> Option<ModelRef> {
        let over = self
            .profiles
            .get(profile)
            .and_then(|p| p.tiers.get(&tier))
            .and_then(|m| m.get(provider));
        let base = self.tiers.get(&tier).and_then(|m| m.get(provider));
        let tm = over.or(base)?;
        Some(ModelRef { provider: provider.to_string(), model: tm.model.clone(), effort: tm.effort.clone() })
    }

    /// Quota thresholds with the profile's overrides.
    pub fn quota_policy(&self, profile: &str) -> QuotaPolicy {
        let mut q = self.quota.clone();
        if let Some(o) = self.profiles.get(profile).and_then(|p| p.quota.as_ref()) {
            if let Some(c) = o.critical_threshold {
                q.critical_threshold = c;
            }
            if let Some(s) = o.stop_at {
                q.stop_at = s;
            }
        }
        q
    }

    /// `providers`, then any extra provider listed in `passive.prefer`.
    pub fn provider_order(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in self.providers.iter().chain(self.passive.prefer.iter()) {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }

    pub fn playbook(&self, id: &str) -> Option<&Playbook> {
        self.playbooks.get(id)
    }

    /// Fallback playbook: "complete", else the last of `pickOrder`, else any.
    pub fn default_playbook(&self) -> Option<String> {
        if self.playbooks.contains_key("complete") {
            return Some("complete".to_string());
        }
        self.pick_order
            .last()
            .map(|r| r.playbook.clone())
            .or_else(|| self.playbooks.keys().next().cloned())
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent.unwrap_or(4).clamp(1, 16)
    }

    /// Pick order and playbook summaries for the lead's choice prompt.
    pub fn pick_menu(&self) -> String {
        let mut s = String::from("ORDEM DE ESCOLHA (use a primeira que servir):\n");
        for r in &self.pick_order {
            s.push_str(&format!("- {} → {}\n", r.when, r.playbook));
        }
        s.push_str("\nPLAYBOOKS:\n");
        for (id, pb) in &self.playbooks {
            s.push_str(&format!("- {id} ({}): {}", pb.name, pb.goal));
            if !pb.pick_when.is_empty() {
                s.push_str(&format!(" Use quando: {}", pb.pick_when));
            }
            if !pb.avoid_when.is_empty() {
                s.push_str(&format!(" Evite quando: {}", pb.avoid_when));
            }
            s.push('\n');
        }
        s
    }
}

/// What a phase rule's conditions are checked against.
#[derive(Debug, Clone, Default)]
pub struct RuleContext {
    /// Unit tags from the plan ("light", "deep", "risky").
    pub tags: Vec<String>,
    pub budget_low: bool,
    /// Usage % per provider.
    pub quota_pct: BTreeMap<String, f64>,
    pub critical: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatch {
    pub apply: RuleUse,
    /// Provider the rule talks about (`use.provider`, or `claude` in `claude-low`).
    pub provider_hint: Option<String>,
}

/// First rule whose conditions all hold.
pub fn match_rule(rules: &[Rule], ctx: &RuleContext) -> Option<RuleMatch> {
    rules.iter().find_map(|r| {
        let mut hint = None;
        for cond in r.when.items() {
            let (ok, h) = eval_condition(cond, ctx);
            if !ok {
                return None;
            }
            if h.is_some() {
                hint = h;
            }
        }
        Some(RuleMatch { apply: r.apply.clone(), provider_hint: r.apply.provider.clone().or(hint) })
    })
}

/// `budget-low`, `<provider>-low`, `<provider>[-<model>]-above:<pct>` or a unit tag.
fn eval_condition(cond: &str, ctx: &RuleContext) -> (bool, Option<String>) {
    let c = cond.trim();
    if c == "budget-low" {
        return (ctx.budget_low, None);
    }
    if let Some((left, n)) = c.split_once(':')
        && let Some(subject) = left.strip_suffix("-above")
    {
        let provider = subject.split('-').next().unwrap_or(subject).to_string();
        let limit: f64 = n.trim().parse().unwrap_or(100.0);
        let used = ctx.quota_pct.get(&provider).copied().unwrap_or(0.0);
        return (used > limit, Some(provider));
    }
    if let Some(provider) = c.strip_suffix("-low") {
        let used = ctx.quota_pct.get(provider).copied().unwrap_or(0.0);
        return (used >= ctx.critical, Some(provider.to_string()));
    }
    (ctx.tags.iter().any(|t| t.eq_ignore_ascii_case(c)), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real() -> Playbooks {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/playbooks.yaml");
        Playbooks::load(&p).expect("config/playbooks.yaml parses")
    }

    #[test]
    fn parses_the_real_playbooks_file() {
        let pb = real();
        assert_eq!(pb.version, 1);
        assert_eq!(pb.providers, ["gemini", "groq", "claude", "agy", "codex"]);
        assert!(pb.principles.len() >= 10);
        assert_eq!(pb.lead[0], ModelRef { provider: "gemini".into(), model: "gemini-4-hydrogen".into(), effort: Some("high".into()) });
        assert_eq!(pb.triage[0].provider, "groq");
        assert_eq!(pb.pick_order.len(), 11);
        for id in [
            "complete",
            "direct",
            "plan-execute",
            "plan-big-execute-small",
            "scout-then-build",
            "advisor-escalation",
            "cascade",
            "parallel-worktrees",
            "best-of-n",
            "review-gate",
            "audit-at-scale",
        ] {
            assert!(pb.playbook(id).is_some(), "playbook {id}");
        }
        assert_eq!(pb.quota.critical_threshold, 80.0);
        assert_eq!(pb.quota.stop_at, 95.0);
        assert_eq!(pb.quota.on_both_critical, "ask");
        assert_eq!(pb.budget.warn_at, 80.0);
        assert_eq!(pb.budget.when_low, "finish-and-report");
        assert_eq!(pb.max_concurrent(), 4);

        let complete = pb.playbook("complete").unwrap();
        let build = complete.phases.iter().find(|p| p.id == "build").unwrap();
        assert_eq!(build.condition.as_deref(), Some("not kind:research"));
        assert_eq!(build.rules.len(), 4);
        assert_eq!(build.rules[0].when.items(), ["deep", "claude-opus-above:50"]);
        assert_eq!(build.rules[1].apply.tier, Some(Tier::Strong));
        let review = complete.phases.iter().find(|p| p.id == "review").unwrap();
        assert_eq!(review.provider.as_deref(), Some("codex"));
        assert!(review.read_only);
        assert!(review.gate.is_some());
        let fix = complete.phases.iter().find(|p| p.id == "fix").unwrap();
        assert!(fix.is_follow_up() && !fix.is_wave());

        let pbes = pb.playbook("plan-big-execute-small").unwrap();
        let waves: Vec<&str> = pbes.phases.iter().filter(|p| p.is_wave()).map(|p| p.id.as_str()).collect();
        assert_eq!(waves, ["investigate"]);
        assert_eq!(pbes.phases[1].workers, Some(4));
        assert!(pb.playbook("direct").unwrap().phases.iter().all(|p| !p.is_wave()));
        assert!(pb.playbook("parallel-worktrees").unwrap().phases[1].isolate);
        assert!(pb.pick_menu().contains("plan-big-execute-small"));
    }

    #[test]
    fn resolves_tiers_with_profile_overrides() {
        let pb = real();
        let m = pb.resolve(Tier::Standard, "gemini", "base").unwrap();
        assert_eq!((m.model.as_str(), m.effort.as_deref()), ("gemini-3.6-flash", Some("medium")));
        // savings: an override replaces model and effort together
        let m = pb.resolve(Tier::Standard, "gemini", "savings").unwrap();
        assert_eq!((m.model.as_str(), m.effort.as_deref()), ("gemini-3.1-flash-lite", None));
        // no override for groq: base entry
        let m = pb.resolve(Tier::Standard, "groq", "savings").unwrap();
        assert_eq!((m.model.as_str(), m.effort.as_deref()), ("openai/gpt-oss-120b", Some("medium")));
        let m = pb.resolve(Tier::Cheap, "claude", "aggressive").unwrap();
        assert_eq!((m.model.as_str(), m.effort.as_deref()), ("sonnet", Some("low")));
        let m = pb.resolve(Tier::Strong, "claude", "savings").unwrap();
        assert_eq!(m.label(), "claude:sonnet@high");
        assert!(pb.resolve(Tier::Strong, "mistral", "base").is_none());

        assert_eq!(pb.active_profile(Some("savings")), "savings");
        assert_eq!(pb.active_profile(Some("nao-existe")), "base");
        assert_eq!(pb.active_profile(None), "base");
        let q = pb.quota_policy("savings");
        assert_eq!((q.critical_threshold, q.stop_at), (70.0, 90.0));
        let q = pb.quota_policy("aggressive");
        assert_eq!((q.critical_threshold, q.stop_at), (95.0, 98.0));
        assert_eq!(pb.quota_policy("base").critical_threshold, 80.0);
    }

    #[test]
    fn rules_match_tags_budget_and_quota() {
        let pb = real();
        let build = pb.playbook("complete").unwrap().phases.iter().find(|p| p.id == "build").unwrap().clone();
        let mut ctx = RuleContext { tags: vec!["deep".into()], critical: 80.0, ..Default::default() };
        // deep with claude under 50%: second rule (tier strong)
        let m = match_rule(&build.rules, &ctx).unwrap();
        assert_eq!(m.apply.tier, Some(Tier::Strong));
        // deep with claude above 50%: first rule, a sonnet hint for claude
        ctx.quota_pct.insert("claude".into(), 60.0);
        let m = match_rule(&build.rules, &ctx).unwrap();
        assert_eq!(m.apply.model.as_deref(), Some("sonnet"));
        assert_eq!(m.provider_hint.as_deref(), Some("claude"));
        ctx.tags = vec!["light".into()];
        assert_eq!(match_rule(&build.rules, &ctx).unwrap().apply.tier, Some(Tier::Cheap));
        ctx.tags.clear();
        assert!(match_rule(&build.rules, &ctx).is_none());

        let fix = pb.playbook("complete").unwrap().phases.iter().find(|p| p.id == "fix").unwrap().clone();
        let mut ctx = RuleContext { critical: 80.0, ..Default::default() };
        ctx.quota_pct.insert("claude".into(), 85.0);
        let m = match_rule(&fix.rules, &ctx).unwrap();
        assert_eq!(m.apply.provider.as_deref(), Some("codex"));
        assert_eq!(m.provider_hint.as_deref(), Some("codex"));
        ctx.budget_low = true;
        assert_eq!(match_rule(&fix.rules, &ctx).unwrap().apply.tier, Some(Tier::Standard));
    }

    #[test]
    fn rejects_broken_files() {
        let bad = "playbooks:\n  x:\n    phases:\n      - { id: a }\n      - { id: a }\n";
        assert!(Playbooks::parse(bad).is_err());
        let bad = "pickOrder:\n  - { when: sempre, playbook: nada }\n";
        assert!(Playbooks::parse(bad).is_err());
        let bad = "playbooks:\n  x:\n    phases:\n      - { id: a, after: b }\n";
        assert!(Playbooks::parse(bad).is_err());
        assert!(Playbooks::parse("version: 1\n").is_ok());
    }

    #[test]
    fn names_and_tiers() {
        assert_eq!(display_name("groq"), "Groq");
        assert_eq!(display_name("agy"), "Antigravity");
        assert_eq!(tier_down(Tier::Strong), Tier::Standard);
        assert_eq!(tier_down(Tier::Cheap), Tier::Cheap);
        assert!(is_cli_provider("codex") && !is_cli_provider("gemini"));
    }
}
