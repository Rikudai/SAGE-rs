//! Skills: o S.A.G.E. enxerga todas as skills (`SKILL.md`) instaladas no PC — as dele
//! (`C:\SAGE\skills`), as do Claude Code, do Codex, do Antigravity e de `~/.agents` —
//! e pode criar skills novas (motor gerador, guiado pela skill `skill-creator`).
//!
//! Ferramentas: `skills_search` (acha pelo pedido), `skill_read` (carrega as instruções e
//! a lista de arquivos) e `skill_create` (escreve uma skill nova e, se pedido, instala
//! também no Claude Code e no Antigravity).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use sage_core::tools::{ToolResult, ToolSpec, Toolbox};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// De onde veio: sage, claude, codex, antigravity, agents.
    pub source: String,
    pub dir: PathBuf,
}

/// Pastas varridas, em ordem de preferência (nome repetido fica com a primeira).
pub fn roots(sage_root: &Path) -> Vec<(String, PathBuf)> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    vec![
        ("sage".into(), sage_root.join("skills")),
        ("claude".into(), home.join(".claude").join("skills")),
        ("antigravity".into(), home.join(".gemini").join("antigravity").join("skills")),
        ("antigravity".into(), home.join(".gemini").join("antigravity-cli")),
        ("agents".into(), home.join(".agents").join("skills")),
        ("codex".into(), home.join(".codex").join("skills")),
        ("claude".into(), home.join(".claude").join("plugins")),
        ("codex".into(), home.join(".codex").join("plugins")),
        ("sage".into(), sage_root.join("data").join("legacy").join("skills")),
    ]
}

const SKIP_DIRS: &[&str] = &[".git", "node_modules", ".tmp", "target", "__pycache__", ".venv", "dist"];

fn scan(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 9 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                scan(&p, depth + 1, out);
            }
        } else if name == "SKILL.md" {
            out.push(p);
        }
    }
}

/// Lê `name` e `description` do frontmatter (aceita `description: >` multilinha).
pub fn parse_frontmatter(raw: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = raw.strip_prefix("---").map(|r| r.trim_start_matches(['\r', '\n'])) else { return (None, None) };
    let front = rest.split("\n---").next().unwrap_or("");
    let mut name = None;
    let mut desc: Option<String> = None;
    let mut in_desc_block = false;
    for line in front.lines() {
        if in_desc_block {
            if line.starts_with(' ') || line.starts_with('\t') {
                let d = desc.get_or_insert_with(String::new);
                if !d.is_empty() {
                    d.push(' ');
                }
                d.push_str(line.trim());
                continue;
            }
            in_desc_block = false;
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().trim_matches(['"', '\'']).to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            let v = v.trim();
            if v.is_empty() || v == ">" || v == "|" || v == ">-" || v == "|-" {
                in_desc_block = true;
            } else {
                desc = Some(v.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    (name.filter(|n| !n.is_empty()), desc)
}

pub struct SkillCatalog {
    sage_root: PathBuf,
    cache: Mutex<Option<(Instant, Vec<SkillInfo>)>>,
}

impl SkillCatalog {
    pub fn new(sage_root: impl Into<PathBuf>) -> Self {
        Self { sage_root: sage_root.into(), cache: Mutex::new(None) }
    }

    /// Catálogo (refeito a cada 2 min ou depois de criar uma skill).
    pub fn list(&self) -> Vec<SkillInfo> {
        let mut c = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((t, v)) = c.as_ref()
            && t.elapsed() < Duration::from_secs(120)
        {
            return v.clone();
        }
        let v = self.scan_all();
        *c = Some((Instant::now(), v.clone()));
        v
    }

    fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn scan_all(&self) -> Vec<SkillInfo> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (source, root) in roots(&self.sage_root) {
            let mut files = Vec::new();
            scan(&root, 0, &mut files);
            files.sort();
            for f in files {
                let Ok(raw) = std::fs::read_to_string(&f) else { continue };
                let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
                let (name, desc) = parse_frontmatter(&raw);
                let name = name.unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                if name.is_empty() || !seen.insert(name.to_lowercase()) {
                    continue;
                }
                out.push(SkillInfo { name, description: desc.unwrap_or_default(), source: source.clone(), dir });
            }
        }
        out
    }

    pub fn find(&self, name: &str) -> Option<SkillInfo> {
        let n = name.trim().to_lowercase();
        let all = self.list();
        all.iter()
            .find(|s| s.name.to_lowercase() == n)
            .or_else(|| all.iter().find(|s| s.name.to_lowercase().ends_with(&format!(":{n}"))))
            .cloned()
    }

    /// Busca por palavras do pedido no nome e na descrição.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SkillInfo> {
        let words: Vec<String> = query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() > 2)
            .map(str::to_string)
            .collect();
        let mut scored: Vec<(usize, SkillInfo)> = self
            .list()
            .into_iter()
            .map(|s| {
                let name = s.name.to_lowercase();
                let desc = s.description.to_lowercase();
                let score = words.iter().map(|w| usize::from(name.contains(w.as_str())) * 3 + usize::from(desc.contains(w.as_str()))).sum();
                (score, s)
            })
            .filter(|(sc, _)| *sc > 0 || words.is_empty())
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
        scored.into_iter().take(limit).map(|(_, s)| s).collect()
    }

    /// Cria uma skill nova em `C:\SAGE\skills\<nome>` e, se pedido, copia para o Claude
    /// Code (`~/.claude/skills`) e o Antigravity (`~/.gemini/antigravity/skills`).
    /// Nunca sobrescreve uma skill existente.
    pub fn create(&self, name: &str, description: &str, body: &str, targets: &str) -> anyhow::Result<Vec<PathBuf>> {
        static NAME: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("regex nome"));
        let name = name.trim();
        anyhow::ensure!(NAME.is_match(name) && name.len() <= 64, "nome inválido: use minúsculas e hífens (ex.: organizar-downloads), até 64 caracteres");
        let description = description.trim().replace(['\n', '\r'], " ");
        anyhow::ensure!(description.chars().count() >= 20, "a descrição precisa dizer o que a skill faz e quando usar (mínimo 20 caracteres)");
        anyhow::ensure!(body.trim().chars().count() >= 40, "as instruções da skill estão vazias ou curtas demais");
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let mut dests = vec![self.sage_root.join("skills").join(name)];
        let t = targets.to_lowercase();
        if t.contains("claude") || t.contains("all") || t.contains("todos") {
            dests.push(home.join(".claude").join("skills").join(name));
        }
        if t.contains("antigravity") || t.contains("all") || t.contains("todos") {
            dests.push(home.join(".gemini").join("antigravity").join("skills").join(name));
        }
        for d in &dests {
            anyhow::ensure!(!d.exists(), "já existe uma skill em {} (não sobrescrevo; escolha outro nome)", d.display());
        }
        let desc_yaml = serde_json::to_string(&description).unwrap_or_else(|_| format!("\"{description}\""));
        let text = format!("---\nname: {name}\ndescription: {desc_yaml}\n---\n\n{}\n", body.trim());
        for d in &dests {
            std::fs::create_dir_all(d)?;
            std::fs::write(d.join("SKILL.md"), &text)?;
        }
        self.invalidate();
        Ok(dests)
    }
}

/// Ferramentas `skills_search`, `skill_read` e `skill_create`.
pub struct SkillTools(pub std::sync::Arc<SkillCatalog>);

impl SkillTools {
    fn specs_with(&self) -> Vec<ToolSpec> {
        let all = self.0.list();
        let mut names: String = all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ");
        if names.chars().count() > 1200 {
            names = names.chars().take(1200).collect::<String>() + "…";
        }
        vec![
            ToolSpec {
                name: "skills_search".into(),
                description: format!(
                    "Procura entre as {} skills instaladas no PC (Claude, Codex, Antigravity e do S.A.G.E.) a que serve para o pedido. Algumas: {names}",
                    all.len()
                ),
                args: json!({"query": "string: o que precisa fazer"}),
                mutates: false,
                needs_confirmation: false,
            },
            ToolSpec {
                name: "skill_read".into(),
                description: "Carrega as instruções completas de uma skill (SKILL.md) e a lista dos arquivos dela, para seguir o método.".into(),
                args: json!({"name": "string: nome da skill"}),
                mutates: false,
                needs_confirmation: false,
            },
            ToolSpec {
                name: "skill_create".into(),
                description: "Cria uma skill nova (SKILL.md) a partir de um processo que deu certo. Antes, leia a skill 'skill-creator' com skill_read e siga o método dela.".into(),
                args: json!({
                    "name": "string: minúsculas-com-hífens",
                    "description": "string: o que faz e QUANDO usar (é o gatilho)",
                    "instructions": "string: corpo em markdown com passos, exemplos e limites",
                    "targets": "sage | claude | antigravity | all (padrão: sage)"
                }),
                mutates: true,
                needs_confirmation: false,
            },
        ]
    }
}

fn arg<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

fn list_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) {
                    stack.push(p);
                }
            } else if let Ok(rel) = p.strip_prefix(dir) {
                files.push(rel.to_string_lossy().replace('\\', "/"));
            }
            if files.len() > 60 {
                return files;
            }
        }
    }
    files.sort();
    files
}

#[async_trait]
impl Toolbox for SkillTools {
    fn specs(&self) -> Vec<ToolSpec> {
        self.specs_with()
    }

    async fn call(&self, name: &str, args: Value) -> ToolResult {
        let cat = self.0.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || match name.as_str() {
            "skills_search" => {
                let hits = cat.search(arg(&args, "query"), 8);
                if hits.is_empty() {
                    return ToolResult::ok("Não achei uma skill para isso.", "Nenhuma skill encontrada. Considere criar uma com skill_create.");
                }
                let details = hits
                    .iter()
                    .map(|s| format!("- {} ({}): {}", s.name, s.source, s.description.chars().take(300).collect::<String>()))
                    .collect::<Vec<_>>()
                    .join("\n");
                ToolResult::ok(format!("Achei {} skills.", hits.len()), details)
            }
            "skill_read" => {
                let Some(s) = cat.find(arg(&args, "name")) else {
                    return ToolResult::err(format!("Não achei a skill '{}'. Use skills_search.", arg(&args, "name")));
                };
                let body = std::fs::read_to_string(s.dir.join("SKILL.md")).unwrap_or_default();
                let body: String = body.chars().take(14_000).collect();
                let files = list_files(&s.dir).join(", ");
                ToolResult::ok(
                    format!("Carreguei a skill {}.", s.name),
                    format!("Skill {} ({}) em {}\nArquivos: {files}\n\n{body}", s.name, s.source, s.dir.display()),
                )
            }
            "skill_create" => {
                let targets = Some(arg(&args, "targets")).filter(|t| !t.is_empty()).unwrap_or("sage");
                match cat.create(arg(&args, "name"), arg(&args, "description"), arg(&args, "instructions"), targets) {
                    Ok(paths) => ToolResult::ok(
                        format!("Criei a skill {}.", arg(&args, "name")),
                        format!("Skill criada em: {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("; ")),
                    ),
                    Err(e) => ToolResult::err(format!("Não criei a skill: {e}")),
                }
            }
            other => ToolResult::err(format!("Ferramenta desconhecida: '{other}'.")),
        })
        .await
        .unwrap_or_else(|e| ToolResult::err(format!("falha interna: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_and_folded_descriptions() {
        let (n, d) = parse_frontmatter("---\nname: x-y\ndescription: \"Faz X. Use quando Y.\"\n---\nbody");
        assert_eq!(n.as_deref(), Some("x-y"));
        assert_eq!(d.as_deref(), Some("Faz X. Use quando Y."));
        let (_, d) = parse_frontmatter("---\nname: z\ndescription: >\n  linha um\n  linha dois\nother: 1\n---\n");
        assert_eq!(d.as_deref(), Some("linha um linha dois"));
    }

    #[test]
    fn creates_finds_and_refuses_overwrite() {
        let root = std::env::temp_dir().join(format!("sage_skills_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cat = SkillCatalog::new(&root);
        let body = "# Organizar downloads\n\n1. Liste a pasta.\n2. Agrupe por extensão.\n3. Mova com confirmação.";
        cat.create("organizar-downloads", "Organiza a pasta Downloads por tipo. Use quando pedirem para arrumar downloads.", body, "sage")
            .expect("create");
        assert!(cat.find("organizar-downloads").is_some());
        assert_eq!(cat.search("arrumar meus downloads", 3)[0].name, "organizar-downloads");
        assert!(cat.create("organizar-downloads", "Organiza a pasta Downloads por tipo, de novo.", body, "sage").is_err());
        assert!(cat.create("Nome Ruim", "Organiza a pasta Downloads por tipo, de novo.", body, "sage").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
