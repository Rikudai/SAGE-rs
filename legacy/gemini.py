"""
Google Gemini client for S.A.G.E. (Google AI Studio API key).
==============================================================
Native ``generativelanguage`` REST API, no SDK dependency:
* ``generate``  - one-shot text (optionally JSON / images)
* ``stream``    - SSE streaming, yields text chunks as they arrive
* ``embed``     - embeddings for the long-term memory
* ``raw``       - one ``generateContent`` call returning the whole JSON (media models)

The key comes from config (``gemini_api_key``) or the ``GEMINI_API_KEY`` /
``GOOGLE_API_KEY`` environment variables.

Rate limits are tracked **per model**: a quota error on an image or music model
never blocks the brain's text model.
"""
from __future__ import annotations

import json
import os
import random
import threading
import time
import urllib.error
import urllib.request
from typing import Any, Callable, Dict, Iterator, List, Optional, Tuple

API_BASE = "https://generativelanguage.googleapis.com/v1beta"
DEFAULT_MODEL = "gemini-3.1-flash-lite"
DEFAULT_EMBED_MODEL = "gemini-embedding-001"


class GeminiError(RuntimeError):
    def __init__(self, message: str, status: int = 0, quota_ids: Optional[List[str]] = None,
                 model: str = ""):
        super().__init__(message)
        self.status = status
        self.quota_ids = list(quota_ids or [])
        self.model = model

    @property
    def daily_quota(self) -> bool:
        """429 because a per-day (or zero, free-tier) quota is used up: no point retrying soon."""
        return self.status == 429 and any("PerDay" in q for q in self.quota_ids)

    @property
    def gone(self) -> bool:
        """The model does not exist / is not available to this key."""
        return self.status in (403, 404, 410)


def resolve_key(explicit: str = "") -> str:
    return (explicit or os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY") or "").strip()


# models that answered "Thinking level MINIMAL is not supported": they get "low" instead
NO_MINIMAL_THINKING: set = set()


def thinking_config(model: str, level: str) -> Optional[Dict[str, Any]]:
    """Thinking settings in the format each model generation accepts (None = send nothing)."""
    m = (model or "").lower()
    if level == "minimal" and m in NO_MINIMAL_THINKING:
        level = "low"
    if not level or any(t in m for t in ("gemma", "-tts", "-image", "lyria", "transcribe",
                                         "embedding", "nano-banana")):
        return None
    if m.startswith("gemini-2.5") or m.startswith("gemini-2.0"):
        budget = {"minimal": 0, "low": 1024, "medium": 4096, "high": 16384}.get(level, 1024)
        if "pro" in m:
            budget = max(budget, 128)          # 2.5 Pro cannot switch thinking off
        return {"thinkingBudget": budget}
    return {"thinkingLevel": level}


class GeminiClient:
    def __init__(self, api_key: str = "", model: str = DEFAULT_MODEL,
                 embed_model: str = DEFAULT_EMBED_MODEL, base_url: str = API_BASE):
        self.api_key = resolve_key(api_key)
        self.model = model or DEFAULT_MODEL
        self.embed_model = embed_model or DEFAULT_EMBED_MODEL
        self.base_url = base_url.rstrip("/")
        self._blocked: Dict[str, Tuple[float, str]] = {}     # model -> (until, why)
        self._lock = threading.Lock()
        self.calls = 0

    # ------------------------------------------------------------------
    def block(self, model: str, seconds: float, why: str) -> None:
        with self._lock:
            self._blocked[model or self.model] = (time.time() + seconds, why)

    def blocked(self, model: str = "") -> str:
        """Why this model is paused right now ("" = usable)."""
        until, why = self._blocked.get(model or self.model, (0.0, ""))
        return why if time.time() < until else ""

    def available(self, model: str = "") -> Tuple[bool, str]:
        if not self.api_key:
            return False, "sem chave de API do Gemini"
        why = self.blocked(model)
        if why:
            return False, why
        return True, "ok"

    def _headers(self) -> Dict[str, str]:
        return {"Content-Type": "application/json", "x-goog-api-key": self.api_key,
                "User-Agent": "SAGE-desktop-assistant"}

    def _body(self, prompt: str, system: Optional[str], images: Optional[List[str]],
              json_mode: bool, temperature: float, max_tokens: int,
              thinking: str, model: str = "") -> Dict[str, Any]:
        parts: List[Dict[str, Any]] = []
        for b64 in images or []:
            parts.append({"inline_data": {"mime_type": "image/png", "data": b64}})
        parts.append({"text": prompt})
        gen: Dict[str, Any] = {"temperature": temperature, "maxOutputTokens": int(max_tokens)}
        if json_mode:
            gen["responseMimeType"] = "application/json"
        tc = thinking_config(model or self.model, thinking)
        if tc:
            gen["thinkingConfig"] = tc
        body: Dict[str, Any] = {"contents": [{"role": "user", "parts": parts}],
                                "generationConfig": gen}
        if system:
            if "gemma" in (model or self.model).lower():
                # Gemma has no system instruction: prepend it to the prompt
                parts[-1]["text"] = system + "\n\n" + prompt
            else:
                body["systemInstruction"] = {"parts": [{"text": system}]}
        return body

    # transient failures (overload, rate limit, network) are retried with exponential back-off
    TRANSIENT_CODES = (408, 429, 500, 502, 503, 504)
    retries = 3
    backoff = 2.0          # seconds: 2, 4, 8 (+ jitter)

    @staticmethod
    def _error_from_http(e: urllib.error.HTTPError, model: str) -> GeminiError:
        detail, quota_ids = "", []
        try:
            err = json.loads(e.read().decode("utf-8", "replace")).get("error", {})
            detail = str(err.get("message", ""))
            for det in err.get("details") or []:
                for v in det.get("violations") or []:
                    if v.get("quotaId"):
                        quota_ids.append(str(v["quotaId"]))
        except Exception:
            pass
        return GeminiError("Gemini HTTP " + str(e.code) + (": " + detail[:200] if detail else ""),
                           e.code, quota_ids, model)

    def _open(self, url: str, body: Dict[str, Any], timeout: float, model: str = "",
              retries: Optional[int] = None):
        model = model or self.model
        ok, why = self.available(model)
        if not ok:
            raise GeminiError(why, 401 if "chave" in why else 429, model=model)
        data = json.dumps(body).encode("utf-8")
        max_retries = self.retries if retries is None else retries
        attempt = 0
        while True:
            req = urllib.request.Request(url, data=data, headers=self._headers())
            self.calls += 1
            try:
                return urllib.request.urlopen(req, timeout=timeout)
            except urllib.error.HTTPError as e:
                err = self._error_from_http(e, model)
                tc = (body.get("generationConfig") or {}).get("thinkingConfig") or {}
                if e.code == 400 and tc.get("thinkingLevel") == "minimal" and                         "thinking level" in str(err).lower():
                    # this model cannot think "minimal": use "low" from now on and resend now
                    NO_MINIMAL_THINKING.add((model or "").lower())
                    tc["thinkingLevel"] = "low"
                    data = json.dumps(body).encode("utf-8")
                    continue
                if err.daily_quota:
                    why = "sem cota gratuita hoje para " + model
                    self.block(model, 6 * 3600, why)
                    raise GeminiError("Gemini: " + why + " (ative o faturamento no Google AI Studio "
                                      "para liberar)", e.code, err.quota_ids, model)
                if err.gone:
                    why = model + " indisponível para esta chave"
                    self.block(model, 6 * 3600, why)
                    raise GeminiError("Gemini: " + why + " - " + str(err)[:160], e.code,
                                      err.quota_ids, model)
                if e.code not in self.TRANSIENT_CODES or attempt >= max_retries:
                    if e.code == 429:
                        self.block(model, 30, "limite de requisições de " + model + " (aguardando)")
                    raise err
            except Exception as e:
                err = GeminiError("Gemini falhou: " + str(e), model=model)
                reason = getattr(e, "reason", e)
                if isinstance(e, TimeoutError) or isinstance(reason, TimeoutError) or                         "timed out" in str(e).lower():
                    # a connection that stayed silent for the whole timeout: retrying it only
                    # multiplies the wait (3 x 75 s) - fail now so the next model/AI takes over
                    raise err
                if attempt >= max_retries:
                    raise err
            attempt += 1
            wait = self.backoff * (2 ** (attempt - 1)) * (0.8 + 0.4 * random.random())
            print("[Gemini] " + str(err)[:120] + " - nova tentativa " + str(attempt) + "/" +
                  str(max_retries) + " em " + str(round(wait, 1)) + " s")
            time.sleep(wait)

    @staticmethod
    def _split(data: Dict[str, Any]) -> Tuple[str, str]:
        """Returns (text, thought_summary) of one response/chunk."""
        text, thought = [], []
        for cand in (data.get("candidates") or [])[:1]:
            for part in ((cand.get("content") or {}).get("parts") or []):
                t = part.get("text") or ""
                (thought if part.get("thought") else text).append(t)
        if not data.get("candidates") and data.get("promptFeedback"):
            reason = data["promptFeedback"].get("blockReason")
            if reason:
                raise GeminiError("Gemini bloqueou o pedido (" + str(reason) + ")")
        return "".join(text), "".join(thought)

    # ------------------------------------------------------------------
    def generate(self, prompt: str, system: Optional[str] = None, *, model: str = "",
                 images: Optional[List[str]] = None, json_mode: bool = False,
                 temperature: float = 0.4, max_tokens: int = 1200, timeout: float = 90,
                 thinking: str = "minimal", retries: Optional[int] = None) -> str:
        model = model or self.model
        url = self.base_url + "/models/" + model + ":generateContent"
        body = self._body(prompt, system, images, json_mode, temperature, max_tokens, thinking, model)
        with self._open(url, body, timeout, model, retries) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        return self._split(data)[0].strip()

    def raw(self, model: str, body: Dict[str, Any], timeout: float = 180,
            retries: Optional[int] = None) -> Dict[str, Any]:
        """One generateContent call with a caller-built body; returns the parsed JSON."""
        url = self.base_url + "/models/" + model + ":generateContent"
        with self._open(url, body, timeout, model, retries) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def stream(self, prompt: str, system: Optional[str] = None, *, model: str = "",
               json_mode: bool = False, temperature: float = 0.4, max_tokens: int = 1200,
               timeout: float = 180, thinking: str = "minimal",
               cancelled: Optional[Callable[[], bool]] = None,
               retries: Optional[int] = None,
               body: Optional[Dict[str, Any]] = None) -> Iterator[Tuple[str, str]]:
        """Yields ("text"|"thinking", chunk) while the answer streams in."""
        model = model or self.model
        url = self.base_url + "/models/" + model + ":streamGenerateContent?alt=sse"
        if body is None:
            body = self._body(prompt, system, None, json_mode, temperature, max_tokens, thinking, model)
            tc = body["generationConfig"].get("thinkingConfig")
            if tc and thinking and thinking != "minimal":
                tc["includeThoughts"] = True
        with self._open(url, body, timeout, model, retries) as resp:
            for raw in resp:
                if cancelled and cancelled():
                    return
                line = raw.decode("utf-8", "replace").strip()
                if not line.startswith("data:"):
                    continue
                try:
                    data = json.loads(line[5:].strip())
                except Exception:
                    continue
                if data.get("error"):
                    raise GeminiError("Gemini: " + str(data["error"].get("message", data["error"])),
                                      model=model)
                text, thought = self._split(data)
                if thought:
                    yield "thinking", thought
                if text:
                    yield "text", text

    def embed(self, text: str, timeout: float = 30) -> Optional[List[float]]:
        if not text.strip() or not self.api_key:
            return None
        url = self.base_url + "/models/" + self.embed_model + ":embedContent"
        body = {"model": "models/" + self.embed_model,
                "content": {"parts": [{"text": text[:8000]}]}}
        with self._open(url, body, timeout, self.embed_model) as resp:
            data = json.loads(resp.read().decode("utf-8"))
        vals = (data.get("embedding") or {}).get("values")
        return vals if isinstance(vals, list) and vals else None
