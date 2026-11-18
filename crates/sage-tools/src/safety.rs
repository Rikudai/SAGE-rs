//! Bloqueio de comandos destrutivos para o `run_powershell`.

use regex::RegexSet;
use std::sync::OnceLock;

const PATTERNS: &[&str] = &[
    r"(?i)\bformat(-volume)?\s+[a-z]:",
    r"(?i)\bformat-volume\b",
    r"(?i)\bdiskpart\b",
    r"(?i)\brmdir\s+(/s|/q)",
    r"(?i)\brd\s+/s",
    r"(?i)\bdel\s+(/[a-z]\s+)*[a-z]:\\",
    r"(?i)\bremove-item\b.*-recurse.*\b[a-z]:\\?(\s|$|'|\x22)",
    r"(?i)\brm\s+-r[f]?\s+/",
    r"(?i)\bbcdedit\b",
    r"(?i)\bcipher\s+/w",
    r"(?i)\bclear-disk\b",
    r"(?i)\bremove-partition\b",
    r"(?i)\binitialize-disk\b",
    r"(?i)\bshutdown\b",
    r"(?i)\brestart-computer\b",
    r"(?i)\bstop-computer\b",
    r"(?i)\breg\s+delete\b",
    r"(?i)\bremove-itemproperty\b.*\bhk(lm|cu)\b",
    r"(?i)\bvssadmin\b.*delete",
    r"(?i)\bwmic\b.*\bdelete\b",
    r"(?i)\bset-mppreference\b.*disable",
    r"(?i)\btakeown\b.*\s/r\b",
];

fn set() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| RegexSet::new(PATTERNS).unwrap_or_else(|_| RegexSet::empty()))
}

/// `Some(motivo)` quando o comando bate com um padrão destrutivo.
pub fn blocked_reason(command: &str) -> Option<String> {
    let idx = set().matches(command).into_iter().next()?;
    Some(format!("Comando bloqueado por segurança (padrão: {}).", PATTERNS[idx]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_destructive() {
        for c in [
            "format C:",
            "Format-Volume -DriveLetter D",
            "diskpart",
            "rmdir /s /q C:\\x",
            "bcdedit /set x",
            "shutdown /s /t 0",
            "reg delete HKLM\\x /f",
            "Remove-Item -Recurse -Force C:\\",
        ] {
            assert!(blocked_reason(c).is_some(), "deveria bloquear: {c}");
        }
    }

    #[test]
    fn allows_benign() {
        for c in ["Get-Process", "dir C:\\Users", "echo format", "Get-ChildItem -Recurse .", "Remove-Item .\\tmp.txt"] {
            assert!(blocked_reason(c).is_none(), "não deveria bloquear: {c}");
        }
    }
}
