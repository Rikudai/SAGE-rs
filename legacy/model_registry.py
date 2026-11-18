"""
Task scopes, model profiles and routing for S.A.G.E.

The assistant picks a *working model* per task instead of using one model for
everything:

    chat      -> fast local model, short answers
    system    -> fast local model, tool/action decisions
    coding    -> code-specialised local model
    reasoning -> balanced or deep local model (complexity based)
    vision    -> local multimodal model (screenshots / UI pointing)
    ocr       -> local multimodal model, transcription style
    research  -> balanced local + optional Antigravity online
    agent     -> Antigravity (online) with local deep as contingency

Only Antigravity is remote. All other profiles are strictly local.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Sequence, Tuple


@dataclass(frozen=True)
class ScopeProfile:
    key: str
    label: str
    model: str
    fallbacks: Tuple[str, ...] = ()
    temperature: float = 0.35
    max_tokens: int = 700
    style: str = "short"          # short | detailed
    online: bool = False
    description: str = ""


@dataclass
class Route:
    scope: str
    label: str
    model: str
    complexity: float
    tier: str
    reason: str
    online: bool = False
    style: str = "short"

    def describe(self) -> str:
        return f"{self.label} · {self.model} · complexidade {self.complexity:.2f}"


SCOPES: Dict[str, ScopeProfile] = {
    "chat": ScopeProfile(
        "chat", "Conversa", "qwen2.5:3b", ("qwen2.5:7b",),
        temperature=0.75, max_tokens=260, style="short",
        description="Conversa rápida, respostas curtas e espirituosas.",
    ),
    "system": ScopeProfile(
        "system", "Sistema", "qwen2.5:3b", ("qwen2.5:7b",),
        temperature=0.15, max_tokens=320, style="short",
        description="Comandos de PC, ferramentas e ações locais.",
    ),
    "coding": ScopeProfile(
        "coding", "Engenharia", "qwen2.5-coder:7b", ("qwen2.5:7b", "qwen2.5:3b"),
        temperature=0.15, max_tokens=1400, style="detailed",
        description="Código, scripts, debugging e automação.",
    ),
    "reasoning": ScopeProfile(
        "reasoning", "Raciocínio", "qwen2.5:7b", ("gemma4:12b", "qwen2.5:3b"),
        temperature=0.30, max_tokens=1100, style="detailed",
        description="Análise, arquitetura, comparações e explicações.",
    ),
    "vision": ScopeProfile(
        "vision", "Visão", "qwen2.5vl:3b", ("qwen2.5:7b", "qwen2.5:3b"),
        temperature=0.20, max_tokens=700, style="short",
        description="Leitura de telas, ícones e apontamento de cliques.",
    ),
    "ocr": ScopeProfile(
        "ocr", "OCR", "qwen2.5vl:3b", ("qwen2.5:3b",),
        temperature=0.05, max_tokens=1600, style="detailed",
        description="Transcrição literal de texto em imagens.",
    ),
    "research": ScopeProfile(
        "research", "Pesquisa", "qwen2.5:7b", ("gemma4:12b", "qwen2.5:3b"),
        temperature=0.35, max_tokens=1200, style="detailed", online=True,
        description="Pesquisa e síntese; pode usar Antigravity online.",
    ),
    "agent": ScopeProfile(
        "agent", "Agente online", "gemma4:12b", ("qwen2.5:7b", "qwen2.5:3b"),
        temperature=0.30, max_tokens=1400, style="detailed", online=True,
        description="Tarefas grandes delegadas ao Antigravity (contingência local).",
    ),
}

CODE_HINTS = (
    "codigo", "código", "script", "programa", "funcao", "função", "classe",
    "refator", "debug", "bug", "erro", "traceback", "regex", "sql", "api",
    "python", "powershell", "javascript", "typescript", "html", "css", "git",
    "docker", "compilar", "teste unitario", "teste unitário", "automatizar",
    "terminal", "bash", "pip", "npm", "json", "yaml",
)
VISION_HINTS = (
    "print", "captura de tela", "screenshot", "tela", "imagem", "foto",
    "onde clico", "onde eu clico", "onde fica", "aponte", "aponta",
    "o que estou vendo", "o que tem na tela", "analisa a tela", "analise a tela",
    "icone", "ícone", "janela", "botao", "botão", "cor", "layout",
)
OCR_HINTS = (
    "ocr", "transcreva", "transcrever", "leia o texto", "ler o texto",
    "texto da imagem", "texto da tela", "extraia o texto", "copie o texto",
)
SYSTEM_HINTS = (
    "abrir", "abre ", "fechar", "fecha ", "volume", "mutar", "mudo",
    "pausar", "tocar", "bloquear", "minimizar", "status", "cpu", "ram",
    "memoria", "memória", "processo", "janela", "mouse", "clicar", "clique",
    "digitar", "digite", "atalho", "powershell", "cmd", "executar", "rode",
    "rodar", "instalar", "desinstalar", "pasta", "arquivo", "arquivos",
    "criar script", "salvar", "copiar", "colar", "clipboard",
)
RESEARCH_HINTS = (
    "pesquisar", "pesquise", "procure", "google", "youtube", "internet",
    "documentacao", "documentação", "noticias", "notícias", "artigo",
    "github", "stack overflow", "antigravity", "gemini", "nuvem", "online",
    "web", "site", "baixar", "download",
)
REASONING_HINTS = (
    "por que", "porque", "como funciona", "explique", "explica", "analise",
    "analise", "compare", "planeje", "plano", "arquitetura", "estrategia",
    "estratégia", "resuma", "resumo", "diferenca", "diferença", "calcule",
    "resolva", "prove", "raciocinio", "raciocínio", "passo a passo",
)
CASUAL_HINTS = (
    "oi", "ola", "olá", "bom dia", "boa tarde", "boa noite", "tudo bem",
    "obrigado", "obrigada", "valeu", "tchau", "piada", "conte algo",
)


def _hits(text: str, hints: Sequence[str]) -> int:
    return sum(1 for h in hints if h in text)


def estimate_complexity(prompt: str) -> float:
    """Heuristic complexity in [0, 1]."""
    if not prompt:
        return 0.0
    text = prompt.lower().strip()
    words = len(text.split())
    sentences = len([s for s in re.split(r"[.!?\n]+", prompt) if s.strip()])

    score = 0.10
    score += min(0.26, words / 320.0)
    score += 0.15 * min(3, _hits(text, CODE_HINTS))
    score += 0.13 * min(3, _hits(text, REASONING_HINTS))
    score += 0.10 * min(2, _hits(text, RESEARCH_HINTS))
    if sentences >= 3:
        score += 0.08
    if text.count("?") >= 2:
        score += 0.05
    if re.search(r"\b(1\.|2\.|primeiro|depois|em seguida|entao|então)\b", text):
        score += 0.07
    if words <= 4:
        score -= 0.10
    if _hits(text, CASUAL_HINTS) and words <= 9:
        score -= 0.22
    return max(0.0, min(1.0, score))


def classify_scope(prompt: str) -> Tuple[str, float, str]:
    """Returns (scope, complexity, short human reason)."""
    text = (prompt or "").lower().strip()
    complexity = estimate_complexity(prompt)

    if _hits(text, OCR_HINTS):
        return "ocr", complexity, "pedido de transcrição/OCR"
    if _hits(text, VISION_HINTS) and not _hits(text, CODE_HINTS):
        return "vision", complexity, "pedido visual"
    if _hits(text, CODE_HINTS):
        return "coding", max(complexity, 0.5), "pedido de engenharia"
    if any(w in text for w in ("antigravity", "agy", "gemini", "nuvem", "agente online")):
        return "agent", max(complexity, 0.6), "pedido explícito do agente online"
    if _hits(text, SYSTEM_HINTS):
        return "system", complexity, "comando de sistema"
    if _hits(text, RESEARCH_HINTS) and complexity >= 0.3:
        return "research", complexity, "pedido de pesquisa"
    if _hits(text, REASONING_HINTS) or complexity >= 0.55:
        return "reasoning", complexity, "pedido analítico"
    return "chat", complexity, "conversa"


class ModelRegistry:
    def __init__(self, profiles: Optional[Dict[str, ScopeProfile]] = None,
                 available: Optional[Sequence[str]] = None,
                 deep_model: str = "gemma4:12b",
                 balanced_model: str = "qwen2.5:7b",
                 code_model: str = "qwen2.5-coder:7b",
                 fast_model: str = "qwen2.5:3b",
                 vision_model: str = "qwen2.5vl:3b"):
        self.profiles = dict(profiles or SCOPES)
        self.available: List[str] = list(available or [])
        # Overrides coming from config
        if deep_model:
            self.profiles["reasoning"] = ScopeProfile(
                "reasoning", "Raciocínio", balanced_model, (deep_model, fast_model),
                temperature=0.30, max_tokens=1100, style="detailed",
                description="Análise; sobe para o modelo profundo em tarefas pesadas.",
            )
            self.profiles["agent"] = ScopeProfile(
                "agent", "Agente online", deep_model, (balanced_model, fast_model),
                temperature=0.30, max_tokens=1400, style="detailed", online=True,
                description="Contingência local para o Antigravity.",
            )
        self.profiles["coding"] = ScopeProfile(
            "coding", "Engenharia", code_model, (balanced_model, fast_model),
            temperature=0.15, max_tokens=1400, style="detailed",
            description="Código, scripts, debugging e automação.",
        )
        self.profiles["chat"] = ScopeProfile(
            "chat", "Conversa", fast_model, (balanced_model,),
            temperature=0.75, max_tokens=260, style="short",
            description="Conversa rápida e curta.",
        )
        self.profiles["system"] = ScopeProfile(
            "system", "Sistema", fast_model, (balanced_model,),
            temperature=0.15, max_tokens=320, style="short",
            description="Comandos e ferramentas.",
        )
        self.profiles["research"] = ScopeProfile(
            "research", "Pesquisa", balanced_model, (deep_model, fast_model),
            temperature=0.35, max_tokens=1200, style="detailed", online=True,
            description="Pesquisa e síntese.",
        )
        self.profiles["vision"] = ScopeProfile(
            "vision", "Visão", vision_model, (balanced_model, fast_model),
            temperature=0.20, max_tokens=700, style="short",
            description="Leitura de tela.",
        )
        self.profiles["ocr"] = ScopeProfile(
            "ocr", "OCR", vision_model, (fast_model,),
            temperature=0.05, max_tokens=1600, style="detailed",
            description="Transcrição de texto.",
        )
        self.deep_model = deep_model
        self.balanced_model = balanced_model
        self.code_model = code_model
        self.fast_model = fast_model
        self.vision_model = vision_model

    # ------------------------------------------------------------------
    def set_available(self, models: Sequence[str]) -> None:
        self.available = [m for m in models if m]

    def is_local_available(self, model: str) -> bool:
        if not self.available:
            return True
        base = model.lower().split(":")[0]
        return any(m.lower() == model.lower() or m.lower().split(":")[0] == base
                   for m in self.available)

    def _first_available(self, models: Sequence[str]) -> str:
        for model in models:
            if model and self.is_local_available(model):
                return model
        return self.fast_model

    def resolve(self, scope: str, complexity: float) -> Route:
        profile = self.profiles.get(scope) or self.profiles["chat"]
        model = profile.model
        tier = "fast"
        reason = profile.description

        if scope == "reasoning":
            if complexity >= 0.72 and self.deep_model:
                model = self.deep_model
                tier = "deep"
                reason = "tarefa complexa: modelo profundo local"
            else:
                model = self.balanced_model
                tier = "balanced"
        elif scope == "coding":
            model = self.code_model
            tier = "code"
        elif scope in ("vision", "ocr"):
            model = self.vision_model
            tier = "vision"
        elif scope == "research":
            model = self.balanced_model
            tier = "balanced"
        elif scope == "agent":
            model = self.deep_model
            tier = "deep"
        else:
            tier = "fast"

        available_model = self._first_available((model,) + tuple(profile.fallbacks))
        if available_model != model:
            reason = reason + " (fallback: " + available_model + ")"
            if available_model == self.fast_model:
                tier = "fast"
        return Route(scope=scope, label=profile.label, model=available_model,
                     complexity=complexity, tier=tier, reason=reason,
                     online=profile.online, style=profile.style)

    def route_prompt(self, prompt: str) -> Route:
        scope, complexity, reason = classify_scope(prompt)
        route = self.resolve(scope, complexity)
        if not route.reason:
            route.reason = reason
        return route

    def status(self) -> Dict[str, object]:
        return {
            "fast": self.fast_model,
            "balanced": self.balanced_model,
            "code": self.code_model,
            "deep": self.deep_model,
            "vision": self.vision_model,
            "available": list(self.available),
            "scopes": {k: v.model for k, v in self.profiles.items()},
        }
