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
    /// Hora (relógio do egui) em que o usuário pediu para sair; o app fecha quando o
    /// slime termina o `teleport_out` (ou depois de 2 s).
    quit_at: Option<f64>,
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
            quit_at: None,
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
                    TrayAction::Quit => {
                        // Sai com o teletransporte do slime em vez de sumir de uma vez.
                        self.pet_visible = false;
                        self.quit_at.get_or_insert(ctx.input(|i| i.time));
                    }
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
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin { left: 18, right: 18, top: 10, bottom: 10 })
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(32.0, 32.0)));
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(1.0);
                        ui.label(theme::bold("S.A.G.E.").size(16.5).color(p.text));
                        ui.label(RichText::new("cérebro orquestrador").size(11.5).color(p.muted));
                    });
                    ui.add_space(14.0);
                    theme::segmented(
                        ui,
                        "pages",
                        &mut self.page,
                        &[(Page::Chat, "Conversa"), (Page::Runs, "Execuções"), (Page::Settings, "Configurações")],
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        theme::switch(ui, &mut self.pet_visible, "Slime")
                            .on_hover_text("Mostra ou esconde o slime na área de trabalho");
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
        let mut open_brain = self.show_brain;
        egui::Panel::right("brain")
            .resizable(true)
            .default_size(340.0)
            .min_size(270.0)
            .max_size(560.0)
            .frame(
                egui::Frame::new()
                    .fill(p.surface)
                    .inner_margin(Margin { left: 18, right: 16, top: 16, bottom: 12 })
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show_collapsible(ui, &mut open_brain, |ui| self.brain_panel(ui));
        self.show_brain = open_brain;

        egui::Panel::bottom("composer")
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 20, right: 20, top: 6, bottom: 12 }))
            .show(ui, |ui| self.composer(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 20, right: 12, top: 18, bottom: 4 }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    // Coluna de leitura centrada (linhas confortáveis em janelas largas).
                    let col = ui.available_width().min(820.0);
                    let pad = ((ui.available_width() - col) / 2.0).max(0.0);
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(col - 8.0);
                            let max = (col * 0.8).clamp(240.0, 640.0);
                            for i in 0..self.chat.len() {
                                self.bubble(ui, i, max);
                                ui.add_space(14.0);
                            }
                            if matches!(self.status.0, AppStatus::Thinking | AppStatus::Orchestrating) {
                                self.typing(ui);
                            }
                            if self.chat.len() <= 1 {
                                self.suggestions(ui);
                            }
                        });
                    });
                });
            });
    }

    fn avatar(&self, ui: &mut egui::Ui, d: f32) {
        let p = theme::palette(ui);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::hover());
        if let Some(logo) = &self.logo {
            egui::Image::new(logo).corner_radius(CornerRadius::same((d * 0.28) as u8)).paint_at(ui, rect);
        } else {
            ui.painter().circle_filled(rect.center(), d / 2.0, p.accent_soft);
        }
    }

    fn bubble(&mut self, ui: &mut egui::Ui, i: usize, max: f32) {
        let p = theme::palette(ui);
        let from_user = self.chat[i].from_user;
        let text = self.chat[i].text.clone();
        let when = self.chat[i].when.clone();
        if from_user {
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                egui::Frame::new()
                    .fill(p.user_bubble)
                    .corner_radius(CornerRadius { nw: 18, ne: 6, sw: 18, se: 18 })
                    .inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 11 })
                    .show(ui, |ui| {
                        ui.set_max_width(max);
                        CommonMarkViewer::new().show(ui, &mut self.md, &text);
                    });
                ui.add_space(-4.0);
                ui.label(RichText::new(format!("Você · {when}")).size(11.5).color(p.subtle));
            });
        } else {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                self.avatar(ui, 30.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(theme::bold("S.A.G.E.").size(13.0).color(p.text));
                        ui.label(RichText::new(when).size(11.5).color(p.subtle));
                    });
                    egui::Frame::new()
                        .fill(p.raised)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius { nw: 6, ne: 18, sw: 18, se: 18 })
                        .inner_margin(Margin { left: 16, right: 16, top: 10, bottom: 11 })
                        .shadow(theme::soft_shadow(&p))
                        .show(ui, |ui| {
                            ui.set_max_width(max);
                            CommonMarkViewer::new().show(ui, &mut self.md, &text);
                        });
                });
            });
        }
    }

    fn typing(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            self.avatar(ui, 30.0);
            egui::Frame::new()
                .fill(p.raised)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius { nw: 6, ne: 18, sw: 18, se: 18 })
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        theme::typing_dots(ui, p.accent);
                        ui.label(RichText::new(status_label(self.status.0)).size(13.0).color(p.muted));
                    });
                });
        });
        ui.add_space(12.0);
    }

    fn suggestions(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::bold("Como posso ajudar?").size(theme::size::HEADING).color(p.text));
            ui.label(RichText::new("Converse, peça uma ação no PC ou entregue uma tarefa grande para o cérebro dividir.").color(p.muted));
        });
        ui.add_space(14.0);
        let items = [
            ("Diagnóstico", "Como está o meu PC agora?"),
            ("Ação rápida", "Abra o Spotify"),
            ("Organizar", "Organize a pasta Downloads por tipo"),
            ("Pesquisa", "Pesquise e compare 3 SSDs NVMe de 1 TB"),
        ];
        let gap = 10.0;
        let w = ui.available_width();
        let cols = if w > 520.0 { 2 } else { 1 };
        let cw = (w - gap * (cols as f32 - 1.0)) / cols as f32;
        let mut picked = None;
        for row in items.chunks(cols) {
            // Mesma altura para os cartões da linha (a do texto mais longo).
            let h = row
                .iter()
                .map(|(_, t)| ui.painter().layout(t.to_string(), egui::FontId::proportional(14.5), p.text, cw - 32.0).size().y)
                .fold(0.0f32, f32::max);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (tag, text) in row {
                    if suggestion_card(ui, &p, tag, text, cw, h).clicked() {
                        picked = Some(text.to_string());
                    }
                }
            });
            ui.add_space(gap - 8.0);
        }
        if let Some(s) = picked {
            self.input = s;
            self.send_input();
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        for n in &self.notices {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                theme::badge(ui, p.accent, "Aviso");
                ui.label(RichText::new(n).size(13.0).color(p.muted));
            });
        }
        if let Some((id, q)) = self.confirm.clone() {
            theme::card(ui).stroke(Stroke::new(1.0, p.warn)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::badge(ui, p.warn, "Confirmação");
                    ui.label(theme::bold(q));
                });
                ui.add_space(4.0);
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
            ui.add_space(8.0);
        }
        let input_id = egui::Id::unique("composer-input");
        let focus = ui.ctx().memory(|m| m.focused()) == Some(input_id);
        let f = ui.ctx().animate_bool_with_time(input_id.with("focus"), focus, 0.15);
        let ring = theme::mix(p.border, p.accent, f);
        let mut shadow = theme::soft_shadow(&p);
        shadow.blur = (10.0 + 8.0 * f) as u8;
        egui::Frame::new()
            .fill(p.raised)
            .stroke(Stroke::new(1.0 + 0.5 * f, ring))
            .corner_radius(CornerRadius::same(theme::RADIUS_LG + 4))
            .inner_margin(Margin { left: 8, right: 8, top: 7, bottom: 7 })
            .shadow(shadow)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let mic_kind = if self.mic_held { theme::Kind::Danger } else { theme::Kind::Ghost };
                    let mic = theme::icon_button(
                        ui,
                        mic_kind,
                        if self.mic_held { theme::Icon::Stop } else { theme::Icon::Mic },
                        "Segure para falar (ou use o atalho de voz)",
                        38.0,
                    );
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
                    let send_w = 46.0;
                    let hint = if self.mic_held { "Ouvindo… solte o botão para enviar" } else { "Mensagem para o S.A.G.E." };
                    let edit = egui::TextEdit::multiline(&mut self.input)
                        .id(input_id)
                        .desired_rows(1)
                        .frame(egui::Frame::NONE)
                        .margin(Margin { left: 4, right: 4, top: 10, bottom: 8 })
                        .font(egui::TextStyle::Body)
                        .desired_width((ui.available_width() - send_w).max(120.0))
                        .hint_text(RichText::new(hint).color(p.subtle));
                    let resp = ui.add(edit);
                    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    if enter {
                        self.input = self.input.trim_end_matches('\n').to_string();
                        self.send_input();
                        resp.request_focus();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can = !self.input.trim().is_empty();
                        let send = ui
                            .add_enabled_ui(can, |ui| theme::icon_button(ui, theme::Kind::Primary, theme::Icon::Send, "Enviar (Enter)", 38.0))
                            .inner;
                        if send.clicked() {
                            self.send_input();
                        }
                    });
                });
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (st, detail) = self.status.clone();
            ui.add_space(6.0);
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.5, status_color(&p, st));
            ui.label(RichText::new(status_label(st)).size(12.5).color(p.text));
            if !detail.is_empty() {
                ui.label(RichText::new(truncate(&detail, 80)).size(12.5).color(p.muted));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.show_brain { "Esconder o cérebro" } else { "Ver o cérebro" };
                if theme::icon_text_button(ui, theme::Kind::Ghost, theme::Icon::PanelRight, label).clicked() {
                    self.show_brain = !self.show_brain;
                }
                if matches!(st, AppStatus::Orchestrating | AppStatus::Thinking)
                    && theme::icon_text_button(ui, theme::Kind::Danger, theme::Icon::Stop, "Parar").clicked()
                {
                    self.rt.send(Command::StopRun);
                }
                ui.label(RichText::new("Enter envia · Shift+Enter quebra a linha").size(12.0).color(p.subtle));
            });
        });
    }

    fn brain_panel(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.label(theme::bold("Cérebro").size(theme::size::TITLE).color(p.text));
        match &self.plan {
            Some((pb, why)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Playbook").size(13.0).color(p.muted));
                    theme::badge(ui, p.accent, pb);
                });
                ui.label(RichText::new(why).size(13.0).color(p.muted));
            }
            None => {
                ui.label(RichText::new("Pedidos grandes são divididos entre workers. Acompanhe aqui, ao vivo.").size(13.0).color(p.muted));
                ui.add_space(12.0);
                let steps = [
                    ("Triagem", "Um modelo rápido decide: conversa, ação no PC ou tarefa."),
                    ("Plano", "O lead escolhe um playbook e escreve um brief por unidade."),
                    ("Workers", "Modelos mais baratos executam em paralelo, com cota vigiada."),
                    ("Revisão e síntese", "Cada entrega passa por um gate; o lead junta tudo."),
                ];
                for (i, (title, desc)) in steps.iter().enumerate() {
                    let r = ui
                        .horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 13.0, p.accent_soft);
                            ui.painter().circle_stroke(rect.center(), 13.0, Stroke::new(1.0, theme::mix(p.accent_soft, p.accent, 0.5)));
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                (i + 1).to_string(),
                                egui::FontId::new(12.5, theme::bold_family()),
                                p.accent,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(theme::bold(*title).size(14.0).color(p.text));
                                ui.label(RichText::new(*desc).size(12.5).color(p.muted));
                            });
                            rect
                        })
                        .inner;
                    if i + 1 < steps.len() {
                        // Linha do tempo ligando os passos.
                        let x = r.center().x;
                        let y0 = r.bottom() + 3.0;
                        ui.add_space(10.0);
                        let y1 = ui.cursor().top() - 1.0;
                        ui.painter().line_segment([egui::pos2(x, y0), egui::pos2(x, y1)], Stroke::new(1.5, p.border_strong));
                    }
                }
            }
        }
        ui.add_space(12.0);
        egui::ScrollArea::vertical().id_salt("brain-scroll").auto_shrink(false).show(ui, |ui| {
            if !self.workers.is_empty() {
                theme::overline(ui, &format!("Workers · {}", self.workers.len()));
                ui.add_space(2.0);
            }
            for (id, w) in &self.workers {
                let accent = w.status.map_or(p.accent, |s| worker_color(&p, s));
                let resp = egui::Frame::new()
                    .fill(p.raised)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin { left: 16, right: 12, top: 10, bottom: 10 })
                    .shadow(theme::soft_shadow(&p))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(theme::bold(id).color(p.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match w.status {
                                None => {
                                    ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                                }
                                Some(s) => theme::badge(ui, worker_color(&p, s), worker_label(s)),
                            });
                        });
                        ui.label(RichText::new(format!("{} · {}", w.role, w.model)).size(12.5).color(p.muted));
                        if w.status.is_some() {
                            let cost = w.cost.map(|c| format!(" · US$ {c:.4}")).unwrap_or_default();
                            ui.label(RichText::new(format!("{} tokens{cost}", fmt_int(w.tokens))).size(12.5).color(p.muted));
                            if !w.summary.is_empty() {
                                ui.label(&w.summary);
                            }
                        } else {
                            ui.label(RichText::new(truncate(&w.brief, 180)).size(13.0));
                        }
                        for (ok, t) in w.tools.iter().rev().take(3) {
                            let c = if *ok { p.ok } else { p.err };
                            let mark = if *ok { "✔" } else { "✖" };
                            ui.label(RichText::new(format!("{mark} {t}")).size(12.0).monospace().color(c));
                        }
                    });
                // Faixa de status à esquerda do cartão.
                let r = resp.response.rect;
                let stripe = egui::Rect::from_min_max(r.min + egui::vec2(1.0, 10.0), egui::pos2(r.min.x + 4.0, r.max.y - 10.0));
                ui.painter().rect_filled(stripe, CornerRadius::same(2), accent);
                ui.add_space(8.0);
            }
            if !self.live.is_empty() {
                ui.add_space(6.0);
                theme::overline(ui, "Raciocínio ao vivo");
                for (src, text) in &self.live {
                    egui::CollapsingHeader::new(RichText::new(src).size(13.0).strong())
                        .id_salt(format!("live-{src}"))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(p.bg)
                                .stroke(Stroke::new(1.0, p.border))
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(RichText::new(truncate_tail(text, 1200)).size(12.0).monospace().color(p.muted));
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
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(theme::bold("Execuções do cérebro").size(theme::size::HEADING).color(p.text));
                ui.label(theme::muted(ui, "Cada orquestração gera um relatório com o playbook, os workers, o tempo, os tokens e o custo."));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::icon_text_button(ui, theme::Kind::Secondary, theme::Icon::Refresh, "Atualizar lista").clicked() {
                    self.runs = load_runs();
                }
            });
        });
        ui.add_space(14.0);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if self.runs.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.set_max_width(460.0);
                    theme::card(ui).inner_margin(Margin::same(28)).show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 26.0, p.accent_soft);
                            theme::paint_icon(ui.painter(), theme::Icon::PanelRight, rect.center(), 24.0, p.accent);
                            ui.add_space(6.0);
                            ui.label(theme::bold("Nenhuma execução ainda").size(theme::size::TITLE).color(p.text));
                            ui.label(
                                RichText::new("Peça uma tarefa grande na Conversa, como \"organize meus downloads por tipo e me diga o que dá para apagar\". O relatório aparece aqui.")
                                    .color(p.muted),
                            );
                            ui.add_space(8.0);
                            if theme::primary(ui, "Ir para a Conversa").clicked() {
                                self.page = Page::Chat;
                            }
                        });
                    });
                });
            }
            for (id, title, md) in &self.runs {
                theme::card(ui).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::CollapsingHeader::new(theme::bold(title.clone()).color(p.text)).id_salt(id).show(ui, |ui| {
                        ui.label(RichText::new(id).size(12.0).monospace().color(p.muted));
                        CommonMarkViewer::new().show(ui, &mut self.md, md);
                    });
                });
                ui.add_space(8.0);
            }
        });
    }
}

/// Cartão clicável de sugestão (estado vazio da conversa).
fn suggestion_card(ui: &mut egui::Ui, p: &Palette, tag: &str, text: &str, w: f32, text_h: f32) -> egui::Response {
    let g = ui.painter().layout(text.to_string(), egui::FontId::proportional(14.5), p.text, w - 32.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, (text_h + 44.0).max(66.0)), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let t = theme::tactile(ui, &resp, true);
        let r = CornerRadius::same(theme::RADIUS_CARD);
        let lift = t.hover * (1.0 - t.press);
        ui.painter().add(
            egui::Shadow { offset: [0, (1.0 + 3.0 * lift) as i8], blur: (6.0 + 10.0 * lift) as u8, spread: 0, color: p.shadow.gamma_multiply(0.5 + 0.5 * lift) }
                .as_shape(t.rect, r),
        );
        ui.painter().rect_filled(t.rect, r, theme::mix(p.raised, p.surface_hi, t.hover));
        ui.painter().rect_stroke(t.rect, r, Stroke::new(1.0, theme::mix(p.border, p.accent, t.hover * 0.7)), egui::StrokeKind::Inside);
        theme::focus_ring(ui, &resp, t.rect, r);
        let x = t.rect.left() + 16.0;
        ui.painter().text(egui::pos2(x, t.rect.top() + 12.0), egui::Align2::LEFT_TOP, tag.to_uppercase(), egui::FontId::new(11.0, theme::bold_family()), p.accent);
        ui.painter().galley(egui::pos2(x, t.rect.top() + 31.0), g, p.text);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    resp
}

impl eframe::App for SageApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();
        self.poll_os(&ctx);
        // Sempre chamado: ligar/desligar toca a entrada/saída do slime até o fim.
        self.pet.set_visible(self.pet_visible);
        self.pet.show(&ctx);
        if let Some(t) = self.quit_at {
            if self.pet.is_gone() || ctx.input(|i| i.time) - t > 2.0 {
                std::process::exit(0);
            }
            ctx.request_repaint();
        }
        self.top_bar(ui);
        let p = theme::palette(ui);
        match self.page {
            Page::Chat => self.chat_page(ui),
            Page::Runs => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 28, right: 28, top: 22, bottom: 16 }))
                    .show(ui, |ui| self.runs_page(ui));
            }
            Page::Settings => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin { left: 28, right: 28, top: 22, bottom: 16 }))
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
