//! Where S.A.G.E. keeps its files. The root is the folder that holds `config/`,
//! found from the executable upward (so `target/release/sage.exe` works in dev),
//! or `SAGE_HOME` when set.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn root() -> &'static Path {
    ROOT.get_or_init(|| {
        if let Ok(home) = std::env::var("SAGE_HOME") {
            return PathBuf::from(home);
        }
        let mut dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        loop {
            if dir.join("config").is_dir() && dir.join("assets").is_dir() {
                return dir;
            }
            if !dir.pop() {
                return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            }
        }
    })
}

pub fn config_file() -> PathBuf {
    root().join("config").join("config.json")
}
pub fn playbooks_file() -> PathBuf {
    root().join("config").join("playbooks.yaml")
}
pub fn assets() -> PathBuf {
    root().join("assets")
}
pub fn data() -> PathBuf {
    root().join("data")
}
pub fn runs() -> PathBuf {
    data().join("runs")
}
pub fn logs() -> PathBuf {
    root().join("logs")
}
