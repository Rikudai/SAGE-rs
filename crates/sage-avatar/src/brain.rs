//! Porte leve do `pet_brain.py`: humor persistente e vontades espontâneas (fidgets, passeio).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::state::Rng;

/// Humor do slime, salvo em `data/pet_mood.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mood {
    pub joy: f32,
    pub affection: f32,
    pub curiosity: f32,
    pub boredom: f32,
}

impl Default for Mood {
    fn default() -> Self {
        Self { joy: 0.5, affection: 0.3, curiosity: 0.4, boredom: 0.2 }
    }
}

impl Mood {
    fn clamp(&mut self) {
        for v in [&mut self.joy, &mut self.affection, &mut self.curiosity, &mut self.boredom] {
            *v = v.clamp(0.0, 1.0);
        }
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Mood>(&t).ok())
            .map(|mut m| {
                m.clamp();
                m
            })
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Eventos que mexem no humor (nomes iguais aos do Python).
#[derive(Debug, Clone, PartialEq)]
pub enum BrainEvent {
    Petted,
    DragStart,
    Drop,
    TaskDone,
    TaskFail,
    Interaction,
    Emotion(String),
}

/// O que o cérebro pede ao slime.
#[derive(Debug, Clone, PartialEq)]
pub enum BrainAction {
    /// Toca uma animação avulsa uma vez.
    Anim(&'static str),
    /// Passeia dentro da coleira.
    Walk { run: bool },
}

const FIDGETS: [&str; 8] =
    ["acenar", "bocejo", "quicar", "danca", "piscadela", "olhar_esq", "olhar_dir", "olhar_cima"];

#[derive(Debug, Clone)]
pub struct PetBrain {
    pub mood: Mood,
    free_will: bool,
    can_walk: bool,
    /// Multiplicador de frequência (personalidade × atividade).
    pace: f32,
    next_fidget: f64,
    next_walk: f64,
    rng: Rng,
}

/// Multiplicador de atividade por personalidade.
pub fn personality_pace(personality: &str) -> f32 {
    match personality {
        "brincalhao" => 1.5,
        "calmo" => 0.6,
        "curioso" => 1.2,
        "dorminhoco" => 0.4,
        "serio" => 0.5,
        _ => 1.0,
    }
}

impl PetBrain {
    pub fn new(mood: Mood, personality: &str, activity: f32, free_will: bool, can_walk: bool, seed: u64) -> Self {
        let pace = (personality_pace(personality) * (0.4 + activity.clamp(0.0, 1.0) * 1.2)).max(0.1);
        let mut rng = Rng::new(seed);
        let next_fidget = rng.range(8.0, 20.0) as f64;
        let next_walk = rng.range(15.0, 40.0) as f64;
        Self { mood, free_will, can_walk, pace, next_fidget, next_walk, rng }
    }

    /// Aplica personalidade/atividade/vontade própria/andar novos (configurações ao vivo).
    pub fn configure(&mut self, personality: &str, activity: f32, free_will: bool, can_walk: bool) {
        self.pace = (personality_pace(personality) * (0.4 + activity.clamp(0.0, 1.0) * 1.2)).max(0.1);
        self.free_will = free_will;
        self.can_walk = can_walk;
    }

    pub fn on_event(&mut self, ev: &BrainEvent) {
        let m = &mut self.mood;
        match ev {
            BrainEvent::Petted => {
                m.affection += 0.04;
                m.joy += 0.15;
            }
            BrainEvent::DragStart => {
                m.curiosity += 0.1;
                m.boredom -= 0.2;
            }
            BrainEvent::Drop => m.joy += 0.03,
            BrainEvent::TaskDone => {
                m.joy += 0.2;
                m.boredom -= 0.4;
            }
            BrainEvent::TaskFail => m.joy -= 0.2,
            BrainEvent::Interaction => {
                m.boredom -= 0.35;
                m.curiosity += 0.1;
            }
            BrainEvent::Emotion(name) => match name.as_str() {
                "feliz" | "amor" | "comemorando" | "rindo" | "orgulhoso" => m.joy += 0.1,
                "triste" | "medo" | "bravo" => m.joy -= 0.1,
                _ => {}
            },
        }
        m.clamp();
    }

    /// Chamado a cada quadro; devolve no máximo uma ação. `quiet` = slime ocioso e acordado.
    pub fn tick(&mut self, now: f64, quiet: bool) -> Option<BrainAction> {
        if !self.free_will || !quiet {
            return None;
        }
        self.mood.boredom = (self.mood.boredom + 0.0005).min(1.0);
        let pace = self.pace as f64;
        if self.can_walk && now >= self.next_walk {
            self.next_walk = now + self.rng.range(20.0, 60.0) as f64 / pace;
            let run = self.rng.unit() < 0.2 + self.mood.joy * 0.2;
            return Some(BrainAction::Walk { run });
        }
        if now >= self.next_fidget {
            self.next_fidget = now + self.rng.range(10.0, 30.0) as f64 / pace;
            return self.rng.pick(&FIDGETS).map(|n| BrainAction::Anim(*n));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_move_mood_within_bounds() {
        let mut b = PetBrain::new(Mood::default(), "", 0.5, true, true, 1);
        let before = b.mood.joy;
        b.on_event(&BrainEvent::Petted);
        assert!(b.mood.joy > before);
        for _ in 0..20 {
            b.on_event(&BrainEvent::TaskFail);
        }
        assert_eq!(b.mood.joy, 0.0);
    }

    #[test]
    fn silent_when_busy_or_without_free_will() {
        let mut b = PetBrain::new(Mood::default(), "", 1.0, true, true, 1);
        assert!(b.tick(10_000.0, false).is_none());
        let mut c = PetBrain::new(Mood::default(), "", 1.0, false, true, 1);
        assert!(c.tick(10_000.0, true).is_none());
    }

    #[test]
    fn eventually_fidgets_and_walks() {
        let mut b = PetBrain::new(Mood::default(), "brincalhao", 1.0, true, true, 3);
        let (mut fidget, mut walk) = (false, false);
        let mut t = 0.0;
        for _ in 0..5000 {
            t += 1.0;
            match b.tick(t, true) {
                Some(BrainAction::Anim(_)) => fidget = true,
                Some(BrainAction::Walk { .. }) => walk = true,
                None => {}
            }
        }
        assert!(fidget && walk);
    }

    #[test]
    fn no_walk_when_disabled() {
        let mut b = PetBrain::new(Mood::default(), "", 1.0, true, false, 3);
        let mut t = 0.0;
        for _ in 0..5000 {
            t += 1.0;
            assert!(!matches!(b.tick(t, true), Some(BrainAction::Walk { .. })));
        }
    }

    #[test]
    fn mood_json_roundtrip_and_garbage() {
        let m = Mood { joy: 0.9, ..Mood::default() };
        let text = serde_json::to_string(&m).unwrap_or_default();
        assert_eq!(serde_json::from_str::<Mood>(&text).ok(), Some(m));
        let partial: Mood = serde_json::from_str(r#"{"joy":0.1}"#).unwrap_or_default();
        assert_eq!(partial.joy, 0.1);
        assert_eq!(partial.affection, Mood::default().affection);
    }
}
