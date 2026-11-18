"""
Claude (via the user's Claude plan) for S.A.G.E.
================================================
Runs the Claude Code CLI headless (``claude -p``), which is already logged in
with the user's Claude subscription, so no API key is required.

Two modes:

* ``text``  - cheapest helper: ``claude-haiku-5-5`` with every built-in tool
  disabled and a short custom system prompt (no Claude Code preamble).
* ``agent`` - full agent with the user's installed **Skills and MCP servers**
  and permission to **create/edit files** (``acceptEdits``). Shell commands are
  only allowed when ``allow_bash`` is on.

The stream-json output is parsed live: text deltas and tool calls are reported
through ``on_event`` so the HUD can show what the clone is doing, and the
``rate_limit_event`` is stored so S.A.G.E. checks the plan usage *before*
calling Claude again.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
from typing import Any, Callable, Dict, List, Optional, Tuple

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATE_PATH = os.path.join(ROOT_DIR, "data", "claude_state.json")

TokenFn = Callable[[str, str], None]          # (kind, text) kind: text|thinking|tool|status

AGENT_TOOLS = ["Read", "Write", "Edit", "Glob", "Grep", "WebSearch", "WebFetch",
               "Skill", "TodoWrite", "NotebookEdit"]


def find_claude_executable() -> str:
    candidates = [
        os.path.join(os.path.expanduser("~"), ".local", "bin", "claude.exe"),
        shutil.which("claude") or "",
        shutil.which("claude.exe") or "",
    ]
    for c in candidates:
        if c and os.path.exists(c):
            return c
    return "claude"


class ClaudeUsage:
    """Remembers the last rate-limit snapshot reported by the CLI."""

    def __init__(self, path: str = STATE_PATH):
        self.path = path
        self.data: Dict[str, Any] = {}
        try:
            with open(path, "r", encoding="utf-8") as f:
                self.data = json.load(f) or {}
        except Exception:
            self.data = {}
        self._lock = threading.Lock()

    def update(self, info: Dict[str, Any], cost: float = 0.0,
               mcp_servers: Optional[List[str]] = None, skills: Optional[List[str]] = None) -> None:
        with self._lock:
            if mcp_servers:
                self.data["mcp_servers"] = mcp_servers
            if skills:
                self.data["skills"] = skills
            if info:
                self.data["rate_limit"] = info
                self.data["checked_at"] = time.time()
            if cost:
                today = time.strftime("%Y-%m-%d")
                if self.data.get("day") != today:
                    self.data["day"], self.data["cost_today"], self.data["calls_today"] = today, 0.0, 0
                self.data["cost_today"] = float(self.data.get("cost_today", 0.0)) + float(cost)
                self.data["calls_today"] = int(self.data.get("calls_today", 0)) + 1
            try:
                os.makedirs(os.path.dirname(self.path), exist_ok=True)
                with open(self.path, "w", encoding="utf-8") as f:
                    json.dump(self.data, f, indent=2)
            except Exception:
                pass

    def status(self, max_utilization: float = 0.9) -> Tuple[bool, str]:
        info = self.data.get("rate_limit") or {}
        if not info:
            return True, "sem dados de uso ainda"
        now = time.time()
        if str(info.get("status", "allowed")).lower() not in ("allowed", "allowed_warning"):
            reset = float(info.get("resetsAt") or 0)
            if reset and reset > now:
                return False, "limite do plano Claude atingido (libera às " + \
                    time.strftime("%H:%M", time.localtime(reset)) + ")"
        windows = info.get("unifiedWindows") or {}
        for name, w in windows.items():
            try:
                util = float(w.get("utilization") or 0)
                reset = float(w.get("resetsAt") or 0)
            except Exception:
                continue
            if util >= max_utilization and reset > now:
                label = {"five_hour": "janela de 5h", "seven_day": "semana"}.get(name, name)
                return False, "uso do plano Claude em " + str(int(util * 100)) + "% na " + label
        return True, "ok"

    def summary(self) -> str:
        info = self.data.get("rate_limit") or {}
        parts = []
        for name, w in (info.get("unifiedWindows") or {}).items():
            try:
                parts.append({"five_hour": "5h", "seven_day": "7d"}.get(name, name) + " " +
                             str(int(float(w.get("utilization") or 0) * 100)) + "%")
            except Exception:
                pass
        return ", ".join(parts) or "desconhecido"


class ClaudeCodeRun:
    """One headless Claude call; exposes liveness for the swarm watchdog."""

    def __init__(self, cmd: List[str], prompt: str, cwd: str, on_token: Optional[TokenFn]):
        self.cmd = cmd
        self.prompt = prompt
        self.cwd = cwd
        self.on_token = on_token
        self.proc: Optional[subprocess.Popen] = None
        self.text_parts: List[str] = []
        self.result_text = ""
        self.is_error = False
        self.error = ""
        self.rate_limit: Dict[str, Any] = {}
        self.cost = 0.0
        self.mcp_servers: List[str] = []
        self.skills: List[str] = []
        self.last_activity = time.time()

    def alive(self) -> bool:
        return self.proc is not None and self.proc.poll() is None

    def kill(self) -> None:
        if self.alive():
            if sys.platform == "win32" and self.proc and getattr(self.proc, "pid", None):
                try:
                    subprocess.run(
                        ["taskkill", "/F", "/T", "/PID", str(self.proc.pid)],
                        capture_output=True,
                        creationflags=subprocess.CREATE_NO_WINDOW,
                        timeout=3,
                    )
                except Exception:
                    pass
            try:
                self.proc.kill()
            except Exception:
                pass

    def _emit(self, kind: str, text: str) -> None:
        self.last_activity = time.time()
        if self.on_token and text:
            try:
                self.on_token(kind, text)
            except Exception:
                pass

    def run(self, timeout: int) -> str:
        flags = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0
        env = os.environ.copy()
        env.pop("ANTHROPIC_API_KEY", None)  # always bill the user's plan login
        self.proc = subprocess.Popen(
            self.cmd, cwd=self.cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
            creationflags=flags, env=env, bufsize=1)
        try:
            self.proc.stdin.write(self.prompt)
            self.proc.stdin.close()
        except Exception:
            pass
        deadline = time.time() + timeout
        for line in iter(self.proc.stdout.readline, ""):
            self.last_activity = time.time()
            self._handle(line)
            if time.time() > deadline:
                self.kill()
                self.error = "tempo limite (" + str(timeout) + "s)"
                break
        try:
            self.proc.wait(timeout=10)
        except Exception:
            self.kill()
        if not self.result_text and not self.error:
            err = ""
            try:
                err = (self.proc.stderr.read() or "").strip()
            except Exception:
                pass
            self.error = err[:400] or "Claude encerrou sem resposta"
        return self.result_text or "".join(self.text_parts).strip()

    def _handle(self, line: str) -> None:
        line = line.strip()
        if not line.startswith("{"):
            return
        try:
            ev = json.loads(line)
        except Exception:
            return
        kind = ev.get("type")
        if kind == "stream_event":
            inner = ev.get("event") or {}
            delta = inner.get("delta") or {}
            if delta.get("type") == "text_delta":
                self.text_parts.append(delta.get("text", ""))
                self._emit("text", delta.get("text", ""))
            elif delta.get("type") == "thinking_delta" and delta.get("thinking"):
                self._emit("thinking", delta.get("thinking", ""))
        elif kind == "assistant":
            for block in (ev.get("message") or {}).get("content") or []:
                if block.get("type") == "tool_use":
                    args = json.dumps(block.get("input") or {}, ensure_ascii=False)[:160]
                    self._emit("tool", block.get("name", "tool") + " " + args)
        elif kind == "user":
            for block in (ev.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_result":
                    content = block.get("content")
                    if isinstance(content, list):
                        content = " ".join(str(c.get("text", "")) for c in content if isinstance(c, dict))
                    self._emit("tool_result", str(content or "")[:200])
        elif kind == "system" and ev.get("subtype") == "init":
            tools = ev.get("tools") or []
            self.mcp_servers = sorted({t.split("__")[1] for t in tools
                                       if isinstance(t, str) and t.startswith("mcp__") and t.count("__") >= 2})
            self.skills = list(ev.get("skills") or [])
        elif kind == "rate_limit_event":
            self.rate_limit = ev.get("rate_limit_info") or {}
        elif kind == "system" and ev.get("subtype") == "status":
            self._emit("status", str(ev.get("status") or ""))
        elif kind == "result":
            self.result_text = str(ev.get("result") or "").strip()
            self.is_error = bool(ev.get("is_error"))
            self.cost = float(ev.get("total_cost_usd") or 0.0)
            if self.is_error:
                self.error = self.result_text or str(ev.get("subtype") or "erro")


class ClaudeCodeBackend:
    """Swarm backend (duck-typed like sage.swarm.Backend)."""

    kind = "cloud"
    slow = False

    def __init__(self, model: str = "claude-haiku-5-5", mode: str = "text",
                 executable: Optional[str] = None, cwd: Optional[str] = None,
                 allow_bash: bool = False, extra_dirs: Optional[List[str]] = None,
                 max_utilization: float = 0.9, effort: str = "low",
                 usage: Optional[ClaudeUsage] = None, max_turns: int = 40):
        self.model = model
        self.mode = mode
        self.executable = executable or find_claude_executable()
        self.cwd = cwd or os.path.expanduser("~")
        self.allow_bash = allow_bash
        self.extra_dirs = [d for d in (extra_dirs or []) if d and os.path.isdir(d)]
        self.max_utilization = max_utilization
        self.effort = effort
        self.usage = usage or ClaudeUsage()
        self.max_turns = max_turns
        short = model.replace("claude-", "").replace("-", " ")
        self.key = "claude:" + mode + ":" + model
        self.label = ("Claude " + short) if mode == "text" else ("Agente Claude " + short)
        self.current: Optional[ClaudeCodeRun] = None

    def available(self) -> Tuple[bool, str]:
        if not (os.path.exists(self.executable) or shutil.which(self.executable)):
            return False, "Claude Code CLI não encontrado"
        return self.usage.status(self.max_utilization)

    def build_cmd(self, system: str) -> List[str]:
        cmd = [self.executable, "-p", "--model", self.model,
               "--output-format", "stream-json", "--verbose", "--include-partial-messages",
               "--no-session-persistence", "--effort", self.effort]
        if self.mode == "text":
            cmd += ["--tools", "", "--system-prompt", system]
        else:
            full = getattr(self, "full_control", False)
            tools = list(AGENT_TOOLS) + (["Bash"] if (self.allow_bash or full) else [])
            cmd += ["--append-system-prompt", system,
                    "--permission-mode", "bypassPermissions" if full else "acceptEdits",
                    "--max-turns", str(self.max_turns),
                    "--allowedTools"] + tools +                 ["mcp__" + name for name in self._mcp_prefixes()]
            dirs = list(self.extra_dirs)
            if full:  # every local drive
                import string
                dirs += [c + ":\\" for c in string.ascii_uppercase if os.path.isdir(c + ":\\")]
            for d in dict.fromkeys(dirs):
                cmd += ["--add-dir", d]
        return cmd

    def _mcp_prefixes(self) -> List[str]:
        names = set(self.usage.data.get("mcp_servers") or [])
        try:
            from sage.claude_admin import list_mcp
            names.update(s["prefix"] for s in list_mcp() if s.get("ok"))
        except Exception:
            pass
        return sorted(names)

    def complete(self, system: str, prompt: str, *, json_mode: bool = False,
                 max_tokens: int = 1200, temperature: float = 0.3, timeout: int = 300,
                 on_token: Optional[TokenFn] = None, monitor=None) -> Tuple[str, str]:
        if json_mode:
            prompt += "\n\nResponda SOMENTE com um objeto JSON válido, sem texto fora dele."
        run = ClaudeCodeRun(self.build_cmd(system), prompt, self.cwd, on_token)
        self.current = run
        if monitor is not None:
            monitor.attach(run)
        try:
            text = run.run(timeout)
        finally:
            self.usage.update(run.rate_limit, run.cost, run.mcp_servers, run.skills)
            self.current = None
        if run.is_error or (not text and run.error):
            from sage.swarm import BackendError
            raise BackendError("Claude: " + (run.error or "falhou"))
        return text, ""
