//! Ícone na bandeja: abrir o chat, mostrar/esconder o slime, sair.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction {
    Open,
    TogglePet,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    open: MenuItem,
    pet: MenuItem,
    quit: MenuItem,
}

impl Tray {
    pub fn new() -> Option<Self> {
        let open = MenuItem::new("Abrir o S.A.G.E.", true, None);
        let pet = MenuItem::new("Mostrar/esconder o slime", true, None);
        let quit = MenuItem::new("Sair", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &pet, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = load_icon()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("S.A.G.E.")
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .build()
            .ok()?;
        Some(Self { _icon: tray, open, pet, quit })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        let mut out = vec![];
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == *self.open.id() {
                out.push(TrayAction::Open);
            } else if ev.id == *self.pet.id() {
                out.push(TrayAction::TogglePet);
            } else if ev.id == *self.quit.id() {
                out.push(TrayAction::Quit);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                out.push(TrayAction::Open);
            }
        }
        out
    }
}

fn load_icon() -> Option<Icon> {
    let bytes = std::fs::read(sage_core::paths::assets().join("app_icon.png")).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}
