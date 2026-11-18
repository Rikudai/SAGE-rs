//! Ferramentas de arquivo (síncronas; o chamador usa `spawn_blocking`).

use sage_core::tools::ToolResult;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_READ: usize = 400_000;
pub const MAX_WRITE: usize = 4 * 1024 * 1024;
pub const MAX_LIST: usize = 200;
pub const MAX_SEARCH_FILES: usize = 4000;
pub const MAX_SEARCH_MATCHES: usize = 60;
pub const MAX_SEARCH_FILE_BYTES: u64 = 1_500_000;

const SKIP_DIRS: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    "appdata",
    "node_modules",
    ".git",
    "windows",
    "$windows.~bt",
    "target",
    "__pycache__",
];

/// Expande `%VAR%` e `~` e devolve o caminho.
pub fn resolve(path: &str) -> PathBuf {
    let mut s = path.trim().to_string();
    if let Some(rest) = s.strip_prefix('~') {
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            s = format!("{home}{rest}");
        }
    }
    // %VAR%
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    PathBuf::from(out)
}

/// Casamento de curinga simples (`*` e `?`), sem diferenciar maiúsculas.
pub fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn is_skipped(name: &str) -> bool {
    let l = name.to_lowercase();
    SKIP_DIRS.contains(&l.as_str())
}

pub fn read(path: &str, max_bytes: Option<usize>) -> ToolResult {
    let p = resolve(path);
    let limit = max_bytes.unwrap_or(MAX_READ).clamp(1, MAX_READ);
    let bytes = match fs::read(&p) {
        Ok(b) => b,
        Err(e) => return ToolResult::err(format!("Não consegui ler {}: {e}", p.display())),
    };
    let size = bytes.len();
    let truncated = size > limit;
    let text = String::from_utf8_lossy(&bytes[..size.min(limit)]).into_owned();
    let mut r = ToolResult::ok(format!("Li o arquivo {}.", file_name(&p)), text);
    r.data = json!({"path": p.display().to_string(), "size": size, "truncated": truncated});
    r
}

pub fn write(path: &str, content: &str) -> ToolResult {
    if content.len() > MAX_WRITE {
        return ToolResult::err("Conteúdo grande demais (limite de 4 MB).");
    }
    let p = resolve(path);
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent) {
                return ToolResult::err(format!("Não consegui criar a pasta: {e}"));
            }
        }
    }
    if p.is_file() {
        let _ = fs::copy(&p, backup_path(&p));
    }
    match fs::write(&p, content) {
        Ok(()) => {
            let mut r = ToolResult::ok(format!("Salvei {}.", file_name(&p)), p.display().to_string());
            r.data = json!({"path": p.display().to_string(), "bytes": content.len()});
            r
        }
        Err(e) => ToolResult::err(format!("Não consegui gravar {}: {e}", p.display())),
    }
}

pub fn edit(path: &str, find: &str, replace: &str) -> ToolResult {
    if find.is_empty() {
        return ToolResult::err("O texto a localizar não pode ser vazio.");
    }
    let p = resolve(path);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => return ToolResult::err(format!("Não consegui ler {}: {e}", p.display())),
    };
    let count = text.matches(find).count();
    if count == 0 {
        return ToolResult::err("Texto não encontrado no arquivo.");
    }
    let _ = fs::copy(&p, backup_path(&p));
    let new = text.replacen(find, replace, 1);
    match fs::write(&p, new) {
        Ok(()) => {
            let mut r = ToolResult::ok(format!("Editei {}.", file_name(&p)), p.display().to_string());
            r.data = json!({"path": p.display().to_string(), "replacements": 1, "occurrences": count});
            r
        }
        Err(e) => ToolResult::err(format!("Não consegui gravar {}: {e}", p.display())),
    }
}

pub fn list(path: &str, pattern: Option<&str>, recursive: bool) -> ToolResult {
    let root = resolve(path);
    if !root.is_dir() {
        return ToolResult::err(format!("Pasta não encontrada: {}", root.display()));
    }
    let pat = pattern.filter(|s| !s.is_empty()).unwrap_or("*");
    let mut entries = Vec::new();
    let mut stack = vec![root.clone()];
    'outer: while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let name = e.file_name().to_string_lossy().into_owned();
            let ep = e.path();
            let is_dir = ep.is_dir();
            if is_dir && is_skipped(&name) {
                continue;
            }
            if wildcard_match(pat, &name) {
                let rel = ep.strip_prefix(&root).unwrap_or(&ep).display().to_string();
                entries.push(if is_dir { format!("{rel}\\") } else { rel });
                if entries.len() >= MAX_LIST {
                    break 'outer;
                }
            }
            if recursive && is_dir {
                stack.push(ep);
            }
        }
    }
    let capped = entries.len() >= MAX_LIST;
    let mut r = ToolResult::ok(format!("Encontrei {} itens.", entries.len()), entries.join("\n"));
    r.data = json!({"path": root.display().to_string(), "entries": entries, "capped": capped});
    r
}

pub fn search(path: &str, query: &str, pattern: Option<&str>) -> ToolResult {
    if query.is_empty() {
        return ToolResult::err("A busca não pode ser vazia.");
    }
    let re = regex::RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
        .or_else(|_| regex::RegexBuilder::new(&regex::escape(query)).case_insensitive(true).build());
    let Ok(re) = re else {
        return ToolResult::err("Busca inválida.");
    };
    let root = resolve(path);
    let pat = pattern.filter(|s| !s.is_empty()).unwrap_or("*");
    let mut matches = Vec::new();
    let mut scanned = 0usize;
    let mut stack = vec![root.clone()];
    if root.is_file() {
        stack.clear();
        scan_file(&root, &re, &mut matches);
    }
    'outer: while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let name = e.file_name().to_string_lossy().into_owned();
            let ep = e.path();
            if ep.is_dir() {
                if !is_skipped(&name) {
                    stack.push(ep);
                }
                continue;
            }
            if !wildcard_match(pat, &name) {
                continue;
            }
            if e.metadata().map(|m| m.len() > MAX_SEARCH_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            scanned += 1;
            if scanned > MAX_SEARCH_FILES {
                break 'outer;
            }
            scan_file(&ep, &re, &mut matches);
            if matches.len() >= MAX_SEARCH_MATCHES {
                break 'outer;
            }
        }
    }
    matches.truncate(MAX_SEARCH_MATCHES);
    let mut r = ToolResult::ok(format!("Encontrei {} ocorrências.", matches.len()), matches.join("\n"));
    r.data = json!({"matches": matches, "files_scanned": scanned});
    r
}

fn scan_file(path: &Path, re: &regex::Regex, out: &mut Vec<String>) {
    let Ok(bytes) = fs::read(path) else { return };
    let text = String::from_utf8_lossy(&bytes);
    for (i, line) in text.lines().enumerate() {
        if re.is_match(line) {
            let l: String = line.trim().chars().take(200).collect();
            out.push(format!("{}:{}: {}", path.display(), i + 1, l));
            if out.len() >= MAX_SEARCH_MATCHES {
                return;
            }
        }
    }
}

fn backup_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sage_tools_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn wildcard() {
        assert!(wildcard_match("*.rs", "Main.RS"));
        assert!(wildcard_match("a?c*", "abcdef"));
        assert!(!wildcard_match("*.rs", "main.py"));
        assert!(wildcard_match("*", ""));
    }

    #[test]
    fn write_read_edit_with_backup() {
        let d = tmp("rw");
        let f = d.join("sub").join("a.txt");
        let fs_ = f.to_str().unwrap();
        assert!(write(fs_, "olá mundo").ok);
        assert!(write(fs_, "olá mundo 2").ok);
        assert!(backup_path(&f).exists());
        let r = read(fs_, None);
        assert!(r.ok);
        assert_eq!(r.details, "olá mundo 2");
        assert!(edit(fs_, "mundo", "Rust").ok);
        assert_eq!(read(fs_, None).details, "olá Rust 2");
        assert!(!edit(fs_, "inexistente", "x").ok);
        assert_eq!(read(fs_, Some(2)).data["truncated"], true);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn list_and_search() {
        let d = tmp("ls");
        fs::create_dir_all(d.join("node_modules")).unwrap();
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("a.txt"), "linha um\nfoo bar\n").unwrap();
        fs::write(d.join("src").join("b.rs"), "fn foo() {}\n").unwrap();
        fs::write(d.join("node_modules").join("c.rs"), "foo\n").unwrap();
        let r = list(d.to_str().unwrap(), Some("*.rs"), true);
        assert!(r.ok);
        assert_eq!(r.data["entries"].as_array().unwrap().len(), 1);
        let s = search(d.to_str().unwrap(), "foo", None);
        assert_eq!(s.data["matches"].as_array().unwrap().len(), 2);
        assert!(!search(d.to_str().unwrap(), "", None).ok);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn read_missing_is_error() {
        assert!(!read("Z:\\nao\\existe.txt", None).ok);
    }
}
