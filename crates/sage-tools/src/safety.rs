//! Bloqueio de comandos destrutivos para o `run_powershell`.

use regex::RegexSet;
use std::sync::OnceLock;

/// Sempre bloqueados: apagam discos ou partições inteiras (nem o acesso total libera).
const CATASTROPHIC: &[&str] = &[
    r"(?i)\bformat(-volume)?\s+[a-z]:",
    r"(?i)\bformat-volume\b",
    r"(?i)\bdiskpart\b",
    r#"(?i)\bdel\s+(/[a-z]\s+)*[a-z]:\\(\*(\.\*)?)?(\s|$|'|")"#,
    r#"(?i)\bremove-item\b.*-recurse.*[\s'"][a-z]:\\?(\*)?(\s|$|'|")"#,
    r"(?i)\brm\s+-r[f]?\s+/(\s|$)",
    r#"(?i)\b(rmdir|rd)\s+(/[sq]\s+)+['"]?[a-z]:\\?(\s|$|'|")"#,
    r"(?i)\bcipher\s+/w",
    r"(?i)\bclear-disk\b",
    r"(?i)\bremove-partition\b",
    r"(?i)\binitialize-disk\b",
];

/// Bloqueados só sem o acesso total: desligar/reiniciar, registro, boot, pastas inteiras.
const GUARDED: &[&str] = &[
    r"(?i)\brmdir\s+(/s|/q)",
    r"(?i)\brd\s+/s",
    r"(?i)\bdel\s+(/[a-z]\s+)*[a-z]:\\",
    r#"(?i)\bremove-item\b.*-recurse.*\b[a-z]:\\?(\s|$|'|")"#,
    r"(?i)\bbcdedit\b",
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

fn set(patterns: &'static [&'static str], cell: &'static OnceLock<RegexSet>) -> &'static RegexSet {
    cell.get_or_init(|| RegexSet::new(patterns).unwrap_or_else(|_| RegexSet::empty()))
}

/// `Some(motivo)` quando o comando bate com um padrão destrutivo. Com `full_access`,
/// só o que apaga um disco ou partição inteira continua bloqueado.
pub fn blocked_reason(command: &str, full_access: bool) -> Option<String> {
    static CAT: OnceLock<RegexSet> = OnceLock::new();
    static GUA: OnceLock<RegexSet> = OnceLock::new();
    if let Some(i) = set(CATASTROPHIC, &CAT).matches(command).into_iter().next() {
        return Some(format!("Comando bloqueado: apagaria um disco inteiro (padrão: {}).", CATASTROPHIC[i]));
    }
    if full_access {
        return None;
    }
    let i = set(GUARDED, &GUA).matches(command).into_iter().next()?;
    Some(format!(
        "Comando bloqueado por segurança (padrão: {}). Ligue o acesso total nas configurações para liberar.",
        GUARDED[i]
    ))
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
            assert!(blocked_reason(c, false).is_some(), "deveria bloquear: {c}");
        }
    }

    #[test]
    fn allows_benign() {
        for c in ["Get-Process", "dir C:\\Users", "echo format", "Get-ChildItem -Recurse .", "Remove-Item .\\tmp.txt"] {
            assert!(blocked_reason(c, false).is_none(), "não deveria bloquear: {c}");
        }
    }

    #[test]
    fn full_access_frees_system_commands_but_not_disk_wipes() {
        for c in [
            "shutdown /s /t 0",
            "Stop-Computer -Force",
            "reg delete HKCU\\Software\\X /f",
            "rmdir /s /q C:\\temp\\x",
            "git push origin main",
            "Remove-Item -Recurse -Force C:\\temp\\velho",
        ] {
            assert!(blocked_reason(c, true).is_none(), "acesso total deveria liberar: {c}");
        }
        for c in [
            "format C:",
            "diskpart",
            "Clear-Disk -Number 1",
            "Remove-Item -Recurse -Force C:\\",
            "Remove-Item -Recurse -Force 'D:\\'",
            "rd /s /q D:\\",
            "del /f /s /q C:\\*",
        ] {
            assert!(blocked_reason(c, true).is_some(), "deveria bloquear sempre: {c}");
        }
    }
}
