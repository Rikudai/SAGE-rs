//! S.A.G.E. — app nativo: chat, configurações, bandeja, atalhos, voz e slime.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkeys;
mod runtime;
mod settings;
mod theme;
mod tray;
mod ui;

use anyhow::Result;
use sage_core::{paths, Config};
use std::sync::{Arc, RwLock};

fn init_logging() {
    let dir = paths::logs();
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("sage.log"));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,wgpu=warn,eframe=warn,egui=warn".into());
    match file {
        Ok(f) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(f))
            .init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

fn load_icon() -> Option<egui::IconData> {
    let bytes = std::fs::read(paths::assets().join("app_icon.png")).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

fn main() -> Result<()> {
    init_logging();
    let cfg = Config::load_default().unwrap_or_else(|e| {
        tracing::error!("config.json inválido ({e}); usando padrões");
        Config::default()
    });
    let cfg = Arc::new(RwLock::new(cfg));
    let rt = runtime::Runtime::start(cfg.clone())?;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("S.A.G.E.")
        .with_inner_size([980.0, 680.0])
        .with_min_inner_size([560.0, 420.0]);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "S.A.G.E.",
        options,
        Box::new(move |cc| Ok(Box::new(ui::SageApp::new(cc, cfg, rt)))),
    )
    .map_err(|e| anyhow::anyhow!("falha ao abrir a janela: {e}"))
}
