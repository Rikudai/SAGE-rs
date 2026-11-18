//! Máquina de estados do slime e dos clones (lógica pura; o tempo entra como parâmetro).

use sage_core::events::{AppStatus, SageEvent};

use crate::anim::{Timing, is_looping};

/// Quantos quadros existem para (forma/escopo, animação). Zero = animação inexistente.
pub trait FrameInfo {
    fn frame_count(&self, scope: &str, anim: &str) -> usize;
}

impl<F: Fn(&str, &str) -> usize> FrameInfo for F {
    fn frame_count(&self, scope: &str, anim: &str) -> usize {
        self(scope, anim)
    }
}

/// Gerador pseudoaleatório pequeno (xorshift) — sem dependência externa e testável.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Número em [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get((self.next_u64() % items.len() as u64) as usize)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Morph {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    pub run: bool,
    /// -1.0 (esquerda) ou 1.0 (direita).
    pub dir: f32,
}

/// Estado do slime principal.
#[derive(Debug, Clone)]
pub struct Pet {
    pub form: String,
    pending_form: Option<String>,
    morph: Option<Morph>,
    status: AppStatus,
    deep_thinking: bool,
    workers: usize,
    emotion: Option<(String, f64)>,
    oneshot: Option<String>,
    dragging: bool,
    landing: bool,
    petted_until: f64,
    last_activity: f64,
    sleeping: bool,
    sleep_after: f64,
    walk: Option<Walk>,
    anim: String,
    frame: usize,
    acc_ms: f64,
    timing: Timing,
    rng: Rng,
}

impl Pet {
    pub fn new(form: &str, sleep_after_secs: u64, timing: Timing, seed: u64) -> Self {
        let form = if form.is_empty() { "slime" } else { form };
        Self {
            form: form.to_string(),
            pending_form: None,
            morph: None,
            status: AppStatus::Idle,
            deep_thinking: false,
            workers: 0,
            emotion: None,
            oneshot: None,
            dragging: false,
            landing: false,
            petted_until: 0.0,
            last_activity: 0.0,
            sleeping: false,
            sleep_after: sleep_after_secs.max(1) as f64,
            walk: None,
            anim: "idle".into(),
            frame: 0,
            acc_ms: 0.0,
            timing,
            rng: Rng::new(seed),
        }
    }

    pub fn anim(&self) -> &str {
        &self.anim
    }
    pub fn frame(&self) -> usize {
        self.frame
    }
    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }
    pub fn walking(&self) -> Option<Walk> {
        self.walk
    }
    pub fn worker_count(&self) -> usize {
        self.workers
    }
    pub fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }

    fn touch(&mut self, now: f64) {
        self.last_activity = now;
        self.sleeping = false;
    }

    /// Reage a um evento do barramento. `now` em segundos.
    pub fn on_event(&mut self, ev: &SageEvent, now: f64) {
        match ev {
            SageEvent::Status { status, .. } => {
                self.status = *status;
                if *status != AppStatus::Idle {
                    self.touch(now);
                }
            }
            SageEvent::UserText { .. } => self.touch(now),
            SageEvent::Emotion { name } => self.set_emotion(name, now),
            SageEvent::Reply { emotion: Some(name), .. } => self.set_emotion(name, now),
            SageEvent::Form { name } => self.change_form(name),
            SageEvent::WorkerStart { .. } => {
                self.workers += 1;
                self.touch(now);
            }
            SageEvent::WorkerEnd { .. } => self.workers = self.workers.saturating_sub(1),
            _ => {}
        }
    }

    /// `sabio` em vez de `pensando` (raciocínio profundo).
    pub fn set_deep_thinking(&mut self, deep: bool) {
        self.deep_thinking = deep;
    }

    /// Emoção temporária; relaxa sozinha após 6–12 s.
    pub fn set_emotion(&mut self, name: &str, now: f64) {
        let secs = self.rng.range(6.0, 12.0) as f64;
        self.emotion = if name.is_empty() || name == "idle" || name == "neutro" {
            None
        } else {
            Some((name.to_string(), now + secs))
        };
        self.touch(now);
    }

    /// Troca de forma: `morph_out` na atual, carrega a nova e `morph_in`.
    pub fn change_form(&mut self, name: &str) {
        if name.is_empty() || name == self.form || self.morph.is_some() {
            return;
        }
        self.pending_form = Some(name.to_string());
        self.morph = Some(Morph::Out);
    }

    /// Toca uma animação avulsa (fidget do cérebro) uma vez.
    pub fn play_once(&mut self, anim: &str) {
        if self.morph.is_none() && !self.dragging {
            self.oneshot = Some(anim.to_string());
        }
    }

    pub fn set_walk(&mut self, walk: Option<Walk>) {
        self.walk = walk;
    }

    pub fn begin_drag(&mut self, now: f64) {
        self.dragging = true;
        self.walk = None;
        self.touch(now);
    }

    /// Soltou o slime: toca `pouso`.
    pub fn end_drag(&mut self, now: f64) {
        if self.dragging {
            self.dragging = false;
            self.landing = true;
        }
        self.touch(now);
    }

    /// Duplo clique (carinho): `feliz` por 2,4 s.
    pub fn pet(&mut self, now: f64) {
        self.petted_until = now + 2.4;
        self.walk = None;
        self.touch(now);
    }

    fn busy_anim(&self) -> Option<&'static str> {
        match self.status {
            AppStatus::Speaking => Some("talk"),
            AppStatus::Listening => Some("ouvindo"),
            AppStatus::Thinking | AppStatus::Transcribing => {
                Some(if self.deep_thinking { "sabio" } else { "pensando" })
            }
            AppStatus::Orchestrating => Some(if self.workers >= 2 { "maestro" } else { "orquestrando" }),
            AppStatus::Idle | AppStatus::Error => None,
        }
    }

    /// Animação desejada agora, por prioridade.
    pub fn desired_anim(&self, now: f64) -> String {
        if let Some(m) = self.morph {
            return match m {
                Morph::Out => "morph_out",
                Morph::In => "morph_in",
            }
            .into();
        }
        if self.dragging {
            return "arrastado".into();
        }
        if self.landing {
            return "pouso".into();
        }
        if now < self.petted_until {
            return "feliz".into();
        }
        if let Some(a) = &self.oneshot {
            return a.clone();
        }
        if self.sleeping {
            return "dormindo".into();
        }
        if let Some(a) = self.busy_anim() {
            return a.into();
        }
        if self.status == AppStatus::Error {
            return "triste".into();
        }
        if let Some((e, _)) = &self.emotion {
            return e.clone();
        }
        match self.walk {
            Some(w) if w.run => "run".into(),
            Some(_) => "walk".into(),
            None => "idle".into(),
        }
    }

    /// Avança o relógio: relaxa emoção, adormece, troca/avança quadros.
    pub fn tick(&mut self, dt: f64, now: f64, info: &dyn FrameInfo) {
        if let Some((_, until)) = &self.emotion
            && now >= *until
        {
            self.emotion = None;
        }
        let quiet = self.status == AppStatus::Idle && self.workers == 0;
        if quiet && !self.sleeping && now - self.last_activity >= self.sleep_after {
            self.sleeping = true;
            self.walk = None;
        }

        let want = self.desired_anim(now);
        // Animação sem arquivos: cai para idle em vez de travar o estado.
        let want = if info.frame_count(&self.form, &want) == 0 { "idle".to_string() } else { want };
        if want != self.anim {
            self.anim = want;
            self.frame = 0;
            self.acc_ms = 0.0;
        }

        let count = info.frame_count(&self.form, &self.anim);
        if count == 0 {
            return;
        }
        self.acc_ms += dt * 1000.0;
        let step = self.timing.frame_ms(&self.anim) as f64;
        while self.acc_ms >= step {
            self.acc_ms -= step;
            self.frame += 1;
            if self.frame >= count {
                if is_looping(&self.anim) {
                    self.frame = 0;
                } else {
                    self.frame = count.saturating_sub(1);
                    self.finish_oneshot();
                    break;
                }
            }
        }
    }

    fn finish_oneshot(&mut self) {
        match self.morph {
            Some(Morph::Out) => {
                if let Some(f) = self.pending_form.take() {
                    self.form = f;
                }
                self.morph = Some(Morph::In);
            }
            Some(Morph::In) => self.morph = None,
            None => {
                if self.landing {
                    self.landing = false;
                } else if self.oneshot.as_deref() == Some(self.anim.as_str()) {
                    self.oneshot = None;
                }
            }
        }
        self.frame = 0;
        self.acc_ms = 0.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClonePhase {
    Spawn,
    Work,
    Done,
    Fail,
    Vanish,
}

impl ClonePhase {
    pub fn anim(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Work => "work",
            Self::Done => "done",
            Self::Fail => "fail",
            Self::Vanish => "vanish",
        }
    }
}

/// Um clone pequeno por worker em execução.
#[derive(Debug, Clone)]
pub struct CloneSprite {
    pub worker: String,
    pub phase: ClonePhase,
    pub frame: usize,
    acc_ms: f64,
    ending: Option<bool>,
    pub finished: bool,
}

impl CloneSprite {
    pub fn new(worker: &str) -> Self {
        Self { worker: worker.into(), phase: ClonePhase::Spawn, frame: 0, acc_ms: 0.0, ending: None, finished: false }
    }

    pub fn is_ending(&self) -> bool {
        self.ending.is_some()
    }

    /// `ok = true` → `done`, senão `fail`; depois `vanish` e some.
    pub fn end(&mut self, ok: bool) {
        self.ending = Some(ok);
        if self.phase == ClonePhase::Work {
            self.advance_phase();
        }
    }

    fn advance_phase(&mut self) {
        self.phase = match (self.phase, self.ending) {
            (ClonePhase::Spawn, None) => ClonePhase::Work,
            (ClonePhase::Spawn | ClonePhase::Work, Some(true)) => ClonePhase::Done,
            (ClonePhase::Spawn | ClonePhase::Work, Some(false)) => ClonePhase::Fail,
            (ClonePhase::Done | ClonePhase::Fail, _) => ClonePhase::Vanish,
            (ClonePhase::Vanish, _) => {
                self.finished = true;
                ClonePhase::Vanish
            }
            (ClonePhase::Work, None) => ClonePhase::Work,
        };
        self.frame = 0;
        self.acc_ms = 0.0;
    }

    pub fn tick(&mut self, dt: f64, timing: &Timing, info: &dyn FrameInfo) {
        let anim = self.phase.anim();
        let count = info.frame_count("clone", anim);
        if count == 0 {
            // Sem arquivos: pula a fase para não prender o clone.
            if self.phase != ClonePhase::Work || self.ending.is_some() {
                self.advance_phase();
            }
            return;
        }
        self.acc_ms += dt * 1000.0;
        let step = timing.frame_ms(anim) as f64;
        while self.acc_ms >= step {
            self.acc_ms -= step;
            self.frame += 1;
            if self.frame >= count {
                if self.phase == ClonePhase::Work && self.ending.is_none() {
                    self.frame = 0;
                } else {
                    self.advance_phase();
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sage_core::llm::{Tier, Usage, WorkerStatus};

    fn info(_: &str, anim: &str) -> usize {
        if anim == "inexistente" { 0 } else { 3 }
    }

    fn pet() -> Pet {
        Pet::new("slime", 60, Timing::default(), 7)
    }

    fn status(s: AppStatus) -> SageEvent {
        SageEvent::Status { status: s, detail: String::new() }
    }

    fn start(w: &str) -> SageEvent {
        SageEvent::WorkerStart {
            run_id: "r".into(),
            worker: w.into(),
            role: "x".into(),
            model: "m".into(),
            tier: Tier::Cheap,
            brief: String::new(),
        }
    }

    fn end(w: &str) -> SageEvent {
        SageEvent::WorkerEnd {
            run_id: "r".into(),
            worker: w.into(),
            status: WorkerStatus::Done,
            summary: String::new(),
            usage: Usage::default(),
        }
    }

    #[test]
    fn status_maps_to_animation() {
        let mut p = pet();
        assert_eq!(p.desired_anim(0.0), "idle");
        p.on_event(&status(AppStatus::Speaking), 0.0);
        assert_eq!(p.desired_anim(0.0), "talk");
        p.on_event(&status(AppStatus::Listening), 0.0);
        assert_eq!(p.desired_anim(0.0), "ouvindo");
        p.on_event(&status(AppStatus::Thinking), 0.0);
        assert_eq!(p.desired_anim(0.0), "pensando");
        p.set_deep_thinking(true);
        assert_eq!(p.desired_anim(0.0), "sabio");
    }

    #[test]
    fn maestro_needs_two_workers() {
        let mut p = pet();
        p.on_event(&status(AppStatus::Orchestrating), 0.0);
        p.on_event(&start("a"), 0.0);
        assert_eq!(p.desired_anim(0.0), "orquestrando");
        p.on_event(&start("b"), 0.0);
        assert_eq!(p.desired_anim(0.0), "maestro");
        p.on_event(&end("b"), 0.0);
        assert_eq!(p.desired_anim(0.0), "orquestrando");
    }

    #[test]
    fn emotion_relaxes_between_6_and_12_seconds() {
        let mut p = pet();
        p.on_event(&SageEvent::Emotion { name: "feliz".into() }, 100.0);
        assert_eq!(p.desired_anim(100.0), "feliz");
        p.tick(0.016, 105.9, &info);
        assert_eq!(p.desired_anim(105.9), "feliz");
        p.tick(0.016, 112.1, &info);
        assert_eq!(p.desired_anim(112.1), "idle");
    }

    #[test]
    fn morph_sequence_changes_form() {
        let mut p = pet();
        p.change_form("alado");
        assert_eq!(p.desired_anim(0.0), "morph_out");
        let mut t = 0.0;
        for _ in 0..200 {
            t += 0.1;
            p.tick(0.1, t, &info);
            if p.desired_anim(t) == "morph_in" {
                break;
            }
        }
        assert_eq!(p.form, "alado");
        for _ in 0..200 {
            t += 0.1;
            p.tick(0.1, t, &info);
        }
        assert_eq!(p.desired_anim(t), "idle");
    }

    #[test]
    fn drag_then_drop_lands() {
        let mut p = pet();
        p.begin_drag(1.0);
        assert_eq!(p.desired_anim(1.0), "arrastado");
        p.end_drag(2.0);
        assert_eq!(p.desired_anim(2.0), "pouso");
        let mut t = 2.0;
        for _ in 0..100 {
            t += 0.1;
            p.tick(0.1, t, &info);
        }
        assert_eq!(p.desired_anim(t), "idle");
    }

    #[test]
    fn petting_is_happy_for_2_4s() {
        let mut p = pet();
        p.pet(10.0);
        assert_eq!(p.desired_anim(12.0), "feliz");
        assert_eq!(p.desired_anim(12.5), "idle");
    }

    #[test]
    fn sleeps_after_idle_and_wakes_on_activity() {
        let mut p = pet();
        p.tick(0.1, 30.0, &info);
        assert!(!p.is_sleeping());
        p.tick(0.1, 61.0, &info);
        assert!(p.is_sleeping());
        assert_eq!(p.desired_anim(61.0), "dormindo");
        p.on_event(&SageEvent::UserText { text: "oi".into(), from_voice: false }, 62.0);
        assert!(!p.is_sleeping());
    }

    #[test]
    fn missing_animation_falls_back_to_idle() {
        let mut p = pet();
        p.on_event(&SageEvent::Emotion { name: "inexistente".into() }, 0.0);
        p.tick(0.1, 1.0, &info);
        assert_eq!(p.anim(), "idle");
    }

    #[test]
    fn clone_lifecycle_done() {
        let t = Timing::default();
        let mut c = CloneSprite::new("w1");
        assert_eq!(c.phase, ClonePhase::Spawn);
        for _ in 0..50 {
            c.tick(0.1, &t, &|_: &str, _: &str| 2usize);
        }
        assert_eq!(c.phase, ClonePhase::Work);
        c.end(true);
        assert_eq!(c.phase, ClonePhase::Done);
        for _ in 0..50 {
            c.tick(0.1, &t, &|_: &str, _: &str| 2usize);
        }
        assert!(c.finished);
    }

    #[test]
    fn clone_failure_goes_to_fail() {
        let mut c = CloneSprite::new("w1");
        c.end(false);
        // Ainda em spawn: termina o spawn e então vai para fail.
        for _ in 0..50 {
            c.tick(0.1, &Timing::default(), &|_: &str, _: &str| 2usize);
            if c.phase == ClonePhase::Fail {
                break;
            }
        }
        assert_eq!(c.phase, ClonePhase::Fail);
    }

    #[test]
    fn rng_is_in_range() {
        let mut r = Rng::new(42);
        for _ in 0..100 {
            let v = r.range(6.0, 12.0);
            assert!((6.0..12.0).contains(&v));
        }
    }
}
