"""
Hybrid Cognitive Cortex for S.A.G.E.
===================================
* Personality: sarcastic, sharp, irreverent and concise by default.
* Task-scoped routing: chat, system, coding, reasoning, vision, OCR,
  research and online agent (see sage.model_registry).
* Structured plans: intent, spoken reply, emotion, PC action, screen pointer.
* Visual perception: screenshots get a coordinate grid and are analysed by
  Gemini (multimodal); OCR mode extracts literal text.
* Embeddings for the long-term memory (gemini-embedding-001).
* Backend: Google Gemini via AI Studio API key (sage.gemini).
"""
from __future__ import annotations

import base64
import io
import json
import re
import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from sage.gemini import DEFAULT_EMBED_MODEL, DEFAULT_MODEL, GeminiClient
from sage.model_registry import ModelRegistry, Route, classify_scope, estimate_complexity

try:
    from PIL import Image, ImageDraw
except Exception:  # pragma: no cover
    Image = None

VALID_ACTIONS = {
    "none", "open_app", "web_search", "open_url", "system_status", "media_control",
    "screen_capture", "lock_pc", "minimize_all", "run_command", "write_script",
    "run_script", "run_python", "click_at", "type_text", "press_keys",
    "analyze_screen", "point_screen", "change_form", "list_windows",
    "focus_window", "clipboard", "file_read", "file_write", "file_edit",
    "file_list", "file_search", "ocr_screen", "ocr_image", "fetch_url",
    "search_web", "shell", "restart_app",
    # auto-evolução (sage/evolution.py)
    "use_skill", "create_skill", "improve_skill", "list_skills", "delete_skill", "self_update", "evolve_core",
}

# Catálogo das skills que o S.A.G.E. escreveu; o orquestrador liga isto ao banco (sage.skills.SkillBank.catalog).
SKILL_CATALOG = lambda: ""

# Regras fixas da auto-evolução: entram em todo prompt do roteador, junto com o catálogo, mesmo que o
# usuário tenha reescrito as regras do roteador no painel (data/persona.json).
EVOLUTION_RULES = """
VOCÊ EVOLUI (intent SYSTEM_ACTION, ações extras além das listadas acima). Suas skills são funções que você mesmo escreveu (lista no fim):
   - use_skill: uma skill existente resolve o pedido -> skill "nome", args {parâmetros dela}. Vem antes
     de search_web, fetch_url e create_skill.
   - create_skill: escrever uma skill nova -> text com a descrição completa (entradas, saída, fonte dos
     dados). Use quando o usuário pedir ("aprenda a...", "crie uma skill/função que...") E por conta
     própria quando o pedido é algo REPETÍVEL que código resolveria (dados de API pública como cotação,
     clima, CEP; cálculo; automação do PC) e nenhuma skill existente resolve. No "ack", diga que vai aprender.
   - improve_skill: corrigir/mudar uma skill (o usuário reclamou do resultado) -> skill "nome", text o que mudar
   - list_skills: "o que você aprendeu?", "quais suas habilidades?"; delete_skill: skill "nome"
   - self_update: "se atualize", "tem atualização?" (traz a versão nova do GitHub e reinicia)
   - evolve_core: o usuário pediu para mudar o SEU PRÓPRIO código-fonte ("implemente em você mesmo...",
     "mude seu comportamento para...") -> text com a tarefa. Só a pedido explícito.
"""

from sage import persona as _persona

# The full personality lives in sage/persona.py (single source of truth).
PERSONA = _persona.core()

ROUTER_RULES = """

Você controla TOTALMENTE o computador do usuário: PowerShell, arquivos, mouse, teclado,
janelas, screenshots, OCR e internet. Tela atual: {screen_w}x{screen_h}.

Responda SEMPRE apenas com um objeto JSON válido:
{
  "intent": "DIRECT_ANSWER" | "SYSTEM_ACTION" | "DELEGATE_AGENT",
  "ack": "confirmação curta e afiada (1 frase)",
  "reply": "resposta curta em português, estilo S.A.G.E. (1-2 frases)",
  "action": "none" | "open_app" | "web_search" | "open_url" | "system_status" |
            "media_control" | "screen_capture" | "lock_pc" | "minimize_all" |
            "run_command" | "shell" | "write_script" | "run_script" | "run_python" |
            "file_read" | "file_write" | "file_edit" | "file_list" | "file_search" |
            "ocr_screen" | "ocr_image" | "click_at" | "type_text" | "press_keys" |
            "analyze_screen" | "point_screen" | "change_form" | "list_windows" |
            "focus_window" | "clipboard" | "fetch_url" | "search_web" | "restart_app" |
            "use_skill" | "create_skill" | "improve_skill" | "list_skills" | "delete_skill" |
            "self_update" | "evolve_core",
  "action_params": {"target": "", "query": "", "command": "", "path": "", "content": "",
                    "script": "", "x": 0, "y": 0, "text": "", "keys": "", "form": "", "url": "",
                    "skill": "", "args": {}},
  "emotion": "idle" | "feliz" | "rindo" | "amor" | "orgulhoso" | "comemorando" | "curioso" | "surpreso" | "pensando" | "determinado" | "alerta" | "confuso" | "envergonhado" | "entediado" | "medo" | "bravo" | "foto" | "triste",
  "requires_screen": false,
  "pointer": null,
  "agent_task": ""
}

REGRAS:
1. SYSTEM_ACTION para comandos no PC:
   - aplicativos -> open_app, target "chrome"/"code"/...
   - pesquisa -> web_search, query; abrir link -> open_url, url
   - status/hardware -> system_status; volume -> media_control, command "mute"/"volume_up"/...
   - capturar tela -> screen_capture; ajuda visual -> analyze_screen
   - OCR de tela -> ocr_screen; OCR de imagem/arquivo -> ocr_image, path "..."
   - ler/alterar arquivo -> file_read / file_write / file_edit, path, content
   - listar/buscar arquivos -> file_list / file_search, path, query
   - terminal -> shell, command "..."; comando avulso -> run_command, command "..."
     (SEMPRE comando PowerShell de Windows; veja SISTEMA OPERACIONAL acima)
   - python -> run_python, content "..."
   - mouse/teclado -> click_at x,y; type_text text; press_keys keys "ctrl+s"
   - janelas -> focus_window target; list_windows; clipboard text
   - internet -> fetch_url url "https://..." (texto da página)
   - pesquisa web -> search_web, query "..." (resultados em texto, sem abrir navegador)
   - mudar forma -> change_form, form "slime"/"humano"/"alado"
   - reiniciar o app -> restart_app
2. AJUDA VISUAL: se o usuário pedir "onde clico", "o que tem na tela", "me mostra", use
   action "analyze_screen" e requires_screen true. O sistema mostrará a imagem para você.
3. POINTER: se souber o ponto exato, preencha "pointer": {"x": px, "y": py, "label": "curto"}.
4. DIRECT_ANSWER para conversa/perguntas: responda em "reply", curto e com personalidade.
5. DELEGATE_AGENT para trabalho pesado (código grande, pesquisa, várias etapas, criar/editar vários
   arquivos, usar Skills/MCPs) — o cérebro principal vai criar clones para isso. Também é
   DELEGATE_AGENT tudo que gera mídia ou exige especialista: gerar/editar IMAGEM, compor MÚSICA,
   narrar/gerar ÁUDIO de fala, TRANSCREVER áudio, TRADUZIR textos longos, PESQUISA PROFUNDA
   (relatório com muitas fontes), escrita criativa longa e documentos técnicos densos.
6. Não invente ações. Em dúvida, DIRECT_ANSWER. Para tarefas reais, prefira ferramentas.
7. CONTEXTO DA TELA: se o pedido aponta para algo que o usuário está vendo ("esse programa",
   "isso aqui", "o que estou vendo", "essa pasta aberta") e você não sabe exatamente o que é,
   marque "requires_screen": true. O sistema tira o print em silêncio e continua o trabalho a
   partir dele: NÃO anuncie o print no "ack" e não chute comandos sem esse contexto.
8. Ações de consulta (run_command, shell, file_*, fetch_url, search_web, ocr_*) entram num laço:
   o resultado volta para você decidir o próximo passo até o pedido estar respondido. Escolha
   aqui só o PRIMEIRO passo; "reply" pode ficar vazio.
"""

ROUTER_SYSTEM = PERSONA + "\n" + _persona.AUTONOMY + _persona.WINDOWS_RULES + ROUTER_RULES

CHAT_STYLE_SHORT = (
    "Responda em no máximo 2 frases curtíssimas, com sarcasmo leve e precisão. "
    "Sem introdução, sem repetir a pergunta, sem emojis."
)
CHAT_STYLE_LONG = (
    "Responda com clareza e profundidade técnica, mas ainda com personalidade afiada. "
    "Use listas curtas quando ajudar. Sem enrolação e sem emojis."
)

VISION_SYSTEM = (
    "Você é S.A.G.E. analisando uma captura de tela. A imagem tem uma grade de referência "
    "a cada 120 px, com coordenadas nas margens. Use a grade para localizar elementos. "
    "Responda apenas JSON."
)
OCR_SYSTEM = (
    "Você é um motor de OCR. Transcreva LITERALMENTE todo o texto visível na imagem, "
    "preservando quebras de linha. Não interprete, não resuma, não adicione comentários."
)



try:
    from sage.pixel_avatar import LLM_EMOTIONS as _LLM_EMOTIONS
except Exception:                                  # keeps the cortex usable without the avatar
    _LLM_EMOTIONS = ("idle", "feliz", "pensando", "bravo", "alerta", "confuso", "foto", "triste")


@dataclass
class CortexDecision:
    intent: str
    ack: str
    reply: str
    action: str = "none"
    action_params: Dict[str, Any] = field(default_factory=dict)
    emotion: str = "idle"
    requires_screen: bool = False
    agent_task: str = ""
    source: str = "local"
    elapsed_seconds: float = 0.0
    pointer: Optional[Dict[str, Any]] = None
    plan_steps: List[str] = field(default_factory=list)
    tier: str = "fast"
    model: str = ""
    scope: str = "chat"
    scope_label: str = "Conversa"
    complexity: float = 0.0


@dataclass
class VisionResult:
    text: str
    pointer: Optional[Dict[str, Any]] = None
    emotion: str = "pensando"
    model: str = ""
    elapsed_seconds: float = 0.0
    raw: str = ""


def _strip_json(text: str) -> str:
    txt = (text or "").strip()
    fence = chr(96) * 3
    txt = txt.replace(fence + "json", "").replace(fence, "").strip()
    start = txt.find("{")
    end = txt.rfind("}")
    if start >= 0 and end > start:
        txt = txt[start:end + 1]
    return txt.strip()


def _safe_pointer(value: Any, screen_w: int, screen_h: int) -> Optional[Dict[str, Any]]:
    if not isinstance(value, dict):
        return None
    try:
        x = float(value.get("x"))
        y = float(value.get("y"))
    except Exception:
        return None
    if not (0 <= x <= screen_w and 0 <= y <= screen_h):
        return None
    return {"x": int(x), "y": int(y), "label": str(value.get("label") or "").strip()[:80]}


class HybridCortex:
    """Cognitive core backed by Google Gemini (AI Studio API key)."""

    def __init__(
        self,
        api_key: str = "",
        model_name: str = DEFAULT_MODEL,
        deep_model: str = "",
        vision_model: str = "",
        routing_mode: str = "scope",
        complexity_threshold: float = 0.45,
        screen_size: tuple = (1920, 1080),
        balanced_model: str = "",
        code_model: str = "",
        embedding_model: str = DEFAULT_EMBED_MODEL,
        client: Optional[GeminiClient] = None,
        model_chain: Any = None,   # sage.llm_chain.ModelChain: cérebro + reservas (None = caminho Gemini antigo)
        **_legacy: Any,   # old Ollama kwargs (ollama_url, gpu_port, ...) are ignored
    ):
        self.model_name = model_name or DEFAULT_MODEL
        self.screen_w, self.screen_h = int(screen_size[0]), int(screen_size[1])
        self.embedding_model = embedding_model or DEFAULT_EMBED_MODEL
        self.routing_mode = (routing_mode or "scope").lower()
        self.complexity_threshold = float(complexity_threshold)
        self.client = client or GeminiClient(api_key, self.model_name, self.embedding_model)
        self.model_chain = model_chain

        m = self.model_name
        self.registry = ModelRegistry(
            deep_model=deep_model or m,
            balanced_model=balanced_model or m,
            code_model=code_model or m,
            fast_model=m,
            vision_model=vision_model or m,
        )
        # compatibility alias used by older UI code
        self.router = self.registry
        self.refresh_models()

    # ------------------------------------------------------------------
    # Backend / models
    # ------------------------------------------------------------------
    def refresh_models(self) -> List[str]:
        """Cloud models are always 'installed'; the list is the configured set."""
        r = self.registry
        models = list(dict.fromkeys(m for m in (r.fast_model, r.balanced_model, r.code_model,
                                                 r.deep_model, r.vision_model) if m))
        r.set_available(models)
        return models

    # ------------------------------------------------------------------
    # Raw generation / embeddings
    # ------------------------------------------------------------------
    def _chain_generate(self, system: str, prompt: str, *, json_mode: bool = False,
                        max_tokens: int = 700, temperature: float = 0.3,
                        prefer_fast: bool = False) -> Optional[str]:
        """Pede ao cérebro configurado (com reservas) ou ao modelo de resposta rápida."""
        chain = self.model_chain
        if chain is None:
            return None
        try:
            try:
                return chain.generate(system, prompt, json_mode=json_mode,
                                      max_tokens=max_tokens, temperature=temperature,
                                      prefer_fast=prefer_fast)
            except TypeError:
                return chain.generate(system, prompt, json_mode=json_mode,
                                      max_tokens=max_tokens, temperature=temperature)
        except Exception as e:
            print("[Cortex] Cadeia de modelos sem resposta (" + str(e)[:200] + "); caminho Gemini.")
            return None

    def _chain_label(self) -> str:
        last = getattr(self.model_chain, "last_used", None)
        return (last.provider + "/" + last.model) if last else "?"

    def _generate(self, model: str, prompt: str, system: Optional[str] = None,
                  images: Optional[List[str]] = None, json_mode: bool = False,
                  timeout: int = 60, temperature: float = 0.45,
                  max_tokens: int = 700) -> str:
        thinking = "low" if (model == self.registry.deep_model and model != self.model_name
                             and not json_mode) else "minimal"
        return self.client.generate(prompt, system, model=model, images=images,
                                    json_mode=json_mode, temperature=temperature,
                                    max_tokens=max_tokens, timeout=timeout, thinking=thinking)

    def embed(self, text: str) -> Optional[List[float]]:
        """Returns an embedding vector from the Gemini embedding model."""
        if not text.strip():
            return None
        try:
            return self.client.embed(text)
        except Exception as e:
            print("[Cortex] Embedding falhou: " + str(e))
            return None

    # ------------------------------------------------------------------
    # Routing
    # ------------------------------------------------------------------
    def route(self, prompt: str) -> Route:
        if self.routing_mode == "fast":
            route = self.registry.resolve("chat", 0.0)
            route.model = self.registry.fast_model
            route.tier = "fast"
            return route
        if self.routing_mode == "deep":
            route = self.registry.resolve("reasoning", 1.0)
            route.model = self.registry.deep_model
            route.tier = "deep"
            return route
        route = self.registry.route_prompt(prompt)
        # Escala automaticamente para o modelo de Raciocínio quando a complexidade do escopo atinge o limiar
        if route.complexity >= self.complexity_threshold and route.tier == "fast":
            route.model = self.registry.deep_model or self.registry.balanced_model or route.model
            route.tier = "deep"
            route.reason = f"escopo complexo ({route.complexity:.2f} >= {self.complexity_threshold:.2f}) -> raciocínio"
        elif route.complexity < self.complexity_threshold and route.scope in ("chat", "system"):
            route.model = self.registry.fast_model
            route.tier = "fast"
        return route

    def classify_and_plan(self, user_prompt: str, complexity: Optional[float] = None,
                          context: str = "") -> CortexDecision:
        start = time.time()
        prompt = (user_prompt or "").strip()
        if not prompt:
            return CortexDecision("DIRECT_ANSWER", "Estou aqui.", "Fala.",
                                  emotion="idle", source="empty", elapsed_seconds=0.0)

        route = self.route(prompt)
        if complexity is None:
            complexity = route.complexity
        system = (ROUTER_SYSTEM
                  .replace("{screen_w}", str(self.screen_w))
                  .replace("{screen_h}", str(self.screen_h)))
        try:
            catalog = SKILL_CATALOG() or "(nenhuma ainda)"
        except Exception:
            catalog = "(nenhuma ainda)"
        system += EVOLUTION_RULES + "\nSUAS SKILLS (funções que você mesmo escreveu):\n" + catalog + "\n"

        user_block = ((("Conversa recente e memória (use para entender referências como 'isso', "
                        "'de novo', 'aquele arquivo'):\n" + context[-6000:] + "\n\n") if context else "")
                      + "Usuário: " + prompt + "\n\nResponda apenas o JSON:")

        if self.model_chain is not None:
            raw = self._chain_generate(system, user_block, json_mode=True, max_tokens=600,
                                       temperature=0.25, prefer_fast=(route.tier == "fast"))
            if raw is not None:
                try:
                    decision = self._coerce_decision(json.loads(_strip_json(raw) or "{}"), prompt)
                    decision.source = "chain:" + self._chain_label()
                    decision.model = route.model
                    decision.tier = route.tier
                    decision.scope = route.scope
                    decision.scope_label = route.label
                    decision.complexity = route.complexity
                    decision.elapsed_seconds = time.time() - start
                    if not decision.reply and decision.action == "none":
                        decision.reply = decision.ack or "Feito."
                    return decision
                except Exception as e:
                    print("[Cortex] Resposta do cérebro sem JSON válido (" + str(e)[:120] + "); caminho Gemini.")

        # Se o escopo pede raciocínio profundo, prioriza o modelo de raciocínio; se for simples, o rápido.
        if route.tier in ("deep", "balanced", "code") and route.model != self.registry.fast_model:
            candidates = [route.model, self.registry.fast_model]
        else:
            candidates = [self.registry.fast_model]
            if route.model not in candidates:
                candidates.append(route.model)

        last_error = None
        for model in candidates:
            try:
                raw = self._generate(
                    model, user_block,
                    system=system, json_mode=True, timeout=35,
                    temperature=0.25, max_tokens=600,
                )
                data = json.loads(_strip_json(raw) or "{}")
                decision = self._coerce_decision(data, prompt)
                decision.source = "gemini:" + model
                decision.model = route.model
                decision.tier = route.tier
                decision.scope = route.scope
                decision.scope_label = route.label
                decision.complexity = route.complexity
                decision.elapsed_seconds = time.time() - start
                if not decision.reply and decision.action == "none":
                    decision.reply = decision.ack or "Feito."
                return decision
            except Exception as e:
                last_error = e
                continue

        print("[Cortex] Planejador indisponível (" + str(last_error) + "). Heurística.")
        decision = self._heuristic_fallback(prompt, time.time() - start)
        decision.tier = route.tier
        decision.model = route.model
        decision.scope = route.scope
        decision.scope_label = route.label
        decision.complexity = route.complexity
        return decision

    def _coerce_decision(self, data: Dict[str, Any], prompt: str) -> CortexDecision:
        intent = str(data.get("intent") or "DIRECT_ANSWER").upper().strip()
        if intent not in ("DIRECT_ANSWER", "SYSTEM_ACTION", "DELEGATE_AGENT"):
            intent = "DIRECT_ANSWER"
        action = str(data.get("action") or "none").lower().strip()
        if action not in VALID_ACTIONS:
            action = "none"
        params = data.get("action_params") or {}
        if not isinstance(params, dict):
            params = {}
        # small models often say DIRECT_ANSWER while choosing a real action:
        # if the action is concrete, run it instead of silently dropping it.
        if intent == "DIRECT_ANSWER" and action != "none":
            no_param_actions = {"system_status", "screen_capture", "lock_pc", "minimize_all",
                                "list_windows", "analyze_screen", "ocr_screen", "restart_app",
                                "list_skills", "self_update"}
            has_args = any(str(v).strip() for k, v in params.items()
                           if k not in ("x", "y") and v not in (None, 0, ""))
            if action in no_param_actions or has_args:
                intent = "SYSTEM_ACTION"
            else:
                action = "none"
        if action in ("use_skill", "create_skill", "improve_skill", "list_skills", "delete_skill",
                      "self_update", "evolve_core"):
            intent = "SYSTEM_ACTION"           # evolução é sempre uma ação, não uma delegação a clones
        emotion = str(data.get("emotion") or "idle").lower().strip()
        if emotion not in _LLM_EMOTIONS:
            emotion = "pensando" if intent != "DIRECT_ANSWER" else "feliz"
        pointer = _safe_pointer(data.get("pointer"), self.screen_w, self.screen_h)
        if pointer is None and action == "point_screen":
            pointer = _safe_pointer(params, self.screen_w, self.screen_h)
        reply = str(data.get("reply") or "").strip()
        ack = str(data.get("ack") or "").strip() or "Deixa comigo."
        return CortexDecision(
            intent=intent, ack=ack, reply=reply, action=action, action_params=params,
            emotion=emotion, requires_screen=bool(data.get("requires_screen")) or action == "analyze_screen",
            agent_task=str(data.get("agent_task") or "").strip(), pointer=pointer,
        )

    # ------------------------------------------------------------------
    # Chat / answers
    # ------------------------------------------------------------------
    def generate_chat_response(self, user_prompt: str, system_prompt: Optional[str] = None,
                               context: Optional[str] = None, force_deep: bool = False,
                               scope: Optional[str] = None, max_tokens: Optional[int] = None) -> str:
        prompt = (user_prompt or "").strip()
        if not prompt:
            return ""
        route = self.route(prompt) if not scope else self.registry.resolve(scope, estimate_complexity(prompt))
        model = self.registry.deep_model if force_deep else route.model
        profile = self.registry.profiles.get(route.scope)
        style = profile.style if profile else "short"
        wants_detail = bool(re.search(r"\b(explique|detalh|passo a passo|por que|como funciona|analise|compare)\b", prompt.lower()))
        style_text = CHAT_STYLE_LONG if (style == "detailed" or wants_detail) else CHAT_STYLE_SHORT

        sys_msg = system_prompt or (PERSONA + "\n\n" + style_text)
        full = ("Conversa recente e memória:\n" + context + "\n\nUsuário: " + prompt + "\nS.A.G.E.:") \
            if context else ("Usuário: " + prompt + "\nS.A.G.E.:")

        tokens = max_tokens or (profile.max_tokens if profile else 700)
        if self.model_chain is not None:
            out = self._chain_generate(sys_msg, full, max_tokens=tokens,
                                       temperature=profile.temperature if profile else 0.4,
                                       prefer_fast=(route.tier == "fast" and not force_deep))
            if out:
                return out
        for candidate in ([model, self.registry.fast_model] if model != self.registry.fast_model else [model]):
            try:
                out = self._generate(candidate, full, system=sys_msg,
                                     timeout=120 if candidate == self.registry.deep_model else 75,
                                     temperature=profile.temperature if profile else 0.4,
                                     max_tokens=tokens)
                if out:
                    return out
            except Exception as e:
                print("[Cortex] Falha ao gerar resposta (" + candidate + "): " + str(e))
        return "Relatório: deu ruim no Gemini. Tenta de novo."

    def generate_detailed(self, prompt: str, context: str = "") -> str:
        return self.generate_chat_response(prompt, context=context, force_deep=True,
                                           scope="reasoning", max_tokens=1600)

    def summarize_memories(self, turns_text: str) -> str:
        """Turns recent conversation into compact durable memories (used when idle)."""
        system = (
            "Você é o módulo de memória do S.A.G.E. Extraia fatos duráveis e úteis do diálogo: "
            "preferências do usuário, projetos, decisões, nomes, caminhos, credenciais (sem segredos), "
            "tarefas pendentes. Responda em até 6 linhas, cada uma começando com '- '. "
            "Sem opinião, sem floreio."
        )
        return self._generate(self.registry.balanced_model,
                             "Diálogo:\n" + turns_text[:12000] + "\n\nMemórias:",
                             system=system, timeout=120, temperature=0.2, max_tokens=500)

    # ------------------------------------------------------------------
    # Vision / OCR
    # ------------------------------------------------------------------
    def _annotate_screenshot(self, path: str, max_width: int = 1280, grid: bool = True):
        img = Image.open(path).convert("RGB")
        orig_w, orig_h = img.size
        scale = min(1.0, max_width / float(orig_w))
        if scale < 1.0:
            img = img.resize((int(orig_w * scale), int(orig_h * scale)), Image.Resampling.LANCZOS)
        if grid:
            draw = ImageDraw.Draw(img, "RGBA")
            step = 120
            for x in range(0, img.width, step):
                draw.line([(x, 0), (x, img.height)], fill=(255, 60, 60, 70), width=1)
                draw.text((x + 3, 3), str(int(x / scale)), fill=(255, 80, 80, 230))
            for y in range(0, img.height, step):
                draw.line([(0, y), (img.width, y)], fill=(255, 60, 60, 70), width=1)
                draw.text((3, y + 1), str(int(y / scale)), fill=(255, 80, 80, 230))
        buf = io.BytesIO()
        img.save(buf, format="PNG")
        return base64.b64encode(buf.getvalue()).decode("ascii"), img.width, img.height, scale

    def vision_analyze(self, image_path: str, question: str, want_pointer: bool = True,
                       ocr: bool = False) -> VisionResult:
        start = time.time()
        model = self.registry.vision_model
        if not self.registry.is_local_available(model):
            model = self.registry.balanced_model
        try:
            b64, w, h, scale = self._annotate_screenshot(image_path, grid=not ocr)
        except Exception as e:
            return VisionResult("Não consegui abrir a captura: " + str(e),
                                model=model, elapsed_seconds=time.time() - start)

        if ocr:
            prompt = ("Transcreva todo o texto desta imagem, literalmente, preservando quebras de linha. "
                      "Se não houver texto, responda 'sem texto'.")
            try:
                raw = self._generate(model, prompt, system=OCR_SYSTEM, images=[b64],
                                     timeout=120, temperature=0.0, max_tokens=1600)
            except Exception as e:
                return VisionResult("OCR falhou: " + str(e), model=model,
                                    elapsed_seconds=time.time() - start)
            return VisionResult(raw.strip(), None, "pensando", model,
                                time.time() - start, raw)

        pointer_rule = ('"pointer": {"x": px, "y": py, "label": "texto curto"} com coordenadas '
                        'em pixels da tela ORIGINAL (' + str(self.screen_w) + 'x' + str(self.screen_h) + ')'
                        if want_pointer else '"pointer": null')
        prompt = (
            "Pergunta do usuário: " + (question or "O que há nesta tela?") + "\n"
            "A imagem tem " + str(w) + "x" + str(h) + " px e grade a cada 120 px com coordenadas da tela original.\n"
            "Responda SOMENTE este JSON:\n"
            '{"reply": "resposta curta e afiada em português", "pointer": null, "emotion": "pensando"}\n'
            "Troque null por " + pointer_rule + " quando houver um ponto exato para clicar/olhar."
        )
        try:
            raw = self._generate(model, prompt, system=VISION_SYSTEM, images=[b64],
                                 json_mode=True, timeout=120, temperature=0.2, max_tokens=600)
        except Exception as e:
            return VisionResult("Visão falhou: " + str(e), model=model,
                                elapsed_seconds=time.time() - start)
        try:
            data = json.loads(_strip_json(raw) or "{}")
        except Exception:
            data = {"reply": raw}
        text = str(data.get("reply") or data.get("text") or raw).strip()
        pointer = _safe_pointer(data.get("pointer"), self.screen_w, self.screen_h)
        emotion = str(data.get("emotion") or "pensando").lower()
        if emotion not in _LLM_EMOTIONS:
            emotion = "pensando"
        return VisionResult(text, pointer, emotion, model, time.time() - start, raw)

    # ------------------------------------------------------------------
    # Heuristic fallback
    # ------------------------------------------------------------------
    def _heuristic_fallback(self, prompt: str, elapsed: float) -> CortexDecision:
        p = prompt.lower().strip()
        from sage.intent_guard import looks_like_task
        if looks_like_task(prompt) and not any(w in p for w in ("onde clico", "onde eu clico", "na tela", "script")):
            # the planner is unavailable: real work goes to the brain + clones, never to small talk
            return CortexDecision("DELEGATE_AGENT", "Pode deixar, vou dividir isso com meus clones.",
                                  "", "none", {}, "pensando", False, prompt, "heuristic", elapsed)

        def mk(action, params, ack, reply="", emotion="pensando", req_screen=False, pointer=None):
            return CortexDecision("SYSTEM_ACTION", ack, reply, action, params, emotion,
                                  req_screen, prompt, "heuristic", elapsed, pointer)

        if any(w in p for w in ("ocr", "transcreva", "texto da imagem", "texto da tela")):
            return mk("ocr_screen", {}, "Lendo o texto da tela.", "", "pensando")
        if any(w in p for w in ("onde clico", "onde eu clico", "o que tem na tela", "analisa a tela",
                                "analise a tela", "me mostra onde", "print", "tela")):
            return mk("analyze_screen", {}, "Dando uma olhada na tela.", "", "foto", True)
        if any(w in p for w in ("abrir chrome", "abre o chrome", "navegador")):
            return mk("open_app", {"target": "chrome"}, "Abrindo o Chrome.", "", "feliz")
        if any(w in p for w in ("status", "hardware", "cpu", "memória ram", "memoria ram")):
            return mk("system_status", {}, "Checando o hardware.", "", "pensando")
        if any(w in p for w in ("volume", "mutar", "mudo", "pausar", "tocar musica")):
            cmd = "mute" if "mut" in p else ("volume_up" if "aument" in p else ("volume_down" if "diminu" in p else "play_pause"))
            return mk("media_control", {"command": cmd}, "Mexendo no áudio.", "", "idle")
        if any(w in p for w in ("pesquisar", "pesquise", "procure no google")):
            query = p.replace("pesquisar", "").replace("pesquise", "").replace("no google", "").strip()
            return mk("web_search", {"query": query}, "Pesquisando " + query + ".", "", "feliz")
        if any(w in p for w in ("ler arquivo", "abrir arquivo", "conteudo do arquivo")):
            return mk("file_read", {"path": prompt}, "Lendo o arquivo.", "", "pensando")
        if "script" in p and any(w in p for w in ("crie", "criar", "escreva", "gere", "faça", "faca")):
            return mk("write_script", {"content": prompt}, "Escrevendo o script.", "", "pensando")
        if any(w in p for w in ("powershell", "cmd", "execute", "executar", "roda ", "rodar ")):
            return mk("shell", {"command": prompt}, "Rodando no terminal.", "", "pensando")
        if any(w in p for w in ("muda de forma", "mude de forma", "transforme", "transforma",
                                "forma de", "vira um", "vire um", "vira uma", "vire uma")):
            from sage.pixel_avatar import FORM_ALIASES
            form = "humano"
            for word in sorted(FORM_ALIASES, key=len, reverse=True):
                if word in p:
                    form = FORM_ALIASES[word]
                    break
            return mk("change_form", {"form": form}, "Metamorfose!", "", "feliz")
        if any(w in p for w in ("antigravity", "agy", "agente externo", "nuvem")):
            return CortexDecision("DELEGATE_AGENT", "Chamando o agente online.",
                                  "", "none", {}, "pensando", True, prompt, "heuristic", elapsed)

        return CortexDecision("DIRECT_ANSWER", "Beleza.", "",
                              "none", {}, "feliz", False, "", "heuristic", elapsed)

    def shutdown(self):
        pass
