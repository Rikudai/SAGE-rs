//! `SpriteLibrary`: carrega quadros PNG sob demanda como texturas egui.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::anim::{Timing, parse_frame_name};
use crate::state::FrameInfo;

pub struct SpriteLibrary {
    root: PathBuf,
    pub timing: Timing,
    /// Contagem de quadros por diretório de escopo ("forms/slime", "clone") e animação.
    counts: RefCell<HashMap<String, HashMap<String, usize>>>,
    textures: RefCell<HashMap<(String, String, usize), Option<TextureHandle>>>,
}

fn scope_dir(scope: &str) -> String {
    if scope == "clone" { "clone".into() } else { format!("forms/{scope}") }
}

impl SpriteLibrary {
    /// `root` = pasta `assets/sprites`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let timing = std::fs::read_to_string(root.join("sprite_meta.json"))
            .map(|t| Timing::from_meta_json(&t))
            .unwrap_or_default();
        Self { root, timing, counts: RefCell::default(), textures: RefCell::default() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn scan(&self, dir: &str) -> HashMap<String, usize> {
        let mut out: HashMap<String, usize> = HashMap::new();
        let Ok(rd) = std::fs::read_dir(self.root.join(dir)) else {
            return out;
        };
        for entry in rd.flatten() {
            let name = entry.file_name();
            if let Some((anim, idx)) = name.to_str().and_then(parse_frame_name) {
                let e = out.entry(anim).or_insert(0);
                *e = (*e).max(idx + 1);
            }
        }
        out
    }

    /// Texturas do quadro `idx` (carrega na primeira vez; falha de leitura é lembrada).
    pub fn texture(&self, ctx: &Context, scope: &str, anim: &str, idx: usize) -> Option<TextureHandle> {
        let key = (scope.to_string(), anim.to_string(), idx);
        if let Some(t) = self.textures.borrow().get(&key) {
            return t.clone();
        }
        let path = self.root.join(scope_dir(scope)).join(format!("{anim}_{idx}.png"));
        let tex = match load_image(&path) {
            Ok(img) => Some(ctx.load_texture(format!("{scope}/{anim}_{idx}"), img, TextureOptions::LINEAR)),
            Err(e) => {
                tracing::warn!("sprite {}: {e}", path.display());
                None
            }
        };
        self.textures.borrow_mut().insert(key, tex.clone());
        tex
    }
}

impl FrameInfo for SpriteLibrary {
    fn frame_count(&self, scope: &str, anim: &str) -> usize {
        let dir = scope_dir(scope);
        if !self.counts.borrow().contains_key(&dir) {
            let scanned = self.scan(&dir);
            self.counts.borrow_mut().insert(dir.clone(), scanned);
        }
        self.counts.borrow().get(&dir).and_then(|m| m.get(anim)).copied().unwrap_or(0)
    }
}

fn load_image(path: &Path) -> anyhow::Result<ColorImage> {
    let img = image::open(path)?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_frames_from_directory() {
        let dir = std::env::temp_dir().join(format!("sage_avatar_test_{}", std::process::id()));
        let forms = dir.join("forms").join("slime");
        let _ = std::fs::create_dir_all(&forms);
        for n in ["idle_0.png", "idle_1.png", "idle_2.png", "bronca_clone_0.png", "lixo.txt"] {
            let _ = std::fs::write(forms.join(n), b"");
        }
        let lib = SpriteLibrary::new(&dir);
        assert_eq!(lib.frame_count("slime", "idle"), 3);
        assert_eq!(lib.frame_count("slime", "bronca_clone"), 1);
        assert_eq!(lib.frame_count("slime", "walk"), 0);
        assert_eq!(lib.frame_count("humano", "idle"), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_dirs() {
        assert_eq!(scope_dir("clone"), "clone");
        assert_eq!(scope_dir("alado"), "forms/alado");
    }
}
