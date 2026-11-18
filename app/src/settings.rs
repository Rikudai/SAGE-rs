//! Página de configurações nativa. Edita um rascunho e grava no config/config.json.

use sage_core::config::{CloudProvider, ModelRef};
use crate::theme;
use sage_core::{paths, Config};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Geral,
    Voz,
    Modelos,
    Cerebro,
    Slime,
}

pub struct SettingsPage {
    draft: Config,
    tab: Tab,
    msg: String,
    show_keys: bool,
    new_provider: String,
}

impl SettingsPage {
    pub fn new(cfg: &Config) -> Self {
        Self { draft: cfg.clone(), tab: Tab::Geral, msg: String::new(), show_keys: false, new_provider: String::new() }
    }

    /// Desenha a página; devolve a config nova quando o usuário salva.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Config> {
        let p = theme::palette(ui);
        ui.label(theme::bold("Configurações").size(theme::size::HEADING).color(p.text));
        ui.label(theme::muted(ui, "Tudo fica no config/config.json, o mesmo arquivo do SAGE antigo."));
        ui.add_space(14.0);
        theme::segmented(
            ui,
            "settings-tabs",
            &mut self.tab,
            &[
                (Tab::Geral, "Geral"),
                (Tab::Voz, "Voz"),
                (Tab::Modelos, "Modelos e chaves"),
                (Tab::Cerebro, "Cérebro"),
                (Tab::Slime, "Slime"),
            ],
        );
        ui.add_space(14.0);
        let (title, desc) = match self.tab {
            Tab::Geral => ("Geral", "Seu nome, atalhos globais e o que o S.A.G.E. pode fazer no PC."),
            Tab::Voz => ("Voz", "Como ele fala (Fish Audio, com Edge TTS de reserva) e como ele ouve você."),
            Tab::Modelos => ("Modelos e chaves", "Chaves de API e modelos por papel. As chaves ficam só neste PC."),
            Tab::Cerebro => ("Cérebro", "O lead planeja e workers mais baratos executam. Ajuste perfil e limites."),
            Tab::Slime => ("Slime", "O avatar na área de trabalho."),
        };
        let footer_h = 64.0;
        egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink(false).show(ui, |ui| {
            ui.set_max_width(860.0);
            theme::card(ui).inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, title, desc);
                egui::Grid::new("settings").num_columns(2).spacing([28.0, 14.0]).min_col_width(230.0).show(ui, |ui| {
                    match self.tab {
                        Tab::Geral => self.geral(ui),
                        Tab::Voz => self.voz(ui),
                        Tab::Modelos => self.modelos(ui),
                        Tab::Cerebro => self.cerebro(ui),
                        Tab::Slime => self.slime(ui),
                    }
                });
            });
            if self.tab == Tab::Modelos {
                ui.add_space(12.0);
                theme::card(ui).inner_margin(egui::Margin::same(22)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    self.providers(ui);
                });
            }
            ui.add_space(8.0);
        });
        ui.add_space(6.0);
        let mut saved = None;
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if theme::primary(ui, "Salvar alterações").clicked() {
                match self.draft.save(&paths::config_file()) {
                    Ok(()) => {
                        self.msg = "Salvo. Atalhos e voz passam a valer quando o S.A.G.E. reiniciar.".into();
                        saved = Some(self.draft.clone());
                    }
                    Err(e) => self.msg = format!("Não consegui salvar o config.json ({e}). Feche outros programas que estejam usando o arquivo e tente de novo."),
                }
            }
            if theme::button(ui, theme::Kind::Secondary, "Descartar alterações").clicked() {
                if let Ok(c) = Config::load_default() {
                    self.draft = c;
                }
                self.msg = "Alterações descartadas.".into();
            }
            if !self.msg.is_empty() {
                let color = if self.msg.starts_with("Não") { p.err } else { p.muted };
                ui.label(egui::RichText::new(self.msg.clone()).size(13.0).color(color));
            }
        });
        saved
    }

    fn geral(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        let mut name = d.user_name.clone().unwrap_or_default();
        field_label(ui, "Seu nome");
        if ui.text_edit_singleline(&mut name).changed() {
            d.user_name = Some(name).filter(|n| !n.trim().is_empty());
        }
        ui.end_row();
        check(ui, "Ações no PC (ferramentas)", &mut d.enable_actions);
        check(ui, "PowerShell liberado", &mut d.enable_shell_tools);
        field_label(ui, "Atalho: segurar para falar");
        ui.text_edit_singleline(&mut d.hotkey_listen);
        ui.end_row();
        field_label(ui, "Atalho: abrir o chat");
        ui.text_edit_singleline(&mut d.hotkey_panel);
        ui.end_row();
        check(ui, "Sons de evento", &mut d.enable_sfx);
    }

    fn voz(&mut self, ui: &mut egui::Ui) {
        let show = self.show_keys;
        let d = &mut self.draft;
        check(ui, "Voz ligada", &mut d.voice_enabled);
        check(ui, "Cumprimentar ao iniciar", &mut d.speak_greetings);
        check(ui, "Pausar música/vídeo enquanto falo ou ouço", &mut d.pause_media_while_speaking);
        combo(ui, "Provedor de voz", &mut d.tts_provider, &["fish_audio", "edge"]);
        secret(ui, "Fish Audio: chave", &mut d.fish_audio_api_key, show);
        text(ui, "Fish Audio: voz (reference_id)", &mut d.fish_audio_reference_id);
        text(ui, "Fish Audio: modelo", &mut d.fish_audio_model);
        text(ui, "Edge TTS: voz", &mut d.voice_name);
        text(ui, "Edge TTS: velocidade", &mut d.voice_rate);
        text(ui, "Edge TTS: tom", &mut d.voice_pitch);
        combo(ui, "Perfil de voz", &mut d.voice_profile, &["rimuru", "sabio", "classica"]);
        let mut wake = d.wake_words.join(", ");
        field_label(ui, "Palavras de ativação");
        if ui.text_edit_singleline(&mut wake).changed() {
            d.wake_words = wake.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
        ui.end_row();
        list(ui, "Modelos de transcrição", &mut d.transcribe_models);
    }

    fn modelos(&mut self, ui: &mut egui::Ui) {
        check(ui, "Mostrar chaves", &mut self.show_keys);
        let show = self.show_keys;
        let d = &mut self.draft;
        secret(ui, "Gemini (AI Studio): chave", &mut d.gemini_api_key, show);
        text(ui, "Gemini: modelo rápido", &mut d.gemini_model);
        text(ui, "Gemini: visão", &mut d.vision_model);
        text(ui, "Gemini: código", &mut d.code_model);
        model_ref(ui, "Cadeia: modelo rápido", &mut d.model_config.fast);
        model_ref(ui, "Cadeia: cérebro", &mut d.model_config.brain);
        check(ui, "Claude Code como worker", &mut d.claude_enabled);
        text(ui, "Claude: modelo de agente", &mut d.claude_agent_model);
        check(ui, "Antigravity (agy) como worker", &mut d.antigravity_enabled);
        text(ui, "agy: caminho do executável", &mut d.agy_path);
        text(ui, "agy: modelo", &mut d.antigravity_model);
    }

    fn providers(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Provedores de nuvem", "Groq, OpenRouter, Cerebras e outros compatíveis com a API da OpenAI.");
        let show = self.show_keys;
        let mut remove = None;
        for (id, p) in self.draft.cloud_providers.iter_mut() {
            egui::CollapsingHeader::new(format!("{id} {}", if p.enabled { "· ligado" } else { "· desligado" }))
                .id_salt(id)
                .show(ui, |ui| {
                    egui::Grid::new(format!("prov-{id}")).num_columns(2).show(ui, |ui| {
                        check(ui, "Ligado", &mut p.enabled);
                        secret(ui, "Chave", &mut p.api_key, show);
                        text(ui, "Modelo", &mut p.model);
                        list(ui, "Modelos reserva", &mut p.fallback_models);
                        let mut url = p.base_url.clone().unwrap_or_default();
                        field_label(ui, "Base URL (opcional)");
                        if ui.text_edit_singleline(&mut url).changed() {
                            p.base_url = Some(url).filter(|u| !u.trim().is_empty());
                        }
                        ui.end_row();
                    });
                    if theme::button(ui, theme::Kind::Danger, "Remover provedor").clicked() {
                        remove = Some(id.clone());
                    }
                });
        }
        if let Some(id) = remove {
            self.draft.cloud_providers.remove(&id);
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_provider).hint_text("id do provedor, ex.: groq").desired_width(220.0));
            if theme::icon_text_button(ui, theme::Kind::Secondary, theme::Icon::Plus, "Adicionar provedor").clicked() && !self.new_provider.trim().is_empty() {
                self.draft
                    .cloud_providers
                    .insert(self.new_provider.trim().to_lowercase(), CloudProvider { enabled: true, ..Default::default() });
                self.new_provider.clear();
            }
        });
    }

    fn cerebro(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        let mut profile = d.brain_profile.clone().unwrap_or_else(|| "base".into());
        combo(ui, "Perfil", &mut profile, &["base", "savings", "aggressive"]);
        d.brain_profile = Some(profile);
        let mut max_workers = d.brain_max_workers.unwrap_or(6);
        field_label(ui, "Máximo de workers por execução");
        ui.add(egui::DragValue::new(&mut max_workers).range(1..=12));
        d.brain_max_workers = Some(max_workers);
        ui.end_row();
        let mut cost = d.brain_max_cost_usd.unwrap_or(0.0);
        field_label(ui, "Teto de custo por execução (US$, 0 = sem teto)");
        ui.add(egui::DragValue::new(&mut cost).range(0.0..=50.0).speed(0.05));
        d.brain_max_cost_usd = Some(cost).filter(|c| *c > 0.0);
        ui.end_row();
        field_label(ui, "Playbooks");
        if theme::button(ui, theme::Kind::Secondary, "Abrir playbooks.yaml").clicked() {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "", &paths::playbooks_file().to_string_lossy()])
                .spawn();
        }
        ui.end_row();
    }

    fn slime(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        check(ui, "Mostrar o slime na tela", &mut d.enable_desktop_pet);
        check(ui, "Andar pela tela", &mut d.enable_pet_walking);
        check(ui, "Vontade própria", &mut d.pet_free_will);
        combo(ui, "Forma", &mut d.pet_form, &["slime", "humano", "alado"]);
        combo(ui, "Personalidade", &mut d.pet_personality, &["brincalhao", "calmo", "curioso", "dorminhoco", "serio"]);
        slider(ui, "Tamanho", &mut d.pet_scale, 0.5..=2.0, 1.0);
        slider(ui, "Opacidade", &mut d.pet_opacity, 0.1..=1.0, 1.0);
        slider(ui, "Atividade", &mut d.pet_activity, 0.0..=1.0, 0.6);
        slider(ui, "Raio da coleira (px)", &mut d.pet_leash_radius, 0.0..=800.0, 260.0);
    }
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    let p = theme::palette(ui);
    ui.label(egui::RichText::new(text).color(p.muted));
}

fn check(ui: &mut egui::Ui, text: &str, v: &mut bool) {
    field_label(ui, text);
    theme::switch(ui, v, "").widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *v, text));
    ui.end_row();
}

fn text(ui: &mut egui::Ui, label: &str, v: &mut String) {
    field_label(ui, label);
    ui.add(egui::TextEdit::singleline(v).desired_width(360.0));
    ui.end_row();
}

fn secret(ui: &mut egui::Ui, label: &str, v: &mut String, show: bool) {
    field_label(ui, label);
    ui.add(egui::TextEdit::singleline(v).password(!show).desired_width(360.0));
    ui.end_row();
}

fn list(ui: &mut egui::Ui, label: &str, v: &mut Vec<String>) {
    let mut s = v.join(", ");
    field_label(ui, label);
    if ui.add(egui::TextEdit::singleline(&mut s).desired_width(360.0)).changed() {
        *v = s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    }
    ui.end_row();
}

fn combo(ui: &mut egui::Ui, label: &str, v: &mut String, options: &[&str]) {
    field_label(ui, label);
    egui::ComboBox::from_id_salt(label).selected_text(v.clone()).show_ui(ui, |ui| {
        for o in options {
            ui.selectable_value(v, o.to_string(), *o);
        }
    });
    ui.end_row();
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut Option<f32>, range: std::ops::RangeInclusive<f32>, default: f32) {
    let mut x = v.unwrap_or(default);
    field_label(ui, label);
    if ui.add(egui::Slider::new(&mut x, range)).changed() {
        *v = Some(x);
    }
    ui.end_row();
}

fn model_ref(ui: &mut egui::Ui, label: &str, m: &mut ModelRef) {
    field_label(ui, label);
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut m.provider).desired_width(90.0).hint_text("provedor"));
        ui.add(egui::TextEdit::singleline(&mut m.model).desired_width(260.0).hint_text("modelo"));
    });
    ui.end_row();
}
