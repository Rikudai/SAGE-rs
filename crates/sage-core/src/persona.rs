//! S.A.G.E. persona: the prompt blocks ported verbatim from the Python
//! `sage/persona.py` and `sage/phrases.py`, plus the user overrides in
//! `data/persona.json` ({"blocks": {"IDENTITY": "...", ...}}).
//!
//! PORT TODO (worker "core-text"): copy every block text verbatim (Portuguese),
//! keep the function signatures below.

/// Avatar emotions the models may return in `"emotion"`.
pub const EMOTIONS: &[&str] = &[];

/// IDENTITY with {name_rule} filled.
pub fn core(_user_name: Option<&str>) -> String {
    unimplemented!("port persona.core")
}

/// System prompt of the lead (planner/synthesizer): core + THINKING + SCOPES +
/// PLANNING + AUTONOMY + the lead line.
pub fn lead(_user_name: Option<&str>) -> String {
    unimplemented!("port persona.brain")
}

/// System prompt for direct chat replies; `long` picks the detailed style.
pub fn chat(_user_name: Option<&str>, _long: bool) -> String {
    unimplemented!("port cortex chat prompt")
}

/// System prompt for a bounded worker with a role description.
pub fn worker(_role_text: &str) -> String {
    unimplemented!("port persona.clone / ROLE_BASE")
}

/// Fixed Windows/PowerShell rules block.
pub fn windows_rules() -> &'static str {
    unimplemented!("port WINDOWS_RULES")
}

/// Greeting for the current local hour (0-23), avoiding the last 3 picks.
pub fn welcome(_hour: u32) -> String {
    unimplemented!("port phrases.WELCOME")
}
pub fn farewell() -> String {
    unimplemented!("port phrases.FAREWELL")
}
/// Clone success / failure quips; `tarefa` fills {tarefa}.
pub fn clone_ok(_tarefa: &str) -> String {
    unimplemented!("port phrases.CLONE_OK")
}
pub fn clone_fail(_tarefa: &str) -> String {
    unimplemented!("port phrases.CLONE_FAIL")
}
