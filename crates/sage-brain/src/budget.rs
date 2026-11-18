//! Per-run budget: calls, wall seconds, reported tokens and reported cost.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use sage_core::llm::Usage;

use crate::playbooks::BudgetPolicy;

pub const DEFAULT_MAX_CALLS: u32 = 80;
pub const DEFAULT_MAX_WALL_SECONDS: u64 = 1800;

/// What to do once `warnAt` % of any limit is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum WhenLow {
    /// Finish the wave in progress, start nothing new, report.
    #[default]
    FinishAndReport,
    /// Keep going one tier cheaper.
    DowngradeTier,
    /// Stop before the next wave.
    Stop,
}

impl WhenLow {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "downgrade-tier" => Self::DowngradeTier,
            "stop" => Self::Stop,
            _ => Self::FinishAndReport,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetLimits {
    #[serde(default)]
    pub max_calls: Option<u32>,
    #[serde(default)]
    pub max_wall_seconds: Option<u64>,
    #[serde(default)]
    pub max_reported_tokens: Option<u64>,
    #[serde(default)]
    pub max_reported_cost_usd: Option<f64>,
}

fn tighter<T: PartialOrd + Copy>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x < y { x } else { y }),
        (x, y) => x.or(y),
    }
}

impl BudgetLimits {
    /// Limits for a run: the plan may only tighten the playbook policy (or the defaults),
    /// and `brain_max_cost_usd` from the config caps the cost.
    pub fn merge(plan: Option<&BudgetLimits>, policy: &BudgetPolicy, cfg_max_cost: Option<f64>) -> Self {
        let p = plan.cloned().unwrap_or_default();
        Self {
            max_calls: tighter(p.max_calls, policy.max_calls.or(Some(DEFAULT_MAX_CALLS))),
            max_wall_seconds: tighter(p.max_wall_seconds, policy.max_wall_seconds.or(Some(DEFAULT_MAX_WALL_SECONDS))),
            max_reported_tokens: tighter(p.max_reported_tokens, policy.max_reported_tokens),
            max_reported_cost_usd: tighter(tighter(p.max_reported_cost_usd, policy.max_reported_cost_usd), cfg_max_cost),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetState {
    Healthy,
    /// At or above `warnAt` % of some limit.
    Low,
    /// Some limit reached.
    Exhausted,
}

#[derive(Debug, Clone)]
pub struct Budget {
    pub limits: BudgetLimits,
    pub warn_at_pct: f64,
    pub when_low: WhenLow,
    pub calls: u32,
    pub usage: Usage,
    started: Instant,
    warned: bool,
}

impl Budget {
    pub fn new(limits: BudgetLimits, warn_at_pct: f64, when_low: WhenLow) -> Self {
        Self { limits, warn_at_pct, when_low, calls: 0, usage: Usage::default(), started: Instant::now(), warned: false }
    }

    pub fn from_policy(policy: &BudgetPolicy, plan: Option<&BudgetLimits>, cfg_max_cost: Option<f64>) -> Self {
        Self::new(BudgetLimits::merge(plan, policy, cfg_max_cost), policy.warn_at, WhenLow::parse(&policy.when_low))
    }

    /// One model call (or one CLI run) and what it reported.
    pub fn record(&mut self, usage: &Usage) {
        self.calls = self.calls.saturating_add(1);
        self.usage.add(usage);
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Highest fraction used among the limits, at `elapsed` seconds into the run.
    pub fn fraction_at(&self, elapsed: f64) -> f64 {
        let mut f: f64 = 0.0;
        if let Some(m) = self.limits.max_calls
            && m > 0
        {
            f = f.max(f64::from(self.calls) / f64::from(m));
        }
        if let Some(m) = self.limits.max_wall_seconds
            && m > 0
        {
            f = f.max(elapsed / m as f64);
        }
        if let Some(m) = self.limits.max_reported_tokens
            && m > 0
        {
            f = f.max(self.usage.total() as f64 / m as f64);
        }
        if let Some(m) = self.limits.max_reported_cost_usd
            && m > 0.0
        {
            f = f.max(self.usage.cost_usd.unwrap_or(0.0) / m);
        }
        f
    }

    pub fn state_at(&self, elapsed: f64) -> BudgetState {
        let f = self.fraction_at(elapsed);
        if f >= 1.0 {
            BudgetState::Exhausted
        } else if f * 100.0 >= self.warn_at_pct {
            BudgetState::Low
        } else {
            BudgetState::Healthy
        }
    }

    pub fn state(&self) -> BudgetState {
        self.state_at(self.elapsed_secs())
    }

    /// Percent used right now (for notices).
    pub fn percent(&self) -> f64 {
        self.fraction_at(self.elapsed_secs()) * 100.0
    }

    /// True exactly once: the first time the budget is seen low or exhausted.
    pub fn take_warning(&mut self) -> bool {
        if !self.warned && self.state() != BudgetState::Healthy {
            self.warned = true;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(tokens: u64, cost: Option<f64>) -> Usage {
        Usage { input_tokens: tokens, output_tokens: 0, cache_read_tokens: 0, cost_usd: cost }
    }

    #[test]
    fn calls_limit_warns_then_stops() {
        let limits = BudgetLimits { max_calls: Some(5), ..Default::default() };
        let mut b = Budget::new(limits, 80.0, WhenLow::FinishAndReport);
        for _ in 0..3 {
            b.record(&usage(10, None));
        }
        assert_eq!(b.state_at(0.0), BudgetState::Healthy);
        assert!(!b.take_warning());
        b.record(&usage(10, None));
        assert_eq!(b.state_at(0.0), BudgetState::Low);
        assert!(b.take_warning());
        assert!(!b.take_warning());
        b.record(&usage(10, None));
        assert_eq!(b.state_at(0.0), BudgetState::Exhausted);
        assert_eq!(b.usage.total(), 50);
    }

    #[test]
    fn wall_tokens_and_cost_limits() {
        let limits = BudgetLimits {
            max_wall_seconds: Some(100),
            max_reported_tokens: Some(1000),
            max_reported_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut b = Budget::new(limits, 80.0, WhenLow::Stop);
        assert_eq!(b.state_at(50.0), BudgetState::Healthy);
        assert_eq!(b.state_at(85.0), BudgetState::Low);
        assert_eq!(b.state_at(100.0), BudgetState::Exhausted);
        b.record(&usage(900, None));
        assert_eq!(b.state_at(0.0), BudgetState::Low);
        let mut b = Budget::new(BudgetLimits { max_reported_cost_usd: Some(1.0), ..Default::default() }, 80.0, WhenLow::Stop);
        b.record(&usage(1, Some(0.6)));
        b.record(&usage(1, Some(0.5)));
        assert_eq!(b.state_at(0.0), BudgetState::Exhausted);
    }

    #[test]
    fn merge_only_tightens() {
        let policy = BudgetPolicy { max_calls: Some(30), ..Default::default() };
        let plan = BudgetLimits { max_calls: Some(500), max_reported_cost_usd: Some(5.0), ..Default::default() };
        let m = BudgetLimits::merge(Some(&plan), &policy, Some(2.0));
        assert_eq!(m.max_calls, Some(30));
        assert_eq!(m.max_reported_cost_usd, Some(2.0));
        assert_eq!(m.max_wall_seconds, Some(DEFAULT_MAX_WALL_SECONDS));
        let plan = BudgetLimits { max_calls: Some(3), ..Default::default() };
        assert_eq!(BudgetLimits::merge(Some(&plan), &policy, None).max_calls, Some(3));
        assert_eq!(BudgetLimits::merge(None, &BudgetPolicy::default(), None).max_calls, Some(DEFAULT_MAX_CALLS));
        assert_eq!(WhenLow::parse("downgrade-tier"), WhenLow::DowngradeTier);
        assert_eq!(WhenLow::parse("??"), WhenLow::FinishAndReport);
    }
}
