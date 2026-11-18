//! Janela principal: Conversa, Execuções e Configurações.

use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::runtime::Runtime;
use crate::settings::SettingsPage;
use crate::theme::{self, Palette};
use crate::tray::{Tray, TrayAction};
use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use sage_core::events::{AppStatus, Command};
use sage_core::{Config, SageEvent, WorkerStatus};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

#[derive(PartialEq, Clone, Copy)]
enum Page {
    Chat,
    Runs,
    Settings,
}

struct ChatLine {
    from_user: bool,
    text: String,
    when: String,
}

#[derive(Default)]
struct WorkerCard {
    role: String,
    model: String,
    brief: String,
    status: Option<WorkerStatus>,
    summary: String,
    tokens: u64,
    cost: Option<f64>,
    tools: Vec<(bool, String)>,
}

pub struct SageApp {
    cfg: Arc<RwLock<Config>>,
    rt: Runtime,
    events: broadcast::Receiver<SageEvent>,
    page: Page,
    input: String,
    chat: Vec<ChatLine>,
    md: CommonMarkCache,
    status: (AppStatus, String),
    live: BTreeMap<String, String>,
    workers: BTreeMap<String, WorkerCard>,
    plan: Option<(String, String)>,
    notices: Vec<String>,
    confirm: Option<(String, String)>,
    runs: Vec<(String, String, String)>,
    pet: sage_avatar::PetView,
    pet_visible: bool,
    settings: SettingsPage,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    mic_held: bool,
    mic_from_button: bool,
    hidden: bool,
    show_brain: bool,
    logo: Option<egui::TextureHandle>,
}

impl SageApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Arc<RwLock<Config>>, rt: Runtime) -> Self {
        theme::install(&cc.egui_ctx);
        let snapshot = cfg.read().map(|c| c.clone()).unwrap_or_default();
        let events = rt.bus.subscribe();
        let pet = sage_avatar::PetView::new(&cc.egui_ctx, &snapshot);
        let hotkeys = Hotkeys::register(&snapshot.hotkey_listen, &snapshot.hotkey_panel);
        // Repinta quando chega evento do cérebro, mesmo com a janela parada.
        {
            let ctx = cc.egui_ctx.clone();
            let mut sub = rt.bus.subscribe();
            rt.rt.spawn(async move {
                while sub.recv().await.is_ok() {
                    ctx.request_repaint();
                }
            });
        }
        let hour = chrono::Local::now().format("%H").to_string().parse().unwrap_or(12);
        let greeting = sage_core::persona::welcome(hour);
        let mut app = Self {
            settings: SettingsPage::new(&snapshot),
            pet_visible: snapshot.enable_desktop_pet,
            cfg,
            events,
            page: Page::Chat,
            input: String::new(),
            chat: vec![],
            md: CommonMarkCache::default(),
            status: (AppStatus::Idle, String::new()),
            live: BTreeMap::new(),
            workers: BTreeMap::new(),
            plan: None,
            notices: vec![],
            confirm: None,
            runs: load_runs(),
            pet,
            tray: Tray::new(),
            hotkeys,
            mic_held: false,
            mic_from_button: false,
            hidden: false,
            show_brain: true,
            logo: load_logo(&cc.egui_ctx),
            rt,
        };
        app.push(false, greeting.clone());
        if snapshot.speak_greetings && snapshot.voice_enabled {
            app.rt.voice.speak(&greeting);
        }
        app
    }

    fn push(&mut self, from_user: bool, text: String) {
        let when = chrono::Local::now().format("%H:%M").to_string();
        self.chat.push(ChatLine { from_user, text, when });
    }

    fn drain_events(&mut self) {
        loop {
            let ev = match self.events.try_recv() {
                Ok(ev) => ev,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            self.pet.on_event(&ev);
            match ev {
                SageEvent::Status { status, detail } => self.status = (status, detail),
                SageEvent::UserText { text, from_voice } if from_voice => self.push(true, text),
                SageEvent::Reply { full, spoken, .. } => {
                    self.push(false, if full.trim().is_empty() { spoken } else { full });
                    self.live.remove("lead");
                }
                SageEvent::Stream { source, text, .. } => {
                    let buf = self.live.entry(source).or_default();
                    buf.push_str(&text);
                    if buf.len() > 6000 {
                        let cut = buf.len() - 6000;
                        let cut = (cut..buf.len()).find(|i| buf.is_char_boundary(*i)).unwrap_or(0);
                        buf.drain(..cut);
                    }
                }
                SageEvent::PlanReady { playbook, why, .. } => {
                    self.workers.clear();
                    self.live.retain(|k, _| k == "lead");
                    self.plan = Some((playbook, why));
                    self.show_brain = true;
                }
                SageEvent::WorkerStart { worker, role, model, brief, .. } => {
                    self.workers.insert(worker, WorkerCard { role, model, brief, ..Default::default() });
                }
                SageEvent::WorkerTool { worker, tool, ok, .. } => {
                    if let Some(w) = self.workers.get_mut(&worker) {
                        w.tools.push((ok, tool));
                    }
                }
                SageEvent::WorkerEnd { worker, status, summary, usage, .. } => {
                    if let Some(w) = self.workers.get_mut(&worker) {
                        w.status = Some(status);
                        w.summary = summary;
                        w.tokens = usage.total();
                        w.cost = usage.cost_usd;
                    }
                }
                SageEvent::ProviderSwitch { from, to, reason } => {
                    self.notices.push(format!("Trocando {from} por {to}: {reason}"));
                }
                SageEvent::RunReport { .. } => self.runs = load_runs(),
                SageEvent::ConfirmRequest { id, question } => self.confirm = Some((id, question)),
                SageEvent::Notice { text } => self.notices.push(text),
                _ => {}
            }
        }
        if self.notices.len() > 4 {
            let n = self.notices.len() - 4;
            self.notices.drain(..n);
        }
    }

    fn poll_os(&mut self, ctx: &egui::Context) {
        if let Some(hk) = &self.hotkeys {
            for a in hk.poll() {
                match a {
                    HotkeyAction::TalkDown => {
                        self.mic_held = true;
                        self.mic_from_button = false;
                        self.rt.ptt_down();
                    }
                    HotkeyAction::TalkUp => {
                        self.mic_held = false;
                        self.rt.ptt_up();
                    }
                    HotkeyAction::TogglePanel => {
                        self.hidden = !self.hidden;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(!self.hidden));
                        if !self.hidden {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                    }
                }
            }
        }
        if let Some(tray) = &self.tray {
            for a in tray.poll() {
                match a {
                    TrayAction::Open => {
                        self.hidden = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    TrayAction::TogglePet => self.pet_visible = !self.pet_visible,
                    TrayAction::Quit => std::process::exit(0),
                }
            }
        }
        // Fechar a janela esconde na bandeja.
        if ctx.input(|i| i.viewport().close_requested()) && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
        }
    }

    fn send_input(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.clear();
        self.push(true, text.clone());
        self.rt.ask(text, false);
    }

    // ------------------------------------------------------------------ top bar

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(p.surface).inner_margin(Margin::symmetric(16, 10)).stroke(Stroke::new(1.0, p.border)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(30.0, 30.0)));
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(theme::bold("S.A.G.E.").size(17.0));
                        ui.label(RichText::new("cérebro orquestrador").size(11.0).color(p.muted));
                    });
                    ui.add_space(18.0);
                    // Controle segmentado das páginas.
                    egui::Frame::new()
                        .fill(p.bg)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(255))
                        .inner_margin(Margin::same(3))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (page, label) in [(Page::Chat, "Conversa"), (Page::Runs, "Execuções"), (Page::Settings, "Configurações")] {
                                let kind = if self.page == page { theme::Kind::Primary } else { theme::Kind::Ghost };
                                if theme::pill(ui, kind, label).clicked() {
                                    self.page = page;
                                }
                            }
                        });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        theme::switch(ui, &mut self.pet_visible, "Slime")
                            .on_hover_text("Mostra ou esconde o slime na área de trabalho");
                        ui.add_space(10.0);
                        let (st, _) = self.status;
                        let busy = !matches!(st, AppStatus::Idle | AppStatus::Error);
                        theme::status_badge(ui, status_color(&p, st), status_label(st), busy);
                    });
                });
            });
    }

    // ------------------------------------------------------------------ chat

    fn chat_page(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        egui::Panel::right("brain")
            .resizable(true)
            .default_size(340.0)
            .min_size(260.0)
            .max_size(560.0)
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(12)).stroke(Stroke::new(1.0, p.border)))
            .show_animated_inside(ui, self.show_brain, |ui| self.brain_panel(ui));

        egui::Panel::bottom("composer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 16, right: 16, top: 8, bottom: 14 }))
            .show(ui, |ui| self.composer(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(16, 12)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    let max = (ui.available_width() * 0.78).clamp(260.0, 760.0);
                    for i in 0..self.chat.len() {
                        self.bubble(ui, i, max);
                        ui.add_space(10.0);
                    }
                    if self.chat.len() <= 1 {
                        self.suggestions(ui);
                    }
                    if matches!(self.status.0, AppStatus::Thinking | AppStatus::Orchestrating) {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(theme::muted(ui, status_label(self.status.0)));
                        });
                    }
                });
            });
    }

    fn bubble(&mut self, ui: &mut egui::Ui, i: usize, max: f32) {
        let p = theme::palette(ui);
        let from_user = self.chat[i].from_user;
        let (fill, stroke, align) = if from_user {
            (p.accent_soft, Stroke::NONE, Align::Max)
        } else {
            (p.surface, Stroke::new(1.0, p.border), Align::Min)
        };
        let who = if from_user { "Você" } else { "S.A.G.E." };
        let header = format!("{who} · {}", self.chat[i].when);
        let text = self.chat[i].text.clone();
        let logo = self.logo.clone();
        ui.with_layout(Layout::top_down(align), |ui| {
            ui.horizontal(|ui| {
                if !from_user {
                    if let Some(logo) = &logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(18.0, 18.0)));
                    }
                }
                ui.label(RichText::new(header).small().color(p.muted));
            });
            let (nw, ne) = if from_user { (14, 4) } else { (4, 14) };
            egui::Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius { nw, ne, sw: 14, se: 14 })
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_max_width(max);
                    CommonMarkViewer::new().show(ui, &mut self.md, &text);
                });
        });
    }

    fn suggestions(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.add_space(8.0);
        ui.label(RichText::new("Experimente").small().color(p.muted));
        ui.horizontal_wrapped(|ui| {
            for s in [
                "Como está o meu PC agora?",
                "Abra o Spotify",
                "Organize a pasta Downloads por tipo",
                "Pesquise e compare 3 SSDs NVMe de 1 TB",
            ] {
                if theme::pill(ui, theme::Kind::Secondary, s).clicked() {
                    self.input = s.to_string();
                    self.send_input();
                }
            }
        });
    }

    fn composer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        for n in &self.notices {
            ui.label(RichText::new(format!("ⓘ {n}")).small().color(p.muted));
        }
        if let Some((id, q)) = self.confirm.clone() {
            theme::card(ui).stroke(Stroke::new(1.0, p.warn)).show(ui, |ui| {
                ui.label(theme::bold(q));
                ui.horizontal(|ui| {
                    if theme::primary(ui, "Pode seguir").clicked() {
                        self.rt.send(Command::Confirm { id: id.clone(), yes: true });
                        self.confirm = None;
                    }
                    if theme::button(ui, theme::Kind::Secondary, "Cancelar").clicked() {
                        self.rt.send(Command::Confirm { id, yes: false });
                        self.confirm = None;
                    }
                });
            });
            ui.add_space(6.0);
        }
        let focus = ui.ctx().memory(|m| m.focused()) == Some(egui::Id::new("composer-input"));
        let ring = if focus { p.accent } else { p.border };
        theme::card(ui).stroke(Stroke::new(if focus { 1.5 } else { 1.0 }, ring)).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let mic_kind = if self.mic_held { theme::Kind::Danger } else { theme::Kind::Secondary };
                let mic = theme::icon_button(ui, mic_kind, if self.mic_held { "■" } else { "🎙" }, "Segure para falar (ou use o atalho de voz)", 38.0);
                if mic.is_pointer_button_down_on() && !self.mic_held {
                    self.mic_held = true;
                    self.rt.ptt_down();
                }
                if self.mic_held && !ui.input(|i| i.pointer.any_down()) && self.mic_from_button {
                    self.mic_held = false;
                    self.rt.ptt_up();
                }
                if mic.is_pointer_button_down_on() {
                    self.mic_from_button = true;
                }
                let send_w = 52.0;
                let hint = if self.mic_held { "Ouvindo… solte o botão para enviar" } else { "Mensagem para o S.A.G.E." };
                let edit = egui::TextEdit::multiline(&mut self.input)
                    .id(egui::Id::new("composer-input"))
                    .desired_rows(1)
                    .frame(egui::Frame::NONE)
                    .margin(Margin::symmetric(4, 8))
                    .font(egui::TextStyle::Body)
                    .desired_width((ui.available_width() - send_w).max(120.0))
                    .hint_text(hint);
                let resp = ui.add(edit);
                let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                if enter {
                    self.input = self.input.trim_end_matches('\n').to_string();
                    self.send_input();
                    resp.request_focus();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can = !self.input.trim().is_empty();
                    let send = ui.add_enabled_ui(can, |ui| theme::icon_button(ui, theme::Kind::Primary, "➤", "Enviar (Enter)", 38.0)).inner;
                    if send.clicked() {
                        self.send_input();
                    }
                });
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let (st, detail) = self.status.clone();
            ui.label(RichText::new(status_label(st)).small().color(status_color(&p, st)));
            if !detail.is_empty() {
                ui.label(RichText::new(detail).small().color(p.muted));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.show_brain { "Esconder o cérebro ›" } else { "‹ Ver o cérebro" };
                if theme::button(ui, theme::Kind::Ghost, label).clicked() {
                    self.show_brain = !self.show_brain;
                }
                if matches!(st, AppStatus::Orchestrating | AppStatus::Thinking) && theme::button(ui, theme::Kind::Danger, "■ Parar").clicked() {
                    self.rt.send(Command::StopRun);
                }
                ui.label(RichText::new("Enter envia · Shift+Enter quebra a linha").small().color(p.muted));
            });
        });
    }

    fn brain_panel(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.label(theme::bold("Cérebro").size(17.0));
        match &self.plan {
            Some((pb, why)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(theme::muted(ui, "Playbook"));
                    theme::badge(ui, p.accent, pb);
                });
                ui.label(RichText::new(why).small().color(p.muted));
            }
            None => {
                ui.label(RichText::new("Pedidos grandes são divididos entre workers. Acompanhe aqui.").small().color(p.muted));
                ui.add_space(8.0);
                for (n, title, desc) in [
                    ("1", "Triagem", "Um modelo rápido decide: conversa, ação no PC ou tarefa."),
                    ("2", "Plano", "O lead escolhe um playbook e escreve um brief por unidade."),
                    ("3", "Workers", "Modelos mais baratos executam em paralelo, com cota vigiada."),
                    ("4", "Revisão e síntese", "Cada entrega passa por um gate; o lead junta tudo."),
                ] {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 12.0, p.accent_soft);
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, n, egui::FontId::new(12.0, egui::FontFamily::Name("bold".into())), p.accent);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(theme::bold(title).size(14.0));
                            ui.label(RichText::new(desc).small().color(p.muted));
                        });
                    });
                    ui.add_space(4.0);
                }
            }
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical().id_salt("brain-scroll").auto_shrink(false).show(ui, |ui| {
            if !self.workers.is_empty() {
                ui.label(theme::bold(format!("Workers ({})", self.workers.len())));
            }
            for (id, w) in &self.workers {
                theme::card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(theme::bold(id));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match w.status {
                            None => {
                                ui.add(egui::Spinner::new().size(12.0));
                            }
                            Some(s) => theme::badge(ui, worker_color(&p, s), worker_label(s)),
                        });
                    });
                    ui.label(RichText::new(format!("{} · {}", w.role, w.model)).small().color(p.muted));
                    if w.status.is_some() {
                        let cost = w.cost.map(|c| format!(" · US$ {c:.4}")).unwrap_or_default();
                        ui.label(RichText::new(format!("{} tokens{cost}", fmt_int(w.tokens))).small().color(p.muted));
                        if !w.summary.is_empty() {
                            ui.label(&w.summary);
                        }
                    } else {
                        ui.label(RichText::new(truncate(&w.brief, 180)).small());
                    }
                    for (ok, t) in w.tools.iter().rev().take(3) {
                        let c = if *ok { p.ok } else { p.err };
                        let mark = if *ok { "✔" } else { "✖" };
                        ui.label(RichText::new(format!("{mark} {t}")).small().monospace().color(c));
                    }
                });
                ui.add_space(6.0);
            }
            if !self.live.is_empty() {
                ui.add_space(4.0);
                ui.label(theme::bold("Raciocínio ao vivo"));
                for (src, text) in &self.live {
                    egui::CollapsingHeader::new(RichText::new(src).small().strong())
                        .id_salt(format!("live-{src}"))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(p.surface)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(RichText::new(truncate_tail(text, 1200)).small().monospace().color(p.muted));
                                });
                        });
                }
            }
        });
    }

    // ------------------------------------------------------------------ runs

    fn runs_page(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            ui.label(theme::bold("Execuções do cérebro").size(20.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::button(ui, theme::Kind::Secondary, "↻ Atualizar lista").clicked() {
                    self.runs = load_runs();
                }
            });
        });
        ui.label(theme::muted(ui, "Cada orquestração gera um relatório com o playbook, os workers, o tempo, os tokens e o custo."));
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if self.runs.is_empty() {
                theme::card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(theme::bold("Nenhuma execução ainda"));
                    ui.label(
                        RichText::new("Peça uma tarefa grande na Conversa, como \"organize meus downloads por tipo e me diga o que dá para apagar\". O relatório aparece aqui.")
                            .color(p.muted),
                    );
                    if theme::primary(ui, "Ir para a Conversa").clicked() {
                        self.page = Page::Chat;
                    }
                });
            }
            for (id, title, md) in &self.runs {
                theme::card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::CollapsingHeader::new(theme::bold(title.clone())).id_salt(id).show(ui, |ui| {
                        ui.label(RichText::new(id).small().color(p.muted));
                        CommonMarkViewer::new().show(ui, &mut self.md, md);
                    });
                });
                ui.add_space(6.0);
            }
        });
    }
}

impl eframe::App for SageApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();
        self.poll_os(&ctx);
        if self.pet_visible {
            self.pet.show(&ctx);
        }
        self.top_bar(ui);
        let p = theme::palette(ui);
        match self.page {
            Page::Chat => self.chat_page(ui),
            Page::Runs => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(20)))
                    .show(ui, |ui| self.runs_page(ui));
            }
            Page::Settings => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(20)))
                    .show(ui, |ui| {
                        if let Some(saved) = self.settings.ui(ui) {
                            if let Ok(mut c) = self.cfg.write() {
                                *c = saved;
                            }
                            self.rt.send(Command::ReloadConfig);
                        }
                    });
            }
        }
        // Animação do slime precisa de quadros contínuos.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn status_label(s: AppStatus) -> &'static str {
    match s {
        AppStatus::Idle => "Pronto",
        AppStatus::Listening => "Ouvindo",
        AppStatus::Transcribing => "Transcrevendo",
        AppStatus::Thinking => "Pensando",
        AppStatus::Orchestrating => "Orquestrando",
        AppStatus::Speaking => "Falando",
        AppStatus::Error => "Erro",
    }
}

fn status_color(p: &Palette, s: AppStatus) -> Color32 {
    match s {
        AppStatus::Idle => p.accent,
        AppStatus::Listening | AppStatus::Transcribing | AppStatus::Speaking => p.ok,
        AppStatus::Thinking | AppStatus::Orchestrating => p.warn,
        AppStatus::Error => p.err,
    }
}

fn worker_label(s: WorkerStatus) -> &'static str {
    match s {
        WorkerStatus::Done => "Concluído",
        WorkerStatus::DoneWithConcerns => "Concluído com ressalvas",
        WorkerStatus::Blocked => "Bloqueado",
        WorkerStatus::NeedsContext => "Precisa de contexto",
        WorkerStatus::Failed => "Falhou",
    }
}

fn worker_color(p: &Palette, s: WorkerStatus) -> Color32 {
    match s {
        WorkerStatus::Done => p.ok,
        WorkerStatus::DoneWithConcerns | WorkerStatus::NeedsContext => p.warn,
        WorkerStatus::Blocked | WorkerStatus::Failed => p.err,
    }
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

fn truncate_tail(s: &str, n: usize) -> String {
    let c = s.chars().count();
    if c <= n { s.to_string() } else { s.chars().skip(c - n).collect() }
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(sage_core::paths::assets().join("app_icon.png")).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("sage-logo", color, egui::TextureOptions::LINEAR))
}

/// (id, título, markdown) das execuções em data/runs, mais recentes primeiro.
fn load_runs() -> Vec<(String, String, String)> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(sage_core::paths::runs()) {
        for e in rd.flatten() {
            let dir = e.path();
            let md = std::fs::read_to_string(dir.join("report.md")).unwrap_or_default();
            if md.is_empty() {
                continue;
            }
            let title = md.lines().next().unwrap_or("").trim_start_matches('#').trim().to_string();
            out.push((e.file_name().to_string_lossy().to_string(), title, md));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.truncate(50);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_thousands() {
        assert_eq!(super::fmt_int(1234567), "1.234.567");
        assert_eq!(super::fmt_int(12), "12");
    }
}
