"""
Task loop: observe -> act -> check against the request -> continue or report.

Before this module a PC action was a single shot: the cortex chose ONE action,
the orchestrator ran it and spoke "Comando executado com sucesso." The command
output never went back to a model, so nothing could notice that the request was
still unanswered. ``TaskLoop`` closes that gap:

1. observe  - optional silent screenshot, attached to every model call;
2. act      - the model picks one action, its (cleaned) output joins the evidence;
3. check    - when the model says "done", a separate call compares the proposed
              answer with the ORIGINAL request and the evidence;
4. continue - if the check finds something missing, that goes back to the model;
   report   - the answer, or, when steps/time run out, what was found and what
              is still missing (never "pronto").

The loop is pure: the model, the actions and the screenshot are injected, so it
is tested with a scripted model (tests/test_task_loop.py).
"""
from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional, Tuple

# think(prompt, system, images) -> raw model text (JSON expected)
ThinkFn = Callable[[str, str, Optional[List[str]]], str]
# act(action, params) -> object with .success / .spoken_summary / .details
ActFn = Callable[[str, Dict[str, Any]], Any]
# look() -> base64 PNG of the screen (or None when the capture failed)
LookFn = Callable[[], Optional[str]]

LOOK_ACTIONS = {"look_screen", "screen_capture", "analyze_screen", "screenshot", "olhar_tela"}
# actions the loop may run; everything else is refused and reported back to the model
ALLOWED_ACTIONS = {
    "run_command", "shell", "run_python", "file_read", "file_list", "file_search",
    "file_write", "file_edit", "fetch_url", "search_web", "ocr_screen", "ocr_image",
    "list_windows", "system_status", "open_app", "open_url", "web_search", "focus_window",
    "click_at", "type_text", "press_keys", "clipboard", "write_script", "run_script",
}

LOOP_SYSTEM = """\
Você é o S.A.G.E., assistente que controla o PC do usuário (Windows 11, PowerShell).
Você está num LAÇO DE TRABALHO: a cada vez você escolhe UM próximo passo, o sistema
executa e te devolve o resultado, e você decide de novo. Você só termina quando o
PEDIDO ORIGINAL estiver respondido de verdade.

Responda SEMPRE apenas com um objeto JSON:
{
  "thought": "o que já sei e o que ainda falta para atender o pedido (1-2 frases)",
  "status": "continue" | "done" | "ask",
  "action": "look_screen" | "run_command" | "run_python" | "file_list" | "file_read" |
            "file_search" | "file_write" | "file_edit" | "fetch_url" | "search_web" |
            "ocr_screen" | "list_windows" | "system_status" | "open_app" | "open_url" |
            "focus_window" | "click_at" | "type_text" | "press_keys" | "clipboard" | "none",
  "action_params": {"command": "", "path": "", "query": "", "url": "", "text": "",
                    "target": "", "pattern": "", "recursive": false, "x": 0, "y": 0, "keys": ""},
  "reply": "só quando status = done ou ask: a resposta final ao usuário",
  "emotion": "feliz" | "orgulhoso" | "curioso" | "pensando" | "confuso" | "surpreso"
}

REGRAS DO LAÇO:
1. CONTEXTO PRIMEIRO. Se o pedido fala de algo que o usuário está vendo ("esse", "isso",
   "aqui", "que estou vendo") e você não tem a imagem da tela, o primeiro passo é
   "look_screen". Nunca chute comandos sem saber do que o usuário está falando.
2. USE O QUE VIU. Se há imagem da tela ou resultado de comando, o próximo passo tem de
   partir deles: caminho da janela aberta, nome do programa, arquivos listados, texto lido.
3. "continue" exige uma "action" diferente de "none". Um passo por vez.
4. Só use "done" quando a resposta atende o pedido INTEIRO com base nas evidências dos
   passos (ou da imagem). Ter listado uma pasta ou tirado um print NÃO é ter terminado:
   leia os arquivos promissores (README, LICENSE, .git/config, package.json, VERSION),
   pesquise na web se preciso, até ter a resposta pedida.
5. Em "done", "reply" traz o RESULTADO concreto (link, caminho, nome, valor) e, em uma
   frase, o que você fez para chegar nele. Português do Brasil, curto, sem emojis.
   Nunca diga só "pronto" ou "comando executado".
6. Não repita um passo que já fez; se algo falhou, tente outro caminho.
7. "ask" só para: ação destrutiva/irreversível que precisa de confirmação, ou quando é
   impossível continuar sem uma informação que só o usuário tem. "reply" é a pergunta.
8. Nunca invente resultado. O que não está nas evidências, você não sabe.
9. Comandos são PowerShell de Windows (Get-ChildItem, Get-Content, Select-String -Path
   com arquivos de TEXTO; não rode Select-String em .exe/.dll). Sem sintaxe bash.
10. "look_screen" tira o print em silêncio: não comente que tirou print.
"""

CHECK_SYSTEM = """\
Você é o verificador de entrega do S.A.G.E. Recebe o PEDIDO ORIGINAL do usuário, as
EVIDÊNCIAS (passos executados e seus resultados, e talvez a imagem da tela) e a RESPOSTA
PROPOSTA. Decida se a resposta entrega o que foi pedido.

Responda apenas JSON: {"complete": true | false, "missing": ["o que falta, objetivo"],
"reason": "1 frase"}

Critérios:
- complete = false se a resposta não traz o resultado pedido (ex.: pediram o link do
  repositório e a resposta só lista arquivos, ou só diz que executou um comando).
- complete = false se a resposta afirma fatos que não aparecem nas evidências nem na imagem.
- complete = false se a resposta só promete fazer ("vou verificar").
- complete = true se o pedido foi atendido, OU se as evidências mostram que a informação
  realmente não existe/não é acessível e a resposta diz isso claramente, com o que foi tentado.
- Conversa simples ou pergunta de conhecimento geral não precisa de evidência de passos.
- Não exija perfeição nem passos extras além do que o usuário pediu.
"""

_ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
_JUNK = re.compile(r"[^\x09\x0a\x0d\x20-\x7e -￿]")


def clean_output(text: Any, limit: int = 2500) -> str:
    """Makes a tool result fit for a model: no ANSI colours, no binary noise, bounded."""
    out = _ANSI.sub("", str(text or ""))
    out = _JUNK.sub("", out).replace("�", "?")
    out = re.sub(r"[ \t]+\n", "\n", out)
    out = re.sub(r"\n{3,}", "\n\n", out).strip()
    if len(out) > limit:
        head = int(limit * 0.7)
        tail = limit - head
        out = (out[:head] + "\n[... " + str(len(out) - limit) + " caracteres cortados ...]\n" +
               out[-tail:])
    return out


def _parse(raw: str) -> Dict[str, Any]:
    txt = (raw or "").strip()
    fence = chr(96) * 3
    txt = txt.replace(fence + "json", "").replace(fence, "").strip()
    a, b = txt.find("{"), txt.rfind("}")
    if a < 0 or b <= a:
        raise ValueError("resposta do modelo sem JSON")
    data = json.loads(txt[a:b + 1])
    if not isinstance(data, dict):
        raise ValueError("JSON do modelo não é um objeto")
    return data


def _as_list(value: Any) -> List[str]:
    if isinstance(value, str):
        value = [value]
    if not isinstance(value, (list, tuple)):
        return []
    return [str(v).strip() for v in value if str(v or "").strip()]


@dataclass
class Step:
    n: int
    action: str
    params: Dict[str, Any] = field(default_factory=dict)
    ok: bool = True
    output: str = ""
    thought: str = ""

    def label(self) -> str:
        if self.action == "look_screen":
            return "olhei a tela"
        if self.action == "check":
            return "conferi a resposta com o pedido"
        arg = ""
        for key in ("command", "path", "url", "query", "target", "text", "keys"):
            if str(self.params.get(key) or "").strip():
                arg = str(self.params[key]).strip()
                break
        return self.action + ((": " + arg[:160]) if arg else "")


@dataclass
class LoopResult:
    reply: str
    complete: bool
    reason: str                       # done | ask | exhausted | timeout | cancelled | error
    missing: List[str] = field(default_factory=list)
    steps: List[Step] = field(default_factory=list)
    checks: int = 0
    verified: bool = False
    emotion: str = "feliz"
    elapsed: float = 0.0

    @property
    def incomplete(self) -> bool:
        return not self.complete and self.reason != "ask"

    @property
    def full(self) -> str:
        """Reply plus the trail of what was done (for the chat panel / memory)."""
        lines = [self.reply.strip()]
        did = [s for s in self.steps if s.action != "check"]
        if did:
            lines.append("")
            lines.append("**O que fiz**")
            for i, s in enumerate(did, 1):
                lines.append(str(i) + ". " + s.label() + ("" if s.ok else " (falhou)"))
        if self.complete and not self.verified:
            lines.append("")
            lines.append("_Não consegui conferir esta resposta contra o pedido (verificador indisponível)._")
        return "\n".join(lines).strip()


class TaskLoop:
    def __init__(self, think: ThinkFn, act: ActFn, look: Optional[LookFn] = None, *,
                 max_steps: int = 8, max_seconds: float = 180.0, max_checks: int = 3,
                 on_event: Optional[Callable[[str, Dict[str, Any]], None]] = None,
                 cancelled: Optional[Callable[[], bool]] = None,
                 clock: Callable[[], float] = time.time,
                 extra_system: str = ""):
        self.think = think
        self.act = act
        self.look = look
        self.max_steps = max(1, int(max_steps))
        self.max_seconds = float(max_seconds)
        self.max_checks = max(1, int(max_checks))
        self.on_event = on_event
        self.cancelled = cancelled or (lambda: False)
        self.clock = clock
        self.system = LOOP_SYSTEM + (("\n" + extra_system) if extra_system else "")
        self.directives: List[str] = []

    def steer(self, text: str) -> None:
        cmd = str(text or "").strip()
        if not cmd:
            return
        self.directives.append(cmd)
        self._emit("steer", text=cmd)

    # ------------------------------------------------------------------
    def _emit(self, kind: str, **data) -> None:
        if self.on_event:
            try:
                self.on_event(kind, data)
            except Exception:
                pass

    @staticmethod
    def _evidence(steps: List[Step], notes: List[str], budget: int = 9000) -> str:
        if not steps and not notes:
            return "(nenhum passo executado ainda)"
        blocks: List[str] = []
        recent = {s.n for s in steps[-3:]}
        for s in steps:
            out = s.output if s.n in recent else clean_output(s.output, 500)
            blocks.append("PASSO " + str(s.n) + " - " + s.label() + " -> " +
                          ("ok" if s.ok else "FALHOU") + "\n" + (out or "(sem saída)"))
        text = "\n\n".join(blocks)
        if len(text) > budget:
            text = "[passos antigos cortados]\n" + text[-budget:]
        if notes:
            text += "\n\nAVISOS DO SISTEMA:\n" + "\n".join("- " + n for n in notes[-4:])
        return text

    def _step_prompt(self, request: str, context: str, steps: List[Step], notes: List[str],
                     has_image: bool, left: int) -> str:
        dir_block = ""
        if self.directives:
            dir_block = (
                "DIRETIVAS DIRETAS DO USUÁRIO (PRIORIDADE MÁXIMA — REALINHAMENTO):\n"
                + "\n".join("- " + d for d in self.directives[-6:])
                + "\n\n"
            )
        return (
            (("Conversa recente e memória (só para entender referências):\n" + context[-3000:] + "\n\n")
             if context else "")
            + "PEDIDO ORIGINAL do usuário: " + request + "\n\n"
            + dir_block
            + ("A imagem anexada é o print ATUAL da tela do usuário. Use-a como contexto.\n\n"
               if has_image else "Você ainda NÃO viu a tela do usuário.\n\n")
            + "EVIDÊNCIAS ATÉ AGORA:\n" + self._evidence(steps, notes) + "\n\n"
            + "Passos restantes: " + str(left) + ". "
            + ("Este é o ÚLTIMO: responda com o que tem (done) ou diga exatamente o que falta. "
               if left <= 1 else "")
            + "Qual é o próximo passo? Responda apenas o JSON."
        )

    def _check(self, request: str, steps: List[Step], reply: str,
               image: Optional[str]) -> Tuple[Optional[bool], List[str]]:
        """(complete, missing). complete is None when the verifier itself failed."""
        prompt = ("PEDIDO ORIGINAL: " + request + "\n\nEVIDÊNCIAS:\n" + self._evidence(steps, []) +
                  ("\n\n(A imagem anexada é o print da tela usado como contexto.)" if image else "") +
                  "\n\nRESPOSTA PROPOSTA:\n" + reply + "\n\nA resposta entrega o pedido? Apenas o JSON.")
        try:
            data = _parse(self.think(prompt, CHECK_SYSTEM, [image] if image else None))
        except Exception as e:
            self._emit("check_error", error=str(e)[:200])
            return None, []
        complete = data.get("complete")
        if isinstance(complete, str):
            complete = complete.strip().lower() in ("true", "sim", "yes", "1")
        missing = _as_list(data.get("missing"))
        if not complete and not missing:
            missing = _as_list(data.get("reason")) or ["a resposta não atende o pedido"]
        return bool(complete), missing

    # ------------------------------------------------------------------
    def run(self, request: str, context: str = "", *,
            first_action: Optional[Tuple[str, Dict[str, Any]]] = None,
            look_first: bool = False) -> LoopResult:
        start = self.clock()
        steps: List[Step] = []
        notes: List[str] = []
        missing: List[str] = []
        image: Optional[str] = None
        checks = 0
        think_errors = 0
        last_reply = ""
        reason = "exhausted"
        seen: set = set()

        def finish(reply: str, complete: bool, why: str, verified: bool = False,
                   emotion: str = "feliz") -> LoopResult:
            res = LoopResult(reply=reply, complete=complete, reason=why, missing=list(missing),
                             steps=steps, checks=checks, verified=verified, emotion=emotion,
                             elapsed=self.clock() - start)
            self._emit("end", reason=why, complete=complete, missing=list(missing),
                       steps=len(steps), reply=reply)
            return res

        def do_look(thought: str = "") -> None:
            nonlocal image
            shot = None
            if self.look:
                try:
                    shot = self.look()
                except Exception as e:
                    notes.append("o print da tela falhou: " + str(e)[:120])
            step = Step(len(steps) + 1, "look_screen", {}, bool(shot),
                        "print da tela capturado e anexado" if shot else "não foi possível capturar a tela",
                        thought)
            if shot:
                image = shot
            steps.append(step)
            self._emit("step", n=step.n, action="look_screen", ok=step.ok, output=step.output)

        pending = first_action
        if pending and pending[0] in LOOK_ACTIONS:
            pending, look_first = None, True
        if look_first:
            do_look()
            # an action chosen before seeing the screen was chosen blind: decide again with context
            if image:
                pending = None

        turn = 0
        seen_directives = 0
        while turn < self.max_steps:
            turn += 1
            if self.cancelled():
                reason = "cancelled"
                break
            if self.clock() - start > self.max_seconds:
                reason = "timeout"
                break
            while seen_directives < len(self.directives):
                d = self.directives[seen_directives]
                seen_directives += 1
                pending = None  # descarta ação anterior para replanejar com o novo alinhamento
                notes.append("DIRETIVA DIRETA DO USUÁRIO (prioridade máxima): " + d)
                # grant an extra step when the user steers the loop so it can act on the directive
                self.max_steps = min(self.max_steps + 2, 24)

            if pending:
                data: Dict[str, Any] = {"status": "continue", "action": pending[0],
                                        "action_params": dict(pending[1] or {})}
                pending = None
            else:
                try:
                    data = _parse(self.think(
                        self._step_prompt(request, context, steps, notes, bool(image),
                                          self.max_steps - turn + 1),
                        self.system, [image] if image else None))
                    think_errors = 0
                except Exception as e:
                    think_errors += 1
                    self._emit("think_error", error=str(e)[:200])
                    if think_errors >= 2:
                        missing.append("o modelo parou de responder (" + str(e)[:120] + ")")
                        reason = "error"
                        break
                    notes.append("sua última resposta não era um JSON válido; responda só o JSON")
                    continue
                if seen_directives < len(self.directives):
                    turn = max(0, turn - 1)
                    continue

            status = str(data.get("status") or "continue").lower().strip()
            action = str(data.get("action") or "none").lower().strip()
            params = data.get("action_params") if isinstance(data.get("action_params"), dict) else {}
            # the JSON template's placeholders ("" / 0 / false) are not arguments
            keep_zero = ("x", "y") if action == "click_at" else ()
            params = {k: v for k, v in params.items()
                      if v not in (None, "") and (k in keep_zero or v not in (0, False))}
            thought = str(data.get("thought") or "").strip()
            reply = str(data.get("reply") or "").strip()
            emotion = str(data.get("emotion") or "feliz").lower().strip()
            if thought:
                self._emit("thought", text=thought)

            if status == "ask" and reply:
                return finish(reply, False, "ask", emotion="confuso")

            if status == "done":
                if not reply:
                    notes.append('status "done" exige "reply" com o resultado concreto')
                    continue
                last_reply = reply
                checks += 1
                complete, miss = self._check(request, steps, reply, image)
                if seen_directives < len(self.directives):
                    turn = max(0, turn - 1)
                    continue
                self._emit("check", complete=complete, missing=miss, reply=reply)
                if complete is None:                      # verifier down: deliver, flagged
                    missing = []
                    return finish(reply, True, "done", verified=False, emotion=emotion)
                if complete:
                    missing = []
                    return finish(reply, True, "done", verified=True, emotion=emotion)
                missing = miss
                steps.append(Step(len(steps) + 1, "check", {}, False,
                                  "resposta recusada pela checagem; falta: " + "; ".join(miss)))
                notes.append('a checagem contra o pedido recusou sua resposta "' + reply[:200] +
                             '". FALTA: ' + "; ".join(miss) + ". Continue trabalhando nisso.")
                if checks >= self.max_checks:
                    reason = "exhausted"
                    break
                continue

            # status == continue (or anything else): an action is mandatory
            if action in LOOK_ACTIONS:
                do_look(thought)
                continue
            if action in ("none", ""):
                notes.append('"continue" exige uma ação. Escolha um passo concreto ou use "done" com a resposta.')
                continue
            if action not in ALLOWED_ACTIONS:
                notes.append('a ação "' + action + '" não existe neste laço; use uma das listadas')
                continue
            key = action + "|" + json.dumps(params, sort_keys=True, ensure_ascii=False, default=str)
            if key in seen:
                notes.append("você já executou exatamente " + action + " com esses parâmetros; "
                             "o resultado está nas evidências. Use-o ou tente outro caminho.")
                continue
            seen.add(key)

            self._emit("action", n=len(steps) + 1, action=action, params=params)
            try:
                res = self.act(action, params)
                ok = bool(getattr(res, "success", False))
                out = clean_output(getattr(res, "details", "") or getattr(res, "spoken_summary", ""))
            except Exception as e:
                ok, out = False, "erro ao executar: " + str(e)[:300]
            step = Step(len(steps) + 1, action, params, ok, out, thought)
            steps.append(step)
            self._emit("step", n=step.n, action=action, params=params, ok=ok, output=out[:600])

        # ---- out of steps / time / cancelled: say what is missing, never "pronto" ----
        return finish(self._unfinished_report(request, steps, missing, last_reply, reason, image),
                      False, reason, emotion="envergonhado")

    def _unfinished_report(self, request: str, steps: List[Step], missing: List[str],
                           last_reply: str, reason: str, image: Optional[str]) -> str:
        why = {"timeout": "o tempo acabou", "cancelled": "você mandou parar",
               "error": "o modelo caiu no meio do caminho",
               "exhausted": "esgotei as tentativas"}.get(reason, "esgotei as tentativas")
        found = ""
        if reason != "cancelled" and steps:
            prompt = ("PEDIDO ORIGINAL: " + request + "\n\nEVIDÊNCIAS:\n" + self._evidence(steps, []) +
                      "\n\nO trabalho parou SEM terminar (" + why + "). Responda apenas JSON: "
                      '{"found": "o que de útil já foi descoberto, em 1-2 frases, só fatos das '
                      'evidências", "missing": ["o que ainda falta para atender o pedido"]}')
            try:
                data = _parse(self.think(prompt, CHECK_SYSTEM.split("\n")[0] +
                                         " Resuma um trabalho inacabado, sem inventar nada.",
                                         [image] if image else None))
                found = str(data.get("found") or "").strip()
                if not missing:
                    missing.extend(_as_list(data.get("missing")))
            except Exception:
                found = ""
        if not found:
            done = [s.label() for s in steps if s.action != "check" and s.ok]
            found = ("Cheguei a: " + "; ".join(done[-4:]) + ".") if done else "Não cheguei a nenhum resultado."
        if not missing:
            missing.append("a resposta ao pedido: " + request[:160])
        text = "Não terminei: " + why + ". " + found.rstrip(".") + ". Falta: " + "; ".join(missing[:4]) + "."
        if last_reply and reason == "exhausted":
            text += " Meu melhor palpite, sem confirmação: " + last_reply
        return text
