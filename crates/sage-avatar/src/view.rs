//! `PetView`: janela própria (viewport imediato transparente) que desenha o slime e os clones.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

use egui::{Color32, Context, Pos2, Rect, Sense, Vec2, ViewportBuilder, ViewportCommand, ViewportId};
use sage_core::config::Config;
use sage_core::events::SageEvent;
use sage_core::llm::WorkerStatus;

use crate::brain::{BrainAction, BrainEvent, Mood, PetBrain};
use crate::sprites::SpriteLibrary;
use crate::state::{CloneSprite, Pet, Presence, Walk};

/// Tamanho lógico do quadro do slime (os PNGs são 2x: 270x216).
const FRAME_W: f32 = 270.0;
const FRAME_H: f32 = 216.0;
/// Tamanho do quadro dos clones (128x116 em disco).
const CLONE_W: f32 = 128.0;
const CLONE_H: f32 = 116.0;
const MAX_CLONES: usize = 6;
/// Folga em volta do slime dentro da janela, para o achatar/esticar não ser cortado.
const PAD: f32 = 14.0;

/// Mola amortecida para o "corpo de gelatina" (achatar, esticar, inclinar).
#[derive(Default, Clone, Copy)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, target: f32, stiffness: f32, damping: f32, dt: f32) {
        // Sub-passos: estável mesmo com quadros longos.
        let n = ((dt / 0.004).ceil() as usize).clamp(1, 16);
        let h = dt / n as f32;
        for _ in 0..n {
            let a = stiffness * (target - self.x) - damping * self.v;
            self.v += a * h;
            self.x += self.v * h;
        }
    }
    fn settled(&self, target: f32) -> bool {
        (self.x - target).abs() < 0.002 && self.v.abs() < 0.01
    }
}

pub struct PetView {
    lib: SpriteLibrary,
    pet: Pet,
    brain: PetBrain,
    clones: Vec<CloneSprite>,
    start: Instant,
    last: f64,
    scale: f32,
    opacity: f32,
    leash: f32,
    mood_path: PathBuf,
    next_save: f64,
    /// Posição atual da janela (canto superior esquerdo) e centro do passeio.
    pos: Option<Pos2>,
    home: Option<Pos2>,
    initial_pos: Option<Pos2>,
    dragging: bool,
    /// Pixels por ponto do monitor (1 pixel do sprite = 1 pixel da tela, como no SAGE antigo).
    ppp: f32,
    /// Tentativas restantes de tirar a borda/sombra do Windows 11 da janela do slime.
    chrome_tries: u32,
    chrome_last: f64,
    /// O app quer o slime na tela (entra/sai com teletransporte quando muda).
    want_visible: bool,
    /// Quadros desde que a janela foi criada; ela só fica visível depois que a moldura
    /// do Windows foi tirada (senão pisca um retângulo branco/escuro ao abrir).
    window_frames: u32,
    /// Medição de quadros por segundo (log em nível debug).
    fps_frames: u32,
    fps_since: f64,
    /// Arrasto próprio (sem o laço modal do Windows, que congelava a animação):
    /// distância do cursor ao canto da janela, em pontos.
    grab: Option<Vec2>,
    /// Botão pressionado sobre o slime (ainda sem arrastar) e mouse por cima.
    pressed: bool,
    hovered: bool,
    squash: Spring,
    tilt: Spring,
    /// Último quadro desenhado: repetido enquanto o próximo ainda decodifica.
    last_tex: RefCell<Option<egui::TextureHandle>>,
}

impl PetView {
    pub fn new(_ctx: &Context, cfg: &Config) -> Self {
        Self::with_dirs(cfg, sage_core::paths::assets().join("sprites"), sage_core::paths::data().join("pet_mood.json"))
    }

    /// Variante com pastas explícitas (exemplo e testes).
    pub fn with_dirs(cfg: &Config, sprites: PathBuf, mood_path: PathBuf) -> Self {
        let lib = SpriteLibrary::new(sprites);
        let timing = lib.timing.clone();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5a6e);
        let sleep_after = cfg.pet_sleep_after_seconds.unwrap_or(300);
        let mut pet = Pet::new(&cfg.pet_form, sleep_after, timing, seed);
        // Começa fora da tela: o primeiro `show` toca o `teleport_in`.
        pet.start_hidden();
        lib.prefetch(&pet.form, "teleport_in");
        lib.prefetch(&pet.form, "idle");
        // Animações das interações: prontas antes do primeiro clique/arrasto.
        for a in ["arrastado", "pouso", "feliz", "walk"] {
            lib.prefetch(&pet.form, a);
        }
        let brain = PetBrain::new(
            Mood::load(&mood_path),
            &cfg.pet_personality,
            cfg.pet_activity.unwrap_or(0.5),
            cfg.pet_free_will,
            cfg.enable_pet_walking,
            seed ^ 0x9e37_79b9,
        );
        Self {
            lib,
            pet,
            brain,
            clones: Vec::new(),
            start: Instant::now(),
            last: 0.0,
            scale: cfg.pet_scale.unwrap_or(1.0).clamp(0.5, 2.0),
            ppp: 1.0,
            chrome_tries: 120,
            chrome_last: 0.0,
            want_visible: true,
            window_frames: 0,
            fps_frames: 0,
            fps_since: 0.0,
            grab: None,
            pressed: false,
            hovered: false,
            squash: Spring::default(),
            tilt: Spring::default(),
            last_tex: RefCell::new(None),
            opacity: cfg.pet_opacity.unwrap_or(1.0).clamp(0.1, 1.0),
            leash: cfg.pet_leash_radius.unwrap_or(260.0).max(0.0),
            mood_path,
            next_save: 30.0,
            pos: None,
            home: None,
            initial_pos: None,
            dragging: false,
        }
    }

    /// Aplica as configurações do slime na hora (tamanho, opacidade, forma, personalidade).
    pub fn apply_config(&mut self, cfg: &Config) {
        self.scale = cfg.pet_scale.unwrap_or(1.0).clamp(0.5, 2.0);
        self.opacity = cfg.pet_opacity.unwrap_or(1.0).clamp(0.1, 1.0);
        self.leash = cfg.pet_leash_radius.unwrap_or(260.0).max(0.0);
        self.brain.configure(&cfg.pet_personality, cfg.pet_activity.unwrap_or(0.5), cfg.pet_free_will, cfg.enable_pet_walking);
        if !cfg.enable_pet_walking {
            self.pet.set_walk(None);
        }
        self.pet.set_sleep_after(cfg.pet_sleep_after_seconds.unwrap_or(300));
        if !cfg.pet_form.is_empty() && cfg.pet_form != self.pet.form {
            for a in ["morph_out", "morph_in", "idle"] {
                self.lib.prefetch(&cfg.pet_form, a);
            }
            self.pet.change_form(&cfg.pet_form);
        }
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Escala em pixels por pixel de quadro: o quadro em disco é 2x o tamanho lógico.
    fn px(&self) -> f32 {
        self.scale / self.ppp.max(0.5)
    }

    /// Janela: slime à esquerda e uma faixa à direita para os clones.
    fn window_size(&self) -> Vec2 {
        let p = self.px();
        Vec2::new((FRAME_W + CLONE_W * 0.5 + 8.0 + PAD * 2.0) * p, (FRAME_H + PAD * 2.0) * p)
    }

    /// Área útil do monitor onde o slime está, em pontos (sem a barra de tarefas).
    fn work_area(&self, ctx: &Context, at: Option<Pos2>) -> Rect {
        let ppp = self.ppp.max(0.5);
        let probe = at.map(|p| p + self.window_size() * 0.5).unwrap_or(Pos2::new(1.0, 1.0));
        if let Some([l, t, r, b]) = crate::chrome::work_area_px(probe.x * ppp, probe.y * ppp) {
            return Rect::from_min_max(Pos2::new(l / ppp, t / ppp), Pos2::new(r / ppp, b / ppp));
        }
        let monitor = ctx.input(|i| i.viewport().monitor_size).unwrap_or(Vec2::new(1920.0, 1080.0));
        Rect::from_min_size(Pos2::ZERO, Vec2::new(monitor.x, monitor.y - 48.0))
    }

    /// Mantém a janela inteira dentro da área útil do monitor.
    fn clamp_to_screen(&self, ctx: &Context, p: Pos2) -> Pos2 {
        let wa = self.work_area(ctx, Some(p));
        let size = self.window_size();
        // O quadro tem folga transparente embaixo: deixa encostar na barra de tarefas.
        let foot = (PAD + 18.0) * self.px();
        let max = Pos2::new((wa.right() - size.x).max(wa.left()), (wa.bottom() - size.y + foot).max(wa.top()));
        Pos2::new(p.x.clamp(wa.left(), max.x), p.y.clamp(wa.top(), max.y))
    }

    /// Liga/desliga o slime. Ao ligar ele entra com `teleport_in`; ao desligar sai com
    /// `teleport_out` e a janela some quando a animação termina. Chame `show` todo quadro
    /// (mesmo desligado) para a saída tocar até o fim.
    pub fn set_visible(&mut self, visible: bool) {
        self.want_visible = visible;
    }

    /// `true` quando o slime já saiu da tela (a saída terminou).
    pub fn is_gone(&self) -> bool {
        self.pet.presence() == Presence::Hidden
    }

    pub fn on_event(&mut self, ev: &SageEvent) {
        let now = self.now();
        self.pet.on_event(ev, now);
        match ev {
            SageEvent::UserText { .. } => self.brain.on_event(&BrainEvent::Interaction),
            SageEvent::Emotion { name } => self.brain.on_event(&BrainEvent::Emotion(name.clone())),
            SageEvent::WorkerStart { worker, .. } => {
                if self.clones.len() < MAX_CLONES {
                    self.clones.push(CloneSprite::new(worker));
                    for a in ["spawn", "work", "done", "fail", "vanish"] {
                        self.lib.prefetch("clone", a);
                    }
                }
            }
            SageEvent::WorkerEnd { worker, status, .. } => {
                let ok = matches!(status, WorkerStatus::Done | WorkerStatus::DoneWithConcerns);
                if let Some(c) = self.clones.iter_mut().find(|c| &c.worker == worker && !c.is_ending()) {
                    c.end(ok);
                }
                self.brain.on_event(&if ok { BrainEvent::TaskDone } else { BrainEvent::TaskFail });
                self.pet.play_once(if ok { "orgulhoso_clone" } else { "bronca_clone" });
            }
            _ => {}
        }
    }

    /// Avança o relógio do slime; devolve o `dt` usado (s).
    fn advance(&mut self, now: f64) -> f64 {
        let dt = (now - self.last).clamp(0.0, 0.25);
        self.last = now;
        self.pet.tick(dt, now, &self.lib);
        let timing = self.lib.timing.clone();
        for c in &mut self.clones {
            c.tick(dt, &timing, &self.lib);
        }
        self.clones.retain(|c| !c.finished);

        let shown = self.pet.presence() == Presence::Shown;
        let quiet = shown && self.pet.desired_anim(now) == "idle" && self.pet.walking().is_none();
        let action = if shown { self.brain.tick(now, quiet) } else { None };
        match action {
            Some(BrainAction::Anim(a)) => self.pet.play_once(a),
            Some(BrainAction::Walk { run }) => {
                let dir = if self.pet.rng().unit() < 0.5 { -1.0 } else { 1.0 };
                self.pet.set_walk(Some(Walk { run, dir }));
            }
            None => {}
        }
        if now >= self.next_save {
            self.next_save = now + 60.0;
            if let Err(e) = self.brain.mood.save(&self.mood_path) {
                tracing::warn!("não foi possível salvar o humor do slime: {e}");
            }
        }
        dt
    }

    /// Anda um passo dentro da coleira; devolve a nova posição se mudou.
    fn walk_step(&mut self, ctx: &Context, dt: f64) -> Option<Pos2> {
        let w = self.pet.walking()?;
        let (pos, home) = (self.pos?, self.home?);
        let speed = if w.run { 4.6 } else { 2.6 } * 60.0 * dt as f32;
        let mut x = pos.x + w.dir * speed;
        let wa = self.work_area(ctx, Some(pos));
        let right = (wa.right() - self.window_size().x).max(wa.left());
        let lo = (home.x - self.leash).max(wa.left());
        let hi = (home.x + self.leash).min(right).max(lo);
        if x <= lo || x >= hi {
            x = x.clamp(lo, hi);
            self.pet.set_walk(None);
        }
        let np = Pos2::new(x, pos.y);
        self.pos = Some(np);
        Some(np)
    }

    /// Abre/atualiza a janela do slime e desenha o quadro atual.
    pub fn show(&mut self, ctx: &Context) {
        self.ppp = ctx.pixels_per_point();
        let now = self.now();
        let was_hidden = self.pet.presence() == Presence::Hidden;
        if self.want_visible {
            self.pet.appear(now);
        } else {
            self.pet.vanish();
        }
        if was_hidden && self.pet.presence() == Presence::Entering {
            // Janela nova: tira a borda do Windows de novo e começa o relógio do zero
            // (senão o `dt` acumulado pularia os primeiros quadros da entrada).
            self.chrome_tries = 120;
            self.window_frames = 0;
            self.last = now;
            // Reabre onde estava (não no canto inicial) depois de esconder e mostrar.
            if self.pos.is_some() {
                self.initial_pos = self.pos;
            }
        }
        let dt = self.advance(now);
        if self.pet.presence() == Presence::Hidden {
            // Fora da tela: não desenhar fecha a janela do viewport.
            self.dragging = false;
            self.window_frames = 0;
            self.last = now;
            return;
        }
        self.lib.prefetch(&self.pet.form, self.pet.anim());
        self.lib.prefetch(&self.pet.form, "teleport_out");
        let size = self.window_size();

        if self.initial_pos.is_none() {
            let wa = self.work_area(ctx, None);
            let p = self.clamp_to_screen(ctx, Pos2::new(wa.right() - size.x - 40.0, wa.bottom() - size.y));
            self.initial_pos = Some(p);
            self.pos = Some(p);
            self.home = Some(p);
        } else if !self.dragging
            && let Some(p) = self.pos
        {
            // Tamanho/monitor mudou ou ele já estava fora: volta para dentro da tela.
            let c = self.clamp_to_screen(ctx, p);
            if (c - p).length() > 0.5 {
                self.pos = Some(c);
                self.home = self.home.map(|h| self.clamp_to_screen(ctx, h));
                self.pet.set_walk(None);
                ctx.send_viewport_cmd_to(ViewportId::from_hash_of("sage-pet"), ViewportCommand::OuterPosition(c));
            }
        }
        let mut builder = ViewportBuilder::default()
            .with_title("S.A.G.E. slime")
            .with_transparent(true)
            .with_decorations(false)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false)
            .with_visible(self.window_frames >= 3)
            .with_inner_size(size);
        if let Some(p) = self.initial_pos {
            builder = builder.with_position(p);
        }

        if !self.dragging
            && let Some(np) = self.walk_step(ctx, dt)
        {
            ctx.send_viewport_cmd_to(ViewportId::from_hash_of("sage-pet"), ViewportCommand::OuterPosition(np));
        }
        self.drag_step(ctx, dt as f32);
        self.physics(dt as f32);

        // O winit reaplica o estilo da janela nos primeiros quadros; repete por um tempo
        // e depois a cada ~2 s (barato) para a borda do Windows 11 não voltar.
        self.chrome_tries = self.chrome_tries.saturating_sub(1);
        if self.chrome_tries > 0 || self.chrome_tick() {
            let found = crate::chrome::strip("S.A.G.E. slime");
            if found || self.window_frames > 0 {
                self.window_frames = self.window_frames.saturating_add(1);
            }
        } else {
            self.window_frames = self.window_frames.saturating_add(1);
        }
        if self.window_frames < 3 {
            // Enquanto a janela está escondida, a entrada não anda (toca inteira depois).
            self.pet_hold_entry();
        }

        let mut petted = false;
        let mut tapped = false;
        let mut drag_started = false;
        let mut press = None;
        let mut hover = false;

        self.draw(ctx, builder, &mut |ev| match ev {
            UiEvent::Petted => petted = true,
            UiEvent::Tapped => tapped = true,
            UiEvent::DragStart => drag_started = true,
            UiEvent::Press(down) => press = Some(down),
            UiEvent::Hover(h) => hover = h,
        });
        self.hovered = hover;
        if let Some(down) = press {
            if down && !self.pressed {
                // Toque: afunda na hora, antes de qualquer troca de animação.
                self.squash.v += 4.0;
            }
            self.pressed = down;
        }

        let shown = self.pet.presence() == Presence::Shown;
        if petted && shown {
            self.pet.pet(now);
            self.brain.on_event(&BrainEvent::Petted);
            self.squash.v -= 7.0;
        } else if tapped && shown {
            // Clique simples: tremidinha de gelatina.
            self.squash.v -= 5.0;
        }
        if drag_started && shown && !self.dragging {
            let ppp = self.ppp.max(0.5);
            if let (Some((cx, cy)), Some(pos)) = (crate::chrome::cursor_px(), self.pos) {
                self.grab = Some(Pos2::new(cx / ppp, cy / ppp) - pos);
                self.dragging = true;
                self.pet.begin_drag(now);
                self.brain.on_event(&BrainEvent::DragStart);
            }
        }
        self.schedule_repaint(ctx, now);
    }

    /// Segue o cursor enquanto o botão está pressionado; solta quando ele é liberado.
    fn drag_step(&mut self, ctx: &Context, dt: f32) {
        if !self.dragging {
            return;
        }
        let ppp = self.ppp.max(0.5);
        let down = crate::chrome::left_button_down().unwrap_or(self.pressed);
        let (Some(grab), Some((cx, cy)), Some(pos)) = (self.grab, crate::chrome::cursor_px(), self.pos) else {
            self.drop_pet();
            return;
        };
        let target = self.clamp_to_screen(ctx, Pos2::new(cx / ppp, cy / ppp) - grab);
        if (target - pos).length() > 0.1 {
            ctx.send_viewport_cmd_to(ViewportId::from_hash_of("sage-pet"), ViewportCommand::OuterPosition(target));
        }
        // Balança para o lado oposto ao movimento (como gelatina pendurada).
        let vx = if dt > 0.0 { (target.x - pos.x) / dt } else { 0.0 };
        self.tilt.v += (-vx * 0.00035).clamp(-0.6, 0.6);
        self.pos = Some(target);
        if !down {
            self.drop_pet();
        }
    }

    fn drop_pet(&mut self) {
        let now = self.now();
        self.dragging = false;
        self.grab = None;
        self.pressed = false;
        self.pet.end_drag(now);
        self.brain.on_event(&BrainEvent::Drop);
        self.home = self.pos;
        // Aterrissa: achata e quica de volta.
        self.squash.v += 9.0;
    }

    fn squash_target(&self) -> f32 {
        if self.dragging {
            -0.35 // pendurado: estica para baixo
        } else if self.pressed {
            0.55 // apertado: afunda
        } else if self.hovered {
            -0.06 // mouse em cima: "respira" para cima
        } else {
            0.0
        }
    }

    fn physics(&mut self, dt: f32) {
        let t = self.squash_target();
        self.squash.step(t, 260.0, 11.0, dt);
        self.squash.x = self.squash.x.clamp(-0.8, 1.2);
        self.tilt.step(0.0, 140.0, 7.0, dt);
        self.tilt.x = self.tilt.x.clamp(-0.35, 0.35);
    }

    fn springs_moving(&self) -> bool {
        !self.squash.settled(self.squash_target()) || !self.tilt.settled(0.0)
    }

    /// Repinta no ritmo do monitor quando há movimento contínuo (andar, arrastar,
    /// entrada/saída, clones); parado, acorda só quando o próximo quadro vence.
    fn schedule_repaint(&mut self, ctx: &Context, now: f64) {
        self.fps_frames += 1;
        if now - self.fps_since >= 5.0 {
            tracing::debug!("slime: {:.1} quadros/s", self.fps_frames as f64 / (now - self.fps_since));
            self.fps_frames = 0;
            self.fps_since = now;
        }
        let moving = self.dragging
            || self.pressed
            || self.springs_moving()
            || self.pet.walking().is_some()
            || !self.clones.is_empty()
            || matches!(self.pet.presence(), Presence::Entering | Presence::Leaving);
        if moving {
            ctx.request_repaint();
        } else {
            // Acorda um pouco antes para o quadro sair no vsync certo.
            let ms = (self.pet.ms_to_next_frame() - 4.0).clamp(1.0, 33.0);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(ms / 1000.0));
        }
    }

    fn pet_hold_entry(&mut self) {
        if self.pet.presence() == Presence::Entering {
            self.pet.restart_anim();
        }
    }

    fn chrome_tick(&mut self) -> bool {
        let now = self.now();
        if now - self.chrome_last > 2.0 {
            self.chrome_last = now;
            true
        } else {
            false
        }
    }

    // O closure do viewport recebe o contexto do viewport; ver "Riscos" no handoff.
    #[allow(deprecated)]
    fn draw(&self, ctx: &Context, builder: ViewportBuilder, emit: &mut dyn FnMut(UiEvent)) {
        let id = ViewportId::from_hash_of("sage-pet");
        ctx.show_viewport_immediate(id, builder, |vctx, _class| {
            let frame = egui::Frame::NONE.fill(Color32::TRANSPARENT);
            egui::CentralPanel::default().frame(frame).show(vctx, |ui| {
                let p = self.px();
                let full = ui.max_rect();
                let slime_rect =
                    Rect::from_min_size(full.min + Vec2::splat(PAD * p), Vec2::new(FRAME_W * p, FRAME_H * p));
                let tint = Color32::from_white_alpha((self.opacity * 255.0) as u8);

                let tex = self.lib.texture(&ui.ctx().clone(), &self.pet.form, self.pet.anim(), self.pet.frame());
                let tex = match tex {
                    Some(t) => {
                        *self.last_tex.borrow_mut() = Some(t.clone());
                        Some(t)
                    }
                    None => self.last_tex.borrow().clone(),
                };
                if let Some(tex) = tex {
                    let flip = self.pet.walking().is_some_and(|w| w.dir < 0.0);
                    let (u0, u1) = if flip { (1.0, 0.0) } else { (0.0, 1.0) };
                    let uv = Rect::from_min_max(Pos2::new(u0, 0.0), Pos2::new(u1, 1.0));
                    // Gelatina: achata/estica mantendo o "pé" no chão e inclina a partir do topo.
                    let sq = self.squash.x;
                    let (sx, sy) = (1.0 + 0.12 * sq, 1.0 - 0.14 * sq);
                    let foot = Pos2::new(slime_rect.center().x, slime_rect.bottom() - 30.0 * p);
                    let hinge = Pos2::new(slime_rect.center().x, slime_rect.top() + 40.0 * p);
                    let (sin, cos) = self.tilt.x.sin_cos();
                    let mut mesh = egui::epaint::Mesh::with_texture(tex.id());
                    mesh.add_rect_with_uv(slime_rect, uv, tint);
                    for v in &mut mesh.vertices {
                        let d = v.pos - foot;
                        let q = foot + Vec2::new(d.x * sx, d.y * sy);
                        let r = q - hinge;
                        v.pos = hinge + Vec2::new(r.x * cos - r.y * sin, r.x * sin + r.y * cos);
                    }
                    ui.painter().add(egui::Shape::mesh(mesh));
                }

                // Clones em coluna à direita, metade do tamanho de disco * escala.
                let cw = CLONE_W * 0.5 * p;
                let ch = CLONE_H * 0.5 * p;
                for (i, c) in self.clones.iter().enumerate() {
                    let row = i as f32;
                    let min = Pos2::new(slime_rect.right() + 4.0 * p, slime_rect.min.y + row * ch * 0.8);
                    let r = Rect::from_min_size(min, Vec2::new(cw, ch));
                    if let Some(tex) = self.lib.texture(&ui.ctx().clone(), "clone", c.phase.anim(), c.frame) {
                        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                        ui.painter().image(tex.id(), r, uv, tint);
                    }
                }

                // Área de toque: o corpo do slime (sem a folga transparente das bordas).
                let body = slime_rect.shrink2(Vec2::new(40.0 * p, 30.0 * p));
                let resp = ui.interact(body, egui::Id::new("sage-pet-body"), Sense::click_and_drag());
                emit(UiEvent::Hover(resp.hovered() || self.dragging));
                if resp.hovered() || self.dragging {
                    ui.ctx().set_cursor_icon(if self.dragging { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
                }
                if resp.double_clicked() {
                    emit(UiEvent::Petted);
                } else if resp.clicked() {
                    emit(UiEvent::Tapped);
                }
                if resp.is_pointer_button_down_on() {
                    emit(UiEvent::Press(true));
                } else if !self.dragging {
                    emit(UiEvent::Press(false));
                }
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    emit(UiEvent::DragStart);
                }
            });
        });
    }
}

enum UiEvent {
    Petted,
    Tapped,
    DragStart,
    Press(bool),
    Hover(bool),
}
