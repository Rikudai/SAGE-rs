"""
Antigravity (agy) online brain.

Only this module talks to a remote model. It:
* runs prompts in JSON mode and captures token usage,
* keeps a local daily token ledger and a quota block marker,
* detects quota/rate-limit failures and raises QuotaExceeded so the
  orchestrator can ask the user before using the slow local contingency model.
"""
from __future__ import annotations

import datetime
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from typing import Any, Dict, Optional

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_STATE_PATH = os.path.join(ROOT_DIR, "data", "antigravity_state.json")

QUOTA_PATTERNS = re.compile(
    r"(resource[_\s-]?exhausted|quota|rate[_\s-]?limit|too many requests|"
    r"limit (?:reached|exceeded)|exceeded (?:your|the) (?:quota|limit)|"
    r"out of (?:credits|tokens)|\b429\b)",
    re.IGNORECASE,
)


class AntigravityError(RuntimeError):
    pass


class QuotaExceeded(AntigravityError):
    pass


@dataclass
class AgentResult:
    text: str
    conversation_id: str = ""
    tokens: int = 0
    status: str = "SUCCESS"
    elapsed: float = 0.0
    model: str = ""
    raw: str = ""


@dataclass
class QuotaStatus:
    available: bool
    reason: str
    used_today: int
    limit: int
    remaining: int
    reset_at: str
    account: str
    model: str
    blocked_until: str = ""


class AntigravityClient:
    def __init__(
        self,
        executable_path: str,
        account: str = "ag1",
        model: str = "gemini-3.8-flash-high",
        timeout_seconds: int = 300,
        daily_token_limit: int = 500000,
        state_path: Optional[str] = None,
        workspace: str = ROOT_DIR,
    ):
        self.executable_path = executable_path
        self.account = account
        self.model = model
        self.timeout_seconds = timeout_seconds
        self.daily_token_limit = int(daily_token_limit or 0)
        self.state_path = state_path or DEFAULT_STATE_PATH
        self.workspace = workspace
        self._state = self._load_state()

    # ------------------------------------------------------------------
    # State / ledger
    # ------------------------------------------------------------------
    def _load_state(self) -> Dict[str, Any]:
        default = {"date": "", "tokens": 0, "calls": 0, "blocked_until": "", "last_error": ""}
        try:
            if os.path.exists(self.state_path):
                with open(self.state_path, "r", encoding="utf-8") as f:
                    data = json.load(f)
                default.update(data or {})
        except Exception:
            pass
        if default.get("date") != datetime.date.today().isoformat():
            default["date"] = datetime.date.today().isoformat()
            default["tokens"] = 0
            default["calls"] = 0
        return default

    def _save_state(self) -> None:
        try:
            os.makedirs(os.path.dirname(os.path.abspath(self.state_path)), exist_ok=True)
            with open(self.state_path, "w", encoding="utf-8") as f:
                json.dump(self._state, f, indent=2, ensure_ascii=False)
        except Exception:
            pass

    def _refresh_day(self) -> None:
        today = datetime.date.today().isoformat()
        if self._state.get("date") != today:
            self._state.update({"date": today, "tokens": 0, "calls": 0})
            self._save_state()

    def record_usage(self, tokens: int) -> None:
        self._refresh_day()
        self._state["tokens"] = int(self._state.get("tokens", 0)) + max(0, int(tokens))
        self._state["calls"] = int(self._state.get("calls", 0)) + 1
        self._save_state()

    @staticmethod
    def _reset_delay(text: str) -> "datetime.timedelta":
        """Uses the service's own 'Resets in 68h9m55s' hint; 6 h when it gives none."""
        m = re.search(r"resets?\s+in\s+(?:(\d+)\s*d)?\s*(?:(\d+)\s*h)?\s*(?:(\d+)\s*m)?\s*(?:(\d+)\s*s)?",
                      text or "", re.IGNORECASE)
        if m and any(m.groups()):
            d, h, mi, s = (int(x) if x else 0 for x in m.groups())
            delay = datetime.timedelta(days=d, hours=h, minutes=mi, seconds=s)
            if delay.total_seconds() > 0:
                return delay + datetime.timedelta(minutes=1)
        return datetime.timedelta(hours=6)

    def _block(self, until: str, error: str) -> None:
        self._state["blocked_until"] = until
        self._state["last_error"] = error[:500]
        self._save_state()

    def quota_status(self) -> QuotaStatus:
        self._refresh_day()
        now = datetime.datetime.now()
        blocked_until = self._state.get("blocked_until") or ""
        used = int(self._state.get("tokens", 0))
        remaining = max(0, self.daily_token_limit - used) if self.daily_token_limit else 10**9
        reset_at = (datetime.date.today() + datetime.timedelta(days=1)).isoformat() + "T00:00:00"

        if blocked_until:
            try:
                until_dt = datetime.datetime.fromisoformat(blocked_until)
                if until_dt > now:
                    return QuotaStatus(False, "limite do Antigravity atingido", used,
                                       self.daily_token_limit, 0, until_dt.isoformat(),
                                       self.account, self.model, blocked_until)
            except Exception:
                pass
        if self.daily_token_limit and used >= self.daily_token_limit:
            return QuotaStatus(False, "orçamento diário de tokens esgotado", used,
                               self.daily_token_limit, 0, reset_at,
                               self.account, self.model, blocked_until)
        return QuotaStatus(True, "disponível", used, self.daily_token_limit,
                           remaining, reset_at, self.account, self.model, blocked_until)

    # ------------------------------------------------------------------
    # Execution
    # ------------------------------------------------------------------
    def _creationflags(self) -> int:
        return subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0

    def run(self, prompt: str, model: Optional[str] = None,
            timeout: Optional[int] = None) -> AgentResult:
        prompt = (prompt or "").strip()
        if not prompt:
            raise AntigravityError("prompt vazio")

        status = self.quota_status()
        if not status.available:
            raise QuotaExceeded(status.reason)

        model = model or self.model
        timeout = int(timeout or self.timeout_seconds)
        # keep the command line safely under the Windows limit
        if len(prompt) > 18000:
            prompt = prompt[:18000] + "\n[conteúdo truncado]"

        cmd = [
            self.executable_path,
            "--dangerously-skip-permissions",
            "--output-format", "json",
            "--model", model,
            "--print=" + prompt,
        ]
        start = time.time()
        try:
            proc = subprocess.run(
                cmd,
                cwd=self.workspace,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                creationflags=self._creationflags(),
            )
        except subprocess.TimeoutExpired:
            raise AntigravityError("timeout do Antigravity (" + str(timeout) + "s)")
        except FileNotFoundError:
            raise AntigravityError("agy.exe não encontrado: " + self.executable_path)

        out = (proc.stdout or "").strip()
        err = (proc.stderr or "").strip()
        combined = out + "\n" + err

        payload: Dict[str, Any] = {}
        if out:
            try:
                payload = json.loads(out)
            except Exception:
                start_idx = out.find("{")
                end_idx = out.rfind("}")
                if start_idx >= 0 and end_idx > start_idx:
                    try:
                        payload = json.loads(out[start_idx:end_idx + 1])
                    except Exception:
                        payload = {}

        status_str = str(payload.get("status") or "").upper()
        if proc.returncode != 0 or status_str in ("ERROR", "FAILED", "QUOTA_EXCEEDED"):
            if QUOTA_PATTERNS.search(combined):
                until = (datetime.datetime.now() + self._reset_delay(combined)).isoformat()
                self._block(until, combined)
                raise QuotaExceeded("limite do Antigravity atingido")
            raise AntigravityError(combined[:800] or "falha no Antigravity")

        usage = payload.get("usage") or {}
        tokens = int(usage.get("total_tokens") or 0)
        if tokens <= 0:
            tokens = max(1, (len(prompt) + len(out)) // 4)
        self.record_usage(tokens)

        return AgentResult(
            text=str(payload.get("response") or out).strip(),
            conversation_id=str(payload.get("conversation_id") or ""),
            tokens=tokens,
            status=status_str or "SUCCESS",
            elapsed=time.time() - start,
            model=model,
            raw=out,
        )

    # Compatibility facade used by the orchestrator/tests
    def run_prompt(self, prompt: str, conversation_id: Optional[str] = None,
                   on_stream_line=None) -> str:
        try:
            result = self.run(prompt)
        except QuotaExceeded:
            raise
        except AntigravityError as e:
            return "Erro no Antigravity: " + str(e)
        if on_stream_line:
            try:
                for line in result.text.splitlines():
                    on_stream_line(line + "\n")
            except Exception:
                pass
        return result.text

    def reset_session(self) -> None:
        # JSON calls are stateless; context comes from the local memory.
        pass
