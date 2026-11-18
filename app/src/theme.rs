//! Identidade visual do S.A.G.E.: fontes, cores (escuro e claro), escala de texto,
//! sombras e o kit de componentes táteis (botão, pílula, botão de ícone, controle
//! segmentado, interruptor, selos) com hover, pressão e foco animados.
//!
//! Contraste conferido (WCAG 2.2): `text` e `muted` passam de 4,5:1 sobre `bg`,
//! `surface`, `surface_hi` e `raised` nos dois temas; `subtle` só em texto secundário
//! sobre `bg`/`surface` (≥ 4,8:1).

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect, Shadow, Stroke,
    TextStyle, Theme, Vec2, Visuals,
};
use std::sync::Arc;

/// Paleta de uma variante do tema. Tudo que tem significado sai daqui ou de `Visuals`.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Fundo da janela.
    pub bg: Color32,
    /// Painéis e barra do topo.
    pub surface: Color32,
    /// Campos, trilhos e controles em repouso.
    pub surface_hi: Color32,
    /// Cartões e balões elevados.
    pub raised: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub subtle: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    /// Balão das mensagens do usuário.
    pub user_bubble: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    /// Cor das sombras (com alfa).
    pub shadow: Color32,
}

pub const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0A, 0x0F, 0x16),
    surface: Color32::from_rgb(0x11, 0x18, 0x21),
    surface_hi: Color32::from_rgb(0x18, 0x21, 0x2D),
    raised: Color32::from_rgb(0x15, 0x1D, 0x28),
    border: Color32::from_rgb(0x24, 0x30, 0x41),
    border_strong: Color32::from_rgb(0x33, 0x42, 0x57),
    text: Color32::from_rgb(0xE8, 0xEE, 0xF6),
    muted: Color32::from_rgb(0x9B, 0xA9, 0xBC),
    subtle: Color32::from_rgb(0x7F, 0x8E, 0xA3),
    accent: Color32::from_rgb(0x4D, 0xB2, 0xFF),
    accent_hover: Color32::from_rgb(0x74, 0xC4, 0xFF),
    accent_soft: Color32::from_rgb(0x14, 0x30, 0x4C),
    on_accent: Color32::from_rgb(0x04, 0x12, 0x1F),
    user_bubble: Color32::from_rgb(0x1A, 0x41, 0x66),
    ok: Color32::from_rgb(0x5F, 0xD3, 0x8A),
    warn: Color32::from_rgb(0xF2, 0xC1, 0x4E),
    err: Color32::from_rgb(0xFF, 0x7A, 0x7A),
    shadow: Color32::from_black_alpha(110),
};

pub const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF3, 0xF6, 0xFA),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_hi: Color32::from_rgb(0xED, 0xF2, 0xF8),
    raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    border: Color32::from_rgb(0xD9, 0xE1, 0xEB),
    border_strong: Color32::from_rgb(0xBF, 0xCB, 0xDA),
    text: Color32::from_rgb(0x0F, 0x1B, 0x2A),
    muted: Color32::from_rgb(0x4F, 0x5E, 0x72),
    subtle: Color32::from_rgb(0x5E, 0x6D, 0x81),
    accent: Color32::from_rgb(0x14, 0x66, 0xBD),
    accent_hover: Color32::from_rgb(0x0F, 0x58, 0xA6),
    accent_soft: Color32::from_rgb(0xDC, 0xEB, 0xFB),
    on_accent: Color32::WHITE,
    user_bubble: Color32::from_rgb(0xD3, 0xE6, 0xFB),
    ok: Color32::from_rgb(0x1B, 0x7F, 0x45),
    warn: Color32::from_rgb(0x8A, 0x5E, 0x00),
    err: Color32::from_rgb(0xBE, 0x2F, 0x2F),
    shadow: Color32::from_black_alpha(28),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// Raios: controles pequenos, cartões e campos grandes (composer).
pub const RADIUS: u8 = 10;
pub const RADIUS_CARD: u8 = 14;
pub const RADIUS_LG: u8 = 20;

/// Escala tipográfica (razão ~1,2 a partir do corpo de 15 pt).
pub mod size {
    pub const SMALL: f32 = 12.5;
    pub const BODY: f32 = 15.0;
    pub const TITLE: f32 = 18.0;
    pub const HEADING: f32 = 22.0;
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals_of(Theme::Dark, visuals(DARK, true));
    ctx.set_visuals_of(Theme::Light, visuals(LIGHT, false));
    ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(size::HEADING, bold_family())),
            (TextStyle::Body, FontId::new(size::BODY, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(size::SMALL, FontFamily::Proportional)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 32.0;
        s.spacing.window_margin = Margin::same(16);
        s.spacing.combo_width = 220.0;
        s.spacing.text_edit_width = 320.0;
        s.spacing.slider_width = 220.0;
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.floating_width = 6.0;
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
    let mut list = vec![];
    if let Ok(bytes) = std::fs::read(dir.join("AtkinsonHyperlegible-Bold.ttf")) {
        fonts.font_data.insert("atkinson-bold".into(), Arc::new(FontData::from_owned(bytes)));
        list.push("atkinson-bold".to_string());
    }
    list.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(bold_family(), list);
    ctx.set_fonts(fonts);
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

fn visuals(p: Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(RADIUS);
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    // Campos de texto: um tom abaixo da superfície, com borda (ver `widgets`).
    v.extreme_bg_color = if dark { Color32::from_rgb(0x0D, 0x13, 0x1B) } else { Color32::WHITE };
    v.text_edit_bg_color = Some(v.extreme_bg_color);
    v.faint_bg_color = p.surface_hi;
    v.code_bg_color = p.surface_hi;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: p.shadow };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: p.shadow };
    v.menu_corner_radius = r;
    v.selection.bg_fill = if dark { Color32::from_rgb(0x1F, 0x4E, 0x7A) } else { p.accent_soft };
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.hyperlink_color = p.accent;
    v.error_fg_color = p.err;
    v.warn_fg_color = p.warn;
    v.override_text_color = Some(p.text);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.striped = false;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (state, fill, stroke) in [
        (&mut w.inactive, p.surface_hi, p.border),
        (&mut w.hovered, mix(p.surface_hi, p.border, 0.5), p.border_strong),
        (&mut w.active, p.accent_soft, p.accent),
        (&mut w.open, p.surface_hi, p.border_strong),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = r;
        state.fg_stroke = Stroke::new(1.5, p.text);
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.expansion = 0.0;
    }
    // Trilho dos sliders e marca das caixas usam a cor de destaque.
    w.inactive.fg_stroke = Stroke::new(1.5, p.text);
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.bg_stroke = Stroke::new(1.5, p.accent);
    v
}

/// Cartão elevado com borda suave e sombra leve.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_CARD))
        .inner_margin(Margin::same(16))
        .shadow(soft_shadow(&p))
}

/// Sombra baixa e difusa para cartões.
pub fn soft_shadow(p: &Palette) -> Shadow {
    Shadow { offset: [0, 2], blur: 10, spread: 0, color: p.shadow.gamma_multiply(0.6) }
}

/// Botão da ação principal de um grupo (cor de destaque, com feedback tátil).
pub fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    button(ui, Kind::Primary, text)
}

/// Selo com ponto colorido e texto (a cor nunca é o único sinal).
pub fn badge(ui: &mut egui::Ui, color: Color32, text: &str) {
    let p = palette(ui);
    egui::Frame::new()
        .fill(mix(p.surface_hi, color, 0.12))
        .stroke(Stroke::new(1.0, mix(p.border, color, 0.35)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(egui::RichText::new(text).size(size::SMALL).color(p.text));
            });
        });
}

/// Título de seção com descrição curta embaixo.
pub fn section(ui: &mut egui::Ui, title: &str, desc: &str) {
    let p = palette(ui);
    ui.label(egui::RichText::new(title).size(size::TITLE).family(bold_family()).color(p.text));
    if !desc.is_empty() {
        ui.add_space(-2.0);
        ui.label(egui::RichText::new(desc).size(13.5).color(p.muted));
    }
    ui.add_space(10.0);
}

/// Rótulo pequeno em caixa alta para agrupar conteúdo ("WORKERS", "RACIOCÍNIO").
pub fn overline(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui);
    ui.label(egui::RichText::new(text.to_uppercase()).size(11.5).family(bold_family()).color(p.muted).extra_letter_spacing(0.8));
}

pub fn muted(ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).color(palette(ui).muted)
}

pub fn bold(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).family(bold_family())
}

// ------------------------------------------------------------------ componentes táteis

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Ação principal (cor de destaque).
    Primary,
    /// Ação comum (superfície elevada com borda).
    Secondary,
    /// Só texto até passar o mouse.
    Ghost,
    /// Destrutiva ou "gravando".
    Danger,
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

struct KindColors {
    bg: Color32,
    bg_hover: Color32,
    fg: Color32,
    fg_hover: Color32,
    stroke: Option<(Color32, Color32)>,
    shadow: bool,
}

fn kind_colors(p: &Palette, kind: Kind) -> KindColors {
    match kind {
        Kind::Primary => KindColors {
            bg: p.accent,
            bg_hover: p.accent_hover,
            fg: p.on_accent,
            fg_hover: p.on_accent,
            stroke: None,
            shadow: true,
        },
        Kind::Secondary => KindColors {
            bg: p.surface_hi,
            bg_hover: mix(p.surface_hi, p.border, 0.6),
            fg: p.text,
            fg_hover: p.text,
            stroke: Some((p.border, p.border_strong)),
            shadow: true,
        },
        Kind::Ghost => KindColors {
            bg: Color32::TRANSPARENT,
            bg_hover: p.surface_hi,
            fg: p.muted,
            fg_hover: p.text,
            stroke: None,
            shadow: false,
        },
        Kind::Danger => KindColors {
            bg: p.err,
            bg_hover: mix(p.err, Color32::WHITE, 0.15),
            fg: if p.on_accent == Color32::WHITE { Color32::WHITE } else { Color32::from_rgb(0x2A, 0x05, 0x05) },
            fg_hover: if p.on_accent == Color32::WHITE { Color32::WHITE } else { Color32::from_rgb(0x2A, 0x05, 0x05) },
            stroke: None,
            shadow: true,
        },
    }
}

/// Estado tátil animado de um widget: hover (sobe 1 px, clareia), pressão (afunda e
/// encolhe), foco por teclado (anel de destaque).
pub struct Tactile {
    pub hover: f32,
    pub press: f32,
    /// Retângulo já deslocado/encolhido para desenhar.
    pub rect: Rect,
}

pub fn tactile(ui: &egui::Ui, resp: &egui::Response, enabled: bool) -> Tactile {
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered() && enabled, 0.12);
    let press = ui.ctx().animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on() && enabled, 0.05);
    let rect = resp.rect.shrink(press * 1.0).translate(egui::vec2(0.0, press * 1.0 - hover * (1.0 - press) * 1.0));
    Tactile { hover, press, rect }
}

/// Anel de foco por teclado em volta de `rect`.
pub fn focus_ring(ui: &egui::Ui, resp: &egui::Response, rect: Rect, radius: CornerRadius) {
    if resp.has_focus() {
        let p = palette(ui);
        ui.painter().rect_stroke(rect.expand(3.0), radius, Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
    }
}

/// Botão com hover animado, "afundar" ao pressionar, cursor de mão e foco visível.
pub fn button(ui: &mut egui::Ui, kind: Kind, text: &str) -> egui::Response {
    button_sized(ui, kind, text, None, CornerRadius::same(RADIUS))
}

/// Pílula (chips, ações curtas).
#[allow(dead_code)]
pub fn pill(ui: &mut egui::Ui, kind: Kind, text: &str) -> egui::Response {
    button_sized(ui, kind, text, None, CornerRadius::same(255))
}

pub fn button_sized(ui: &mut egui::Ui, kind: Kind, text: &str, min: Option<Vec2>, radius: CornerRadius) -> egui::Response {
    let p = palette(ui);
    let enabled = ui.is_enabled();
    let c = kind_colors(&p, kind);
    let font = if matches!(kind, Kind::Primary | Kind::Danger) {
        FontId::new(14.5, bold_family())
    } else {
        TextStyle::Button.resolve(ui.style())
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, c.fg);
    let pad = egui::vec2(16.0, 8.0);
    let mut size = galley.size() + pad * 2.0;
    size.y = size.y.max(34.0);
    if let Some(m) = min {
        size = size.max(m);
    }
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed });
    if ui.is_rect_visible(rect) {
        let t = tactile(ui, &resp, enabled);
        paint_surface(ui, &p, &c, &t, radius, enabled);
        focus_ring(ui, &resp, t.rect, radius);
        let fg = mix(c.fg, c.fg_hover, t.hover);
        let text_color = if enabled { fg } else { mix(fg, p.bg, 0.5) };
        let pos = t.rect.center() - galley.size() / 2.0;
        ui.painter().galley_with_override_text_color(pos, galley, text_color);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    resp
}

fn paint_surface(ui: &egui::Ui, p: &Palette, c: &KindColors, t: &Tactile, radius: CornerRadius, enabled: bool) {
    let mut fill = mix(c.bg, c.bg_hover, t.hover);
    if t.press > 0.0 {
        fill = mix(fill, Color32::BLACK, 0.14 * t.press);
    }
    if !enabled {
        fill = mix(fill, p.bg, 0.55);
    }
    if c.shadow && enabled && fill.a() > 0 {
        // Sombra que cresce no hover e some ao pressionar (sensação de "apertar").
        let lift = t.hover * (1.0 - t.press);
        let s = Shadow {
            offset: [0, (1.0 + 2.0 * lift) as i8],
            blur: (4.0 + 8.0 * lift) as u8,
            spread: 0,
            color: p.shadow.gamma_multiply(0.55 + 0.45 * lift),
        };
        ui.painter().add(s.as_shape(t.rect, radius));
    }
    ui.painter().rect_filled(t.rect, radius, fill);
    if let Some((s0, s1)) = c.stroke {
        let col = if enabled { mix(s0, s1, t.hover) } else { s0 };
        ui.painter().rect_stroke(t.rect, radius, Stroke::new(1.0, col), egui::StrokeKind::Inside);
    }
    if c.shadow && enabled && fill.a() == 255 {
        // Brilho fino no topo: dá volume ao botão.
        let top = Rect::from_min_max(t.rect.min + egui::vec2(radius.nw as f32 * 0.6, 1.0), Pos2::new(t.rect.max.x - radius.ne as f32 * 0.6, t.rect.min.y + 1.0));
        ui.painter().line_segment([top.left_top(), top.right_top()], Stroke::new(1.0, Color32::from_white_alpha(if p.on_accent == Color32::WHITE { 40 } else { 18 })));
    }
}

/// Ícones vetoriais desenhados com o pintor (nítidos em qualquer escala).
#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Mic,
    Stop,
    Send,
    Refresh,
    PanelRight,
    Plus,
}

pub fn paint_icon(painter: &egui::Painter, icon: Icon, c: Pos2, s: f32, color: Color32) {
    let st = Stroke::new((s * 0.11).max(1.4), color);
    match icon {
        Icon::Mic => {
            let body = Rect::from_center_size(c + egui::vec2(0.0, -s * 0.12), egui::vec2(s * 0.36, s * 0.56));
            painter.rect_filled(body, CornerRadius::same(255), color);
            let r = s * 0.32;
            let cy = c.y - s * 0.06;
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 16.0;
                    Pos2::new(c.x - r * a.cos(), cy + r * a.sin())
                })
                .collect();
            painter.line(pts, st);
            painter.line_segment([Pos2::new(c.x, cy + r), Pos2::new(c.x, c.y + s * 0.42)], st);
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(c, egui::vec2(s * 0.42, s * 0.42)), CornerRadius::same(2), color);
        }
        Icon::Send => {
            // Seta para cima (estilo de apps de chat modernos).
            let st = Stroke::new((s * 0.13).max(1.6), color);
            let top = c + egui::vec2(0.0, -s * 0.34);
            painter.line_segment([c + egui::vec2(0.0, s * 0.34), top], st);
            painter.line(vec![c + egui::vec2(-s * 0.3, -s * 0.04), top, c + egui::vec2(s * 0.3, -s * 0.04)], st);
        }
        Icon::Refresh => {
            let r = s * 0.32;
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = -0.6 + 4.9 * i as f32 / 20.0;
                    Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                })
                .collect();
            let end = pts[0];
            painter.line(pts, st);
            painter.line(vec![end + egui::vec2(-s * 0.2, -s * 0.02), end, end + egui::vec2(s * 0.02, -s * 0.2)], st);
        }
        Icon::PanelRight => {
            let r = Rect::from_center_size(c, egui::vec2(s * 0.8, s * 0.66));
            painter.rect_stroke(r, CornerRadius::same(3), st, egui::StrokeKind::Middle);
            let x = r.right() - r.width() * 0.36;
            painter.line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())], st);
        }
        Icon::Plus => {
            painter.line_segment([c + egui::vec2(-s * 0.3, 0.0), c + egui::vec2(s * 0.3, 0.0)], st);
            painter.line_segment([c + egui::vec2(0.0, -s * 0.3), c + egui::vec2(0.0, s * 0.3)], st);
        }
    }
}

/// Botão redondo só com ícone; `label` vira o nome acessível e a dica.
pub fn icon_button(ui: &mut egui::Ui, kind: Kind, icon: Icon, label: &str, d: f32) -> egui::Response {
    let p = palette(ui);
    let enabled = ui.is_enabled();
    let c = kind_colors(&p, kind);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(d, d), egui::Sense::click());
    let resp = resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed });
    if ui.is_rect_visible(rect) {
        let t = tactile(ui, &resp, enabled);
        let radius = CornerRadius::same(255);
        paint_surface(ui, &p, &c, &t, radius, enabled);
        focus_ring(ui, &resp, t.rect, radius);
        let fg = mix(c.fg, c.fg_hover, t.hover);
        let color = if enabled { fg } else { mix(fg, p.bg, 0.5) };
        paint_icon(ui.painter(), icon, t.rect.center(), d * 0.5, color);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp.on_hover_text(label)
}

/// Botão com ícone e texto (ações secundárias como "Atualizar").
pub fn icon_text_button(ui: &mut egui::Ui, kind: Kind, icon: Icon, text: &str) -> egui::Response {
    let p = palette(ui);
    let enabled = ui.is_enabled();
    let c = kind_colors(&p, kind);
    let galley = ui.painter().layout_no_wrap(text.to_string(), TextStyle::Button.resolve(ui.style()), c.fg);
    let icon_w = 16.0;
    let size = egui::vec2(galley.size().x + icon_w + 8.0 + 28.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed });
    if ui.is_rect_visible(rect) {
        let t = tactile(ui, &resp, enabled);
        let radius = CornerRadius::same(RADIUS);
        paint_surface(ui, &p, &c, &t, radius, enabled);
        focus_ring(ui, &resp, t.rect, radius);
        let fg = mix(c.fg, c.fg_hover, t.hover);
        let ic = Pos2::new(t.rect.left() + 14.0 + icon_w / 2.0, t.rect.center().y);
        paint_icon(ui.painter(), icon, ic, icon_w, fg);
        let pos = Pos2::new(ic.x + icon_w / 2.0 + 8.0, t.rect.center().y - galley.size().y / 2.0);
        ui.painter().galley_with_override_text_color(pos, galley, fg);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    resp
}

/// Controle segmentado com "polegar" que desliza até a opção escolhida.
/// Devolve `true` quando a escolha mudou.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = palette(ui);
    let font = TextStyle::Button.resolve(ui.style());
    let bold = FontId::new(font.size, bold_family());
    let pad_x = 16.0;
    let h = 34.0;
    let inset = 3.0;
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), bold.clone(), p.text).size().x + pad_x * 2.0)
        .collect();
    let total = widths.iter().sum::<f32>() + inset * 2.0;
    let (track, _) = ui.allocate_exact_size(egui::vec2(total, h + inset * 2.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(track, CornerRadius::same(255), p.bg);
    painter.rect_stroke(track, CornerRadius::same(255), Stroke::new(1.0, p.border), egui::StrokeKind::Inside);

    let sel = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let x_of = |i: usize| track.left() + inset + widths[..i].iter().sum::<f32>();
    let base = egui::Id::unique(("segmented", id));
    let tx = ui.ctx().animate_value_with_time(base.with("x"), x_of(sel), 0.18);
    let tw = ui.ctx().animate_value_with_time(base.with("w"), widths[sel], 0.18);
    let thumb = Rect::from_min_size(Pos2::new(tx, track.top() + inset), egui::vec2(tw, h));
    painter.add(Shadow { offset: [0, 1], blur: 6, spread: 0, color: p.shadow.gamma_multiply(0.8) }.as_shape(thumb, CornerRadius::same(255)));
    painter.rect_filled(thumb, CornerRadius::same(255), p.raised);
    painter.rect_stroke(thumb, CornerRadius::same(255), Stroke::new(1.0, p.border_strong), egui::StrokeKind::Inside);

    let mut changed = false;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x_of(i), track.top() + inset), egui::vec2(widths[i], h));
        let resp = ui.interact(r, base.with(i), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let selected = i == sel;
        let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered() && !selected, 0.12);
        let press = ui.ctx().animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on(), 0.05);
        if hover > 0.0 && !selected {
            painter.rect_filled(r.shrink(1.0), CornerRadius::same(255), mix(Color32::TRANSPARENT, p.surface_hi, hover));
        }
        let color = if selected { p.text } else { mix(p.muted, p.text, hover) };
        let f = if selected { bold.clone() } else { font.clone() };
        let g = painter.layout_no_wrap(label.to_string(), f, color);
        painter.galley(r.center() - g.size() / 2.0 + egui::vec2(0.0, press), g, color);
        if resp.has_focus() {
            painter.rect_stroke(r.expand(1.0), CornerRadius::same(255), Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
        }
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, *label));
        if resp.clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Interruptor animado (liga/desliga) com rótulo opcional (vazio = só o interruptor).
pub fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let p = palette(ui);
    let w = egui::vec2(38.0, 22.0);
    let galley = (!label.is_empty()).then(|| ui.painter().layout_no_wrap(label.to_string(), TextStyle::Body.resolve(ui.style()), p.text));
    let text_w = galley.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let size = egui::vec2(w.x + text_w, w.y.max(galley.as_ref().map_or(0.0, |g| g.size().y)).max(24.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click());
    resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.12);
    let press = ui.ctx().animate_bool_with_time(resp.id.with("p"), resp.is_pointer_button_down_on(), 0.05);
    let track = Rect::from_min_size(egui::pos2(rect.min.x, rect.center().y - w.y / 2.0), w);
    let off = mix(p.surface_hi, p.border, hover * 0.8);
    let on_c = mix(p.accent, p.accent_hover, hover);
    ui.painter().rect_filled(track, CornerRadius::same(255), mix(off, on_c, t));
    ui.painter().rect_stroke(track, CornerRadius::same(255), Stroke::new(1.0, mix(p.border_strong, on_c, t)), egui::StrokeKind::Inside);
    // O botão estica um pouco ao pressionar (como no iOS/Win11).
    let knob_r = 8.0;
    let stretch = press * 4.0;
    let x = egui::lerp(track.left() + 11.0..=track.right() - 11.0 - stretch, t);
    let knob = Rect::from_min_max(Pos2::new(x - knob_r, track.center().y - knob_r), Pos2::new(x + knob_r + stretch, track.center().y + knob_r));
    ui.painter().add(Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(knob, CornerRadius::same(255)));
    ui.painter().rect_filled(knob, CornerRadius::same(255), if *on { p.on_accent } else { mix(p.muted, p.text, hover) });
    if resp.has_focus() {
        ui.painter().rect_stroke(track.expand(2.5), CornerRadius::same(255), Stroke::new(2.0, p.accent), egui::StrokeKind::Outside);
    }
    if let Some(g) = galley {
        ui.painter().galley(egui::pos2(track.right() + 10.0, rect.center().y - g.size().y / 2.0), g, p.text);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *on, label));
    resp
}

/// Selo com ponto que pulsa quando `busy`.
pub fn status_badge(ui: &mut egui::Ui, color: Color32, text: &str, busy: bool) {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface_hi)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin { left: 10, right: 12, top: 5, bottom: 5 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                if busy {
                    let t = (ui.input(|i| i.time) * 1.6).fract() as f32;
                    ui.painter().circle_filled(rect.center(), 3.5 + 5.0 * t, color.gamma_multiply(0.45 * (1.0 - t)));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
                }
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(egui::RichText::new(text).size(13.0).color(p.text));
            });
        });
}

/// Indicador à esquerda de uma linha de acordeão.
#[derive(Clone, Copy, PartialEq)]
pub enum RowStatus {
    None,
    Busy,
    Ok,
    Warn,
    Err,
}

fn paint_status(ui: &egui::Ui, p: &Palette, status: RowStatus, c: Pos2) {
    match status {
        RowStatus::None => {}
        RowStatus::Busy => {
            egui::Spinner::new().size(12.0).color(p.accent).paint_at(ui, Rect::from_center_size(c, Vec2::splat(12.0)));
        }
        RowStatus::Ok | RowStatus::Warn | RowStatus::Err => {
            let col = match status {
                RowStatus::Ok => p.ok,
                RowStatus::Warn => p.warn,
                _ => p.err,
            };
            ui.painter().circle_filled(c, 6.0, col.gamma_multiply(0.18));
            ui.painter().circle_filled(c, 3.2, col);
        }
    }
}

fn paint_chevron(painter: &egui::Painter, c: Pos2, open: f32, color: Color32) {
    // Seta ">" que gira 90° ao abrir.
    let a = open * std::f32::consts::FRAC_PI_2;
    let (s, k) = a.sin_cos();
    let rot = |x: f32, y: f32| c + Vec2::new(x * k - y * s, x * s + y * k);
    let st = Stroke::new(1.6, color);
    painter.line(vec![rot(-2.0, -4.0), rot(2.0, 0.0), rot(-2.0, 4.0)], st);
}

/// Linha clicável de acordeão no estilo de barra lateral (chevron, status, título com
/// reticências e informação à direita), com hover, pressão e foco animados. O corpo
/// abre/fecha animado. Devolve a resposta da linha.
#[allow(clippy::too_many_arguments)]
pub fn disclosure(
    ui: &mut egui::Ui,
    id: egui::Id,
    default_open: bool,
    status: RowStatus,
    title: &str,
    meta: &str,
    strong: bool,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let p = palette(ui);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    let h = if strong { 34.0 } else { 30.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        state.toggle(ui);
    }
    let open = state.openness(ui.ctx());
    if ui.is_rect_visible(rect) {
        let t = tactile(ui, &resp, true);
        let r = CornerRadius::same(8);
        let fill = mix(p.surface, p.surface_hi, (t.hover * 0.9 + open * 0.35).min(1.0));
        let fill = mix(fill, p.border, t.press * 0.6);
        ui.painter().rect_filled(t.rect, r, fill);
        if t.hover > 0.01 {
            ui.painter().rect_stroke(t.rect, r, Stroke::new(1.0, p.border.gamma_multiply(t.hover)), egui::StrokeKind::Inside);
        }
        focus_ring(ui, &resp, t.rect, r);
        let cy = t.rect.center().y;
        let mut x = t.rect.left() + 12.0;
        paint_chevron(ui.painter(), Pos2::new(x, cy), open, mix(p.subtle, p.text, t.hover));
        x += 14.0;
        if status != RowStatus::None {
            paint_status(ui, &p, status, Pos2::new(x + 5.0, cy));
            x += 18.0;
        }
        let meta_g = ui.painter().layout_no_wrap(meta.to_string(), FontId::proportional(11.5), p.subtle);
        let right = t.rect.right() - 10.0;
        if !meta.is_empty() {
            ui.painter().galley(Pos2::new(right - meta_g.size().x, cy - meta_g.size().y / 2.0), meta_g.clone(), p.subtle);
        }
        let max_w = (right - meta_g.size().x - 10.0 - x).max(20.0);
        let font = if strong { FontId::new(13.5, bold_family()) } else { FontId::proportional(13.0) };
        let mut job = egui::text::LayoutJob::simple_singleline(title.to_string(), font, p.text);
        job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.painter().layout_job(job);
        ui.painter().galley(Pos2::new(x, cy - g.size().y / 2.0), g, p.text);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
    state.show_body_unindented(ui, body);
    resp
}

/// Indicador "digitando…" (três pontos que ondulam).
pub fn typing_dots(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 14.0), egui::Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    for i in 0..3 {
        let phase = (t * 5.0 - i as f32 * 0.7).sin() * 0.5 + 0.5;
        let c = Pos2::new(rect.left() + 6.0 + i as f32 * 11.0, rect.center().y - phase * 3.0);
        ui.painter().circle_filled(c, 3.2, color.gamma_multiply(0.45 + 0.55 * phase));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f32 {
        let f = |x: u8| {
            let x = x as f32 / 255.0;
            if x <= 0.03928 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn ratio(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_contrast_meets_wcag_aa() {
        for p in [DARK, LIGHT] {
            for bg in [p.bg, p.surface, p.surface_hi, p.raised] {
                assert!(ratio(p.text, bg) >= 4.5);
                assert!(ratio(p.muted, bg) >= 4.5);
                assert!(ratio(p.accent, bg) >= 4.5);
            }
            assert!(ratio(p.text, p.user_bubble) >= 4.5);
            assert!(ratio(p.on_accent, p.accent) >= 4.5);
        }
    }
}
