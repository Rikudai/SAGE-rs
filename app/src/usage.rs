//! Página "Uso": cota restante de cada provedor (janelas de sessão, semana, mês) e o
//! consumo de cada modelo nas execuções do cérebro.
//!
//! Fontes: `%APPDATA%\Alethe\alethe-usage.json` (Claude, Codex e Antigravity, lido das
//! próprias CLIs), `data/quota.json` (limites que as APIs devolveram ao SAGE) e
//! `data/runs/*/report.json` (tokens e custo por modelo).

use crate::theme::{self, Palette};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use egui::{Color32, CornerRadius, Margin, Pos2, Rect, RichText, Stroke};
use sage_core::Config;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Window {
    pub label: String,
    pub left_pct: f32,
    pub resets_at: Option<DateTime<Utc>>,
    /// Duração da janela (para a marca de "ritmo" na barra).
    pub span: Option<ChronoDuration>,
}

#[derive(Clone)]
pub struct ProviderUsage {
    pub id: String,
    pub name: String,
    pub windows: Vec<Window>,
    pub observed: Option<DateTime<Utc>>,
    pub note: String,
}

#[derive(Clone, Default)]
pub struct ModelUse {
    pub calls: u32,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Default)]
pub struct UsageData {
    pub providers: Vec<ProviderUsage>,
    pub models: Vec<(String, ModelUse)>,
}

pub struct UsagePage {
    data: UsageData,
    loaded_at: f64,
    refreshing: Arc<Mutex<Option<String>>>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    /// Já leu as cotas sozinho nesta sessão (ao abrir a aba com dados de mais de 1 h).
    auto_done: bool,
}

fn alethe_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("Alethe")
}

fn provider_name(id: &str) -> String {
    match id {
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "agy" => "Antigravity".into(),
        "gemini" => "Gemini".into(),
        "groq" => "Groq".into(),
        "openrouter" => "OpenRouter".into(),
        "cerebras" => "Cerebras".into(),
        "mistral" => "Mistral".into(),
        "nvidia" => "NVIDIA".into(),
        "sambanova" => "SambaNova".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn window_meta(key: &str) -> (String, Option<ChronoDuration>, u8) {
    // (rótulo, duração, ordem)
    match key {
        "5h" => ("Sessão".into(), Some(ChronoDuration::hours(5)), 0),
        "week" => ("Semanal".into(), Some(ChronoDuration::days(7)), 1),
        "opus" => ("Semanal (Opus)".into(), Some(ChronoDuration::days(7)), 2),
        "month" | "monthly" => ("Mensal".into(), Some(ChronoDuration::days(30)), 3),
        "3p-5h" => ("Sessão (modelos de terceiros)".into(), Some(ChronoDuration::hours(5)), 4),
        "3p-week" => ("Semanal (modelos de terceiros)".into(), Some(ChronoDuration::days(7)), 5),
        "limit" => ("Limite da API".into(), None, 6),
        other => (other.to_string(), None, 7),
    }
}

fn parse_time(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc))
}

/// Janela que já renovou volta a 100%.
fn window_from(key: &str, used: f64, resets_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> (u8, Window) {
    let (label, span, order) = window_meta(key);
    let renewed = resets_at.is_some_and(|r| r <= now);
    let left = if renewed { 100.0 } else { (100.0 - used).clamp(0.0, 100.0) } as f32;
    (order, Window { label, left_pct: left, resets_at: resets_at.filter(|r| *r > now), span })
}

pub fn load(cfg: &Config) -> UsageData {
    let now = Utc::now();
    let mut map: BTreeMap<String, ProviderUsage> = BTreeMap::new();
    fn add(map: &mut BTreeMap<String, ProviderUsage>, id: &str, mut w: Vec<(u8, Window)>, observed: Option<DateTime<Utc>>, note: String) {
        w.sort_by_key(|(o, _)| *o);
        map.insert(
            id.to_string(),
            ProviderUsage { id: id.to_string(), name: provider_name(id), windows: w.into_iter().map(|(_, w)| w).collect(), observed, note },
        );
    }

    // Assinaturas (lidas pelo Alethe das próprias CLIs).
    if let Ok(raw) = std::fs::read_to_string(alethe_dir().join("alethe-usage.json"))
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(provs) = v.get("providers").and_then(Value::as_object)
    {
        for (id, p) in provs {
            let mut ws = Vec::new();
            if let Some(windows) = p.get("windows").and_then(Value::as_object) {
                for (k, w) in windows {
                    if k == "argon" {
                        continue;
                    }
                    let used = w.get("usedPercent").and_then(Value::as_f64).unwrap_or(0.0);
                    ws.push(window_from(k, used, parse_time(w.get("resetsAt")), now));
                }
            }
            let src = p.get("source").and_then(Value::as_str).unwrap_or("").to_string();
            add(&mut map, id, ws, parse_time(p.get("observedAt")), format!("lido via {src}"));
        }
    }

    // APIs: limites observados pelo SAGE (429 e cotas esgotadas).
    if let Ok(raw) = std::fs::read_to_string(sage_core::paths::data().join("quota.json"))
        && let Ok(v) = serde_json::from_str::<Value>(&raw)
        && let Some(provs) = v.get("providers").and_then(Value::as_object)
    {
        for (id, p) in provs {
            if map.contains_key(id) {
                continue;
            }
            let mut ws = Vec::new();
            let mut observed = None;
            if let Some(windows) = p.get("windows").and_then(Value::as_object) {
                for (k, w) in windows {
                    let used = w.get("used_pct").and_then(Value::as_f64).unwrap_or(0.0);
                    observed = observed.max(parse_time(w.get("observed_at")));
                    ws.push(window_from(k, used, parse_time(w.get("resets_at")), now));
                }
            }
            let n = p.get("rate_limits").and_then(Value::as_u64).unwrap_or(0);
            add(&mut map, id, ws, observed, format!("{n} limites atingidos registrados"));
        }
    }

    // Provedores configurados sem nenhuma leitura ainda.
    let mut configured: Vec<String> = Vec::new();
    if cfg.api_key("gemini").is_some() {
        configured.push("gemini".into());
    }
    for (id, p) in &cfg.cloud_providers {
        if p.enabled && cfg.api_key(id).is_some() {
            configured.push(id.clone());
        }
    }
    if cfg.claude_enabled {
        configured.push("claude".into());
    }
    if cfg.antigravity_enabled {
        configured.push("agy".into());
    }
    for id in configured {
        if !map.contains_key(&id) {
            add(&mut map, &id, Vec::new(), None, "sem limite atingido até agora".into());
        }
    }

    // Ordem: assinaturas primeiro, depois APIs por nome.
    let rank = |id: &str| match id {
        "claude" => 0,
        "codex" => 1,
        "agy" => 2,
        _ => 3,
    };
    let mut providers: Vec<ProviderUsage> = map.into_values().collect();
    providers.sort_by(|a, b| rank(&a.id).cmp(&rank(&b.id)).then(a.name.cmp(&b.name)));

    UsageData { providers, models: load_models(now) }
}

/// Tokens e custo por modelo nas execuções dos últimos 30 dias.
fn load_models(now: DateTime<Utc>) -> Vec<(String, ModelUse)> {
    let mut by: BTreeMap<String, ModelUse> = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(sage_core::paths::runs()) else { return Vec::new() };
    for e in rd.flatten() {
        let Ok(raw) = std::fs::read_to_string(e.path().join("report.json")) else { continue };
        let Ok(r) = serde_json::from_str::<Value>(&raw) else { continue };
        if parse_time(r.get("started_at")).is_some_and(|t| now - t > ChronoDuration::days(30)) {
            continue;
        }
        for wave in r.get("waves").and_then(Value::as_array).into_iter().flatten() {
            for w in wave.get("workers").and_then(Value::as_array).into_iter().flatten() {
                let model = w.get("model").and_then(Value::as_str).unwrap_or("?");
                let model = model.split('@').next().unwrap_or(model).to_string();
                let u = w.get("usage").cloned().unwrap_or_default();
                let tok = ["input_tokens", "output_tokens"].iter().filter_map(|k| u.get(*k).and_then(Value::as_u64)).sum::<u64>();
                let m = by.entry(model).or_default();
                m.calls += 1;
                m.tokens += tok;
                m.cost += u.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
            }
        }
    }
    let mut v: Vec<(String, ModelUse)> = by.into_iter().collect();
    v.sort_by(|a, b| b.1.tokens.cmp(&a.1.tokens));
    v
}

fn fmt_left(d: ChronoDuration) -> String {
    let m = d.num_minutes().max(0);
    if m >= 24 * 60 {
        format!("{}d {}h", m / (24 * 60), (m / 60) % 24)
    } else if m >= 60 {
        format!("{}h {}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1} mi", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1} mil", n as f64 / 1e3),
        n => n.to_string(),
    }
}

fn provider_color(p: &Palette, id: &str) -> Color32 {
    match id {
        "claude" => Color32::from_rgb(0xC8, 0x6A, 0x4A),
        "codex" => Color32::from_rgb(0x9A, 0xA4, 0xB2),
        "agy" | "gemini" => Color32::from_rgb(0x5B, 0x8D, 0xF6),
        "groq" => Color32::from_rgb(0xF5, 0x7C, 0x3B),
        "openrouter" => Color32::from_rgb(0x8B, 0x7C, 0xF6),
        _ => p.accent,
    }
}

impl UsagePage {
    pub fn new() -> Self {
        Self {
            data: UsageData::default(),
            loaded_at: -1.0,
            refreshing: Arc::new(Mutex::new(None)),
            busy: Arc::default(),
            auto_done: false,
        }
    }

    /// Lê as cotas agora pelas próprias CLIs (Alethe: uma chamada curtinha ao Haiku e
    /// `agy /usage`, que não gasta tokens), em segundo plano.
    fn refresh_live(&self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let script = [
            home.join(".claude/plugins/marketplaces/alethe/skills/alethe/scripts/orchestrate.py"),
        ]
        .into_iter()
        .find(|p| p.exists());
        let (msg, busy, ctx) = (self.refreshing.clone(), self.busy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let text = match script {
                None => "Não achei o script do Alethe para ler as cotas.".to_string(),
                Some(s) => {
                    let mut cmd = std::process::Command::new("python");
                    cmd.arg(s).args(["usage", "--refresh-claude", "--refresh-agy"]);
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        cmd.creation_flags(0x0800_0000); // sem janela de console
                    }
                    match cmd.output() {
                        Ok(o) if o.status.success() => "Cotas lidas agora.".to_string(),
                        Ok(o) => format!("A leitura falhou: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")),
                        Err(e) => format!("Não consegui rodar o Python ({e})."),
                    }
                }
            };
            *msg.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text);
            busy.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cfg: &Config) {
        let p = theme::palette(ui);
        let now_t = ui.input(|i| i.time);
        let done_msg = self.refreshing.lock().ok().and_then(|mut m| m.take());
        if self.loaded_at < 0.0 || now_t - self.loaded_at > 30.0 || done_msg.is_some() {
            self.data = load(cfg);
            self.loaded_at = now_t;
        }
        if !self.auto_done {
            self.auto_done = true;
            let newest = self.data.providers.iter().filter(|p| matches!(p.id.as_str(), "claude" | "agy")).filter_map(|p| p.observed).max();
            if newest.is_none_or(|t| Utc::now() - t > ChronoDuration::hours(1)) {
                self.refresh_live(ui.ctx());
            }
        }
        let busy = self.busy.load(std::sync::atomic::Ordering::SeqCst);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(theme::bold("Uso e cotas").size(theme::size::HEADING).color(p.text));
                ui.label(theme::muted(ui, "Quanto resta de cada provedor e quanto cada modelo gastou nas execuções."));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if busy { "Lendo…" } else { "Ler cotas agora" };
                if ui.add_enabled_ui(!busy, |ui| theme::icon_text_button(ui, theme::Kind::Secondary, theme::Icon::Refresh, label)).inner.clicked() {
                    self.refresh_live(ui.ctx());
                }
                if busy {
                    ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                }
            });
        });
        if let Some(m) = done_msg {
            ui.label(RichText::new(m).size(12.5).color(p.muted));
        }
        ui.add_space(12.0);
        let now = Utc::now();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let col = ui.available_width().min(980.0);
            let pad = ((ui.available_width() - col) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    ui.set_width(col);
                    for prov in &self.data.providers {
                        provider_section(ui, &p, prov, now);
                        ui.add_space(14.0);
                    }
                    if !self.data.models.is_empty() {
                        models_section(ui, &p, &self.data.models);
                    }
                });
            });
        });
        // Os relógios de "renova em" andam sozinhos.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));
    }
}

fn provider_icon(ui: &mut egui::Ui, color: Color32, name: &str) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 9.0, color.gamma_multiply(0.22));
    ui.painter().text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        name.chars().next().unwrap_or('?').to_string(),
        egui::FontId::new(11.0, theme::bold_family()),
        color,
    );
}

fn provider_section(ui: &mut egui::Ui, p: &Palette, prov: &ProviderUsage, now: DateTime<Utc>) {
    let color = provider_color(p, &prov.id);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        provider_icon(ui, color, &prov.name);
        ui.label(theme::bold(&prov.name).size(15.0).color(p.text));
        let when = prov.observed.map(|t| format!(" · atualizado há {}", fmt_left(now - t))).unwrap_or_default();
        ui.label(RichText::new(format!("{}{when}", prov.note)).size(11.5).color(p.subtle));
    });
    ui.add_space(6.0);
    if prov.windows.is_empty() {
        window_card(ui, p, color, &Window { label: "Sem leitura de cota".into(), left_pct: 100.0, resets_at: None, span: None }, now, true);
        return;
    }
    for w in &prov.windows {
        window_card(ui, p, color, w, now, false);
        ui.add_space(6.0);
    }
}

fn window_card(ui: &mut egui::Ui, p: &Palette, color: Color32, w: &Window, now: DateTime<Utc>, unknown: bool) {
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin { left: 18, right: 16, top: 14, bottom: 14 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(theme::bold(&w.label).size(13.5).color(p.text));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let big = if unknown { "—".to_string() } else { format!("{:.0}%", w.left_pct) };
                        ui.label(theme::bold(big).size(28.0).color(p.text));
                        ui.label(RichText::new(if unknown { "sem dados" } else { "restante" }).size(12.5).color(p.muted));
                    });
                });
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
                bar(ui, p, color, rect, w, now, unknown);
                if let (Some(r), true) = (w.resets_at, resp.hovered()) {
                    let local = r.with_timezone(&chrono::Local).format("%d/%m %H:%M");
                    resp.on_hover_text(format!("Renova em {local}. A marca branca mostra quanto tempo ainda resta na janela: barra abaixo dela = você está gastando mais rápido que o ritmo."));
                }
            });
        });
}

fn bar(ui: &egui::Ui, p: &Palette, color: Color32, rect: Rect, w: &Window, now: DateTime<Utc>, unknown: bool) {
    let painter = ui.painter_at(rect.expand(1.0));
    let r = CornerRadius::same(7);
    painter.rect_filled(rect, r, p.bg);
    // Hachura diagonal no fundo (o que já foi gasto).
    let hatch = painter.with_clip_rect(rect.shrink(1.0));
    let mut x = rect.left() - rect.height();
    while x < rect.right() {
        hatch.line_segment(
            [Pos2::new(x, rect.bottom()), Pos2::new(x + rect.height(), rect.top())],
            Stroke::new(1.0, color.gamma_multiply(0.18)),
        );
        x += 6.0;
    }
    painter.rect_stroke(rect, r, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    if !unknown {
        let left = (w.left_pct / 100.0).clamp(0.0, 1.0);
        let anim = ui.ctx().animate_value_with_time(egui::Id::new(("usage-bar", &w.label, color)), left, 0.5);
        if anim > 0.002 {
            let fill = Rect::from_min_size(rect.min, egui::vec2(rect.width() * anim, rect.height()));
            painter.rect_filled(fill, r, color.gamma_multiply(0.55));
        }
        // Marca de ritmo: fração do tempo que ainda falta na janela.
        if let (Some(reset), Some(span)) = (w.resets_at, w.span) {
            let frac = ((reset - now).num_seconds() as f32 / span.num_seconds().max(1) as f32).clamp(0.0, 1.0);
            let x = rect.left() + rect.width() * frac;
            painter.line_segment([Pos2::new(x, rect.top() - 2.0), Pos2::new(x, rect.bottom() + 2.0)], Stroke::new(2.0, p.text));
        }
    }
    if let Some(reset) = w.resets_at {
        let txt = format!("↻ {}", fmt_left(reset - now));
        let g = painter.layout_no_wrap(txt, egui::FontId::new(11.5, theme::bold_family()), p.text);
        let chip = Rect::from_min_size(
            Pos2::new(rect.right() - g.size().x - 18.0, rect.center().y - g.size().y / 2.0 - 3.0),
            egui::vec2(g.size().x + 12.0, g.size().y + 6.0),
        );
        painter.rect_filled(chip, CornerRadius::same(5), p.bg.gamma_multiply(0.92));
        painter.galley(chip.min + egui::vec2(6.0, 3.0), g, p.text);
    }
}

fn models_section(ui: &mut egui::Ui, p: &Palette, models: &[(String, ModelUse)]) {
    ui.add_space(6.0);
    ui.label(theme::bold("Consumo por modelo").size(15.0).color(p.text));
    ui.label(RichText::new("Execuções do cérebro nos últimos 30 dias.").size(11.5).color(p.subtle));
    ui.add_space(6.0);
    let max = models.iter().map(|(_, m)| m.tokens).max().unwrap_or(1).max(1) as f32;
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (name, m) in models {
                let provider = name.split(':').next().unwrap_or("");
                let color = provider_color(p, provider);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.add_sized([260.0, 22.0], egui::Label::new(RichText::new(name).size(13.0).monospace().color(p.text)).truncate());
                    let w = (ui.available_width() - 230.0).max(60.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 8.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(4), p.bg);
                    let f = Rect::from_min_size(rect.min, egui::vec2(rect.width() * (m.tokens as f32 / max), rect.height()));
                    ui.painter().rect_filled(f, CornerRadius::same(4), color.gamma_multiply(0.7));
                    let cost = if m.cost > 0.0 { format!(" · US$ {:.2}", m.cost) } else { String::new() };
                    ui.label(RichText::new(format!("{} tokens · {} chamadas{cost}", fmt_tokens(m.tokens), m.calls)).size(12.0).color(p.muted));
                });
            }
        });
}
