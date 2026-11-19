//! Auto-atualização do S.A.G.E. (porte do `sage/self_update.py` do SAGE antigo).
//!
//! * `sage_update`: traz do GitHub o que há de novo (`git pull --ff-only`), compila
//!   (`cargo build --release`) e instala o `sage.exe` novo ao lado do que está rodando.
//!   Se a versão nova não compilar, volta ao commit anterior (`git reset --keep`).
//!   Sem Rust no PC, baixa o `sage.exe` pronto do release `sage-latest` do GitHub.
//! * `sage_restart`: reabre o S.A.G.E. (a versão instalada passa a valer).
//! * `sage_publish`: commit + push das mudanças locais, conferindo antes que nenhuma
//!   chave de API vai junto, e sobe o `sage.exe` para o release `sage-latest`.
//!
//! O que veio na última atualização fica em `data/ultima_atualizacao.json`, para ele
//! saber contar as novidades (`update_context`).

use async_trait::async_trait;
use regex::Regex;
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Tag do release do GitHub que guarda o `sage.exe` pronto.
pub const RELEASE_TAG: &str = "sage-latest";

#[derive(Debug, Clone, Default)]
pub struct UpdateStatus {
    pub branch: String,
    pub head: String,
    /// Títulos dos commits que estão no GitHub e ainda não aqui.
    pub incoming: Vec<String>,
    /// Commits locais que ainda não foram para o GitHub.
    pub ahead: u32,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateResult {
    pub ok: bool,
    pub message: String,
    pub changed: bool,
    pub restart_needed: bool,
}

impl UpdateResult {
    fn fail(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), ..Default::default() }
    }
}

// ------------------------------------------------------------------ processos

fn no_window(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Roda um programa e devolve a saída (stdout) ou a última linha do erro.
pub(crate) async fn run(exe: &Path, args: &[&str], cwd: &Path, timeout: Duration, path_extra: &[PathBuf]) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).current_dir(cwd).stdin(std::process::Stdio::null()).kill_on_drop(true);
    if !path_extra.is_empty() {
        let mut paths: Vec<PathBuf> = path_extra.to_vec();
        if let Some(p) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&p));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    no_window(&mut cmd);
    let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return Err(format!("{name} {} demorou demais", args.first().unwrap_or(&""))),
        Ok(Err(e)) => return Err(format!("não consegui rodar o {name}: {e}")),
        Ok(Ok(o)) => o,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| l.starts_with("error") || l.contains("fatal") || l.contains("rror"))
        .last()
        .or_else(|| stderr.lines().map(str::trim).filter(|l| !l.is_empty()).last())
        .unwrap_or("falhou")
        .chars()
        .take(400)
        .collect::<String>();
    Err(format!("{name} {}: {last}", args.first().unwrap_or(&"")))
}

fn git_exe() -> PathBuf {
    which::which("git").unwrap_or_else(|_| PathBuf::from("git"))
}

pub(crate) async fn git(root: &Path, args: &[&str], timeout_secs: u64) -> Result<String, String> {
    run(&git_exe(), args, root, Duration::from_secs(timeout_secs), &[]).await
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// O `cargo` e as pastas que o build precisa no PATH (o toolchain gnu usa o MinGW).
pub(crate) fn cargo_env() -> Option<(PathBuf, Vec<PathBuf>)> {
    let mut extra = Vec::new();
    for p in [PathBuf::from(r"C:\mingw64\bin"), PathBuf::from(r"C:\msys64\mingw64\bin")] {
        if p.is_dir() {
            extra.push(p);
        }
    }
    let cargo_bin = home().map(|h| h.join(".cargo").join("bin"));
    if let Some(b) = cargo_bin.clone().filter(|b| b.is_dir()) {
        extra.push(b);
    }
    let exe = which::which("cargo")
        .ok()
        .or_else(|| cargo_bin.map(|b| b.join(if cfg!(windows) { "cargo.exe" } else { "cargo" })).filter(|p| p.is_file()))?;
    Some((exe, extra))
}

fn gh_exe() -> Option<PathBuf> {
    which::which("gh").ok().or_else(|| {
        let p = PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe");
        p.is_file().then_some(p)
    })
}

// ------------------------------------------------------------------ status / update

pub async fn status(root: &Path, fetch_timeout_secs: u64) -> Result<UpdateStatus, String> {
    if !root.join(".git").exists() {
        return Err("esta pasta não é um clone do repositório (sem .git)".into());
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"], 30).await?;
    if branch == "HEAD" {
        return Err("não estou em nenhum ramo (HEAD solto)".into());
    }
    let head = git(root, &["rev-parse", "HEAD"], 30).await?;
    git(root, &["fetch", "--quiet", "origin", &branch], fetch_timeout_secs).await?;
    let upstream = format!("origin/{branch}");
    let incoming = git(root, &["log", "--format=%s", &format!("HEAD..{upstream}")], 30)
        .await?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let ahead = git(root, &["rev-list", "--count", &format!("{upstream}..HEAD")], 30).await?.trim().parse().unwrap_or(0);
    Ok(UpdateStatus { branch, head, incoming, ahead })
}

/// Compila o app (`cargo build --release -p sage`) e devolve o exe gerado.
async fn build(root: &Path) -> Result<PathBuf, String> {
    let (cargo, extra) = cargo_env().ok_or("o Rust (cargo) não está instalado neste PC")?;
    run(&cargo, &["build", "--release", "-p", "sage"], root, Duration::from_secs(40 * 60), &extra).await?;
    let exe = root.join("target").join("release").join(if cfg!(windows) { "sage.exe" } else { "sage" });
    if exe.is_file() { Ok(exe) } else { Err("o build terminou mas o sage.exe não apareceu".into()) }
}

/// Sem Rust: baixa o `sage.exe` do release `sage-latest` (precisa do `gh` logado).
async fn download_release(root: &Path) -> Result<PathBuf, String> {
    let gh = gh_exe().ok_or("não tenho Rust nem o GitHub CLI (gh) para pegar a versão nova")?;
    let dir = root.join("target").join("download");
    std::fs::create_dir_all(&dir).map_err(|e| format!("não consegui criar {}: {e}", dir.display()))?;
    let d = dir.to_string_lossy().into_owned();
    run(&gh, &["release", "download", RELEASE_TAG, "-p", "sage.exe", "-D", &d, "--clobber"], root, Duration::from_secs(600), &[])
        .await?;
    let exe = dir.join("sage.exe");
    if exe.is_file() { Ok(exe) } else { Err("o download terminou mas o sage.exe não veio".into()) }
}

/// Põe o exe novo em `<raiz>/sage.exe`. O que está rodando é renomeado para `sage.old.exe`
/// (o Windows deixa renomear um exe aberto, mas não sobrescrever).
pub fn install_exe(root: &Path, built: &Path) -> Result<PathBuf, String> {
    let dest = root.join(if cfg!(windows) { "sage.exe" } else { "sage" });
    if same_file(built, &dest) {
        return Ok(dest);
    }
    if dest.exists() {
        let old = root.join("sage.old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&dest, &old).map_err(|e| format!("não consegui tirar o sage.exe antigo do lugar: {e}"))?;
    }
    std::fs::copy(built, &dest).map_err(|e| format!("não consegui copiar o sage.exe novo: {e}"))?;
    Ok(dest)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Procura, baixa, compila e instala. `restart_needed` quando um exe novo foi instalado.
pub async fn apply(root: &Path) -> UpdateResult {
    let st = match status(root, 120).await {
        Ok(s) => s,
        Err(e) => return UpdateResult::fail(format!("Não consegui procurar atualização: {e}.")),
    };
    if st.incoming.is_empty() {
        return UpdateResult { ok: true, message: "Já estou na versão mais nova do GitHub.".into(), ..Default::default() };
    }
    let old = st.head.clone();
    // Os dados locais (memória, humor) não vêm do GitHub, então o fast-forward passa sem mexer neles.
    if let Err(e) = git(root, &["pull", "--ff-only", "--quiet", "origin", &st.branch], 300).await {
        let why = if st.ahead > 0 {
            format!(" Tenho {} commit(s) locais que não estão no GitHub; publique-os primeiro (sage_publish).", st.ahead)
        } else {
            String::new()
        };
        return UpdateResult::fail(format!("Não consegui aplicar a atualização ({e}). Nada mudou.{why}"));
    }
    let built = match build(root).await {
        Ok(p) => Ok(p),
        Err(e) if cargo_env().is_none() => download_release(root).await.map_err(|d| format!("{e}; {d}")),
        Err(e) => Err(e),
    };
    let built = match built {
        Ok(p) => p,
        Err(e) => {
            let _ = git(root, &["reset", "--keep", "--quiet", &old], 60).await;
            return UpdateResult::fail(format!("A versão nova não compilou ({e}). Voltei para a anterior."));
        }
    };
    if let Err(e) = install_exe(root, &built) {
        return UpdateResult::fail(format!("Baixei e compilei, mas não consegui instalar: {e}."));
    }
    let new = git(root, &["rev-parse", "HEAD"], 30).await.unwrap_or_default();
    remember_update(root, &old, &new, &st.incoming);
    let n = st.incoming.len();
    let titles = st.incoming.iter().take(3).cloned().collect::<Vec<_>>().join("; ");
    UpdateResult {
        ok: true,
        message: format!(
            "Atualizei: {n} {} ({titles}). A versão nova vale quando eu reiniciar.",
            if n == 1 { "novidade" } else { "novidades" }
        ),
        changed: true,
        restart_needed: true,
    }
}

fn last_update_file(root: &Path) -> PathBuf {
    root.join("data").join("ultima_atualizacao.json")
}

fn remember_update(root: &Path, old: &str, new: &str, incoming: &[String]) {
    let v = json!({
        "quando": chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        "de": old.chars().take(10).collect::<String>(),
        "para": new.chars().take(10).collect::<String>(),
        "novidades": incoming,
    });
    let path = last_update_file(root);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default());
    log_line(root, &format!("{} -> {}: {} novidade(s)", &v["de"], &v["para"], incoming.len()));
}

/// Uma linha em `logs/atualizacao.log`.
pub fn log_line(root: &Path, text: &str) {
    use std::io::Write;
    let dir = root.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("atualizacao.log")) {
        let _ = writeln!(f, "{} {text}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

static ABOUT_UPDATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)atualiz|novidade|vers[aã]o|mudou|mudan[cç]a|changelog|o que (tem|h[aá]|veio) de novo")
        .expect("regex novidades")
});

/// Bloco para o prompt quando o pedido é sobre novidades ("" nos outros casos).
pub fn update_context(root: &Path, query: &str) -> String {
    if query.trim().is_empty() || !ABOUT_UPDATES.is_match(query) {
        return String::new();
    }
    let Ok(raw) = std::fs::read_to_string(last_update_file(root)) else { return String::new() };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return String::new() };
    let items: Vec<String> = v["novidades"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).take(15).map(|s| format!("- {s}")).collect())
        .unwrap_or_default();
    if items.is_empty() {
        return String::new();
    }
    format!(
        "SUA ÚLTIMA ATUALIZAÇÃO ({}), o que mudou em você (títulos dos commits; explique em linguagem simples, sem jargão de git):\n{}",
        v["quando"].as_str().unwrap_or(""),
        items.join("\n")
    )
}

// ------------------------------------------------------------------ reiniciar

/// Reabre o S.A.G.E. daqui a `delay` (dá tempo da resposta sair) e encerra este processo.
/// O processo novo herda as permissões deste (se este roda como administrador, o novo também).
pub fn schedule_restart(root: &Path, delay: Duration) {
    let exe = {
        let installed = root.join(if cfg!(windows) { "sage.exe" } else { "sage" });
        if installed.is_file() { installed } else { std::env::current_exe().unwrap_or(installed) }
    };
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        log_line(&root, &format!("reiniciando com {}", exe.display()));
        let mut cmd = std::process::Command::new(&exe);
        cmd.current_dir(&root).arg("--depois-de-reiniciar");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        }
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => log_line(&root, &format!("não consegui reabrir: {e}")),
        }
    });
}

// ------------------------------------------------------------------ publicar

static KEY_SHAPES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(sk-[A-Za-z0-9_\-]{20,}|gsk_[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_\-]{30,}|AQ\.[A-Za-z0-9_\-]{30,}|ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|xox[bp]-[A-Za-z0-9\-]{20,})")
        .expect("regex chaves")
});

/// Valores que parecem chaves no `config/config.json` (as chaves reais deste PC).
pub(crate) fn configured_secrets(root: &Path) -> Vec<String> {
    fn walk(v: &Value, key: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, v)| walk(v, k, out)),
            Value::Array(a) => a.iter().for_each(|v| walk(v, key, out)),
            Value::String(s) => {
                let k = key.to_lowercase();
                if (k.contains("key") || k.contains("token") || k.contains("secret") || k.contains("password"))
                    && s.trim().len() >= 12
                {
                    out.push(s.trim().to_string());
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(root.join("config").join("config.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}')) {
            walk(&v, "", &mut out);
        }
    }
    out
}

/// Arquivos do diff (linhas adicionadas) que contêm uma chave. Ignora o cofre criptografado.
pub fn leaked_files(diff: &str, secrets: &[String]) -> Vec<String> {
    let mut file = String::new();
    let mut hits: Vec<String> = Vec::new();
    for line in diff.lines() {
        if let Some(f) = line.strip_prefix("+++ b/") {
            file = f.to_string();
            continue;
        }
        if !line.starts_with('+') || line.starts_with("+++") || file.ends_with(".enc") {
            continue;
        }
        let bad = KEY_SHAPES.is_match(line) || secrets.iter().any(|s| line.contains(s.as_str()));
        if bad && !hits.contains(&file) {
            hits.push(file.clone());
        }
    }
    hits
}

pub async fn publish(root: &Path, message: &str, release: bool) -> Result<String, String> {
    if git(root, &["check-ignore", "-q", "config/config.json"], 30).await.is_err() {
        return Err("o config/config.json (com as chaves) não está no .gitignore; não publico assim".into());
    }
    let changed = git(root, &["status", "--porcelain"], 60).await?;
    let mut parts = Vec::new();
    if !changed.trim().is_empty() {
        git(root, &["add", "-A"], 120).await?;
        let diff = git(root, &["diff", "--cached", "--no-color", "--unified=0"], 120).await?;
        let leaks = leaked_files(&diff, &configured_secrets(root));
        if !leaks.is_empty() {
            let _ = git(root, &["reset", "--quiet"], 60).await;
            return Err(format!(
                "parei: estes arquivos têm chave de API e não vão para o GitHub: {}. Para levar as chaves, use o cofre (scripts/chaves-exportar.ps1)",
                leaks.join(", ")
            ));
        }
        let msg = if message.trim().is_empty() { "Atualização do S.A.G.E." } else { message.trim() };
        let full = format!("{msg}\n\nPublicado pelo próprio S.A.G.E.");
        git(root, &["commit", "--quiet", "-m", &full], 120).await?;
        parts.push(format!("commit \"{msg}\""));
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"], 30).await?;
    let ahead = git(root, &["rev-list", "--count", &format!("origin/{branch}..HEAD")], 30).await.unwrap_or_default();
    if ahead.trim() != "0" {
        git(root, &["push", "--quiet", "origin", &branch], 300).await?;
        parts.push(format!("push de {} commit(s) para origin/{branch}", ahead.trim()));
    }
    if release {
        match upload_release(root).await {
            Ok(m) => parts.push(m),
            Err(e) => parts.push(format!("release não atualizado ({e})")),
        }
    }
    if parts.is_empty() {
        return Ok("Nada para publicar: o GitHub já tem tudo.".into());
    }
    Ok(format!("Publiquei: {}.", parts.join("; ")))
}

/// Sobe o `sage.exe` instalado para o release `sage-latest` (cria o release se faltar).
async fn upload_release(root: &Path) -> Result<String, String> {
    let gh = gh_exe().ok_or("sem o GitHub CLI (gh)")?;
    let exe = root.join("sage.exe");
    if !exe.is_file() {
        return Err("não há sage.exe instalado".into());
    }
    let t = Duration::from_secs(900);
    let head = git(root, &["rev-parse", "--short", "HEAD"], 30).await.unwrap_or_default();
    if run(&gh, &["release", "view", RELEASE_TAG], root, Duration::from_secs(60), &[]).await.is_err() {
        let notes = "Executável pronto do S.A.G.E. (Windows x64). O sage_update baixa daqui quando o PC não tem Rust.";
        run(&gh, &["release", "create", RELEASE_TAG, "--title", "S.A.G.E. (última versão)", "--notes", notes, "--latest"], root, t, &[])
            .await?;
    }
    let e = exe.to_string_lossy().into_owned();
    run(&gh, &["release", "upload", RELEASE_TAG, &e, "--clobber"], root, t, &[]).await?;
    Ok(format!("sage.exe ({head}) no release {RELEASE_TAG}"))
}

// ------------------------------------------------------------------ ferramentas

pub struct UpdateTools {
    pub root: PathBuf,
}

impl UpdateTools {
    pub fn all_specs() -> Vec<ToolSpec> {
        let s = |name: &str, desc: &str, args: Value| ToolSpec {
            name: name.into(),
            description: desc.into(),
            args,
            mutates: true,
            needs_confirmation: false,
        };
        vec![
            s(
                "sage_update",
                "Auto-atualização do S.A.G.E.: procura a versão nova no GitHub; com apply=true baixa, compila e instala (volta atrás se não compilar). Depois chame sage_restart para valer.",
                json!({"apply": "bool? padrão true; false só verifica"}),
            ),
            s("sage_restart", "Reinicia o S.A.G.E. (a versão instalada passa a valer). Avise antes de chamar.", json!({})),
            s(
                "sage_publish",
                "Sobe o projeto do S.A.G.E. para o GitHub: commit das mudanças locais + push, sem chaves de API, e atualiza o sage.exe do release.",
                json!({"message": "string? mensagem do commit", "release": "bool? padrão true"}),
            ),
        ]
    }
}

#[async_trait]
impl Toolbox for UpdateTools {
    fn specs(&self) -> Vec<ToolSpec> {
        Self::all_specs()
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        match name {
            "sage_update" => {
                let apply_it = args.get("apply").and_then(Value::as_bool).unwrap_or(true);
                if !apply_it {
                    return match status(&self.root, 60).await {
                        Ok(st) if st.incoming.is_empty() => ToolResult::ok("Estou na versão mais nova.", "Estou na versão mais nova do GitHub."),
                        Ok(st) => {
                            let msg = format!("Tem {} novidade(s) no GitHub: {}.", st.incoming.len(), st.incoming.join("; "));
                            ToolResult::ok(msg.clone(), msg)
                        }
                        Err(e) => ToolResult::err(format!("Não consegui verificar: {e}.")),
                    };
                }
                let r = apply(&self.root).await;
                log_line(&self.root, &format!("[ferramenta] {}", r.message));
                if r.ok { ToolResult::ok(r.message.clone(), r.message) } else { ToolResult::err(r.message) }
            }
            "sage_restart" => {
                schedule_restart(&self.root, Duration::from_secs(6));
                ToolResult::ok("Reiniciando em alguns segundos.", "Vou reiniciar em 6 segundos.")
            }
            "sage_publish" => {
                let msg = args.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                let release = args.get("release").and_then(Value::as_bool).unwrap_or(true);
                match publish(&self.root, &msg, release).await {
                    Ok(m) => ToolResult::ok(m.clone(), m),
                    Err(e) => ToolResult::err(format!("Não publiquei: {e}.")),
                }
            }
            _ => ToolResult::err(format!("Ferramenta desconhecida: '{name}'.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leak_scan_finds_real_and_shaped_keys_only_in_added_lines() {
        // Chave falsa montada em tempo de execução (o literal não fica no código).
        let fake = format!("{}_{}", "gsk", "abcdefghijklmnopqrstuvwxyz");
        let diff = format!(
            "diff --git a/x.rs b/x.rs\n+++ b/x.rs\n+let k = \"{fake}\";\n-old {fake}\n+++ b/notes.md\n+minha chave super-secreta-1234567\n+++ b/config/secrets.enc\n+{fake}\n+++ b/ok.md\n+nada aqui\n"
        );
        let hits = leaked_files(&diff, &["super-secreta-1234567".to_string()]);
        assert_eq!(hits, vec!["x.rs".to_string(), "notes.md".to_string()]);
    }

    #[test]
    fn update_context_only_for_update_questions() {
        let dir = std::env::temp_dir().join(format!("sage-upd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        remember_update(&dir, "aaaaaaaaaaaa", "bbbbbbbbbbbb", &["Voz nova".into(), "Slime mais rápido".into()]);
        assert!(update_context(&dir, "abre o spotify").is_empty());
        let c = update_context(&dir, "o que tem de novo na sua versão?");
        assert!(c.contains("- Voz nova") && c.contains("- Slime mais rápido"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_exe_moves_the_running_one_aside() {
        let dir = std::env::temp_dir().join(format!("sage-inst-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("t")).unwrap();
        std::fs::write(dir.join("sage.exe"), b"velho").unwrap();
        std::fs::write(dir.join("t").join("sage.exe"), b"novo").unwrap();
        let dest = install_exe(&dir, &dir.join("t").join("sage.exe")).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"novo");
        assert_eq!(std::fs::read(dir.join("sage.old.exe")).unwrap(), b"velho");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
