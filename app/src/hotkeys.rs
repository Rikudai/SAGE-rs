//! Atalhos globais: `hotkey_listen` (segurar para falar) e `hotkey_panel` (mostra o chat).

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

pub enum HotkeyAction {
    TalkDown,
    TalkUp,
    TogglePanel,
}

pub struct Hotkeys {
    _manager: GlobalHotKeyManager,
    talk: Option<u32>,
    panel: Option<u32>,
}

pub fn parse(spec: &str) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    let mut code = None;
    for part in spec.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" => mods |= Modifiers::ALT,
            "win" | "super" | "meta" => mods |= Modifiers::SUPER,
            "space" => code = Some(Code::Space),
            "enter" => code = Some(Code::Enter),
            "tab" => code = Some(Code::Tab),
            "esc" | "escape" => code = Some(Code::Escape),
            k if k.len() == 1 => {
                let c = k.chars().next()?;
                code = Some(match c {
                    'a'..='z' => format!("Key{}", c.to_ascii_uppercase()).parse().ok()?,
                    '0'..='9' => format!("Digit{c}").parse().ok()?,
                    _ => return None,
                });
            }
            k if k.starts_with('f') => code = format!("F{}", &k[1..]).parse().ok(),
            _ => return None,
        }
    }
    Some(HotKey::new(Some(mods).filter(|m| !m.is_empty()), code?))
}

impl Hotkeys {
    pub fn register(talk: &str, panel: &str) -> Option<Self> {
        let manager = GlobalHotKeyManager::new().ok()?;
        let reg = |spec: &str, fallback: &str| -> Option<u32> {
            for s in [spec, fallback] {
                if let Some(hk) = parse(s) {
                    if manager.register(hk).is_ok() {
                        tracing::info!("atalho registrado: {s}");
                        return Some(hk.id());
                    }
                    tracing::warn!("atalho {s} em uso por outro programa");
                }
            }
            None
        };
        let talk = reg(talk, "ctrl+shift+space");
        let panel = reg(panel, "alt+j");
        Some(Self { _manager: manager, talk, panel })
    }

    pub fn poll(&self) -> Vec<HotkeyAction> {
        let mut out = vec![];
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if Some(ev.id) == self.talk {
                out.push(match ev.state {
                    HotKeyState::Pressed => HotkeyAction::TalkDown,
                    HotKeyState::Released => HotkeyAction::TalkUp,
                });
            } else if Some(ev.id) == self.panel && ev.state == HotKeyState::Pressed {
                out.push(HotkeyAction::TogglePanel);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_combos() {
        assert!(parse("ctrl+shift+space").is_some());
        assert!(parse("alt+j").is_some());
        assert!(parse("f9").is_some());
        assert!(parse("ctrl+???").is_none());
    }
}
