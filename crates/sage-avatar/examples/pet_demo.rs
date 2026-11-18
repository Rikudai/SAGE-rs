//! Demo: mostra o slime e troca de estado com o teclado.
//!
//! Teclas: 1 falando, 2 ouvindo, 3 pensando, 4 orquestrando, 0 ocioso,
//! F troca de forma, E emoção, W solta um clone, X termina um clone (ok), Z termina com falha.

use eframe::egui;
use sage_avatar::PetView;
use sage_core::config::Config;
use sage_core::events::{AppStatus, SageEvent};
use sage_core::llm::{Tier, Usage, WorkerStatus};

struct Demo {
    pet: PetView,
    forms: [&'static str; 3],
    form_i: usize,
    workers: Vec<String>,
    counter: u32,
}

impl Demo {
    fn status(&mut self, s: AppStatus) {
        self.pet.on_event(&SageEvent::Status { status: s, detail: String::new() });
    }
}

impl eframe::App for Demo {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let keys = ctx.input(|i| {
            [
                (egui::Key::Num0, i.key_pressed(egui::Key::Num0)),
                (egui::Key::Num1, i.key_pressed(egui::Key::Num1)),
                (egui::Key::Num2, i.key_pressed(egui::Key::Num2)),
                (egui::Key::Num3, i.key_pressed(egui::Key::Num3)),
                (egui::Key::Num4, i.key_pressed(egui::Key::Num4)),
                (egui::Key::F, i.key_pressed(egui::Key::F)),
                (egui::Key::E, i.key_pressed(egui::Key::E)),
                (egui::Key::W, i.key_pressed(egui::Key::W)),
                (egui::Key::X, i.key_pressed(egui::Key::X)),
                (egui::Key::Z, i.key_pressed(egui::Key::Z)),
            ]
        });
        for (key, down) in keys {
            if !down {
                continue;
            }
            match key {
                egui::Key::Num0 => self.status(AppStatus::Idle),
                egui::Key::Num1 => self.status(AppStatus::Speaking),
                egui::Key::Num2 => self.status(AppStatus::Listening),
                egui::Key::Num3 => self.status(AppStatus::Thinking),
                egui::Key::Num4 => self.status(AppStatus::Orchestrating),
                egui::Key::F => {
                    self.form_i = (self.form_i + 1) % self.forms.len();
                    self.pet.on_event(&SageEvent::Form { name: self.forms[self.form_i].into() });
                }
                egui::Key::E => self.pet.on_event(&SageEvent::Emotion { name: "feliz".into() }),
                egui::Key::W => {
                    self.counter += 1;
                    let w = format!("w{}", self.counter);
                    self.workers.push(w.clone());
                    self.pet.on_event(&SageEvent::WorkerStart {
                        run_id: "demo".into(),
                        worker: w,
                        role: "demo".into(),
                        model: "demo".into(),
                        tier: Tier::Cheap,
                        brief: String::new(),
                    });
                }
                egui::Key::X | egui::Key::Z => {
                    if let Some(w) = self.workers.pop() {
                        let status = if key == egui::Key::X { WorkerStatus::Done } else { WorkerStatus::Failed };
                        self.pet.on_event(&SageEvent::WorkerEnd {
                            run_id: "demo".into(),
                            worker: w,
                            status,
                            summary: String::new(),
                            usage: Usage::default(),
                        });
                    }
                }
                _ => {}
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("Slime em janela própria. 0-4 estados · F forma · E emoção · W/X/Z clones");
        });
        self.pet.show(ctx);
    }
}

fn main() -> eframe::Result {
    let cfg = Config::default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 80.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Demo do slime",
        options,
        Box::new(move |cc| {
            let pet = PetView::new(&cc.egui_ctx, &cfg);
            Ok(Box::new(Demo { pet, forms: ["slime", "humano", "alado"], form_i: 0, workers: vec![], counter: 0 }))
        }),
    )
}
