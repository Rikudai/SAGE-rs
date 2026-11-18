"""S.A.G.E. Desktop MCP - keyboard, mouse and window control (PyAutoGUI + pywinauto).

Replaces AutoHotkey / Shogun "modo escrito". Runs over stdio:
    claude mcp add -s user desktop -- python C:\\SAGE\\tools\\desktop_mcp.py
Safety: PyAutoGUI FAILSAFE is on (move the mouse to a screen corner to abort).
"""
from __future__ import annotations

import os
import subprocess
import time
from typing import List, Optional

import pyautogui
try:  # mcp 2.x
    from mcp.server.mcpserver import MCPServer as FastMCP
except ImportError:  # mcp 1.x
    from mcp.server.fastmcp import FastMCP

pyautogui.FAILSAFE = True
pyautogui.PAUSE = 0.05

mcp = FastMCP("desktop")


@mcp.tool()
def screen_info() -> dict:
    """Screen size and current mouse position."""
    w, h = pyautogui.size()
    x, y = pyautogui.position()
    return {"width": w, "height": h, "mouse_x": x, "mouse_y": y}


@mcp.tool()
def type_text(text: str, interval: float = 0.01, use_clipboard: bool = True) -> str:
    """Type text into the focused window. Clipboard paste handles accents/unicode."""
    if use_clipboard:
        subprocess.run(["powershell", "-NoProfile", "-Command", "Set-Clipboard -Value $input"],
                       input=text, text=True, encoding="utf-8", check=False)
        pyautogui.hotkey("ctrl", "v")
    else:
        pyautogui.write(text, interval=interval)
    return f"typed {len(text)} chars"


@mcp.tool()
def press_keys(keys: List[str], hotkey: bool = True) -> str:
    """Press keys. hotkey=True presses them together (e.g. ['ctrl','s']); else in sequence."""
    if hotkey:
        pyautogui.hotkey(*keys)
    else:
        for k in keys:
            pyautogui.press(k)
    return "ok: " + "+".join(keys)


@mcp.tool()
def mouse_click(x: Optional[int] = None, y: Optional[int] = None, button: str = "left",
                clicks: int = 1) -> str:
    """Click at (x, y) or at the current position."""
    pyautogui.click(x=x, y=y, button=button, clicks=clicks)
    return f"clicked {button} x{clicks} at {pyautogui.position()}"


@mcp.tool()
def mouse_move(x: int, y: int, duration: float = 0.2) -> str:
    """Move the mouse to (x, y)."""
    pyautogui.moveTo(x, y, duration=duration)
    return "ok"


@mcp.tool()
def scroll(amount: int) -> str:
    """Scroll the mouse wheel (positive = up)."""
    pyautogui.scroll(amount)
    return "ok"


@mcp.tool()
def screenshot(path: str = "") -> str:
    """Save a screenshot as PNG and return its path."""
    path = path or os.path.join(os.environ.get("TEMP", "."), f"sage_shot_{int(time.time())}.png")
    pyautogui.screenshot(path)
    return path


@mcp.tool()
def list_windows() -> List[dict]:
    """List visible top-level windows (title, handle, process id)."""
    from pywinauto import Desktop
    out = []
    for w in Desktop(backend="uia").windows():
        try:
            title = w.window_text()
            if title and w.is_visible():
                out.append({"title": title, "handle": w.handle, "pid": w.process_id()})
        except Exception:
            pass
    return out


@mcp.tool()
def focus_window(title_contains: str) -> str:
    """Bring to front the first window whose title contains the text (case-insensitive)."""
    from pywinauto import Desktop
    needle = title_contains.lower()
    for w in Desktop(backend="uia").windows():
        try:
            if needle in w.window_text().lower():
                if w.is_minimized():
                    w.restore()
                w.set_focus()
                return "focused: " + w.window_text()
        except Exception:
            continue
    return "not found: " + title_contains


@mcp.tool()
def window_controls(title_contains: str, max_items: int = 80) -> List[dict]:
    """List clickable/editable controls of a window (name, type, rectangle) via UI Automation."""
    from pywinauto import Desktop
    needle = title_contains.lower()
    for w in Desktop(backend="uia").windows():
        if needle in w.window_text().lower():
            items = []
            for c in w.descendants():
                try:
                    r = c.rectangle()
                    items.append({"name": c.window_text(), "type": c.element_info.control_type,
                                  "center": [r.mid_point().x, r.mid_point().y]})
                except Exception:
                    continue
                if len(items) >= max_items:
                    break
            return items
    return []


@mcp.tool()
def open_program(target: str) -> str:
    """Open a program, file, folder or URL with the Windows shell (like Win+R)."""
    os.startfile(target)  # noqa: S606 - intentional local launcher
    return "opened: " + target


@mcp.tool()
def run_powershell(command: str, admin: bool = False, timeout: int = 120) -> dict:
    """Run a PowerShell command and return stdout/stderr. admin=True runs elevated (UAC)."""
    if admin:
        out = os.path.join(os.environ.get("TEMP", "."), f"sage_admin_{int(time.time()*1000)}.txt")
        inner = f"& {{ {command} }} *> '{out}'"
        launcher = ("Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList "
                    "'-NoProfile','-ExecutionPolicy','Bypass','-Command'," +
                    "'" + inner.replace("'", "''") + "'")
        p = subprocess.run(["powershell", "-NoProfile", "-Command", launcher],
                           capture_output=True, text=True, timeout=timeout)
        text = open(out, encoding="utf-16", errors="replace").read() if os.path.exists(out) else ""
        return {"code": p.returncode, "stdout": text[-20000:], "stderr": p.stderr[-4000:]}
    p = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command],
                       capture_output=True, text=True, encoding="utf-8", errors="replace",
                       timeout=timeout)
    return {"code": p.returncode, "stdout": p.stdout[-20000:], "stderr": p.stderr[-4000:]}


@mcp.tool()
def list_processes(name_contains: str = "", limit: int = 60) -> List[dict]:
    """List running processes (pid, name, memory MB), sorted by memory."""
    import psutil
    rows = []
    for p in psutil.process_iter(["pid", "name", "memory_info"]):
        n = p.info["name"] or ""
        if name_contains.lower() in n.lower():
            mem = p.info["memory_info"].rss / 1e6 if p.info["memory_info"] else 0
            rows.append({"pid": p.info["pid"], "name": n, "mem_mb": round(mem, 1)})
    return sorted(rows, key=lambda r: -r["mem_mb"])[:limit]


@mcp.tool()
def kill_process(pid: int = 0, name: str = "") -> str:
    """Terminate a process by pid or by exact name (all matches)."""
    import psutil
    killed = []
    for p in psutil.process_iter(["pid", "name"]):
        if (pid and p.info["pid"] == pid) or (name and (p.info["name"] or "").lower() == name.lower()):
            try:
                p.kill()
                killed.append(f"{p.info['name']}({p.info['pid']})")
            except Exception as e:
                killed.append(f"fail {p.info['pid']}: {e}")
    return ", ".join(killed) or "nothing matched"


@mcp.tool()
def power(action: str, delay_seconds: int = 0) -> str:
    """Power control: lock | sleep | shutdown | restart | logoff | cancel."""
    cmds = {"lock": "rundll32.exe user32.dll,LockWorkStation",
            "sleep": "rundll32.exe powrprof.dll,SetSuspendState 0,1,0",
            "shutdown": f"shutdown /s /t {delay_seconds}",
            "restart": f"shutdown /r /t {delay_seconds}",
            "logoff": "shutdown /l", "cancel": "shutdown /a"}
    if action not in cmds:
        return "unknown action"
    subprocess.Popen(cmds[action], shell=True)
    return "ok: " + action


@mcp.tool()
def set_volume(percent: int) -> str:
    """Set master volume 0-100 (uses media keys)."""
    for _ in range(50):
        pyautogui.press("volumedown")
    for _ in range(max(0, min(100, percent)) // 2):
        pyautogui.press("volumeup")
    return f"volume ~{percent}%"


@mcp.tool()
def look_at_screen():
    """Return a screenshot image so the model can SEE the screen and decide where to click."""
    try:
        from mcp.server.mcpserver.utilities.types import Image  # mcp 2.x
    except ImportError:
        from mcp.server.fastmcp.utilities.types import Image  # type: ignore  # mcp 1.x
    import io
    buf = io.BytesIO()
    img = pyautogui.screenshot()
    img.thumbnail((1600, 1600))
    img.save(buf, format="PNG")
    w, h = pyautogui.size()
    return [f"screen {w}x{h}; image scaled to {img.size[0]}x{img.size[1]} (rescale coords)",
            Image(data=buf.getvalue(), format="png")]


if __name__ == "__main__":
    mcp.run()
