"""
Cadeia de modelos do S.A.G.E.: cérebro principal + reservas, em ordem.
======================================================================
* Cérebro com timeout (padrão 25 s), 429, 5xx ou resposta vazia -> tenta a próxima reserva.
  Qualquer outra falha do cérebro também passa para a reserva (o motivo vai para o log).
* Quando o cérebro cai, ele é pulado até o health check (padrão 5 min); nesse momento o
  próximo pedido real volta a ele. Se ele responder, volta a ser o cérebro e registra no log.
* ``get_model_config``: config salva (config.model_config) > model_ranking.json (primeira
  execução) > config antiga digitada à mão (gemini_model/brain_model e cloud_providers).
"""
from __future__ import annotations

import socket
import threading
import time
from dataclasses import dataclass
from typing import Any, Callable, Dict, List, Optional, Sequence

from sage import llm_catalog
from sage.llm_catalog import ProviderError

DEFAULT_TIMEOUT_SECONDS = 25.0
DEFAULT_HEALTH_INTERVAL_SECONDS = 300.0
MAX_FALLBACKS = 12


@dataclass(frozen=True)
class Target:
    provider: str
    model: str

    def as_dict(self) -> Dict[str, str]:
        return {"provider": self.provider, "model": self.model}


class ChainError(RuntimeError):
    def __init__(self, message: str, attempts: Sequence[str] = ()):
        super().__init__(message)
        self.attempts = list(attempts)


class EmptyReply(RuntimeError):
    pass


def failure_reason(exc: BaseException) -> str:
    """Motivo curto da falha, usado no log e para decidir o fallback."""
    if isinstance(exc, EmptyReply):
        return "resposta vazia"
    status = int(getattr(exc, "status", 0) or 0)
    if isinstance(exc, (TimeoutError, socket.timeout)) or "timed out" in str(exc).lower():
        return "timeout"
    if status == 429:
        return "429"
    if 500 <= status < 600:
        return "5xx"
    if status == 0:
        return "erro de conexão"
    return "erro " + str(status)


class ModelChain:
    """Executa um pedido no cérebro e cai para as reservas quando preciso.

    ``complete(target, system, prompt, *, json_mode, max_tokens, temperature, timeout)``
    devolve o texto da resposta ou levanta exceção (é o cliente LLM; nos testes, um mock).
    """

    def __init__(self, brain: Target, fallbacks: Sequence[Target],
                 complete: Callable[..., str], *,
                 fast: Optional[Target] = None,
                 timeout: float = DEFAULT_TIMEOUT_SECONDS,
                 health_interval: float = DEFAULT_HEALTH_INTERVAL_SECONDS,
                 clock: Callable[[], float] = time.monotonic,
                 log: Optional[Callable[[str], None]] = None):
        self.brain = brain
        self.fast = fast if (fast is not None and fast != brain) else None
        dedup: List[Target] = []
        for target in fallbacks:
            if target != brain and target not in dedup:
                dedup.append(target)
        self.fallbacks = dedup
        self._complete = complete
        self.timeout = float(timeout)
        self.health_interval = float(health_interval)
        self._clock = clock
        self._log = log or print
        self._lock = threading.Lock()
        self.brain_down = False
        self.brain_reason = ""
        self._next_check = 0.0
        self.fast_down = False
        self.fast_reason = ""
        self._fast_next_check = 0.0
        self.last_used: Optional[Target] = None

    def targets(self) -> List[Target]:
        return [self.brain] + self.fallbacks

    def _label(self, target: Target) -> str:
        return target.provider + "/" + target.model

    def _brain_turn(self, now: float) -> bool:
        with self._lock:
            return (not self.brain_down) or now >= self._next_check

    def _fast_turn(self, now: float) -> bool:
        with self._lock:
            return (not self.fast_down) or now >= self._fast_next_check

    def _brain_failed(self, now: float, reason: str) -> None:
        with self._lock:
            self.brain_down = True
            self.brain_reason = reason
            self._next_check = now + self.health_interval

    def _fast_failed(self, now: float, reason: str) -> None:
        with self._lock:
            self.fast_down = True
            self.fast_reason = reason
            self._fast_next_check = now + self.health_interval

    def _brain_recovered(self) -> bool:
        with self._lock:
            was_down = self.brain_down
            self.brain_down = False
            self.brain_reason = ""
            return was_down

    def _fast_recovered(self) -> bool:
        with self._lock:
            was_down = self.fast_down
            self.fast_down = False
            self.fast_reason = ""
            return was_down

    def generate(self, system: str, prompt: str, *, json_mode: bool = False,
                 max_tokens: int = 700, temperature: float = 0.3,
                 prefer_fast: bool = False) -> str:
        now = self._clock()
        order: List[Target] = []
        if prefer_fast and self.fast is not None and self._fast_turn(now):
            order.append(self.fast)
        if self._brain_turn(now) and self.brain not in order:
            order.append(self.brain)
        for fb in self.fallbacks:
            if fb not in order:
                order.append(fb)
        attempts: List[str] = []
        for target in order:
            is_brain = target == self.brain
            is_fast = self.fast is not None and target == self.fast
            try:
                text = self._complete(target, system, prompt, json_mode=json_mode,
                                      max_tokens=max_tokens, temperature=temperature,
                                      timeout=self.timeout)
                text = (text or "").strip()
                if not text:
                    raise EmptyReply("resposta vazia")
            except Exception as exc:
                reason = failure_reason(exc)
                attempts.append(self._label(target) + ": " + reason)
                if is_fast:
                    self._fast_failed(now, reason)
                    self._log("[LLM] resposta rápida " + self._label(target) + " falhou (" + reason
                              + "); escalando para o cérebro/reservas.")
                elif is_brain:
                    self._brain_failed(now, reason)
                    self._log("[LLM] cérebro " + self._label(target) + " falhou (" + reason
                              + "); indo para a próxima reserva.")
                else:
                    self._log("[LLM] reserva " + self._label(target) + " falhou (" + reason + ").")
                continue
            if is_fast:
                if self._fast_recovered():
                    self._log("[LLM] resposta rápida " + self._label(target) + " voltou a responder.")
            elif is_brain:
                if self._brain_recovered():
                    self._log("[LLM] cérebro " + self._label(target) + " voltou a responder.")
            elif self.brain_down:
                self._log("[LLM] cérebro fora do ar; respondeu a reserva " + self._label(target) + ".")
            self.last_used = target
            return text
        raise ChainError("nenhum modelo respondeu (" + "; ".join(attempts) + ")", attempts)


# ---------------------------------------------------------------- configuração de modelos
def _pick(provider: Any, model: Any) -> Optional[Dict[str, str]]:
    provider = str(provider or "").strip()
    model = str(model or "").strip()
    if not provider or not model:
        return None
    return {"provider": provider, "model": model}


def _dedupe(picks: List[Dict[str, str]]) -> List[Dict[str, str]]:
    out: List[Dict[str, str]] = []
    for pick in picks:
        if pick and pick not in out:
            out.append(pick)
    return out


def _configured_set(config) -> Dict[str, Any]:
    return {pid: spec for pid, spec in llm_catalog.provider_specs(config).items() if spec.configured}


def ranking_picks(ranking: Any) -> Dict[str, Any]:
    """Cérebro e reservas do model_ranking.json, sem os modelos que falharam no último teste (ok=false).
    Um cérebro com ok=false vira None: a UI mostra "o ranking ainda não tem um cérebro"."""
    if not isinstance(ranking, dict):
        return {"brain": None, "fallbacks": []}
    failed = llm_catalog.failed_models(ranking)
    brain_raw = ranking.get("brain") if isinstance(ranking.get("brain"), dict) else {}
    brain = _pick(brain_raw.get("provider"), brain_raw.get("model"))
    if brain and (brain["provider"], brain["model"]) in failed:
        brain = None
    fallbacks = []
    for fb in ranking.get("fallbacks") or []:
        if isinstance(fb, dict):
            pick = _pick(fb.get("provider"), fb.get("model"))
            if pick and (pick["provider"], pick["model"]) not in failed:
                fallbacks.append(pick)
    return {"brain": brain, "fallbacks": _dedupe(fallbacks)}


def _from_ranking(config, ranking: Any) -> Optional[Dict[str, Any]]:
    if not isinstance(ranking, dict):
        return None
    usable = _configured_set(config)
    got = ranking_picks(ranking)
    picks = [p for p in _dedupe([got["brain"]] + got["fallbacks"]) if p["provider"] in usable]
    if not picks:
        return None
    return {"brain": picks[0], "fallbacks": picks[1:], "source": "ranking"}


def _legacy(config) -> Optional[Dict[str, Any]]:
    """Config antiga: gemini_model/brain_model + brain_fallback_models + cloud_providers[*].model."""
    usable = _configured_set(config)
    picks: List[Optional[Dict[str, str]]] = []
    if "gemini" in usable:
        picks.append(_pick("gemini", getattr(config, "brain_model", "") or getattr(config, "gemini_model", "")))
        for model in getattr(config, "brain_fallback_models", None) or []:
            picks.append(_pick("gemini", model))
    for pid, spec in usable.items():
        if pid != "gemini":
            picks.append(_pick(pid, spec.model))
    picks = _dedupe([p for p in picks if p])
    if not picks:
        return None
    return {"brain": picks[0], "fallbacks": picks[1:], "source": "legacy"}


def _default_fast(config, usable: Dict[str, Any], brain: Optional[Dict[str, str]],
                  fallbacks: List[Dict[str, str]]) -> Optional[Dict[str, str]]:
    saved = getattr(config, "model_config", None) or {}
    if isinstance(saved, dict) and isinstance(saved.get("fast"), dict):
        fp = _pick(saved["fast"].get("provider"), saved["fast"].get("model"))
        if fp and fp["provider"] in usable:
            return fp
    local_m = str(getattr(config, "local_model", "") or getattr(config, "gemini_model", "")).strip()
    if local_m and "gemini" in usable:
        return {"provider": "gemini", "model": local_m}
    if fallbacks:
        return dict(fallbacks[-1]) if len(fallbacks) > 1 else dict(fallbacks[0])
    return dict(brain) if brain else None


def get_model_config(config, ranking: Any = llm_catalog._LOAD) -> Dict[str, Any]:
    """{brain: {provider, model} | None, fast: {provider, model} | None, fallbacks: [{provider, model}], source}."""
    usable = _configured_set(config)
    saved = getattr(config, "model_config", None) or {}
    if isinstance(saved, dict) and isinstance(saved.get("brain"), dict) and saved["brain"].get("model"):
        brain = _pick(saved["brain"].get("provider"), saved["brain"].get("model"))
        fallbacks = [p for p in (_pick(f.get("provider"), f.get("model"))
                                 for f in saved.get("fallbacks") or [] if isinstance(f, dict)) if p]
        fast_raw = saved.get("fast") if isinstance(saved.get("fast"), dict) else {}
        fast = _pick(fast_raw.get("provider"), fast_raw.get("model"))
        if brain:
            dedup_fb = _dedupe([f for f in fallbacks if f != brain])
            return {
                "brain": brain,
                "fast": fast or _default_fast(config, usable, brain, dedup_fb),
                "fallbacks": dedup_fb,
                "source": "saved",
            }
    loaded = llm_catalog.load_ranking() if ranking is llm_catalog._LOAD else ranking
    found = _from_ranking(config, loaded) or _legacy(config)
    if found is None:
        return {"brain": None, "fast": None, "fallbacks": [], "source": "none"}
    found["fast"] = _default_fast(config, usable, found.get("brain"), found.get("fallbacks") or [])
    return found


def validate_model_config(config, payload: Any) -> Dict[str, Any]:
    """Valida {brain, fallbacks, fast?} vindo da UI. Só provedores configurados; modelo digitado à mão é aceito."""
    if not isinstance(payload, dict):
        raise ValueError("configuração de modelos inválida")
    usable = _configured_set(config)

    def check(item: Any, label: str) -> Dict[str, str]:
        if not isinstance(item, dict):
            raise ValueError(label + " deve ter provider e model")
        provider = str(item.get("provider") or "").strip()
        model = str(item.get("model") or "").strip()
        if not model:
            raise ValueError(label + ": informe o modelo")
        if provider not in usable:
            raise ValueError(label + ": provedor '" + provider + "' não está configurado (sem chave)")
        return {"provider": provider, "model": model}

    brain = check(payload.get("brain"), "cérebro")
    raw = payload.get("fallbacks") or []
    if not isinstance(raw, list):
        raise ValueError("fallbacks deve ser uma lista")
    if len(raw) > MAX_FALLBACKS:
        raise ValueError("no máximo " + str(MAX_FALLBACKS) + " modelos de reserva")
    fallbacks = _dedupe([check(item, "reserva " + str(i + 1)) for i, item in enumerate(raw)])
    out: Dict[str, Any] = {"brain": brain, "fallbacks": [f for f in fallbacks if f != brain]}

    raw_fast = payload.get("fast")
    if isinstance(raw_fast, dict) and (raw_fast.get("provider") or raw_fast.get("model")):
        out["fast"] = check(raw_fast, "resposta rápida")
    else:
        prev = getattr(config, "model_config", None) or {}
        if isinstance(prev, dict) and isinstance(prev.get("fast"), dict):
            fp = _pick(prev["fast"].get("provider"), prev["fast"].get("model"))
            if fp and fp["provider"] in usable:
                out["fast"] = fp
    return out


def save_model_config(config, payload: Any) -> Dict[str, Any]:
    cleaned = validate_model_config(config, payload)
    config.model_config = cleaned
    if cleaned.get("brain") and cleaned["brain"].get("model"):
        config.brain_model = cleaned["brain"]["model"]
    if cleaned.get("fast") and cleaned["fast"].get("model"):
        config.local_model = cleaned["fast"]["model"]
    if "fallbacks" in cleaned:
        config.brain_fallback_models = [f["model"] for f in (cleaned.get("fallbacks") or []) if f.get("model")]
    config.save()
    return get_model_config(config)


def make_completer(config) -> Callable[..., str]:
    """Cliente real: chama o provedor do alvo com a chave da config (nunca devolve chave)."""

    def complete(target: Target, system: str, prompt: str, *, json_mode: bool = False,
                 max_tokens: int = 700, temperature: float = 0.3,
                 timeout: float = DEFAULT_TIMEOUT_SECONDS) -> str:
        spec = llm_catalog.provider_specs(config).get(target.provider)
        if spec is None or not spec.configured:
            raise ProviderError("provedor " + target.provider + " não configurado", 401)
        return llm_catalog.call_chat(spec, target.model, system, prompt, json_mode=json_mode,
                                     max_tokens=max_tokens, temperature=temperature,
                                     timeout=timeout)

    return complete


def build_chain(config, complete: Optional[Callable[..., str]] = None) -> Optional[ModelChain]:
    """Cadeia pronta a partir da config (ou None se nenhum modelo estiver utilizável)."""
    current = get_model_config(config)
    if not current["brain"]:
        return None
    usable = _configured_set(config)
    picks = [p for p in [current["brain"]] + current["fallbacks"] if p["provider"] in usable]
    if not picks:
        return None
    if picks[0] != current["brain"]:
        print("[LLM] cérebro configurado sem chave; usando " + picks[0]["provider"] + "/"
              + picks[0]["model"] + " como cérebro.")
    targets = [Target(p["provider"], p["model"]) for p in picks]
    fast_target: Optional[Target] = None
    saved = getattr(config, "model_config", None) or {}
    if isinstance(saved, dict) and isinstance(saved.get("fast"), dict):
        fp = _pick(saved["fast"].get("provider"), saved["fast"].get("model"))
        if fp and fp["provider"] in usable:
            fast_target = Target(fp["provider"], fp["model"])
    return ModelChain(
        targets[0], targets[1:], complete or make_completer(config),
        fast=fast_target,
        timeout=float(getattr(config, "model_timeout_seconds", DEFAULT_TIMEOUT_SECONDS) or DEFAULT_TIMEOUT_SECONDS),
        health_interval=float(getattr(config, "model_health_interval_seconds",
                                      DEFAULT_HEALTH_INTERVAL_SECONDS) or DEFAULT_HEALTH_INTERVAL_SECONDS),
    )
