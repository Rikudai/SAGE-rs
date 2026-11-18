"""
Text processing module for S.A.G.E.:
- Sanitizes technical AI responses into natural, pleasant human speech.
- Extracts code snippets, commands, and URLs for easy 1-click clipboard actions.
"""
import re
from typing import List, Dict
from urllib.parse import urlparse

# Friendly domain mappings for natural speech pronunciation in Portuguese
FRIENDLY_DOMAINS = {
    "youtube": "YouTube",
    "github": "GitHub",
    "google": "Google",
    "reddit": "Reddit",
    "twitter": "Twitter",
    "wikipedia": "Wikipédia",
    "steampowered": "Steam",
    "steamcommunity": "Steam",
    "nexusmods": "Nexus Mods",
    "stackoverflow": "Stack Overflow",
    "discord": "Discord",
    "microsoft": "Microsoft",
    "apple": "Apple",
    "spotify": "Spotify",
    "amazon": "Amazon",
    "instagram": "Instagram",
    "facebook": "Facebook",
    "linkedin": "LinkedIn",
    "pypi": "PyPI",
    "npmjs": "NPM",
    "twitch": "Twitch",
    "chatgpt": "ChatGPT",
    "openai": "OpenAI",
}

CODE_BLOCK_REGEX = re.compile(r"```([a-zA-Z0-9_\-\+]*)\n([\s\S]*?)```")
INLINE_CODE_REGEX = re.compile(r"`([^`]+)`")
WINDOWS_PATH_REGEX = re.compile(r"[a-zA-Z]:\\(?:[^\\/:*?\"<>|\r\n]+\\)+[^\\/:*?\"<>|\r\n]*")
GIT_HASH_REGEX = re.compile(r"\b[0-9a-fA-F]{20,40}\b")
ANSI_ESCAPE_REGEX = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")
TOOL_LOG_REGEX = re.compile(r"^\[Tool.*?\].*$", re.MULTILINE)


def clean_cli_response(raw_output: str) -> str:
    """
    Cleans raw CLI output from Antigravity:
    - Strips ANSI escape sequences.
    - Filters out internal diagnostic lines (jetski permissions, conversation warnings).
    - Removes internal tool call log echoes.
    - Returns clean conversational response for user/speech.
    """
    if not raw_output or not raw_output.strip():
        return "Ação concluída com sucesso."

    # Remove ANSI escape sequences
    text = ANSI_ESCAPE_REGEX.sub("", raw_output)

    # Remove tool call lines
    text = TOOL_LOG_REGEX.sub("", text)

    # Filter line by line
    clean_lines = []
    for line in text.splitlines():
        line_stripped = line.strip()
        # Filter jetski permission warnings
        if line_stripped.startswith("jetski:"):
            continue
        # Filter conversation warnings
        if line_stripped.startswith("warning: conversation") or line_stripped.startswith("warning:"):
            continue
        # Filter tool echoes
        if line_stripped.startswith("[Tool:") or line_stripped.startswith("[Tool Result:"):
            continue
        clean_lines.append(line)

    result = "\n".join(clean_lines).strip()
    return result if result else "Ação concluída com sucesso."


def simplify_url_for_speech(url: str) -> str:
    """
    Converts a raw URL into a natural speech name.
    E.g. 'https://youtube.com.br/watch?v=...' -> 'YouTube'
    """
    try:
        parsed = urlparse(url)
        netloc = (parsed.netloc or "").lower()
        if netloc.startswith("www."):
            netloc = netloc[4:]

        # Special short domains
        if netloc in ("x.com", "t.co"):
            return "X"
        if netloc in ("youtu.be",):
            return "YouTube"

        # Check friendly keyword in netloc
        for key, friendly_name in FRIENDLY_DOMAINS.items():
            if key in netloc:
                return friendly_name

        # Fallback: clean domain basename
        parts = netloc.split(".")
        if len(parts) >= 2:
            base_name = parts[-3] if len(parts) >= 3 and parts[-1] in ("br", "uk") and parts[-2] in ("com", "org", "gov") else parts[0]
            clean_name = re.sub(r"[^a-zA-Z0-9\-]", " ", base_name).strip()
            return f"site {clean_name}"

        return "o link indicado no painel"
    except Exception:
        return "o link indicado no painel"


def extract_code_blocks(text: str) -> List[Dict[str, str]]:
    """
    Extracts all code blocks (fenced with ```) from text.
    Returns a list of dicts: [{'lang': '...', 'code': '...'}]
    """
    matches = CODE_BLOCK_REGEX.findall(text)
    blocks = []
    for lang, code in matches:
        blocks.append({
            "lang": lang.strip() if lang else "text",
            "code": code.strip()
        })
    return blocks


def extract_urls(text: str) -> List[str]:
    """
    Extracts all unique URLs found in text in order of appearance.
    """
    matches = re.findall(r"https?://[^\s'\"\)\]]+", text)
    seen = set()
    urls = []
    for u in matches:
        # Strip trailing punctuation that might be caught
        cleaned = u.rstrip(".,;!?:")
        if cleaned not in seen:
            seen.add(cleaned)
            urls.append(cleaned)
    return urls


# ---------------------------------------------------------------------------
# Pronúncia do nome: escrito S.A.G.E., falado "Seije"
# ---------------------------------------------------------------------------
SPOKEN_NAME = "Seije"
_NAME_DOTTED = re.compile(r"(?<![\w./])S\.\s?A\.\s?G\.\s?E\b\.?", re.IGNORECASE)
_NAME_WORD = re.compile(r"(?<![\w./])sage(?![\w.-]*\.(?:py|com|json|exe)\b)(?!\w)", re.IGNORECASE)


def pronounce_name(text: str) -> str:
    """Troca o nome escrito (S.A.G.E., SAGE, Sage) pela pronúncia "Seije" antes de virar voz.

    Em "S.A.G.E." o último ponto é da sigla; ele continua como ponto final só quando o nome fecha
    a frase (fim do texto ou próxima palavra com maiúscula).
    """
    if not text:
        return text

    def dotted(m: "re.Match") -> str:
        if not m.group(0).endswith("."):
            return SPOKEN_NAME
        rest = text[m.end():]
        closes = not rest.strip() or (rest[:1].isspace() and rest.lstrip()[:1].isupper())
        return SPOKEN_NAME + ("." if closes else "")
    return _NAME_WORD.sub(SPOKEN_NAME, _NAME_DOTTED.sub(dotted, text))


def sanitize_for_speech(text: str, max_sentences: int = 4) -> str:
    """
    Transforms raw AI technical output into natural, clean spoken Portuguese.
    - Code blocks are summarized as notifications.
    - URLs are replaced by conversational service names.
    - Markdown symbols, headers, and bullet chars are stripped.
    - File paths are spoken comfortably.
    - Overly long output is politely condensed for TTS.
    """
    if not text:
        return ""

    processed = text

    # 1. Replace multi-line code blocks with speech-friendly transition
    has_code_blocks = bool(CODE_BLOCK_REGEX.search(processed))
    processed = CODE_BLOCK_REGEX.sub(" [O código detalhado foi disponibilizado no seu painel.] ", processed)

    # 2. Replace URLs with friendly names
    def _replace_url(match):
        full_match = match.group(0)
        return f" {simplify_url_for_speech(full_match)} "

    processed = re.sub(r"https?://[^\s'\"\)\]]+", _replace_url, processed)

    # 3. Replace Windows file paths with clean verbal reference
    def _replace_path(match):
        full_path = match.group(0)
        filename = full_path.split("\\")[-1]
        if filename:
            return f" na pasta do sistema, arquivo {filename} "
        return " no caminho indicado no painel "

    processed = WINDOWS_PATH_REGEX.sub(_replace_path, processed)

    # 4. Replace long git commit hashes
    processed = GIT_HASH_REGEX.sub(" código identificador ", processed)

    # 5. Clean inline code backticks: `cmd` -> cmd
    processed = INLINE_CODE_REGEX.sub(r" \1 ", processed)

    # 6. Clean markdown headers (#, ##, ###)
    processed = re.sub(r"^[ \t]*#+[ \t]*", "", processed, flags=re.MULTILINE)

    # 7. Clean bold and italic markdown markers
    processed = re.sub(r"\*\*([^*]+)\*\*", r"\1", processed)
    processed = re.sub(r"\*([^*]+)\*", r"\1", processed)
    processed = re.sub(r"__([^_]+)__", r"\1", processed)
    processed = re.sub(r"_([^_]+)_", r"\1", processed)
    processed = re.sub(r"~~([^~]+)~~", r"\1", processed)

    # 8. Clean blockquotes and bullet points
    processed = re.sub(r"^[ \t]*>[ \t]*", "", processed, flags=re.MULTILINE)
    processed = re.sub(r"^[ \t]*[-*+][ \t]+", "", processed, flags=re.MULTILINE)
    processed = re.sub(r"^[ \t]*\d+\.[ \t]+", "", processed, flags=re.MULTILINE)

    # 9. Clean HTML tags
    processed = re.sub(r"<[^>]+>", " ", processed)

    # 10. Clean multiple spaces and blank lines
    lines = [line.strip() for line in processed.splitlines() if line.strip()]
    cleaned_text = " ".join(lines)
    cleaned_text = re.sub(r"\s+", " ", cleaned_text).strip()

    # 11. Limit spoken length to max_sentences for comfortable listening
    sentence_delimiters = re.split(r"(?<=[.!?])\s+", cleaned_text)
    if len(sentence_delimiters) > max_sentences:
        shortened = " ".join(sentence_delimiters[:max_sentences])
        if not shortened.endswith((".", "!", "?")):
            shortened += "."
        if "painel" not in shortened.lower():
            if has_code_blocks:
                shortened += " Os comandos e códigos completos estão no seu painel para você copiar."
            else:
                shortened += " O restante dos detalhes foi salvo no painel."
        return shortened

    return cleaned_text
