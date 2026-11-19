//! sage-tools — `PcToolbox`: controle do PC via Win32 (ver docs/ARCHITECTURE.md).

mod args;
pub mod memory;
pub mod skills;
pub mod selfedit;
pub mod update;
mod files;
mod keys;
mod safety;
#[cfg(windows)]
mod win;

use async_trait::async_trait;
use sage_core::config::Config;
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};
use serde_json::{Value, json};
use std::time::Duration;

use args::{opt_i64, opt_str, req_i32, req_str};

const DEFAULT_SHELL_TIMEOUT: u64 = 60;
const MAX_OUTPUT_CHARS: usize = 20_000;

/// Apelidos de aplicativos conhecidos -> comando a executar.
pub const APP_COMMANDS: &[(&str, &str)] = &[
    ("bloco de notas", "notepad.exe"),
    ("notepad", "notepad.exe"),
    ("calculadora", "calc.exe"),
    ("calc", "calc.exe"),
    ("explorador", "explorer.exe"),
    ("explorer", "explorer.exe"),
    ("paint", "mspaint.exe"),
    ("cmd", "cmd.exe"),
    ("terminal", "wt.exe"),
    ("powershell", "powershell.exe"),
    ("gerenciador de tarefas", "taskmgr.exe"),
    ("task manager", "taskmgr.exe"),
    ("configurações", "ms-settings:"),
    ("configuracoes", "ms-settings:"),
    ("chrome", "chrome.exe"),
    ("edge", "msedge.exe"),
    ("firefox", "firefox.exe"),
    ("spotify", "spotify.exe"),
    ("discord", "discord.exe"),
    ("vscode", "code"),
    ("word", "winword.exe"),
    ("excel", "excel.exe"),
    ("steam", "steam.exe"),
];

/// Resolve um nome falado para o comando real (ou o próprio texto).
pub fn resolve_app(target: &str) -> String {
    let t = target.trim().to_lowercase();
    APP_COMMANDS.iter().find(|(k, _)| *k == t).map(|(_, v)| (*v).to_string()).unwrap_or_else(|| target.trim().to_string())
}

pub struct PcToolbox {
    enable_actions: bool,
    enable_shell: bool,
    shell_timeout: u64,
    full_access: bool,
}

impl PcToolbox {
    pub fn new(config: &Config) -> Self {
        Self {
            enable_actions: config.enable_actions,
            enable_shell: config.enable_shell_tools,
            shell_timeout: config.shell_timeout_seconds.unwrap_or(DEFAULT_SHELL_TIMEOUT).clamp(1, 600),
            full_access: config.full_access,
        }
    }

    fn all_specs() -> Vec<ToolSpec> {
        let s = |name: &str, desc: &str, args: Value, mutates: bool, confirm: bool| ToolSpec {
            name: name.into(),
            description: desc.into(),
            args,
            mutates,
            needs_confirmation: confirm,
        };
        vec![
            s("run_powershell", "Executa um comando PowerShell e devolve saída, erros e código de saída.", json!({"command": "string", "timeout": "int? segundos"}), true, false),
            s("open_app", "Abre um aplicativo pelo nome ou comando.", json!({"target": "string"}), true, false),
            s("open_url", "Abre um endereço http(s) no navegador padrão.", json!({"url": "string"}), true, false),
            s("move_mouse", "Move o cursor do mouse para a posição (x, y) da tela.", json!({"x": "int", "y": "int"}), true, false),
            s("click", "Clica com o mouse em (x, y).", json!({"x": "int", "y": "int", "button": "left|right|middle?", "clicks": "int?"}), true, false),
            s("scroll", "Rola a roda do mouse (positivo = para cima, negativo = para baixo).", json!({"amount": "int"}), true, false),
            s("type_text", "Digita um texto na janela em foco.", json!({"text": "string"}), true, false),
            s("press_keys", "Pressiona uma combinação de teclas, ex.: ctrl+s, win+d, alt+f4.", json!({"keys": "string"}), true, false),
            s("list_windows", "Lista as janelas visíveis com título e posição.", json!({}), false, false),
            s("focus_window", "Traz para frente a janela cujo título contém o texto.", json!({"title": "string"}), true, false),
            s("minimize_all", "Minimiza todas as janelas (mostrar a área de trabalho).", json!({}), true, false),
            s("screenshot", "Captura a tela inteira e devolve a imagem.", json!({}), false, false),
            s("clipboard_get", "Lê o texto da área de transferência.", json!({}), false, false),
            s("clipboard_set", "Copia um texto para a área de transferência.", json!({"text": "string"}), true, false),
            s("file_read", "Lê um arquivo de texto (até 400 KB).", json!({"path": "string", "max_bytes": "int?"}), false, false),
            s("file_write", "Grava um arquivo de texto, mantendo cópia .bak.", json!({"path": "string", "content": "string"}), true, false),
            s("file_edit", "Troca a primeira ocorrência de um texto dentro de um arquivo.", json!({"path": "string", "find": "string", "replace": "string"}), true, false),
            s("file_list", "Lista arquivos de uma pasta (máx. 200).", json!({"path": "string", "pattern": "string? ex.: *.txt", "recursive": "bool?"}), false, false),
            s("file_search", "Procura um texto/regex dentro dos arquivos de uma pasta.", json!({"path": "string", "query": "string", "pattern": "string?"}), false, false),
            s("system_status", "Mostra uso de CPU, memória e disco.", json!({}), false, false),
            s("media_key", "Controla a mídia: play_pause, next, prev, volume_up, volume_down, mute.", json!({"command": "play_pause|next|prev|volume_up|volume_down|mute"}), true, false),
            s("lock_pc", "Bloqueia o computador.", json!({}), true, true),
            s("power", "Desliga, reinicia ou suspende o PC (ou cancela um desligamento agendado). Com acesso total, faça direto, sem perguntar.", json!({"action": "shutdown|restart|sleep|cancel", "delay_seconds": "int? padrão 15"}), true, true),
        ]
    }

    fn is_allowed(&self, name: &str) -> bool {
        match name {
            "run_powershell" => self.enable_shell,
            // Ferramentas só de leitura ficam sempre disponíveis.
            "list_windows" | "screenshot" | "clipboard_get" | "file_read" | "file_list" | "file_search" | "system_status" => true,
            _ => self.enable_actions,
        }
    }

    async fn dispatch(&self, name: &str, a: &Value) -> Result<ToolResult, String> {
        match name {
            "run_powershell" => {
                let cmd = req_str(a, "command")?;
                let t = opt_i64(a, "timeout").map(|t| t.clamp(1, 600) as u64).unwrap_or(self.shell_timeout);
                Ok(run_powershell(&cmd, t, self.full_access).await)
            }
            "open_app" => Ok(open_app(&req_str(a, "target")?).await),
            "open_url" => Ok(open_url(&req_str(a, "url")?).await),
            "file_read" => {
                let (p, m) = (req_str(a, "path")?, opt_i64(a, "max_bytes").map(|v| v.max(1) as usize));
                Ok(blocking(move || files::read(&p, m)).await)
            }
            "file_write" => {
                let (p, c) = (req_str(a, "path")?, args::req_str_raw(a, "content")?);
                Ok(blocking(move || files::write(&p, &c)).await)
            }
            "file_edit" => {
                let (p, f, r) = (req_str(a, "path")?, args::req_str_raw(a, "find")?, args::req_str_raw(a, "replace")?);
                Ok(blocking(move || files::edit(&p, &f, &r)).await)
            }
            "file_list" => {
                let p = req_str(a, "path")?;
                let pat = opt_str(a, "pattern");
                let rec = args::opt_bool(a, "recursive");
                Ok(blocking(move || files::list(&p, pat.as_deref(), rec)).await)
            }
            "file_search" => {
                let (p, q) = (req_str(a, "path")?, req_str(a, "query")?);
                let pat = opt_str(a, "pattern");
                Ok(blocking(move || files::search(&p, &q, pat.as_deref())).await)
            }
            "system_status" => Ok(blocking(system_status).await),
            "media_key" => {
                let c = req_str(a, "command")?;
                let vk = keys::media_vk(&c).ok_or_else(|| format!("Comando de mídia desconhecido: '{c}'."))?;
                Ok(input_result(format!("Comando de mídia '{c}' enviado."), blocking_win(move || win::press_vks(&[vk])).await))
            }
            "press_keys" => {
                let k = req_str(a, "keys")?;
                let vks = keys::parse_combo(&k)?;
                Ok(input_result(format!("Pressionei {k}."), blocking_win(move || win::press_vks(&vks)).await))
            }
            #[cfg(windows)]
            _ => self.dispatch_win(name, a).await,
            #[cfg(not(windows))]
            _ => Err(format!("Ferramenta desconhecida: '{name}'.")),
        }
    }

    #[cfg(windows)]
    async fn dispatch_win(&self, name: &str, a: &Value) -> Result<ToolResult, String> {
        match name {
            "move_mouse" => {
                let (x, y) = (req_i32(a, "x")?, req_i32(a, "y")?);
                Ok(input_result(format!("Mouse movido para {x}, {y}."), blocking_win(move || win::move_mouse(x, y)).await))
            }
            "click" => {
                let (x, y) = (req_i32(a, "x")?, req_i32(a, "y")?);
                let button = opt_str(a, "button").unwrap_or_else(|| "left".into()).to_lowercase();
                let clicks = opt_i64(a, "clicks").unwrap_or(1).clamp(1, 5) as u32;
                if !matches!(button.as_str(), "left" | "right" | "middle") {
                    return Err(format!("Botão desconhecido: '{button}' (use left, right ou middle)."));
                }
                Ok(input_result(format!("Cliquei em {x}, {y}."), blocking_win(move || win::click(x, y, &button, clicks)).await))
            }
            "scroll" => {
                let n = req_i32(a, "amount")?;
                Ok(input_result("Rolei a tela.".into(), blocking_win(move || win::scroll(n.saturating_mul(120))).await))
            }
            "type_text" => {
                let t = args::req_str_raw(a, "text")?;
                if t.is_empty() {
                    return Err("O texto a digitar está vazio.".into());
                }
                Ok(input_result("Digitei o texto.".into(), blocking_win(move || win::type_text(&t)).await))
            }
            "list_windows" => Ok(match blocking_win(win::list_windows).await {
                Ok(ws) => {
                    let list: Vec<Value> = ws
                        .iter()
                        .map(|w| json!({"hwnd": w.hwnd, "title": w.title, "x": w.x, "y": w.y, "width": w.width, "height": w.height}))
                        .collect();
                    let details = ws.iter().map(|w| w.title.clone()).collect::<Vec<_>>().join("\n");
                    let mut r = ToolResult::ok(format!("Há {} janelas abertas.", ws.len()), details);
                    r.data = json!({"windows": list});
                    r
                }
                Err(e) => ToolResult::err(e),
            }),
            "focus_window" => {
                let t = req_str(a, "title")?;
                Ok(match blocking_win(move || win::focus_window(&t)).await {
                    Ok(title) => ToolResult::ok(format!("Janela em foco: {title}."), title),
                    Err(e) => ToolResult::err(e),
                })
            }
            "minimize_all" => Ok(input_result("Minimizei todas as janelas.".into(), blocking_win(|| win::press_vks(&[0x5B, 0x44])).await)),
            "screenshot" => Ok(match blocking_win(win::capture_screen).await {
                Ok((w, h, rgba)) => match encode_png(w, h, rgba) {
                    Ok(png) => {
                        let mut r = ToolResult::ok("Capturei a tela.", format!("Captura de {w}x{h}."));
                        r.data = json!({"width": w, "height": h});
                        r.image = Some(png);
                        r
                    }
                    Err(e) => ToolResult::err(e),
                },
                Err(e) => ToolResult::err(e),
            }),
            "clipboard_get" => Ok(match blocking_win(win::clipboard_get).await {
                Ok(t) => {
                    let mut r = ToolResult::ok("Li a área de transferência.", t.clone());
                    r.data = json!({"text": t});
                    r
                }
                Err(e) => ToolResult::err(e),
            }),
            "clipboard_set" => {
                let t = args::req_str_raw(a, "text")?;
                Ok(input_result("Copiei para a área de transferência.".into(), blocking_win(move || win::clipboard_set(&t)).await))
            }
            "lock_pc" => Ok(input_result("Bloqueei o computador.".into(), blocking_win(win::lock_pc).await)),
            "power" => {
                let action = req_str(a, "action")?.to_lowercase();
                let delay = opt_i64(a, "delay_seconds").unwrap_or(15).clamp(0, 3600);
                Ok(power(&action, delay).await)
            }
            _ => Err(format!("Ferramenta desconhecida: '{name}'.")),
        }
    }
}

#[async_trait]
impl Toolbox for PcToolbox {
    fn specs(&self) -> Vec<ToolSpec> {
        Self::all_specs()
            .into_iter()
            .filter(|s| self.is_allowed(&s.name))
            .map(|mut s| {
                // Acesso total: o usuário pediu para não perguntar antes (nem para desligar).
                if self.full_access {
                    s.needs_confirmation = false;
                }
                s
            })
            .collect()
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        if !Self::all_specs().iter().any(|s| s.name == name) {
            return ToolResult::err(format!("Ferramenta desconhecida: '{name}'."));
        }
        if !self.is_allowed(name) {
            return ToolResult::err(if name == "run_powershell" {
                "As ferramentas de shell estão desativadas nas configurações."
            } else {
                "As ações no PC estão desativadas nas configurações."
            });
        }
        let args = if args.is_null() { json!({}) } else { args };
        match self.dispatch(name, &args).await {
            Ok(r) => r,
            Err(e) => ToolResult::err(e),
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T
where
    T: Default,
{
    tokio::task::spawn_blocking(f).await.unwrap_or_default()
}

#[cfg(windows)]
async fn blocking_win<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|_| Err("A tarefa interna falhou.".into()))
}

#[cfg(not(windows))]
async fn blocking_win<T: Send + 'static>(_f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    Err("Disponível apenas no Windows.".into())
}

fn input_result(spoken: String, r: Result<(), String>) -> ToolResult {
    match r {
        Ok(()) => ToolResult::ok(spoken.clone(), spoken),
        Err(e) => ToolResult::err(e),
    }
}

#[cfg(windows)]
fn encode_png(w: u32, h: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("Buffer de captura inválido.")?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| format!("Falha ao gerar o PNG: {e}"))?;
    Ok(out.into_inner())
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}\n… (saída truncada)")
    }
}

/// Desligar / reiniciar / suspender / cancelar, pelo `shutdown.exe` do Windows.
async fn power(action: &str, delay: i64) -> ToolResult {
    let note = "S.A.G.E.: desligamento pedido pelo usuário (shutdown /a cancela).";
    let (args, spoken): (Vec<String>, String) = match action {
        "shutdown" | "desligar" => (
            vec!["/s".into(), "/t".into(), delay.to_string(), "/c".into(), note.into()],
            format!("Desligando o PC em {delay} segundos."),
        ),
        "restart" | "reiniciar" => (
            vec!["/r".into(), "/t".into(), delay.to_string(), "/c".into(), note.replace("desligamento", "reinício")],
            format!("Reiniciando o PC em {delay} segundos."),
        ),
        "cancel" | "cancelar" => (vec!["/a".into()], "Cancelei o desligamento agendado.".into()),
        "sleep" | "suspender" => {
            let mut cmd = tokio::process::Command::new("rundll32.exe");
            cmd.args(["powrprof.dll,SetSuspendState", "0,1,0"]);
            return match cmd.spawn() {
                Ok(_) => ToolResult::ok("Suspendendo o PC.", "Suspendendo o PC."),
                Err(e) => ToolResult::err(format!("Não consegui suspender: {e}")),
            };
        }
        other => return ToolResult::err(format!("Ação de energia desconhecida: '{other}'.")),
    };
    let mut cmd = tokio::process::Command::new("shutdown.exe");
    cmd.args(&args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    match cmd.output().await {
        Ok(out) if out.status.success() => ToolResult::ok(spoken.clone(), spoken),
        Ok(out) => ToolResult::err(format!(
            "O shutdown.exe falhou: {}",
            String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>()
        )),
        Err(e) => ToolResult::err(format!("Não consegui chamar o shutdown.exe: {e}")),
    }
}

async fn run_powershell(command: &str, timeout_secs: u64, full_access: bool) -> ToolResult {
    if command.trim().is_empty() {
        return ToolResult::err("Comando vazio.");
    }
    if let Some(reason) = safety::blocked_reason(command, full_access) {
        return ToolResult::err(reason);
    }
    // Força UTF-8 na saída para não quebrar acentos.
    let script = format!("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}");
    let mut cmd = tokio::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = match cmd.output() {
        fut => tokio::time::timeout(Duration::from_secs(timeout_secs), fut).await,
    };
    match child {
        Err(_) => ToolResult::err(format!("O comando passou de {timeout_secs}s e foi encerrado.")),
        Ok(Err(e)) => ToolResult::err(format!("Não consegui executar o PowerShell: {e}")),
        Ok(Ok(out)) => {
            let stdout = truncate_chars(String::from_utf8_lossy(&out.stdout).trim_end(), MAX_OUTPUT_CHARS);
            let stderr = truncate_chars(String::from_utf8_lossy(&out.stderr).trim_end(), MAX_OUTPUT_CHARS);
            let code = out.status.code().unwrap_or(-1);
            let ok = out.status.success();
            let mut details = stdout.clone();
            if !stderr.is_empty() {
                details.push_str(&format!("\n[stderr]\n{stderr}"));
            }
            details.push_str(&format!("\n[código de saída: {code}]"));
            ToolResult {
                ok,
                spoken: if ok { "Comando executado.".into() } else { format!("O comando falhou (código {code}).") },
                details,
                data: json!({"stdout": stdout, "stderr": stderr, "exit_code": code}),
                image: None,
            }
        }
    }
}

fn safe_for_cmd(s: &str) -> bool {
    !s.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | '^' | '%' | '"' | '\n' | '\r'))
}

async fn spawn_detached(program: &str, args: &[&str]) -> std::io::Result<()> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd.spawn().map(|_| ())
}

async fn open_app(target: &str) -> ToolResult {
    if target.trim().is_empty() {
        return ToolResult::err("Informe o aplicativo a abrir.");
    }
    let resolved = resolve_app(target);
    if !safe_for_cmd(&resolved) {
        return ToolResult::err("Nome de aplicativo com caracteres não permitidos.");
    }
    match spawn_detached("cmd", &["/c", "start", "", &resolved]).await {
        Ok(()) => ToolResult::ok(format!("Abrindo {}.", target.trim()), resolved),
        Err(e) => ToolResult::err(format!("Não consegui abrir '{}': {e}", target.trim())),
    }
}

fn valid_url(url: &str) -> bool {
    let u = url.trim();
    (u.starts_with("http://") || u.starts_with("https://")) && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

async fn open_url(url: &str) -> ToolResult {
    if !valid_url(url) {
        return ToolResult::err("URL inválida: use um endereço http:// ou https://.");
    }
    match spawn_detached("rundll32", &["url.dll,FileProtocolHandler", url.trim()]).await {
        Ok(()) => ToolResult::ok("Abrindo o endereço no navegador.", url.trim().to_string()),
        Err(e) => ToolResult::err(format!("Não consegui abrir o endereço: {e}")),
    }
}

fn system_status() -> ToolResult {
    use sysinfo::{Disks, System};
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(250));
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let cpu = sys.global_cpu_usage();
    let (used, total) = (sys.used_memory(), sys.total_memory());
    let gb = |b: u64| b as f64 / 1_073_741_824.0;
    let disks: Vec<Value> = Disks::new_with_refreshed_list()
        .iter()
        .map(|d| {
            json!({
                "mount": d.mount_point().display().to_string(),
                "total_gb": (gb(d.total_space()) * 10.0).round() / 10.0,
                "free_gb": (gb(d.available_space()) * 10.0).round() / 10.0,
            })
        })
        .collect();
    let ram_pct = if total > 0 { used as f64 * 100.0 / total as f64 } else { 0.0 };
    let mut details = format!("CPU: {cpu:.0}%\nRAM: {:.1} de {:.1} GB ({ram_pct:.0}%)", gb(used), gb(total));
    for d in &disks {
        details.push_str(&format!("\nDisco {}: {} GB livres de {} GB", d["mount"].as_str().unwrap_or("?"), d["free_gb"], d["total_gb"]));
    }
    let mut r = ToolResult::ok(format!("CPU em {cpu:.0}% e memória em {ram_pct:.0}%."), details);
    r.data = json!({"cpu_percent": cpu, "ram_used_gb": gb(used), "ram_total_gb": gb(total), "ram_percent": ram_pct, "disks": disks});
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolbox(actions: bool, shell: bool) -> PcToolbox {
        PcToolbox { enable_actions: actions, enable_shell: shell, shell_timeout: 5, full_access: false }
    }

    #[tokio::test]
    async fn unknown_tool_is_error() {
        let r = toolbox(true, true).call("nao_existe", json!({})).await;
        assert!(!r.ok);
        assert!(r.details.contains("desconhecida"));
    }

    #[tokio::test]
    async fn bad_args_are_errors() {
        let t = toolbox(true, true);
        assert!(!t.call("move_mouse", json!({"x": 1})).await.ok);
        assert!(!t.call("press_keys", json!({"keys": "ctrl+nada"})).await.ok);
        assert!(!t.call("open_url", json!({"url": "file:///c:/x"})).await.ok);
        assert!(!t.call("media_key", json!({"command": "boom"})).await.ok);
        assert!(!t.call("file_read", json!({})).await.ok);
    }

    #[tokio::test]
    async fn powershell_blocklist_and_flags() {
        assert!(!toolbox(true, true).call("run_powershell", json!({"command": "format C:"})).await.ok);
        let r = toolbox(true, false).call("run_powershell", json!({"command": "echo oi"})).await;
        assert!(!r.ok && r.details.contains("desativadas"));
    }

    #[test]
    fn specs_respect_flags() {
        let names = |t: &PcToolbox| t.specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
        let off = names(&toolbox(false, false));
        assert!(off.contains(&"file_read".to_string()));
        assert!(!off.contains(&"click".to_string()));
        assert!(!off.contains(&"run_powershell".to_string()));
        let all = toolbox(true, true).specs();
        let lock = all.iter().find(|s| s.name == "lock_pc");
        assert!(lock.is_some_and(|s| s.needs_confirmation && s.mutates));
        assert!(all.iter().find(|s| s.name == "screenshot").is_some_and(|s| !s.mutates));
    }

    #[test]
    fn app_resolution_and_url() {
        assert_eq!(resolve_app("Calculadora"), "calc.exe");
        assert_eq!(resolve_app("foo.exe"), "foo.exe");
        assert!(valid_url("https://example.com/?a=1&b=2"));
        assert!(!valid_url("javascript:alert(1)"));
        assert!(!safe_for_cmd("calc & del x"));
    }

    #[tokio::test]
    async fn file_tools_via_toolbox() {
        let d = std::env::temp_dir().join(format!("sage_tools_tb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("x.txt").display().to_string();
        let t = toolbox(true, true);
        assert!(t.call("file_write", json!({"path": f, "content": "abc"})).await.ok);
        let r = t.call("file_read", json!({"path": f})).await;
        assert_eq!(r.details, "abc");
        let _ = std::fs::remove_dir_all(&d);
    }
}
