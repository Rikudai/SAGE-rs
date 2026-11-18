//! Identidade visual do S.A.G.E.: fontes, cores (escuro e claro) e componentes
//! pequenos reaproveitados pelas telas (chip de status, selo, botão primário).

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Visuals};
use std::sync::Arc;

/// Paleta de uma variante do tema. Tudo que tem significado sai daqui ou de `Visuals`.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_hi: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0E, 0x13, 0x1A),
    surface: Color32::from_rgb(0x15, 0x1C, 0x26),
    surface_hi: Color32::from_rgb(0x1C, 0x26, 0x33),
    border: Color32::from_rgb(0x27, 0x33, 0x44),
    text: Color32::from_rgb(0xE6, 0xED, 0xF5),
    muted: Color32::from_rgb(0x93, 0xA3, 0xB8),
    accent: Color32::from_rgb(0x4D, 0xB2, 0xFF),
    accent_soft: Color32::from_rgb(0x16, 0x34, 0x52),
    on_accent: Color32::from_rgb(0x05, 0x12, 0x20),
    ok: Color32::from_rgb(0x5F, 0xD3, 0x8A),
    warn: Color32::from_rgb(0xF2, 0xC1, 0x4E),
    err: Color32::from_rgb(0xFF, 0x7A, 0x7A),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF3, 0xF6, 0xFA),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_hi: Color32::from_rgb(0xEA, 0xF0, 0xF7),
    border: Color32::from_rgb(0xD5, 0xDE, 0xEA),
    text: Color32::from_rgb(0x13, 0x1F, 0x2E),
    muted: Color32::from_rgb(0x55, 0x65, 0x7A),
    accent: Color32::from_rgb(0x15, 0x6F, 0xC2),
    accent_soft: Color32::from_rgb(0xDC, 0xEC, 0xFB),
    on_accent: Color32::WHITE,
    ok: Color32::from_rgb(0x1E, 0x8A, 0x4C),
    warn: Color32::from_rgb(0x9A, 0x6A, 0x00),
    err: Color32::from_rgb(0xC0, 0x32, 0x32),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

pub const RADIUS: u8 = 10;

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals_of(Theme::Dark, visuals(DARK, true));
    ctx.set_visuals_of(Theme::Light, visuals(LIGHT, false));
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.window_margin = Margin::same(14);
        s.animation_time = 0.15;
    });
}

fn install_fonts(ctx: &egui::Context) {
    let dir = sage_core::paths::assets().join("fonts");
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str, family: FontFamily| {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            // Na frente da lista; as fontes padrão do egui ficam como reserva (emoji, símbolos).
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    };
    add("atkinson", "AtkinsonHyperlegible-Regular.ttf", FontFamily::Proportional);
    add("jetbrains", "JetBrainsMono.ttf", FontFamily::Monospace);
    if let Ok(bytes) = std::fs::read(dir.join("AtkinsonHyperlegible-Bold.ttf")) {
        fonts.font_data.insert("atkinson-bold".into(), Arc::new(FontData::from_owned(bytes)));
        let mut list = vec!["atkinson-bold".to_string()];
        list.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
        fonts.families.insert(FontFamily::Name("bold".into()), list);
    }
    ctx.set_fonts(fonts);
}

fn visuals(p: Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(RADIUS);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.surface_hi;
    v.code_bg_color = p.surface_hi;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = r;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.err;
    v.warn_fg_color = p.warn;
    v.override_text_color = Some(p.text);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (state, fill) in [
        (&mut w.inactive, p.surface_hi),
        (&mut w.hovered, p.border),
        (&mut w.active, p.accent_soft),
        (&mut w.open, p.surface_hi),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = r;
        state.fg_stroke = Stroke::new(1.0, p.text);
        state.bg_stroke = Stroke::NONE;
    }
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    w.hovered.expansion = 1.0;
    w.active.expansion = 0.0;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    v
}

/// Cartão de superfície com borda suave.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
}

/// Botão da ação principal de um grupo (cor de destaque, com feedback tátil).
pub fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    button(ui, Kind::Primary, text)
}

/// Selo com ponto colorido e texto (a cor nunca é o único sinal).
pub fn badge(ui: &mut egui::Ui, color: Color32, text: &str) {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface_hi)
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(egui::RichText::new(text).small().color(p.text));
            });
        });
}

/// Título de seção com descrição curta embaixo.
pub fn section(ui: &mut egui::Ui, title: &str, desc: &str) {
    let p = palette(ui);
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).size(16.0).family(FontFamily::Name("bold".into())).color(p.text));
    if !desc.is_empty() {
        ui.label(egui::RichText::new(desc).small().color(p.muted));
    }
    ui.add_space(4.0);
}

pub fn muted(ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).color(palette(ui).muted)
}

pub fn bold(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).family(FontFamily::Name("bold".into()))
}

// ------------------------------------------------------------------ componentes táteis

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Ação principal (cor de destaque).
    Primary,
    /// Ação comum (superfície elevada).
    Secondary,
    /// Só texto até passar o mouse.
    Ghost,
    /// Destrutiva ou "gravando".
    Danger,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

fn kind_colors(p: &Palette, kind: Kind) -> (Color32, Color32, Color32) {
    // (fundo, fundo no hover, texto)
    match kind {
        Kind::Primary => (p.accent, mix(p.accent, Color32::WHITE, 0.18), p.on_accent),
        Kind::Secondary => (p.surface_hi, p.border, p.text),
        Kind::Ghost => (Color32::TRANSPARENT, p.surface_hi, p.muted),
        Kind::Danger => (p.err, mix(p.err, Color32::WHITE, 0.15), Color32::WHITE),
    }
}

/// Botão com hover animado, "afundar" ao pressionar, cursor de mão e foco visível.
pub fn button(ui: &mut egui::Ui, kind: Kind, text: &str) -> egui::Response {
    button_sized(ui, kind, text, None, CornerRadius::same(RADIUS))
}

/// Pílula (abas, chips).
pub fn pill(ui: &mut egui::Ui, kind: Kind, text: &str) -> egui::Response {
    button_sized(ui, kind, text, None, CornerRadius::same(255))
}

pub fn button_sized(ui: &mut egui::Ui, kind: Kind, text: &str, min: Option<egui::Vec2>, radius: CornerRadius) -> egui::Response {
    let p = palette(ui);
    let enabled = ui.is_enabled();
    let (bg, bg_hover, fg) = kind_colors(&p, kind);
    let font = if kind == Kind::Primary { FontId::new(14.5, FontFamily::Name("bold".into())) } else { TextStyle::Button.resolve(ui.style()) };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let pad = egui::vec2(14.0, 7.0);
    let mut size = galley.size() + pad * 2.0;
    if let Some(m) = min {
        size = size.max(m);
    }
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed });
    if ui.is_rect_visible(rect) {
        let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered() && enabled, 0.12);
        let press = ui.ctx().animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on() && enabled, 0.06);
        let mut fill = mix(bg, bg_hover, hover);
        if press > 0.0 {
            fill = mix(fill, Color32::BLACK, 0.18 * press);
        }
        if !enabled {
            fill = mix(fill, p.bg, 0.55);
        }
        // Afunda 1 px e encolhe levemente ao pressionar; cresce 1 px no hover.
        let r = rect.expand(hover * 1.0 - press * 1.5).translate(egui::vec2(0.0, press * 1.0));
        if kind == Kind::Primary && enabled {
            ui.painter().rect_filled(r.translate(egui::vec2(0.0, 2.0 - press * 2.0)), radius, mix(Color32::TRANSPARENT, Color32::from_black_alpha(90), 1.0 - press));
        }
        ui.painter().rect_filled(r, radius, fill);
        if kind == Kind::Secondary {
            ui.painter().rect_stroke(r, radius, Stroke::new(1.0, mix(p.border, p.accent, hover * 0.6)), egui::StrokeKind::Inside);
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(r.expand(2.0), radius, Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
        }
        let text_color = if kind == Kind::Ghost { mix(p.muted, p.text, hover) } else if enabled { fg } else { mix(fg, p.bg, 0.5) };
        let pos = r.center() - galley.size() / 2.0;
        ui.painter().galley_with_override_text_color(pos, galley, text_color);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    resp
}

/// Botão redondo só com ícone; `label` vira o nome acessível e a dica.
pub fn icon_button(ui: &mut egui::Ui, kind: Kind, icon: &str, label: &str, d: f32) -> egui::Response {
    let resp = button_sized(ui, kind, icon, Some(egui::vec2(d, d)), CornerRadius::same(255));
    resp.on_hover_text(label)
}

/// Interruptor animado (liga/desliga) com rótulo.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let p = palette(ui);
    let w = egui::vec2(36.0, 20.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), TextStyle::Body.resolve(ui.style()), p.text);
    let size = egui::vec2(w.x + 8.0 + galley.size().x, w.y.max(galley.size().y));
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp_c = resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp_c.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let track = egui::Rect::from_min_size(egui::pos2(rect.min.x, rect.center().y - w.y / 2.0), w);
    let fill = mix(mix(p.surface_hi, p.border, hover), p.accent, t);
    ui.painter().rect_filled(track, CornerRadius::same(255), fill);
    let x = egui::lerp(track.left() + 10.0..=track.right() - 10.0, t);
    ui.painter().circle_filled(egui::pos2(x, track.center().y), 7.5 + hover, if *on { p.on_accent } else { p.text });
    if resp.has_focus() {
        ui.painter().rect_stroke(track.expand(2.0), CornerRadius::same(255), Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
    }
    ui.painter().galley(egui::pos2(track.right() + 8.0, rect.center().y - galley.size().y / 2.0), galley, p.text);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CheckBox, true, *on, label));
    resp
}

/// Selo com ponto que pulsa quando `busy`.
pub fn status_badge(ui: &mut egui::Ui, color: Color32, text: &str, busy: bool) {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface_hi)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                if busy {
                    let t = (ui.input(|i| i.time) * 2.0).sin() as f32 * 0.5 + 0.5;
                    ui.painter().circle_filled(rect.center(), 4.0 + 3.0 * t, color.gamma_multiply(0.35 * (1.0 - t)));
                    ui.ctx().request_repaint();
                }
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.label(egui::RichText::new(text).small().color(p.text));
            });
        });
}
