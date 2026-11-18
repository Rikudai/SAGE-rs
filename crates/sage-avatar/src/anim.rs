//! Nomes de quadros e tempos das animações (lógica pura, sem egui).

use std::collections::HashMap;

/// Tempo padrão por quadro (ms) quando a animação não está na tabela.
pub const DEFAULT_FRAME_MS: u64 = 70;

/// Divide `idle_12.png` em `("idle", 12)`. O índice é o que vem depois do ÚLTIMO `_`,
/// então `bronca_clone_3.png` vira `("bronca_clone", 3)`.
pub fn parse_frame_name(file_name: &str) -> Option<(String, usize)> {
    let stem = file_name.strip_suffix(".png")?;
    let (anim, idx) = stem.rsplit_once('_')?;
    if anim.is_empty() || idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((anim.to_string(), idx.parse().ok()?))
}

/// Tempos padrão de `FRAME_MS` / `ANIM_FRAME_MS` do slime antigo.
fn builtin_ms(anim: &str) -> u64 {
    match anim {
        "idle" => 70,
        "walk" => 55,
        "run" => 45,
        "talk" => 60,
        "pensando" | "sabio" | "orquestrando" | "maestro" => 75,
        "dormindo" => 110,
        "teleport" | "teleport_in" | "teleport_out" => 40,
        "spawn" | "work" | "done" | "fail" | "vanish" => 60,
        _ => DEFAULT_FRAME_MS,
    }
}

/// Tabela de tempos; `sprite_meta.json` pode sobrescrever animações.
#[derive(Debug, Clone, Default)]
pub struct Timing {
    overrides: HashMap<String, u64>,
}

impl Timing {
    /// Lê `sprite_meta.json`. Aceita `{"frame_ms": {"idle": 80}}` ou `{"idle": 80}` direto.
    /// Conteúdo inválido é ignorado (usa só os padrões).
    pub fn from_meta_json(text: &str) -> Self {
        let mut overrides = HashMap::new();
        if let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text) {
            let map = match root.get("frame_ms") {
                Some(serde_json::Value::Object(m)) => m.clone(),
                _ => root,
            };
            for (k, v) in map {
                if let Some(ms) = v.as_u64().filter(|ms| *ms > 0) {
                    overrides.insert(k, ms);
                }
            }
        }
        Self { overrides }
    }

    pub fn frame_ms(&self, anim: &str) -> u64 {
        self.overrides.get(anim).copied().unwrap_or_else(|| builtin_ms(anim))
    }
}

/// Animações que ficam em laço; todas as outras tocam uma vez e terminam.
pub fn is_looping(anim: &str) -> bool {
    matches!(
        anim,
        "idle" | "walk" | "run" | "talk" | "ouvindo" | "pensando" | "sabio" | "orquestrando"
            | "maestro" | "dormindo" | "arrastado" | "work"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_and_underscored_names() {
        assert_eq!(parse_frame_name("idle_0.png"), Some(("idle".into(), 0)));
        assert_eq!(parse_frame_name("idle_39.png"), Some(("idle".into(), 39)));
        assert_eq!(parse_frame_name("bronca_clone_3.png"), Some(("bronca_clone".into(), 3)));
    }

    #[test]
    fn rejects_bad_names() {
        assert_eq!(parse_frame_name("idle.png"), None);
        assert_eq!(parse_frame_name("idle_x.png"), None);
        assert_eq!(parse_frame_name("idle_1.jpg"), None);
        assert_eq!(parse_frame_name("_1.png"), None);
    }

    #[test]
    fn builtin_timings() {
        let t = Timing::default();
        assert_eq!(t.frame_ms("idle"), 70);
        assert_eq!(t.frame_ms("run"), 45);
        assert_eq!(t.frame_ms("dormindo"), 110);
        assert_eq!(t.frame_ms("qualquer_outra"), DEFAULT_FRAME_MS);
    }

    #[test]
    fn meta_overrides_both_shapes() {
        let a = Timing::from_meta_json(r#"{"frame_ms":{"idle":90}}"#);
        assert_eq!(a.frame_ms("idle"), 90);
        assert_eq!(a.frame_ms("walk"), 55);
        let b = Timing::from_meta_json(r#"{"walk":30,"idle":0}"#);
        assert_eq!(b.frame_ms("walk"), 30);
        assert_eq!(b.frame_ms("idle"), 70);
        assert_eq!(Timing::from_meta_json("lixo").frame_ms("idle"), 70);
    }

    #[test]
    fn looping_set() {
        assert!(is_looping("idle"));
        assert!(!is_looping("acenar"));
        assert!(!is_looping("morph_out"));
    }
}
