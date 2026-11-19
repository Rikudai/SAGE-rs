//! Auto-edição do S.A.G.E.: ele muda o próprio código sem mexer na versão que está rodando.
//!
//! Fluxo de `sage_self_edit` (roda em segundo plano; o chat segue livre):
//! 1. **Cópia**: `git worktree` em `<pai da raiz>/SAGE-dev/<id>`, num ramo `sage/<id>`.
//! 2. **Código**: um agente de código (Codex, Claude ou Antigravity, com acesso total)
//!    implementa o pedido só nessa cópia.
//! 3. **Compilar** e 4. **Testes**: `cargo build --release -p sage` e `cargo test --workspace`
//!    na cópia (pasta `target` compartilhada com a raiz, para sair rápido). Se algo falhar,
//!    o agente recebe o erro e tenta consertar uma vez.
//! 5. **Aplicar**: commit no ramo, `merge --ff-only` no `main`, push, e o
//!    `scripts/reiniciar-atualizado.ps1` troca o exe, reabre, confere se abriu (senão volta
//!    a versão anterior) e deixa o resultado em `data/resultado_atualizacao.json`, que o
//!    S.A.G.E. novo anuncia ("Estou atualizado").
//!
//! Cada etapa aparece no painel de tarefas como um worker (PlanReady / WorkerStart / WorkerEnd).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use sage_core::events::{EventBus, SageEvent};
use sage_core::llm::{AgentMode, AgentTask, FailureKind, ModelHub, Tier, Usage, WorkerStatus};
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};

use crate::update::{self, log_line};

const STEPS: [&str; 5] = ["copia", "codigo", "compilar", "testes", "aplicar"];
const AGENT_TIMEOUT_SECS: u64 = 40 * 60;
const BUILD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Resultado dos testes (`cargo test`), somado de todas as linhas `test result:`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestTally {
    pub passed: u32,
    pub failed: u32,
    /// Nomes dos testes que falharam (até 10).
    pub failures: Vec<String>,
}

impl TestTally {
    pub fn parse(output: &str) -> Self {
        let mut t = Self::default();
        for line in output.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("test result:") {
                for part in rest.split(';') {
                    let mut it = part.split_whitespace().rev();
                    let (Some(word), Some(num)) = (it.next(), it.next()) else { continue };
                    let n: u32 = num.trim_end_matches('.').parse().unwrap_or(0);
                    match word {
                        "passed" => t.passed += n,
                        "failed" => t.failed += n,
                        _ => {}
                    }
                }
            } else if l.starts_with("test ") && l.ends_with("FAILED") && t.failures.len() < 10 {
                t.failures.push(l.trim_start_matches("test ").trim_end_matches("... FAILED").trim().to_string());
            }
        }
        t
    }

    pub fn line(&self) -> String {
        if self.failed == 0 {
            format!("{} testes passaram", self.passed)
        } else {
            format!("{} passaram, {} falharam ({})", self.passed, self.failed, self.failures.join(", "))
        }
    }
}

/// Saída de um comando: sucesso e o texto (stdout + stderr).
struct CmdOut {
    ok: bool,
    text: String,
}

async fn cargo(cwd: &Path, target: &Path, args: &[&str]) -> CmdOut {
    let Some((exe, extra)) = update::cargo_env() else {
        return CmdOut { ok: false, text: "não achei o cargo (Rust) neste PC".into() };
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).current_dir(cwd).env("CARGO_TARGET_DIR", target).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let mut paths = extra;
    if let Some(p) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&p));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    match tokio::time::timeout(BUILD_TIMEOUT, cmd.output()).await {
        Err(_) => CmdOut { ok: false, text: "o cargo demorou demais".into() },
        Ok(Err(e)) => CmdOut { ok: false, text: format!("não consegui rodar o cargo: {e}") },
        Ok(Ok(o)) => CmdOut {
            ok: o.status.success(),
            text: format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)),
        },
    }
}

/// As linhas que importam de um build que falhou (erros do compilador), para o agente consertar.
fn error_digest(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut keep = 0usize;
    for line in text.lines() {
        if line.starts_with("error") || line.contains("panicked at") || line.contains("FAILED") {
            keep = 12;
        }
        if keep > 0 {
            out.push_str(line);
            out.push('\n');
            keep -= 1;
        }
        if out.len() > max {
            break;
        }
    }
    if out.trim().is_empty() {
        let tail: Vec<&str> = text.lines().rev().take(40).collect();
        out = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    }
    out.chars().take(max).collect()
}

/// Pasta das cópias de trabalho: ao lado da raiz (`C:\SAGE-dev`).
pub fn dev_root(root: &Path) -> PathBuf {
    root.parent().unwrap_or(root).join("SAGE-dev")
}

fn brief(request: &str, root: &Path) -> String {
    format!(
        "Você está editando o código do próprio S.A.G.E. (assistente desktop em Rust: workspace \
cargo, edition 2024, egui/eframe) numa CÓPIA de trabalho isolada (git worktree), não na versão que \
está rodando em {root}.\n\n\
PEDIDO DO USUÁRIO:\n{request}\n\n\
REGRAS:\n\
- Trabalhe só nesta pasta (a cópia). Leia docs/ARCHITECTURE.md antes de mudar a arquitetura.\n\
- Build: `cargo build -p sage`; testes: `cargo test --workspace`. No Windows o toolchain é gnu: \
coloque C:\\mingw64\\bin no PATH antes do cargo. Deixe tudo compilando e os testes passando; \
escreva testes para o que for testável.\n\
- Comentários e textos da interface em português do Brasil, no estilo do código que já existe.\n\
- Não faça commit, push, nem reinicie o S.A.G.E.: o fluxo de atualização faz isso depois dos testes.\n\
- Nunca grave chaves de API nem mexa em config/config.json e data/.\n\
- Termine com uma linha `SUMMARY:` dizendo em uma frase o que mudou.",
        root = root.display()
    )
}

#[derive(Clone)]
pub struct SelfEdit {
    pub root: PathBuf,
    pub hub: Arc<dyn ModelHub>,
    pub bus: EventBus,
    running: Arc<Mutex<Option<String>>>,
}

impl SelfEdit {
    pub fn new(root: PathBuf, hub: Arc<dyn ModelHub>, bus: EventBus) -> Self {
        Self { root, hub, bus, running: Arc::default() }
    }

    fn step_start(&self, run: &str, step: &str, role: &str, model: &str, brief: &str) {
        self.bus.emit(SageEvent::WorkerStart {
            run_id: run.into(),
            worker: step.into(),
            role: role.into(),
            model: model.into(),
            tier: Tier::Strong,
            brief: brief.into(),
        });
    }

    fn step_end(&self, run: &str, step: &str, ok: bool, summary: &str, usage: Usage) {
        self.bus.emit(SageEvent::WorkerEnd {
            run_id: run.into(),
            worker: step.into(),
            status: if ok { WorkerStatus::Done } else { WorkerStatus::Failed },
            summary: summary.into(),
            usage,
        });
    }

    fn stream(&self, step: &str, text: &str) {
        self.bus.emit(SageEvent::Stream { source: step.into(), text: text.into(), thought: false });
    }

    /// Roda o agente de código na cópia; tenta o próximo agente quando um bate no limite.
    async fn agent(&self, run: &str, prefer: &str, prompt: String, dir: &Path, resume: Option<(String, String)>) -> Result<(String, String, Option<String>, Usage), String> {
        let order: Vec<&str> = match (prefer, &resume) {
            (_, Some((p, _))) => vec![p.as_str()],
            ("codex" | "claude" | "agy", _) => vec![prefer],
            _ => vec!["codex", "claude", "agy"],
        };
        let mut last = String::from("nenhum agente de código (Codex, Claude, Antigravity) está instalado");
        for p in order {
            let Some(cli) = self.hub.agent(p).filter(|c| c.installed()) else { continue };
            let task = AgentTask {
                id: format!("{run}-codigo"),
                prompt: prompt.clone(),
                model: String::new(),
                effort: None,
                mode: AgentMode::Write,
                cwd: dir.to_path_buf(),
                timeout_secs: AGENT_TIMEOUT_SECS,
                resume_session: resume.as_ref().map(|(_, s)| s.clone()),
            };
            let me = self.clone();
            let sink: sage_core::llm::DeltaSink = Arc::new(move |t: &str, _thought: bool| me.stream("codigo", t));
            match cli.run(task, Some(sink)).await {
                Ok(out) if matches!(out.status, WorkerStatus::Done | WorkerStatus::DoneWithConcerns) => {
                    return Ok((p.to_string(), out.summary, out.session_id, out.usage));
                }
                Ok(out) if out.failure == Some(FailureKind::RateLimited) => {
                    last = format!("{p} sem cota: {}", out.summary);
                    self.stream("codigo", &format!("\n[{p} sem cota; tentando o próximo agente]\n"));
                }
                Ok(out) => return Err(format!("{p}: {}", out.summary)),
                Err(e) => last = format!("{p}: {e}"),
            }
        }
        Err(last)
    }

    /// O fluxo inteiro. Devolve a mensagem final para o usuário.
    pub async fn run_job(&self, id: String, request: String, prefer: String, apply: bool, force: bool) -> String {
        let started = Instant::now();
        let root = self.root.clone();
        let dir = dev_root(&root).join(&id);
        let branch = format!("sage/{id}");
        let target = root.join("target");
        let title = format!("Auto-atualização: {}", short(&request, 40));
        self.bus.emit(SageEvent::PlanReady {
            run_id: id.clone(),
            title: title.clone(),
            playbook: "auto-atualizacao".into(),
            why: "Edita uma cópia de si mesmo, compila, testa e só então aplica e reinicia.".into(),
            units: STEPS.iter().map(|s| s.to_string()).collect(),
        });
        log_line(&root, &format!("[auto-edição {id}] começou: {request}"));
        let mut usage = Usage::default();

        // 1. cópia
        self.step_start(&id, "copia", "git worktree", "git", &format!("Cópia isolada em {}", dir.display()));
        let _ = std::fs::create_dir_all(dev_root(&root));
        let d = dir.display().to_string();
        if let Err(e) = update::git(&root, &["worktree", "add", "-b", &branch, &d, "HEAD"], 120).await {
            self.step_end(&id, "copia", false, &e, Usage::default());
            return self.finish(&id, &title, started, usage, false, &format!("Não consegui criar a cópia de trabalho: {e}"));
        }
        self.step_end(&id, "copia", true, &format!("Cópia em {d}, ramo {branch}."), Usage::default());

        // 2. código
        self.step_start(&id, "codigo", "agente de código", &prefer, &short(&request, 300));
        let (agent, mut summary, session, u) = match self.agent(&id, &prefer, brief(&request, &root), &dir, None).await {
            Ok(v) => v,
            Err(e) => {
                self.step_end(&id, "codigo", false, &e, Usage::default());
                return self.finish(&id, &title, started, usage, false, &format!("O agente de código não conseguiu: {e}. A cópia ficou em {d}."));
            }
        };
        add_usage(&mut usage, &u);
        self.step_end(&id, "codigo", true, &summary, u);
        let changed = update::git(&dir, &["status", "--porcelain"], 60).await.unwrap_or_default();
        if changed.trim().is_empty() {
            self.cleanup(&dir, &branch).await;
            return self.finish(&id, &title, started, usage, false, &format!("O agente ({agent}) não mudou nenhum arquivo: {summary}"));
        }

        // 3 e 4. compilar e testar, com uma rodada de conserto
        let mut fix_round = 0;
        let (build_ok, tally) = loop {
            self.step_start(&id, "compilar", "cargo build --release", "cargo", "Compila a versão nova na cópia.");
            let b = cargo(&dir, &target, &["build", "--release", "-p", "sage"]).await;
            self.step_end(&id, "compilar", b.ok, if b.ok { "Compilou." } else { "Não compilou." }, Usage::default());
            let (tally, test_text) = if b.ok {
                self.step_start(&id, "testes", "cargo test --workspace", "cargo", "Roda todos os testes na cópia.");
                let t = cargo(&dir, &target, &["test", "--workspace"]).await;
                let tally = TestTally::parse(&t.text);
                let ok = t.ok && tally.failed == 0;
                self.step_end(&id, "testes", ok, &tally.line(), Usage::default());
                (Some(tally), if ok { None } else { Some(t.text) })
            } else {
                (None, Some(b.text))
            };
            let Some(err_text) = test_text else { break (true, tally.unwrap_or_default()) };
            if fix_round >= 1 || session.is_none() {
                break (b.ok, tally.unwrap_or_default());
            }
            fix_round += 1;
            self.step_start(&id, "codigo", "agente de código (conserto)", &agent, "Conserta o build ou os testes que falharam.");
            let prompt = format!(
                "O build ou os testes falharam depois da sua mudança. Conserte (sem desligar testes) e termine com SUMMARY:.\n\n{}",
                error_digest(&err_text, 6000)
            );
            match self.agent(&id, &agent, prompt, &dir, Some((agent.clone(), session.clone().unwrap_or_default()))).await {
                Ok((_, s, _, u)) => {
                    add_usage(&mut usage, &u);
                    summary = s.clone();
                    self.step_end(&id, "codigo", true, &s, u);
                }
                Err(e) => {
                    self.step_end(&id, "codigo", false, &e, Usage::default());
                    break (b.ok, tally.unwrap_or_default());
                }
            }
        };
        if !build_ok {
            return self.finish(&id, &title, started, usage, false, &format!(
                "A versão nova não compilou, então não apliquei nada. A cópia ficou em {d} (ramo {branch}) para olhar."
            ));
        }
        if tally.failed > 0 && !force {
            return self.finish(&id, &title, started, usage, false, &format!(
                "Compilou, mas os testes não passaram ({}). Não apliquei. Diga \"aplica a atualização {id} mesmo assim\" se quiser usar assim; a cópia está em {d}.",
                tally.line()
            ));
        }
        if !apply {
            return self.finish(&id, &title, started, usage, true, &format!(
                "Pronto na cópia, sem aplicar: {summary} ({}). Diga \"aplica a atualização {id}\" para instalar.",
                tally.line()
            ));
        }
        match self.apply_worktree(&id, &dir, &branch, &summary, &tally).await {
            Ok(msg) => self.finish(&id, &title, started, usage, true, &msg),
            Err(e) => self.finish(&id, &title, started, usage, false, &format!("Testado, mas não consegui aplicar: {e}. A cópia ficou em {d}.")),
        }
    }

    /// Commit na cópia, merge no main, push, exe novo preparado e reinício pelo script.
    pub async fn apply_worktree(&self, id: &str, dir: &Path, branch: &str, summary: &str, tally: &TestTally) -> Result<String, String> {
        let root = &self.root;
        self.step_start(id, "aplicar", "commit, merge, reinício", "git", "Aplica no main e reinicia com a versão nova.");
        let secrets = update::configured_secrets(root);
        update::git(dir, &["add", "-A"], 60).await?;
        let diff = update::git(dir, &["diff", "--cached"], 60).await.unwrap_or_default();
        let leaks = update::leaked_files(&diff, &secrets);
        if !leaks.is_empty() {
            let e = format!("parei: parece haver chave de API em {}", leaks.join(", "));
            self.step_end(id, "aplicar", false, &e, Usage::default());
            return Err(e);
        }
        let msg = format!("S.A.G.E. se atualizou: {}\n\n{}\n\nCo-Authored-By: S.A.G.E. <sage@localhost>", short(summary, 70), tally.line());
        if let Err(e) = update::git(dir, &["commit", "-m", &msg], 120).await {
            self.step_end(id, "aplicar", false, &e, Usage::default());
            return Err(e);
        }
        if let Err(e) = update::git(root, &["merge", "--ff-only", branch], 120).await {
            self.step_end(id, "aplicar", false, &e, Usage::default());
            return Err(format!("o main andou enquanto eu trabalhava ({e})"));
        }
        let head = update::git(root, &["rev-parse", "--short", "HEAD"], 30).await.unwrap_or_default();
        let pushed = match update::git(root, &["push"], 180).await {
            Ok(_) => "e foi para o GitHub".to_string(),
            Err(e) => format!("mas o push falhou ({e})"),
        };
        let built = root.join("target").join("release").join(if cfg!(windows) { "sage.exe" } else { "sage" });
        let staged = root.join(if cfg!(windows) { "sage.new.exe" } else { "sage.new" });
        if let Err(e) = std::fs::copy(&built, &staged) {
            self.step_end(id, "aplicar", false, &e.to_string(), Usage::default());
            return Err(format!("não consegui preparar o exe novo: {e}"));
        }
        let pending = json!({
            "id": id,
            "resumo": summary,
            "testes": tally.line(),
            "commit": head,
        });
        let _ = std::fs::write(root.join("data").join("atualizacao_pendente.json"), serde_json::to_string_pretty(&pending).unwrap_or_default());
        self.cleanup(dir, branch).await;
        self.step_end(id, "aplicar", true, &format!("Commit {head} no main {pushed}; reiniciando."), Usage::default());
        launch_restart(root, Some(&staged), false, id)?;
        Ok(format!(
            "Atualização pronta: {} {}. Commit {head} {pushed}. Vou reiniciar em alguns segundos para aplicar; quando eu voltar, confirmo se deu tudo certo.",
            short(summary, 200),
            tally.line()
        ))
    }

    async fn cleanup(&self, dir: &Path, branch: &str) {
        let d = dir.display().to_string();
        let _ = update::git(&self.root, &["worktree", "remove", "--force", &d], 120).await;
        let _ = update::git(&self.root, &["branch", "-D", branch], 30).await;
    }

    fn finish(&self, id: &str, title: &str, started: Instant, usage: Usage, ok: bool, message: &str) -> String {
        let secs = started.elapsed().as_secs_f64();
        log_line(&self.root, &format!("[auto-edição {id}] {message}"));
        let dir = self.root.join("data").join("runs").join(id);
        let _ = std::fs::create_dir_all(&dir);
        let md = format!(
            "# {title}\n\n**Status:** {} · **Duração:** {:.0} s\n\n## Resposta\n\n{message}\n",
            if ok { "concluída" } else { "parcial" },
            secs
        );
        let _ = std::fs::write(dir.join("report.md"), md);
        self.bus.emit(SageEvent::RunReport {
            run_id: id.into(),
            summary: short(message, 160),
            usage,
            path: dir.join("report.md").display().to_string(),
        });
        message.to_string()
    }
}

fn add_usage(total: &mut Usage, u: &Usage) {
    total.input_tokens += u.input_tokens;
    total.output_tokens += u.output_tokens;
    total.cache_read_tokens += u.cache_read_tokens;
    if let Some(c) = u.cost_usd {
        total.cost_usd = Some(total.cost_usd.unwrap_or(0.0) + c);
    }
}

pub(crate) fn short(s: &str, max: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max { one } else { format!("{}…", one.chars().take(max).collect::<String>()) }
}

/// Abre o `scripts/reiniciar-atualizado.ps1` fora do S.A.G.E. (ele não depende do app):
/// com `novo`, instala esse exe; com `compilar`, ele mesmo compila e roda os testes antes.
/// O script fecha este processo, troca o exe, reabre, confere e volta atrás se precisar.
pub fn launch_restart(root: &Path, novo: Option<&Path>, compilar: bool, id: &str) -> Result<(), String> {
    let script = root.join("scripts").join("reiniciar-atualizado.ps1");
    if !script.is_file() {
        return Err(format!("não achei {}", script.display()));
    }
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.current_dir(root)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(&script)
        .args(["-EsperarPid", &std::process::id().to_string(), "-Id", id]);
    if let Some(n) = novo {
        cmd.arg("-Novo").arg(n);
    }
    if compilar {
        cmd.arg("-Compilar");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000); // DETACHED | NEW_GROUP | NO_WINDOW
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("não consegui abrir o script de reinício: {e}"))?;
    log_line(root, &format!("[reinício {id}] script aberto (compilar={compilar})"));
    Ok(())
}

fn new_id() -> String {
    format!("upd-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

#[async_trait]
impl Toolbox for SelfEdit {
    fn specs(&self) -> Vec<ToolSpec> {
        let s = |name: &str, desc: &str, args: Value| ToolSpec {
            name: name.into(),
            description: desc.into(),
            args,
            mutates: true,
            needs_confirmation: false,
        };
        vec![
            s(
                "sage_self_edit",
                "Muda o PRÓPRIO código do S.A.G.E. (novas funções, consertos, melhorias nele mesmo): edita uma cópia isolada com um agente de código, compila, roda todos os testes e, se passarem, aplica, sobe pro GitHub e reinicia sozinho na versão nova. Roda em segundo plano; responda que começou.",
                json!({"pedido": "string: o que mudar, com todos os detalhes do usuário", "agente": "string? auto|codex|claude|agy", "aplicar": "bool? padrão true", "aplicar_mesmo_com_falhas": "bool? padrão false"}),
            ),
            s(
                "sage_rebuild_restart",
                "Recompila o S.A.G.E. a partir do código atual da pasta dele, roda os testes (passando ou não), reinicia na versão nova, confere se abriu e avisa que está atualizado. Use depois de mudanças no código ou quando pedirem para reiniciar aplicando a build.",
                json!({}),
            ),
            s(
                "sage_self_edit_apply",
                "Aplica uma auto-atualização que ficou na cópia (por exemplo com testes falhando, quando o usuário manda aplicar mesmo assim).",
                json!({"id": "string: id da atualização (upd-...)"}),
            ),
        ]
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        match name {
            "sage_self_edit" => {
                let request = args.get("pedido").and_then(Value::as_str).unwrap_or("").trim().to_string();
                if request.is_empty() {
                    return ToolResult::err("Diga o que mudar no meu código.");
                }
                {
                    let mut r = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(id) = r.as_ref() {
                        return ToolResult::err(format!("Já estou me atualizando ({id}); espere terminar."));
                    }
                    *r = Some(new_id());
                }
                let id = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_else(new_id);
                let prefer = args.get("agente").and_then(Value::as_str).unwrap_or("auto").to_string();
                let apply = args.get("aplicar").and_then(Value::as_bool).unwrap_or(true);
                let force = args.get("aplicar_mesmo_com_falhas").and_then(Value::as_bool).unwrap_or(false);
                let me = self.clone();
                let id2 = id.clone();
                tokio::spawn(async move {
                    let msg = me.run_job(id2, request, prefer, apply, force).await;
                    *me.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    me.bus.emit(SageEvent::Reply { spoken: short(&msg, 220), full: msg, emotion: None });
                });
                let m = format!("Comecei a me atualizar ({id}) numa cópia de mim mesmo; vou compilar, testar e reiniciar sozinho. Sigo aqui conversando enquanto isso.");
                ToolResult::ok(m.clone(), m)
            }
            "sage_rebuild_restart" => match launch_restart(&self.root, None, true, &new_id()) {
                Ok(()) => ToolResult::ok(
                    "Recompilando e rodando os testes; quando estiver pronto eu reinicio e aviso.",
                    "Abri o script de reinício: ele compila, testa, reinicia e confere a versão nova.",
                ),
                Err(e) => ToolResult::err(e),
            },
            "sage_self_edit_apply" => {
                let id = args.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let dir = dev_root(&self.root).join(&id);
                if id.is_empty() || !dir.is_dir() {
                    return ToolResult::err(format!("Não achei a cópia da atualização '{id}'."));
                }
                let me = self.clone();
                tokio::spawn(async move {
                    let tally = TestTally { passed: 0, failed: 0, failures: vec!["aplicada sem passar nos testes".into()] };
                    let msg = match me.apply_worktree(&id, &dir, &format!("sage/{id}"), "aplicada a pedido, sem passar em todos os testes", &tally).await {
                        Ok(m) => m,
                        Err(e) => format!("Não consegui aplicar: {e}"),
                    };
                    me.bus.emit(SageEvent::Reply { spoken: short(&msg, 220), full: msg, emotion: None });
                });
                ToolResult::ok("Aplicando a atualização; reinicio em seguida.", "Aplicando.")
            }
            _ => ToolResult::err(format!("Ferramenta desconhecida: '{name}'.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tallies_cargo_test_output() {
        let out = "test a::b ... ok\ntest c::d ... FAILED\ntest result: FAILED. 3 passed; 1 failed; 0 ignored\n\
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.1s\n";
        let t = TestTally::parse(out);
        assert_eq!((t.passed, t.failed), (13, 1));
        assert_eq!(t.failures, vec!["c::d".to_string()]);
        assert!(t.line().contains("13 passaram, 1 falharam"));
        assert_eq!(TestTally::parse("test result: ok. 5 passed; 0 failed;").line(), "5 testes passaram");
    }

    #[test]
    fn error_digest_keeps_compiler_errors() {
        let text = "Compiling x\nwarning: y\nerror[E0425]: cannot find value `z`\n  --> src/a.rs:1:1\nmore\n";
        let d = error_digest(text, 1000);
        assert!(d.starts_with("error[E0425]"));
        assert!(!d.contains("warning"));
    }

    #[test]
    fn dev_root_sits_next_to_the_install() {
        assert_eq!(dev_root(Path::new(r"C:\SAGE")), PathBuf::from(r"C:\SAGE-dev"));
    }
}
