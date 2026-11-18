//! Where the brain gets persona texts and text heuristics.
//!
//! The real texts live in `sage_core::persona` and `sage_core::text`. They sit behind
//! [`Prompts`] so tests can inject their own, and so a branch where the port of those
//! functions is unfinished still works with short built-in fallbacks.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Prompt and heuristic source used by the brain.
pub trait Prompts: Send + Sync {
    /// System prompt of the lead (planner and synthesizer).
    fn lead(&self, user_name: Option<&str>) -> String;
    /// System prompt for direct chat replies.
    fn chat(&self, user_name: Option<&str>, long: bool) -> String;
    /// System prompt of a bounded worker with a role description.
    fn worker(&self, role: &str) -> String;
    /// Windows/PowerShell rules for workers that control the PC.
    fn windows_rules(&self) -> String;
    /// Emotion names the avatar understands.
    fn emotions(&self) -> Vec<String>;
    /// Heuristic: does the text look like a task rather than chit-chat?
    fn looks_like_task(&self, text: &str) -> bool;
    /// Request complexity in 0.0..=1.0.
    fn complexity(&self, text: &str) -> f32;
}

/// [`Prompts`] backed by `sage_core::persona` and `sage_core::text`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CorePrompts;

const FALLBACK_LEAD: &str = "Você é o S.A.G.E., um assistente de desktop no Windows. Como lead, você \
decompõe pedidos em unidades independentes, escreve briefs claros para workers e sintetiza a \
resposta final só a partir dos relatórios deles. Responda sempre em português do Brasil.";

const FALLBACK_CHAT: &str = "Você é o S.A.G.E., um assistente de desktop no Windows, direto, \
simpático e útil. Responda sempre em português do Brasil, em poucas frases.";

const FALLBACK_EMOTIONS: &[&str] = &["idle", "feliz", "pensando"];

/// Runs `f`; if it panics (a persona function still `unimplemented!`), uses `fallback`.
fn guarded<T>(f: impl FnOnce() -> T, fallback: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| fallback())
}

fn with_name(base: &str, user_name: Option<&str>) -> String {
    match user_name {
        Some(n) if !n.trim().is_empty() => format!("{base} O usuário se chama {}.", n.trim()),
        _ => base.to_string(),
    }
}

impl Prompts for CorePrompts {
    fn lead(&self, user_name: Option<&str>) -> String {
        guarded(|| sage_core::persona::lead(user_name), || with_name(FALLBACK_LEAD, user_name))
    }

    fn chat(&self, user_name: Option<&str>, long: bool) -> String {
        guarded(
            || sage_core::persona::chat(user_name, long),
            || with_name(FALLBACK_CHAT, user_name),
        )
    }

    fn worker(&self, role: &str) -> String {
        guarded(
            || sage_core::persona::worker(role),
            || format!("Você é um worker do S.A.G.E. com um papel limitado: {role}. Responda em português do Brasil."),
        )
    }

    fn windows_rules(&self) -> String {
        guarded(|| sage_core::persona::windows_rules().to_string(), String::new)
    }

    fn emotions(&self) -> Vec<String> {
        let list: Vec<String> = sage_core::persona::EMOTIONS.iter().map(|s| (*s).to_string()).collect();
        if list.is_empty() {
            FALLBACK_EMOTIONS.iter().map(|s| (*s).to_string()).collect()
        } else {
            list
        }
    }

    fn looks_like_task(&self, text: &str) -> bool {
        guarded(|| sage_core::text::looks_like_task(text), || false)
    }

    fn complexity(&self, text: &str) -> f32 {
        guarded(|| sage_core::text::complexity(text), || 0.0)
    }
}
