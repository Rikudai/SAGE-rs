//! Interpretação de combinações de teclas ("ctrl+s") para códigos VK. Lógica pura.

/// Converte um nome de tecla para o código virtual (VK) do Windows.
pub fn vk_for(name: &str) -> Option<u16> {
    let n = name.trim().to_lowercase();
    let vk = match n.as_str() {
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" | "menu" => 0x12,
        "win" | "windows" | "super" | "cmd" => 0x5B,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "tab" => 0x09,
        "space" | "espaco" | "espaço" => 0x20,
        "backspace" | "bksp" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "capslock" => 0x14,
        "printscreen" | "prtsc" => 0x2C,
        "volumemute" => 0xAD,
        "volumedown" => 0xAE,
        "volumeup" => 0xAF,
        "nexttrack" => 0xB0,
        "prevtrack" => 0xB1,
        "playpause" => 0xB3,
        _ => {
            let b = n.as_bytes();
            if b.len() == 1 && b[0].is_ascii_lowercase() {
                // 'A'..'Z' têm VK igual ao ASCII maiúsculo.
                u16::from(b[0].to_ascii_uppercase())
            } else if b.len() == 1 && b[0].is_ascii_digit() {
                u16::from(b[0])
            } else if let Some(num) = n.strip_prefix('f').and_then(|r| r.parse::<u16>().ok()) {
                if (1..=12).contains(&num) { 0x70 + num - 1 } else { return None }
            } else {
                return None;
            }
        }
    };
    Some(vk)
}

/// "ctrl+shift+s" -> [0x11, 0x10, 0x53]. Erro em português se alguma tecla for desconhecida.
pub fn parse_combo(keys: &str) -> Result<Vec<u16>, String> {
    let parts: Vec<&str> = keys.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("Nenhuma tecla informada.".into());
    }
    parts
        .iter()
        .map(|p| vk_for(p).ok_or_else(|| format!("Tecla desconhecida: '{p}'.")))
        .collect()
}

/// Comando de mídia -> VK.
pub fn media_vk(command: &str) -> Option<u16> {
    match command.trim().to_lowercase().as_str() {
        "play_pause" => Some(0xB3),
        "next" => Some(0xB0),
        "prev" => Some(0xB1),
        "volume_up" => Some(0xAF),
        "volume_down" => Some(0xAE),
        "mute" => Some(0xAD),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combo() {
        assert_eq!(parse_combo("ctrl+s").unwrap(), vec![0x11, 0x53]);
        assert_eq!(parse_combo("Win + D").unwrap(), vec![0x5B, 0x44]);
        assert_eq!(parse_combo("alt+f4").unwrap(), vec![0x12, 0x73]);
        assert_eq!(parse_combo("enter").unwrap(), vec![0x0D]);
    }

    #[test]
    fn rejects_unknown() {
        assert!(parse_combo("ctrl+blah").is_err());
        assert!(parse_combo("").is_err());
        assert!(parse_combo("f13").is_err());
    }

    #[test]
    fn media_map() {
        assert_eq!(media_vk("mute"), Some(0xAD));
        assert_eq!(media_vk("explodir"), None);
    }
}
