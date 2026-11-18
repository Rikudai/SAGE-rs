//! Text helpers ported from `legacy/text_processing.py`, `legacy/intent_guard.py`
//! and `legacy/model_registry.py`.
//!
//! The `regex` crate has no lookaround, so the boundary checks of the Python
//! patterns (`(?<![\w./])`, `(?![\w.-]*\.(?:py|...))`, `(?=\s+assume)`, ...) are
//! done by hand around plain matches (see `replace_checked`).

use std::sync::LazyLock;

use regex::{Captures, Match, Regex};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Compiles a regex written in this file; the patterns are constants, checked by the tests.
fn fixed(pattern: &str) -> Regex {
    Regex::new(pattern).expect("regex fixa do sage-core")
}

static CODE_BLOCK: LazyLock<Regex> = LazyLock::new(|| fixed(r"```([a-zA-Z0-9_+\-]*)\n([\s\S]*?)```"));
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| fixed(r"`([^`]+)`"));
static URL: LazyLock<Regex> = LazyLock::new(|| fixed(r#"https?://[^\s'")\]]+"#));
static WINDOWS_PATH: LazyLock<Regex> =
    LazyLock::new(|| fixed(r#"[a-zA-Z]:\\(?:[^\\/:*?"<>|\r\n]+\\)+[^\\/:*?"<>|\r\n]*"#));
static GIT_HASH: LazyLock<Regex> = LazyLock::new(|| fixed(r"\b[0-9a-fA-F]{20,40}\b"));
static HEADER: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?m)^[ \t]*#+[ \t]*"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| fixed(r"\*\*([^*]+)\*\*"));
static ITALIC: LazyLock<Regex> = LazyLock::new(|| fixed(r"\*([^*]+)\*"));
static UNDERSCORE_BOLD: LazyLock<Regex> = LazyLock::new(|| fixed(r"__([^_]+)__"));
static UNDERSCORE_ITALIC: LazyLock<Regex> = LazyLock::new(|| fixed(r"_([^_]+)_"));
static STRIKE: LazyLock<Regex> = LazyLock::new(|| fixed(r"~~([^~]+)~~"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?m)^[ \t]*>[ \t]*"));
static BULLET: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?m)^[ \t]*[-*+][ \t]+"));
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?m)^[ \t]*\d+\.[ \t]+"));
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| fixed(r"<[^>]+>"));
static WHITESPACE_RUN: LazyLock<Regex> = LazyLock::new(|| fixed(r"\s+"));

/// "S.A.G.E." with optional spaces; the trailing dot belongs to the acronym when present.
static NAME_DOTTED: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?i)S\.\s?A\.\s?G\.\s?E\b\.?"));
/// "sage" as a word (the file-name check is done separately).
static NAME_WORD: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?i)sage"));
/// What follows "sage" when it is a file name such as `sage.py`.
static FILE_AFTER_NAME: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"^[\w.-]*\.(?:py|com|json|exe)\b"));

static DEMONSTRATIVE: LazyLock<Regex> = LazyLock::new(|| {
    fixed(r"\b(?:n|d)?(?:ess[ea]s?|est[ea]s?|aquel[ea]s?)\s+([a-z0-9_-]{3,})")
});
static PROMISE: LazyLock<Regex> = LazyLock::new(|| {
    fixed(
        r"(?i)\b(vou|irei|farei|faço|faco|já vou|ja vou|deixa comigo|pode deixar|vou já|estou fazendo|vou (fazer|buscar|procurar|pesquisar|criar|montar|gerar|listar|analisar|verificar|preparar))\b",
    )
});
static COMPLEXITY_MARK: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"\b(1\.|2\.|primeiro|depois|em seguida|entao|então)\b"));

/// Transcription repairs, in the order the Python module applies them.
static TRANSCRIPTION_FIXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\b(beach|bitch|guit|git|kit|gift)\s*-?\s*hub\b", "GitHub"),
        (r"(?i)\bgit\s*rub\b", "GitHub"),
        (r"(?i)\bpai\s*ton\b|\bpaiton\b", "Python"),
        (r"(?i)\bpower\s*shell\b", "PowerShell"),
        (r"(?i)\bvs\s*code\b|\bvisual\s*code\b", "VS Code"),
        (r"(?i)\bollama\b|\bo\s*lama\b", "Ollama"),
        (r"(?i)\bcloud\s*code\b|\bclaud\s*code\b", "Claude Code"),
        (r"(?i)\bclaud\b", "Claude"),
        // "cloud" only before a verb: the lookahead of the Python pattern becomes a capture.
        (r"(?i)\bcloud\b(\s+(?:assume|pensa|faz))", "Claude${1}"),
    ]
    .into_iter()
    .map(|(pattern, replacement)| (fixed(pattern), replacement))
    .collect()
});

/// Words the recogniser hears instead of "SAGE" at the start of a request.
const WAKE_MISHEARS: &[&str] = &[
    "cg", "c g", "seige", "saige", "sajé", "saje", "sei ge", "sage", "s a g e", "ceagê", "cê gê",
];

/// Phrases that announce a task (normalized before the comparison).
const TASK_VERBS: &[&str] = &[
    "busque", "buscar", "busca ", "procure", "procurar", "pesquise", "pesquisar", "pesquisa ",
    "crie", "criar", "cria ", "gere", "gerar", "liste", "listar", "lista ", "monte", "montar",
    "faça", "faca", "fazer uma", "escreva", "escrever", "compare", "comparar", "analise",
    "analisar", "encontre", "encontrar", "levante", "levantar", "organize", "organizar",
    "resuma", "resumir", "traduza", "planeje", "planejar", "implemente", "implementar",
    "desenvolva", "programe", "calcule", "verifique", "verificar", "baixe", "relatório",
    "relatorio", "tabela", "planilha", "documento", "tarefa complexa", "tarefa",
];

/// Words after "esse/essa/este..." that talk about the conversation itself, not a screen object.
const TALK_NOUNS: &[&str] = &[
    "conversa", "assunto", "ideia", "vez", "vezes", "caso", "jeito", "forma", "sentido", "momento",
    "semana", "dia", "ano", "mes", "pergunta", "resposta", "tarefa", "tipo", "ponto", "motivo",
    "problema", "questao", "noite", "tarde", "manha", "hora", "trabalho", "pedido", "que", "aqui",
    "ultimo", "ultima", "mesmo", "mesma", "seu", "sua", "meu", "minha", "negocio", "coisa", "papo",
];

/// Phrases that say the user is looking at something right now.
const SEEING: &[&str] = &[
    "estou vendo", "to vendo", "tou vendo", "eu vejo", "na tela", "minha tela", "na minha frente",
    "aberto aqui", "aberta aqui", "que abri", "que eu abri", "que esta aberto", "que esta aberta",
    "ta aberto", "ta aberta", "janela aberta", "olha a tela", "olha minha tela", "isso aqui",
    "esse aqui", "essa aqui",
];

const CODE_HINTS: &[&str] = &[
    "codigo", "código", "script", "programa", "funcao", "função", "classe", "refator", "debug", "bug",
    "erro", "traceback", "regex", "sql", "api", "python", "powershell", "javascript", "typescript",
    "html", "css", "git", "docker", "compilar", "teste unitario", "teste unitário", "automatizar",
    "terminal", "bash", "pip", "npm", "json", "yaml",
];
const REASONING_HINTS: &[&str] = &[
    "por que", "porque", "como funciona", "explique", "explica", "analise", "analise", "compare",
    "planeje", "plano", "arquitetura", "estrategia", "estratégia", "resuma", "resumo", "diferenca",
    "diferença", "calcule", "resolva", "prove", "raciocinio", "raciocínio", "passo a passo",
];
const RESEARCH_HINTS: &[&str] = &[
    "pesquisar", "pesquise", "procure", "google", "youtube", "internet", "documentacao",
    "documentação", "noticias", "notícias", "artigo", "github", "stack overflow", "antigravity",
    "gemini", "nuvem", "online", "web", "site", "baixar", "download",
];
const CASUAL_HINTS: &[&str] = &[
    "oi", "ola", "olá", "bom dia", "boa tarde", "boa noite", "tudo bem", "obrigado", "obrigada",
    "valeu", "tchau", "piada", "conte algo",
];

const SPOKEN_NAME: &str = "Seije";
const CODE_NOTICE: &str = " [O código detalhado foi disponibilizado no seu painel.] ";
const LINK_FALLBACK: &str = "o link indicado no painel";

/// Friendly names for the domains that are read aloud, in the order they are tried.
const FRIENDLY_DOMAINS: &[(&str, &str)] = &[
    ("youtube", "YouTube"),
    ("github", "GitHub"),
    ("google", "Google"),
    ("reddit", "Reddit"),
    ("twitter", "Twitter"),
    ("wikipedia", "Wikipédia"),
    ("steampowered", "Steam"),
    ("steamcommunity", "Steam"),
    ("nexusmods", "Nexus Mods"),
    ("stackoverflow", "Stack Overflow"),
    ("discord", "Discord"),
    ("microsoft", "Microsoft"),
    ("apple", "Apple"),
    ("spotify", "Spotify"),
    ("amazon", "Amazon"),
    ("instagram", "Instagram"),
    ("facebook", "Facebook"),
    ("linkedin", "LinkedIn"),
    ("pypi", "PyPI"),
    ("npmjs", "NPM"),
    ("twitch", "Twitch"),
    ("chatgpt", "ChatGPT"),
    ("openai", "OpenAI"),
];

/// Runs `re` over `text`, letting `decide` accept each match. `decide` sees the whole text,
/// so it can look at the characters around the match (the lookaround of the Python patterns).
/// A rejected match is skipped and the search resumes one character later, as `re.sub` does.
fn replace_checked<'t>(
    re: &Regex,
    text: &'t str,
    mut decide: impl FnMut(&str, Match<'t>) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while let Some(found) = re.find_at(text, pos) {
        let resume = match decide(text, found) {
            Some(replacement) => {
                out.push_str(&text[last..found.start()]);
                out.push_str(&replacement);
                last = found.end();
                found.end()
            }
            None => found.start() + next_char_len(&text[found.start()..]),
        };
        // An empty match must still move the search forward.
        pos = if found.is_empty() {
            resume.max(found.end()) + next_char_len(&text[found.end()..])
        } else {
            resume
        };
        if pos > text.len() {
            break;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn next_char_len(s: &str) -> usize {
    s.chars().next().map_or(1, char::len_utf8)
}

fn prev_char(text: &str, index: usize) -> Option<char> {
    text[..index].chars().next_back()
}

/// `\w` of the Python patterns: letters, digits and underscore.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lowercases and strips accents (NFKD without combining marks), like `_norm` in intent_guard.
/// Each normalized char is paired with the byte offset where its source char ends in `text`.
fn fold(text: &str) -> Vec<(char, usize)> {
    let mut out = Vec::with_capacity(text.len());
    for (index, c) in text.char_indices() {
        let end = index + c.len_utf8();
        let lower: String = c.to_lowercase().collect();
        for d in lower.as_str().nfkd().filter(|d| !is_combining_mark(*d)) {
            out.push((d, end));
        }
    }
    out
}

fn fold_str(text: &str) -> String {
    fold(text).into_iter().map(|(c, _)| c).collect()
}

/// Replaces the name written in the text ("S.A.G.E.", "SAGE", "Sage") by its pronunciation
/// "Seije" before it becomes voice. File names like `sage.py` are left alone.
///
/// In "S.A.G.E." the last dot is the acronym's: it stays as a full stop only when the name
/// closes the sentence (end of text, or a capitalised word follows).
pub fn pronounce_name(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let dotted = replace_checked(&NAME_DOTTED, text, |src, found| {
        if prev_char(src, found.start()).is_some_and(|c| is_word_char(c) || c == '.' || c == '/') {
            return None;
        }
        if !found.as_str().ends_with('.') {
            return Some(SPOKEN_NAME.to_string());
        }
        let rest = &src[found.end()..];
        let closes = rest.trim().is_empty()
            || (rest.starts_with(char::is_whitespace)
                && rest.trim_start().chars().next().is_some_and(char::is_uppercase));
        Some(if closes {
            format!("{SPOKEN_NAME}.")
        } else {
            SPOKEN_NAME.to_string()
        })
    });
    replace_checked(&NAME_WORD, &dotted, |src, found| {
        if prev_char(src, found.start()).is_some_and(|c| is_word_char(c) || c == '.' || c == '/') {
            return None;
        }
        let after = &src[found.end()..];
        if after.chars().next().is_some_and(is_word_char) || FILE_AFTER_NAME.is_match(after) {
            return None;
        }
        Some(SPOKEN_NAME.to_string())
    })
}

/// Speaks a URL as a service name: "https://youtube.com.br/x" -> "YouTube",
/// "https://docs.rs/x" -> "site docs". Anything unreadable becomes "o link indicado no painel".
pub fn simplify_url_for_speech(url: &str) -> String {
    let Some(netloc) = netloc_of(url) else {
        return LINK_FALLBACK.to_string();
    };
    let netloc = netloc.to_lowercase();
    let host = netloc.strip_prefix("www.").unwrap_or(netloc.as_str());

    match host {
        "x.com" | "t.co" => return "X".to_string(),
        "youtu.be" => return "YouTube".to_string(),
        _ => {}
    }
    if let Some((_, friendly)) = FRIENDLY_DOMAINS
        .iter()
        .find(|(key, _)| host.contains(*key))
    {
        return (*friendly).to_string();
    }

    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 {
        return LINK_FALLBACK.to_string();
    }
    let count = parts.len();
    let base = if count >= 3
        && matches!(parts[count - 1], "br" | "uk")
        && matches!(parts[count - 2], "com" | "org" | "gov")
    {
        parts[count - 3]
    } else {
        parts[0]
    };
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();
    format!("site {}", clean.trim())
}

/// The `host[:port]` part of an URL with a scheme, or `None` when there is none.
fn netloc_of(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or(""))
}

/// Splits a text into sentences the way `re.split(r"(?<=[.!?])\s+")` does.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for run in WHITESPACE_RUN.find_iter(text) {
        if prev_char(text, run.start()).is_some_and(|c| matches!(c, '.' | '!' | '?')) {
            parts.push(&text[start..run.start()]);
            start = run.end();
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Turns raw answer text into something to read aloud: code blocks become a notice, URLs
/// and Windows paths become spoken words, Markdown is stripped, and the result is cut to
/// `max_sentences` (with a pointer to the panel for the rest).
pub fn sanitize_for_speech(text: &str, max_sentences: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let has_code_blocks = CODE_BLOCK.is_match(text);
    let s = CODE_BLOCK.replace_all(text, CODE_NOTICE).into_owned();
    let s = URL
        .replace_all(&s, |caps: &Captures| {
            format!(" {} ", simplify_url_for_speech(&caps[0]))
        })
        .into_owned();
    let s = WINDOWS_PATH
        .replace_all(&s, |caps: &Captures| {
            let filename = caps[0].rsplit('\\').next().unwrap_or("");
            if filename.is_empty() {
                " no caminho indicado no painel ".to_string()
            } else {
                format!(" na pasta do sistema, arquivo {filename} ")
            }
        })
        .into_owned();
    let s = GIT_HASH.replace_all(&s, " código identificador ").into_owned();
    let s = INLINE_CODE.replace_all(&s, " $1 ").into_owned();
    let s = HEADER.replace_all(&s, "").into_owned();
    let s = BOLD.replace_all(&s, "$1").into_owned();
    let s = ITALIC.replace_all(&s, "$1").into_owned();
    let s = UNDERSCORE_BOLD.replace_all(&s, "$1").into_owned();
    let s = UNDERSCORE_ITALIC.replace_all(&s, "$1").into_owned();
    let s = STRIKE.replace_all(&s, "$1").into_owned();
    let s = QUOTE.replace_all(&s, "").into_owned();
    let s = BULLET.replace_all(&s, "").into_owned();
    let s = NUMBERED.replace_all(&s, "").into_owned();
    let s = HTML_TAG.replace_all(&s, " ").into_owned();

    let cleaned = s.split_whitespace().collect::<Vec<_>>().join(" ");

    let sentences = split_sentences(&cleaned);
    if sentences.len() <= max_sentences {
        return cleaned;
    }
    let mut shortened = sentences
        .iter()
        .take(max_sentences)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if !shortened.ends_with(['.', '!', '?']) {
        shortened.push('.');
    }
    if !shortened.to_lowercase().contains("painel") {
        shortened.push_str(if has_code_blocks {
            " Os comandos e códigos completos estão no seu painel para você copiar."
        } else {
            " O restante dos detalhes foi salvo no painel."
        });
    }
    shortened
}

/// Repairs words the speech recogniser often gets wrong ("Beach Hub" -> "GitHub").
pub fn fix_transcription(text: &str) -> String {
    let mut out = text.to_string();
    for (re, replacement) in TRANSCRIPTION_FIXES.iter() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    out
}

/// Removes a leading wake word ("sage", "ei sage", ...) and the misheard variants, plus the
/// configured `wake_words`. "CG, eu quero um print" -> "eu quero um print".
pub fn strip_wake_word(text: &str, wake_words: &[String]) -> String {
    let t = text.trim();
    let folded = fold(t);

    let mut candidates: Vec<&str> = WAKE_MISHEARS
        .iter()
        .copied()
        .chain(wake_words.iter().map(String::as_str))
        .collect();
    // Longest first, so "sei ge" is tried before "sei".
    candidates.sort_by_key(|word| std::cmp::Reverse(word.chars().count()));

    for word in candidates {
        let needle: Vec<char> = fold(word).into_iter().map(|(c, _)| c).collect();
        let n = needle.len();
        if n == 0 || folded.len() < n {
            continue;
        }
        if !folded.iter().zip(&needle).all(|((c, _), wanted)| c == wanted) {
            continue;
        }
        if folded.len() == n {
            return String::new();
        }
        if matches!(folded[n].0, ' ' | ',' | '.') {
            let cut = folded[n - 1].1;
            return t[cut..]
                .trim_start_matches([' ', ',', '.', ':', ';', '!', '-'])
                .to_string();
        }
    }
    t.to_string()
}

/// Heuristic: does this look like a task (vs chit-chat)? Two task verbs, or one in a long
/// request, or a very long request.
pub fn looks_like_task(text: &str) -> bool {
    let p = format!(" {} ", fold_str(text));
    let words = p.split_whitespace().count();
    if p.contains("tarefa complexa") {
        return true;
    }
    let hits = TASK_VERBS
        .iter()
        .filter(|verb| p.contains(fold_str(verb).as_str()))
        .count();
    hits >= 2 || (hits >= 1 && words >= 14) || words >= 40
}

/// Does the request refer to what is on screen? ("esse launcher", "o que estou vendo")
pub fn needs_screen_context(text: &str) -> bool {
    let p = fold_str(text);
    if SEEING.iter().any(|phrase| p.contains(*phrase)) {
        return true;
    }
    DEMONSTRATIVE.captures_iter(&p).any(|caps| {
        let noun = &caps[1];
        !TALK_NOUNS.contains(&noun)
    })
}

/// Reply only promises work ("vou fazer", "deixa comigo") without doing it.
pub fn is_empty_promise(text: &str) -> bool {
    let r = text.trim();
    !r.is_empty() && r.chars().count() < 400 && PROMISE.is_match(r)
}

/// Request complexity in 0.0..=1.0 (port of `estimate_complexity` in model_registry).
pub fn complexity(text: &str) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let low = text.to_lowercase();
    let low = low.trim();
    let words = low.split_whitespace().count() as f32;
    let sentences = text
        .split(['.', '!', '?', '\n'])
        .filter(|segment| !segment.trim().is_empty())
        .count();

    let mut score = 0.10_f32;
    score += (words / 320.0).min(0.26);
    score += 0.15 * hits(low, CODE_HINTS).min(3) as f32;
    score += 0.13 * hits(low, REASONING_HINTS).min(3) as f32;
    score += 0.10 * hits(low, RESEARCH_HINTS).min(2) as f32;
    if sentences >= 3 {
        score += 0.08;
    }
    if low.matches('?').count() >= 2 {
        score += 0.05;
    }
    if COMPLEXITY_MARK.is_match(low) {
        score += 0.07;
    }
    if words <= 4.0 {
        score -= 0.10;
    }
    if hits(low, CASUAL_HINTS) > 0 && words <= 9.0 {
        score -= 0.22;
    }
    score.clamp(0.0, 1.0)
}

/// How many hints appear in the text (a hint listed twice counts twice, as in Python).
fn hits(text: &str, hints: &[&str]) -> usize {
    hints.iter().filter(|hint| text.contains(**hint)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_pronounced_and_the_acronym_dot_is_kept_only_at_the_end() {
        assert_eq!(pronounce_name("Eu sou o S.A.G.E."), "Eu sou o Seije.");
        assert_eq!(pronounce_name("S.A.G.E. abriu o app"), "Seije abriu o app");
        assert_eq!(pronounce_name("Oi, SAGE! Tudo bem?"), "Oi, Seije! Tudo bem?");
        assert_eq!(pronounce_name("isso é S.A.G.E"), "isso é Seije");
    }

    #[test]
    fn file_names_and_longer_words_keep_their_spelling() {
        assert_eq!(pronounce_name("abra o sage.py"), "abra o sage.py");
        assert_eq!(pronounce_name("o sage.exe e o sage.json"), "o sage.exe e o sage.json");
        assert_eq!(pronounce_name("meu_sage e /sage"), "meu_sage e /sage");
        assert_eq!(pronounce_name("sagely"), "sagely");
        assert_eq!(pronounce_name(""), "");
    }

    #[test]
    fn code_blocks_become_a_notice_and_the_text_is_cut_to_the_limit() {
        let out = sanitize_for_speech("Pronto.\n```rust\nfn main() {}\n```\nFeito.", 4);
        assert_eq!(
            out,
            "Pronto. [O código detalhado foi disponibilizado no seu painel.] Feito."
        );
        assert!(!out.contains("```"));
    }

    #[test]
    fn long_answers_are_shortened_with_a_pointer_to_the_panel() {
        assert_eq!(
            sanitize_for_speech("Um. Dois. Três. Quatro. Cinco.", 2),
            "Um. Dois. O restante dos detalhes foi salvo no painel."
        );
    }

    #[test]
    fn urls_become_service_names() {
        assert_eq!(
            sanitize_for_speech("Veja https://www.youtube.com/watch?v=1 agora.", 4),
            "Veja YouTube agora."
        );
        assert_eq!(simplify_url_for_speech("https://x.com/a"), "X");
        assert_eq!(simplify_url_for_speech("https://youtu.be/abc"), "YouTube");
        assert_eq!(simplify_url_for_speech("https://docs.rs/tokio"), "site docs");
        assert_eq!(
            simplify_url_for_speech("https://minha-loja.com.br/x"),
            "site minha-loja"
        );
        assert_eq!(simplify_url_for_speech("https://"), LINK_FALLBACK);
    }

    #[test]
    fn markdown_is_stripped_for_speech() {
        assert_eq!(
            sanitize_for_speech(
                "## Título\n**negrito** e `cmd` e - item\n- um\n1. dois",
                4
            ),
            "Título negrito e cmd e - item um dois"
        );
    }

    #[test]
    fn windows_paths_are_spoken_by_file_name() {
        assert_eq!(
            sanitize_for_speech(r"Abra C:\Users\Rikudai\notas.txt agora.", 4),
            "Abra na pasta do sistema, arquivo notas.txt agora."
        );
    }

    #[test]
    fn transcription_mistakes_are_fixed() {
        assert_eq!(
            fix_transcription("abri o Beach Hub e o Power Shell"),
            "abri o GitHub e o PowerShell"
        );
        assert_eq!(fix_transcription("o git rub"), "o GitHub");
        assert_eq!(fix_transcription("cloud assume a tarefa"), "Claude assume a tarefa");
        assert_eq!(fix_transcription("o cloud code"), "o Claude Code");
        assert_eq!(fix_transcription("a cloud é nuvem"), "a cloud é nuvem");
    }

    #[test]
    fn the_wake_word_is_removed_only_as_a_whole_word() {
        assert_eq!(strip_wake_word("CG, eu quero um print", &[]), "eu quero um print");
        assert_eq!(strip_wake_word("sage abre o navegador", &[]), "abre o navegador");
        assert_eq!(strip_wake_word("Sajé, faz isso", &[]), "faz isso");
        assert_eq!(strip_wake_word("cgx faça", &[]), "cgx faça");
        assert_eq!(strip_wake_word("CG", &[]), "");
        assert_eq!(strip_wake_word("nada a ver", &[]), "nada a ver");
    }

    #[test]
    fn configured_wake_words_are_removed_too() {
        let configured = vec!["Nexo".to_string()];
        assert_eq!(
            strip_wake_word("nexo, liste arquivos", &configured),
            "liste arquivos"
        );
    }

    #[test]
    fn task_requests_need_two_verbs_or_a_long_text() {
        assert!(looks_like_task("crie um script e liste os arquivos"));
        assert!(looks_like_task("tarefa complexa de organização"));
        assert!(looks_like_task(
            "eu queria que você analise esse caso com calma e sem pressa porque ele é importante para mim"
        ));
        assert!(!looks_like_task("oi, tudo bem?"));
        assert!(!looks_like_task("crie algo"));
    }

    #[test]
    fn promises_without_work_are_detected() {
        assert!(is_empty_promise("Vou fazer isso agora mesmo."));
        assert!(is_empty_promise("Deixa comigo!"));
        assert!(!is_empty_promise("Aqui está a lista: 1, 2, 3."));
        assert!(!is_empty_promise("   "));
        let long = format!("Vou fazer. {}", "palavra ".repeat(60));
        assert!(!is_empty_promise(&long));
    }

    #[test]
    fn screen_references_are_detected() {
        assert!(needs_screen_context("o que estou vendo nessa tela?"));
        assert!(needs_screen_context("abre esse launcher aí"));
        assert!(needs_screen_context("quero desse arquivo aqui"));
        assert!(!needs_screen_context("sobre essa conversa"));
        assert!(!needs_screen_context("não quero isso"));
    }

    #[test]
    fn complexity_follows_the_registry_heuristic() {
        assert_eq!(complexity(""), 0.0);
        assert!((complexity("abrir o navegador") - 0.009_375).abs() < 1e-5);
        assert!(complexity("bom dia, tudo bem?") <= 0.001);
        assert!(
            complexity(
                "explique passo a passo a arquitetura do script python, por que o erro acontece? \
                 e como corrigir? primeiro, revise o código, depois teste"
            ) >= 0.55
        );
    }

    #[test]
    fn sentence_split_follows_the_lookbehind_of_the_python_code() {
        assert_eq!(split_sentences("Oi. Tudo bem? Sim"), vec!["Oi.", "Tudo bem?", "Sim"]);
        assert_eq!(split_sentences("sem pontuação"), vec!["sem pontuação"]);
    }

    #[test]
    fn replace_checked_resumes_after_a_rejected_match() {
        let re = fixed("aa");
        // The first "aa" follows an 'x', so it is rejected and the search goes on from the next char.
        let out = replace_checked(&re, "xaa aa", |src, found| {
            (prev_char(src, found.start()) != Some('x')).then(|| "b".to_string())
        });
        assert_eq!(out, "xaa b");
    }
}
