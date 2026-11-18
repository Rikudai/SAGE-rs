"""
Guards that keep S.A.G.E. from "talking instead of doing".

* ``fix_transcription``: repairs words the speech recogniser often gets wrong
  ("Beach Hub" -> GitHub, "CG" at the start -> wake word).
* ``looks_like_task``: does the request ask for real work (search, create,
  list, analyse...) rather than conversation?
* ``is_empty_promise``: a reply that only *promises* to do something.
* ``needs_screen_context``: the request points at something the user is looking
  at ("esse launcher", "o que estou vendo"), so a screenshot is the context.
* ``mentions_screenshot``: an ack that would announce the (silent) screenshot.
"""
from __future__ import annotations

import re
import unicodedata

TRANSCRIPTION_FIXES = [
    (r"\b(beach|bitch|guit|git|kit|gift)\s*-?\s*hub\b", "GitHub"),
    (r"\bgit\s*rub\b", "GitHub"),
    (r"\bpai\s*ton\b|\bpaiton\b", "Python"),
    (r"\bpower\s*shell\b", "PowerShell"),
    (r"\bvs\s*code\b|\bvisual\s*code\b", "VS Code"),
    (r"\bollama\b|\bo\s*lama\b", "Ollama"),
    (r"\bcloud\s*code\b|\bclaud\s*code\b", "Claude Code"),
    (r"\bclaud\b|\bcloud\b(?=\s+(assume|pensa|faz))", "Claude"),
]
# things the recogniser hears instead of "SAGE" at the start of a request
WAKE_MISHEARS = ("cg", "c g", "seige", "saige", "sajé", "saje", "sei ge", "sage", "s a g e", "ceagê", "cê gê")

TASK_VERBS = (
    "busque", "buscar", "busca ", "procure", "procurar", "pesquise", "pesquisar", "pesquisa ",
    "crie", "criar", "cria ", "gere", "gerar", "liste", "listar", "lista ", "monte", "montar",
    "faça", "faca", "fazer uma", "escreva", "escrever", "compare", "comparar", "analise",
    "analisar", "encontre", "encontrar", "levante", "levantar", "organize", "organizar",
    "resuma", "resumir", "traduza", "planeje", "planejar", "implemente", "implementar",
    "desenvolva", "programe", "calcule", "verifique", "verificar", "baixe", "relatório",
    "relatorio", "tabela", "planilha", "documento", "tarefa complexa", "tarefa",
)
PROMISE = re.compile(
    r"\b(vou|irei|farei|faço|faco|já vou|ja vou|deixa comigo|pode deixar|vou já|estou fazendo|"
    r"vou (fazer|buscar|procurar|pesquisar|criar|montar|gerar|listar|analisar|verificar|preparar))\b",
    re.IGNORECASE)


def _norm(text: str) -> str:
    t = unicodedata.normalize("NFKD", (text or "").lower())
    return "".join(c for c in t if not unicodedata.combining(c))


def fix_transcription(text: str) -> str:
    out = text or ""
    for pattern, repl in TRANSCRIPTION_FIXES:
        out = re.sub(pattern, repl, out, flags=re.IGNORECASE)
    return out


def strip_misheard_wake(text: str) -> str:
    """Removes a leading misheard wake word ("CG, eu quero..." -> "eu quero...")."""
    t = (text or "").strip()
    low = _norm(t)
    for w in sorted(WAKE_MISHEARS, key=len, reverse=True):
        wn = _norm(w)
        if low == wn:
            return ""
        if low.startswith(wn + " ") or low.startswith(wn + ",") or low.startswith(wn + "."):
            return t[len(w):].lstrip(" ,.:;!-")
    return t


def looks_like_task(text: str) -> bool:
    p = " " + _norm(text) + " "
    words = len(p.split())
    hits = sum(1 for v in TASK_VERBS if _norm(v) in p)
    if "tarefa complexa" in p:
        return True
    return (hits >= 2) or (hits >= 1 and words >= 14) or words >= 40


def is_empty_promise(reply: str) -> bool:
    r = (reply or "").strip()
    return bool(r) and len(r) < 400 and bool(PROMISE.search(r))


# "esse/essa/este..." + a noun that is NOT about the conversation itself
_DEMONSTRATIVE = re.compile(
    r"\b(?:n|d)?(?:ess[ea]s?|est[ea]s?|aquel[ea]s?)\s+([a-z0-9_-]{3,})")
_TALK_NOUNS = {
    "conversa", "assunto", "ideia", "vez", "vezes", "caso", "jeito", "forma", "sentido", "momento",
    "semana", "dia", "ano", "mes", "pergunta", "resposta", "tarefa", "tipo", "ponto", "motivo",
    "problema", "questao", "noite", "tarde", "manha", "hora", "trabalho", "pedido", "que", "aqui",
    "ultimo", "ultima", "mesmo", "mesma", "seu", "sua", "meu", "minha", "negocio", "coisa", "papo",
}
_SEEING = ("estou vendo", "to vendo", "tou vendo", "eu vejo", "na tela", "minha tela", "na minha frente",
           "aberto aqui", "aberta aqui", "que abri", "que eu abri", "que esta aberto", "que esta aberta",
           "ta aberto", "ta aberta", "janela aberta", "olha a tela", "olha minha tela", "isso aqui",
           "esse aqui", "essa aqui")
_SHOT_WORDS = re.compile(r"\b(print|prints|captur\w*|screenshot|foto|fotografar|tela)\b")


def needs_screen_context(text: str) -> bool:
    """True when the user refers to something on screen without naming it fully."""
    p = _norm(text)
    if any(s in p for s in _SEEING):
        return True
    return any(noun not in _TALK_NOUNS for noun in _DEMONSTRATIVE.findall(p))


def mentions_screenshot(text: str) -> bool:
    return bool(_SHOT_WORDS.search(_norm(text)))
