"""
Full desktop control layer for S.A.G.E..

Provides the "hands" of the assistant: PowerShell/cmd execution, auxiliary
script creation and execution, screen capture, mouse and keyboard automation,
window management and clipboard access. All operations are audit-logged.

Only standard library + Pillow are used; Windows APIs are reached through
ctypes so no extra dependency is required.
"""
from __future__ import annotations

import ctypes
import datetime
import json
import os
import html as html_lib
import queue
import re
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from ctypes import wintypes
from html.parser import HTMLParser
from typing import Any, Dict, List, Optional, Tuple

from PIL import ImageGrab

from sage.result import ActionResult

IS_WINDOWS = sys.platform == "win32"
ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPTS_DIR = os.path.join(ROOT_DIR, "scripts")
LOGS_DIR = os.path.join(ROOT_DIR, "logs")

# Models often write bash (&&, {1..5}, mkdir -p, ls -R, ~/...) - route those to Git Bash.
_BASH_HINTS = re.compile(r"\{\d+\.\.\d+\}|\bmkdir\s+-p\b|\bls\s+-[a-zA-Z]+|\brm\s+-rf?\b|\btouch\s|"
                         r"\bcp\s+-r\b|\bexport\s+\w+=|\$\(|\bgrep\s|\bcat\s+<<|^\s*~/|\s~/")


def _looks_like_bash(command: str) -> bool:
    return bool(_BASH_HINTS.search(command))


def _git_bash() -> Optional[str]:
    for p in (r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe"):
        if os.path.exists(p):
            return p
    return None


def _powershell_exe() -> str:
    """PowerShell 7 (pwsh) understands && and ||; fall back to Windows PowerShell 5.1."""
    import shutil
    return shutil.which("pwsh") or "powershell.exe"

# Catastrophic operations that are never executed automatically.
_BLOCKED_PATTERNS = [
    r"\bformat\s+[a-z]:",
    r"\bdiskpart\b",
    r"\bcipher\s+/w",
    r"\bdel\s+/[sfq].*\b[a-z]:\\",
    r"\brmdir\s+/s.*\b[a-z]:\\",
    r"\bRemove-Item\s+-Recurse.*[a-z]:\\\\?\s*$",
    r"\bshutdown\s+/s",
    r"\bClear-Disk\b",
    r"\bbcdedit\b",
]


def _now() -> str:
    return datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")


def _ssl_context():
    """SSL context with a working CA bundle (Windows/Python quirks)."""
    try:
        import certifi
        return ssl.create_default_context(cafile=certifi.where())
    except Exception:
        try:
            return ssl._create_unverified_context()  # last resort for local tools
        except Exception:
            return None


class _TextExtractor(HTMLParser):
    def __init__(self):
        super().__init__()
        self.parts = []
        self.title = ""
        self._skip = False
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style", "noscript", "svg"):
            self._skip = True
        if tag == "title":
            self._in_title = True
        if tag in ("p", "div", "br", "li", "h1", "h2", "h3", "tr"):
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in ("script", "style", "noscript", "svg"):
            self._skip = False
        if tag == "title":
            self._in_title = False

    def handle_data(self, data):
        if self._skip:
            return
        text = data.strip()
        if not text:
            return
        if self._in_title:
            self.title += text
        self.parts.append(text + " ")


class PowerShellSession:
    """Persistent PowerShell process shared by the assistant."""

    def __init__(self, cwd: str = ROOT_DIR):
        self.cwd = cwd
        self.proc = None
        self._queue: queue.Queue = queue.Queue()
        self._lock = threading.Lock()

    def start(self) -> None:
        if self.proc and self.proc.poll() is None:
            return
        creationflags = subprocess.CREATE_NO_WINDOW if IS_WINDOWS else 0
        self.proc = subprocess.Popen(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "-"],
            cwd=self.cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace",
            bufsize=1, creationflags=creationflags,
        )
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self) -> None:
        try:
            for line in iter(self.proc.stdout.readline, ""):
                self._queue.put(line)
        except Exception:
            pass

    def run(self, command: str, timeout: int = 60) -> str:
        with self._lock:
            self.start()
            marker = "__SAGE_DONE_" + str(int(time.time() * 1000)) + "__"
            try:
                self.proc.stdin.write(command + "\nWrite-Output '" + marker + "'\n")
                self.proc.stdin.flush()
            except Exception as e:
                return "Falha ao enviar comando: " + str(e)
            deadline = time.time() + timeout
            output = []
            while time.time() < deadline:
                try:
                    line = self._queue.get(timeout=0.2)
                except queue.Empty:
                    if self.proc.poll() is not None:
                        break
                    continue
                if marker in line:
                    break
                output.append(line.rstrip())
            return "\n".join(output).strip()

    def close(self) -> None:
        try:
            if self.proc and self.proc.poll() is None:
                self.proc.terminate()
        except Exception:
            pass


class PCController:
    """Thin, safe-ish wrapper around the Windows desktop."""

    def __init__(
        self,
        screen_path: str = os.path.join(ROOT_DIR, "screen.png"),
        audit_dir: str = LOGS_DIR,
        allow_shell: bool = True,
        default_timeout: int = 120,
    ):
        self.screen_path = screen_path
        self.audit_dir = audit_dir
        self.allow_shell = allow_shell
        self.default_timeout = default_timeout
        self._send_input = None
        self._shell: Optional[PowerShellSession] = None
        if IS_WINDOWS:
            try:
                self.user32 = ctypes.windll.user32
                self.kernel32 = ctypes.windll.kernel32
                self._init_win_api()
            except Exception:
                self.user32 = None
                self.kernel32 = None
        else:
            self.user32 = None
            self.kernel32 = None

    def _init_win_api(self) -> None:
        """Declares 64-bit-safe prototypes for the clipboard/global memory APIs."""
        u, k = self.user32, self.kernel32
        u.GetClipboardData.restype = wintypes.HANDLE
        u.GetClipboardData.argtypes = [wintypes.UINT]
        u.SetClipboardData.restype = wintypes.HANDLE
        u.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
        u.OpenClipboard.restype = wintypes.BOOL
        u.OpenClipboard.argtypes = [wintypes.HWND]
        u.EmptyClipboard.restype = wintypes.BOOL
        u.CloseClipboard.restype = wintypes.BOOL
        k.GlobalAlloc.restype = wintypes.HGLOBAL
        k.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
        k.GlobalLock.restype = ctypes.c_void_p
        k.GlobalLock.argtypes = [wintypes.HANDLE]
        k.GlobalUnlock.restype = wintypes.BOOL
        k.GlobalUnlock.argtypes = [wintypes.HANDLE]

    # ------------------------------------------------------------------
    # Audit
    # ------------------------------------------------------------------
    def _audit(self, kind: str, payload: str) -> None:
        try:
            os.makedirs(self.audit_dir, exist_ok=True)
            path = os.path.join(self.audit_dir, f"pc_control_{datetime.date.today().isoformat()}.log")
            with open(path, "a", encoding="utf-8") as f:
                f.write(f"[{_now()}] {kind}: {payload}\n")
        except Exception:
            pass

    @staticmethod
    def _is_blocked(command: str) -> Optional[str]:
        low = command.lower()
        for pattern in _BLOCKED_PATTERNS:
            if re.search(pattern, low):
                return pattern
        return None

    # ------------------------------------------------------------------
    # Shell / scripts
    # ------------------------------------------------------------------
    def run_command(
        self,
        command: str,
        shell: str = "powershell",
        cwd: Optional[str] = None,
        timeout: Optional[int] = None,
    ) -> ActionResult:
        if not command or not command.strip():
            return ActionResult(False, "Nenhum comando informado.")
        if not self.allow_shell:
            return ActionResult(False, "A execução de comandos está desativada na configuração.")

        blocked = self._is_blocked(command)
        if blocked:
            self._audit("BLOCKED", f"{command} ({blocked})")
            return ActionResult(False, "Esse comando é destrutivo demais, então eu bloqueei.",
                                f"Blocked by pattern: {blocked}")

        timeout = timeout or self.default_timeout
        self._audit("RUN", f"[{shell}] {command}")

        if shell.lower() in ("cmd", "batch", "bat"):
            cmd = ["cmd.exe", "/c", command]
        elif shell.lower() in ("bash", "sh") or (_looks_like_bash(command) and _git_bash()):
            cmd = [_git_bash() or "bash", "-lc", command]
        else:
            cmd = [_powershell_exe(), "-NoProfile", "-NonInteractive",
                   "-ExecutionPolicy", "Bypass", "-Command", command]

        creationflags = subprocess.CREATE_NO_WINDOW if IS_WINDOWS else 0
        try:
            proc = subprocess.run(
                cmd,
                cwd=cwd or ROOT_DIR,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout,
                creationflags=creationflags,
            )
            out = (proc.stdout or "").strip()
            err = (proc.stderr or "").strip()
            ok = proc.returncode == 0
            details = out or err or f"returncode={proc.returncode}"
            spoken = "Comando executado com sucesso." if ok else "O comando retornou um erro."
            self._audit("RESULT", f"rc={proc.returncode} out={out[:400]} err={err[:200]}")
            return ActionResult(ok, spoken, details, {"returncode": proc.returncode,
                                                     "stdout": out, "stderr": err})
        except subprocess.TimeoutExpired:
            self._audit("TIMEOUT", command)
            return ActionResult(False, "O comando demorou demais e foi interrompido.",
                                f"Timeout after {timeout}s")
        except Exception as e:
            self._audit("ERROR", f"{command} -> {e}")
            return ActionResult(False, "Falha ao executar o comando.", str(e))

    def write_script(self, name: str, content: str, language: str = "python") -> ActionResult:
        """Creates an auxiliary script inside <project>/scripts."""
        if not content:
            return ActionResult(False, "Nenhum conteúdo de script informado.")
        ext_map = {"python": ".py", "py": ".py", "powershell": ".ps1", "ps1": ".ps1",
                   "batch": ".bat", "bat": ".bat", "cmd": ".bat", "javascript": ".js", "js": ".js"}
        ext = ext_map.get(language.lower(), ".py")
        safe = re.sub(r"[^A-Za-z0-9_.-]+", "_", name).strip("._") or f"script_{int(time.time())}"
        if not safe.lower().endswith(ext):
            safe += ext
        os.makedirs(SCRIPTS_DIR, exist_ok=True)
        path = os.path.join(SCRIPTS_DIR, safe)
        try:
            with open(path, "w", encoding="utf-8") as f:
                f.write(content)
            self._audit("WRITE_SCRIPT", path)
            return ActionResult(True, f"Script {safe} criado.", path, {"path": path})
        except Exception as e:
            return ActionResult(False, "Não consegui escrever o script.", str(e))

    def run_script(self, path: str, args: Optional[List[str]] = None, timeout: Optional[int] = None) -> ActionResult:
        if not os.path.isabs(path):
            path = os.path.join(SCRIPTS_DIR, path)
        if not os.path.exists(path):
            return ActionResult(False, "Script não encontrado.", path)
        ext = os.path.splitext(path)[1].lower()
        args = args or []
        if ext == ".py":
            cmd = [sys.executable, path] + args
            shell_cmd = " ".join([f'"{sys.executable}"', f'"{path}"'] + args)
        elif ext == ".ps1":
            cmd = ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", path] + args
            shell_cmd = " ".join(["powershell", "-File", f'"{path}"'] + args)
        elif ext == ".bat":
            cmd = ["cmd.exe", "/c", path] + args
            shell_cmd = " ".join([f'"{path}"'] + args)
        else:
            cmd = [path] + args
            shell_cmd = " ".join([f'"{path}"'] + args)
        self._audit("RUN_SCRIPT", shell_cmd)
        creationflags = subprocess.CREATE_NO_WINDOW if IS_WINDOWS else 0
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8",
                                  errors="replace", timeout=timeout or self.default_timeout,
                                  cwd=SCRIPTS_DIR, creationflags=creationflags)
            out = (proc.stdout or "").strip()
            err = (proc.stderr or "").strip()
            ok = proc.returncode == 0
            return ActionResult(ok, "Script executado." if ok else "O script falhou.",
                                out or err or f"returncode={proc.returncode}",
                                {"path": path, "stdout": out, "stderr": err,
                                 "returncode": proc.returncode})
        except Exception as e:
            return ActionResult(False, "Falha ao executar o script.", str(e))

    def run_python(self, code: str, timeout: int = 60) -> ActionResult:
        """Runs a temporary Python snippet and returns its output."""
        try:
            fd, path = tempfile.mkstemp(suffix=".py", prefix="sage_snippet_", dir=SCRIPTS_DIR
                                        if os.path.isdir(SCRIPTS_DIR) else None)
            os.close(fd)
            with open(path, "w", encoding="utf-8") as f:
                f.write(code)
            res = self.run_script(path, timeout=timeout)
            try:
                os.remove(path)
            except Exception:
                pass
            res.details = res.details
            return res
        except Exception as e:
            return ActionResult(False, "Falha ao executar código Python.", str(e))

    def shell_run(self, command: str, timeout: int = 60) -> ActionResult:
        """Runs a command in a persistent PowerShell session (state is preserved)."""
        if not command.strip():
            return ActionResult(False, "Comando vazio.")
        blocked = self._is_blocked(command)
        if blocked:
            self._audit("BLOCKED", command)
            return ActionResult(False, "Comando bloqueado por segurança.", blocked)
        try:
            if self._shell is None:
                self._shell = PowerShellSession(ROOT_DIR)
            self._shell.start()
            output = self._shell.run(command, timeout=timeout)
            self._audit("SHELL", command[:300])
            return ActionResult(True, "Comando executado no shell.", output or "(sem saída)",
                                {"stdout": output})
        except Exception as e:
            return ActionResult(False, "Falha no shell persistente.", str(e))

    def close_shell(self) -> None:
        if self._shell:
            self._shell.close()
            self._shell = None

    # ------------------------------------------------------------------
    # Internet
    # ------------------------------------------------------------------
    def fetch_url(self, url: str, max_chars: int = 14000) -> ActionResult:
        if not url or not re.match(r"^https?://", url, re.I):
            return ActionResult(False, "URL inválida (use http:// ou https://).", url)
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) SAGE/1.0",
                "Accept-Language": "pt-BR,pt;q=0.9,en;q=0.8",
            })
            with urllib.request.urlopen(req, timeout=25, context=_ssl_context()) as resp:
                raw = resp.read(2_000_000)
                ctype = resp.headers.get("Content-Type", "")
            if "html" in ctype or raw.lstrip().startswith(b"<"):
                parser = _TextExtractor()
                parser.feed(raw.decode("utf-8", errors="replace"))
                text = re.sub(r"\n{3,}", "\n\n", " ".join(parser.parts))
                text = re.sub(r"[ \t]{2,}", " ", text).strip()
                title = parser.title.strip()
            else:
                text = raw.decode("utf-8", errors="replace")
                title = ""
            self._audit("FETCH", url)
            body = (title + "\n\n" if title else "") + text[:max_chars]
            return ActionResult(True, "Página lida.", body,
                                {"url": url, "title": title, "chars": len(body)})
        except Exception as e:
            return ActionResult(False, "Falha ao acessar a URL.", str(e))

    def web_search_text(self, query: str, limit: int = 5) -> ActionResult:
        """DuckDuckGo Lite search, returning titles/snippets without opening a browser."""
        if not query.strip():
            return ActionResult(False, "Consulta vazia.")
        url = "https://lite.duckduckgo.com/lite/?q=" + urllib.parse.quote(query)
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) SAGE/1.0",
                "Accept-Language": "pt-BR,pt;q=0.9",
            })
            with urllib.request.urlopen(req, timeout=20, context=_ssl_context()) as resp:
                html = resp.read().decode("utf-8", errors="replace")

            anchors = re.findall(
                r'<a[^>]+href="([^"]+)"[^>]*class=[\'"]result-link[\'"][^>]*>(.*?)</a>',
                html, re.DOTALL | re.IGNORECASE)
            snippets = re.findall(
                r'<td class=[\'"]result-snippet[\'"]>(.*?)</td>',
                html, re.DOTALL | re.IGNORECASE)

            lines = []
            for idx, (href, title_html) in enumerate(anchors[:limit]):
                clean_title = html_lib.unescape(re.sub(r"<[^>]+>", "", title_html))
                clean_title = re.sub(r"\s+", " ", clean_title).strip()
                snippet = ""
                if idx < len(snippets):
                    snippet = html_lib.unescape(re.sub(r"<[^>]+>", "", snippets[idx]))
                    snippet = re.sub(r"\s+", " ", snippet).strip()
                link = href
                if "uddg=" in link:
                    try:
                        qs = urllib.parse.parse_qs(urllib.parse.urlparse(link).query)
                        link = qs.get("uddg", [link])[0]
                    except Exception:
                        pass
                if link.startswith("//"):
                    link = "https:" + link
                lines.append("- " + clean_title + " (" + link + ")\n  " + snippet[:240])

            self._audit("SEARCH_WEB", query)
            if not lines:
                return ActionResult(False, "Nenhum resultado textual.", query)
            return ActionResult(True, "Pesquisa concluída.", "\n".join(lines),
                                {"results": lines, "query": query})
        except Exception as e:
            return ActionResult(False, "Falha na pesquisa web.", str(e))

    # ------------------------------------------------------------------
    # Screen
    # ------------------------------------------------------------------
    def screenshot(self, output_path: Optional[str] = None) -> ActionResult:
        target = output_path or self.screen_path
        try:
            os.makedirs(os.path.dirname(os.path.abspath(target)), exist_ok=True)
            img = ImageGrab.grab(all_screens=True)
            img.save(target)
            self._audit("SCREENSHOT", target)
            return ActionResult(True, "Tela capturada.", f"Imagem salva em: {target}",
                                {"path": target, "width": img.width, "height": img.height})
        except Exception as e:
            return ActionResult(False, "Falha ao capturar a tela.", str(e))

    def screen_size(self) -> Tuple[int, int]:
        if self.user32:
            try:
                return int(self.user32.GetSystemMetrics(0)), int(self.user32.GetSystemMetrics(1))
            except Exception:
                pass
        try:
            img = ImageGrab.grab()
            return img.width, img.height
        except Exception:
            return 1920, 1080

    # ------------------------------------------------------------------
    # Mouse / keyboard
    # ------------------------------------------------------------------
    def move_mouse(self, x: int, y: int) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Controle de mouse indisponível.")
        self.user32.SetCursorPos(int(x), int(y))
        self._audit("MOUSE_MOVE", f"{x},{y}")
        return ActionResult(True, "Mouse movido.", f"({x}, {y})")

    def click(self, x: Optional[int] = None, y: Optional[int] = None,
              button: str = "left", clicks: int = 1) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Controle de mouse indisponível.")
        if x is not None and y is not None:
            self.user32.SetCursorPos(int(x), int(y))
            time.sleep(0.05)
        flags = {
            "left": (0x0002, 0x0004),
            "right": (0x0008, 0x0010),
            "middle": (0x0020, 0x0040),
        }.get(button.lower(), (0x0002, 0x0004))
        for _ in range(max(1, int(clicks))):
            self.user32.mouse_event(flags[0], 0, 0, 0, 0)
            time.sleep(0.02)
            self.user32.mouse_event(flags[1], 0, 0, 0, 0)
            time.sleep(0.08)
        self._audit("CLICK", f"{button} x{clicks} at {x},{y}")
        return ActionResult(True, "Clique executado.", f"{button} @ ({x}, {y})")

    def double_click(self, x: int, y: int) -> ActionResult:
        return self.click(x, y, clicks=2)

    def scroll(self, amount: int) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Controle de mouse indisponível.")
        self.user32.mouse_event(0x0800, 0, 0, int(amount), 0)
        self._audit("SCROLL", str(amount))
        return ActionResult(True, "Rolagem executada.", str(amount))

    def _type_unicode(self, text: str) -> None:
        """Types arbitrary Unicode text using SendInput KEYEVENTF_UNICODE."""
        if not self.user32:
            return
        ULONG_PTR = ctypes.POINTER(ctypes.c_ulong)

        class KEYBDINPUT(ctypes.Structure):
            _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD),
                        ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD),
                        ("dwExtraInfo", ULONG_PTR)]

        class MOUSEINPUT(ctypes.Structure):
            _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG),
                        ("mouseData", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                        ("time", wintypes.DWORD), ("dwExtraInfo", ULONG_PTR)]

        class HARDWAREINPUT(ctypes.Structure):
            _fields_ = [("uMsg", wintypes.DWORD), ("wParamL", wintypes.WORD),
                        ("wParamH", wintypes.WORD)]

        class _INPUTunion(ctypes.Union):
            _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT), ("hi", HARDWAREINPUT)]

        class INPUT(ctypes.Structure):
            _fields_ = [("type", wintypes.DWORD), ("union", _INPUTunion)]

        KEYEVENTF_UNICODE = 0x0004
        KEYEVENTF_KEYUP = 0x0002
        inputs = (INPUT * (len(text) * 2))()
        idx = 0
        for ch in text:
            for up in (False, True):
                inp = inputs[idx]
                inp.type = 1  # INPUT_KEYBOARD
                inp.union.ki.wVk = 0
                inp.union.ki.wScan = ord(ch)
                inp.union.ki.dwFlags = KEYEVENTF_UNICODE | (KEYEVENTF_KEYUP if up else 0)
                inp.union.ki.time = 0
                inp.union.ki.dwExtraInfo = None
                idx += 1
        self.user32.SendInput(idx, inputs, ctypes.sizeof(INPUT))

    def type_text(self, text: str, interval: float = 0.0) -> ActionResult:
        if not text:
            return ActionResult(False, "Nenhum texto informado.")
        try:
            if interval and interval > 0:
                for ch in text:
                    self._type_unicode(ch)
                    time.sleep(interval)
            else:
                self._type_unicode(text)
            self._audit("TYPE", text[:200])
            return ActionResult(True, "Texto digitado.", text[:200])
        except Exception as e:
            return ActionResult(False, "Falha ao digitar o texto.", str(e))

    def press_keys(self, combo: str) -> ActionResult:
        """Presses a key combination such as 'ctrl+shift+esc' or 'win+d'."""
        if not self.user32:
            return ActionResult(False, "Teclado virtual indisponível.")
        vk_map = {
            "ctrl": 0x11, "control": 0x11, "shift": 0x10, "alt": 0x12, "win": 0x5B,
            "enter": 0x0D, "return": 0x0D, "esc": 0x1B, "escape": 0x1B, "tab": 0x09,
            "space": 0x20, "backspace": 0x08, "delete": 0x2E, "home": 0x24, "end": 0x23,
            "up": 0x26, "down": 0x28, "left": 0x25, "right": 0x27,
            "f1": 0x70, "f2": 0x71, "f3": 0x72, "f4": 0x73, "f5": 0x74, "f6": 0x75,
            "f7": 0x76, "f8": 0x77, "f9": 0x78, "f10": 0x79, "f11": 0x7A, "f12": 0x7B,
        }
        for ch in "abcdefghijklmnopqrstuvwxyz0123456789":
            vk_map[ch] = ord(ch.upper())
        parts = [p.strip().lower() for p in combo.replace("-", "+").split("+") if p.strip()]
        if not parts:
            return ActionResult(False, "Combinação de teclas inválida.")
        codes = []
        for p in parts:
            if p in vk_map:
                codes.append(vk_map[p])
            elif len(p) == 1:
                codes.append(ord(p.upper()))
            else:
                return ActionResult(False, f"Tecla desconhecida: {p}")
        try:
            for code in codes:
                self.user32.keybd_event(code, 0, 0, 0)
                time.sleep(0.02)
            for code in reversed(codes):
                self.user32.keybd_event(code, 0, 0x0002, 0)
                time.sleep(0.02)
            self._audit("KEYS", combo)
            return ActionResult(True, "Atalho enviado.", combo)
        except Exception as e:
            return ActionResult(False, "Falha ao enviar atalho.", str(e))

    # ------------------------------------------------------------------
    # Windows / clipboard
    # ------------------------------------------------------------------
    def list_windows(self) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Listagem de janelas indisponível.")
        results: List[Dict[str, Any]] = []
        WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

        def _cb(hwnd, _):
            if not self.user32.IsWindowVisible(hwnd):
                return True
            length = self.user32.GetWindowTextLengthW(hwnd)
            if length == 0:
                return True
            buf = ctypes.create_unicode_buffer(length + 1)
            self.user32.GetWindowTextW(hwnd, buf, length + 1)
            title = buf.value
            if title:
                rect = wintypes.RECT()
                self.user32.GetWindowRect(hwnd, ctypes.byref(rect))
                results.append({"hwnd": int(hwnd), "title": title,
                                "x": rect.left, "y": rect.top,
                                "width": rect.right - rect.left,
                                "height": rect.bottom - rect.top})
            return True

        try:
            self.user32.EnumWindows(WNDENUMPROC(_cb), 0)
            return ActionResult(True, f"{len(results)} janelas abertas.",
                                json.dumps(results, ensure_ascii=False), {"windows": results})
        except Exception as e:
            return ActionResult(False, "Falha ao listar janelas.", str(e))

    def focus_window(self, title: str) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Foco de janelas indisponível.")
        found = ctypes.wintypes.HWND(0)
        WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        target = title.lower()

        def _cb(hwnd, _):
            nonlocal found
            length = self.user32.GetWindowTextLengthW(hwnd)
            if length == 0:
                return True
            buf = ctypes.create_unicode_buffer(length + 1)
            self.user32.GetWindowTextW(hwnd, buf, length + 1)
            if target in buf.value.lower():
                found = hwnd
                return False
            return True

        try:
            self.user32.EnumWindows(WNDENUMPROC(_cb), 0)
            if not found:
                return ActionResult(False, f"Janela não encontrada: {title}")
            self.user32.ShowWindow(found, 9)  # SW_RESTORE
            self.user32.SetForegroundWindow(found)
            return ActionResult(True, f"Janela em foco: {title}")
        except Exception as e:
            return ActionResult(False, "Falha ao focar janela.", str(e))

    def get_clipboard(self) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Área de transferência indisponível.")
        CF_UNICODETEXT = 13
        try:
            if not self.user32.OpenClipboard(None):
                return ActionResult(False, "Não consegui abrir a área de transferência.")
            try:
                handle = self.user32.GetClipboardData(CF_UNICODETEXT)
                if not handle:
                    return ActionResult(True, "Área de transferência vazia.", "")
                ptr = self.kernel32.GlobalLock(handle)
                if not ptr:
                    return ActionResult(False, "Não consegui ler a área de transferência.")
                try:
                    text = ctypes.wstring_at(ptr)
                finally:
                    self.kernel32.GlobalUnlock(handle)
                return ActionResult(True, "Conteúdo copiado.", text, {"text": text})
            finally:
                self.user32.CloseClipboard()
        except Exception as e:
            return ActionResult(False, "Falha ao ler a área de transferência.", str(e))

    def set_clipboard(self, text: str) -> ActionResult:
        if not self.user32:
            return ActionResult(False, "Área de transferência indisponível.")
        CF_UNICODETEXT = 13
        GMEM_MOVEABLE = 0x0002
        try:
            data = ctypes.create_unicode_buffer(text)
            size = ctypes.sizeof(data)
            handle = self.kernel32.GlobalAlloc(GMEM_MOVEABLE, size)
            if not handle:
                return ActionResult(False, "Falha ao alocar memória para a área de transferência.")
            ptr = self.kernel32.GlobalLock(handle)
            if not ptr:
                return ActionResult(False, "Falha ao travar memória da área de transferência.")
            ctypes.memmove(ptr, ctypes.addressof(data), size)
            self.kernel32.GlobalUnlock(handle)
            if not self.user32.OpenClipboard(None):
                return ActionResult(False, "Não consegui abrir a área de transferência.")
            try:
                self.user32.EmptyClipboard()
                self.user32.SetClipboardData(CF_UNICODETEXT, handle)
            finally:
                self.user32.CloseClipboard()
            return ActionResult(True, "Texto copiado para a área de transferência.")
        except Exception as e:
            return ActionResult(False, "Falha ao escrever na área de transferência.", str(e))
