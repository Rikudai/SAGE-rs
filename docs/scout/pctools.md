# S.A.G.E. PC Control Tools — Rust Port Specification

Target: Rust native desktop assistant (egui UI, no web UI). Python desktop automation is ported to native Windows APIs (`windows-rs` / Win32).

---

## 1. Tools & Actions Catalog

Every tool callable by the assistant via `ActionOrchestrator` ([sage/actions.py#L219-L376](file:///C:/SAGE/sage/actions.py#L219-L376)), `PCController` ([sage/pc_control.py#L184-L757](file:///C:/SAGE/sage/pc_control.py#L184-L757)), `FileTools` ([sage/files.py#L46-L184](file:///C:/SAGE/sage/files.py#L46-L184)), `ocr` ([sage/ocr.py#L1-L71](file:///C:/SAGE/sage/ocr.py#L1-L71)), and MCP ([tools/desktop_mcp.py#L26-L234](file:///C:/SAGE/tools/desktop_mcp.py#L26-L234)):

| Category / Name | Args | Windows API / Mechanism | Safety Rules | Output Shape | Essential / Extra |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **run_command** | `command: str`, `shell: str = "powershell"` | `CreateProcessW` / `powershell.exe -ExecutionPolicy Bypass -Command` or `cmd.exe /c` ([pc_control.py#L274-L293](file:///C:/SAGE/sage/pc_control.py#L274-L293)) | Regex blocklist (`format`, `diskpart`, `rmdir /s`, `bcdedit`, etc.) ([pc_control.py#L63-L73](file:///C:/SAGE/sage/pc_control.py#L63-L73)); `enable_shell_tools` config key ([pc_control.py#L262-L264](file:///C:/SAGE/sage/pc_control.py#L262-L264)) | `ActionResult(ok, spoken, details, {returncode, stdout, stderr})` ([pc_control.py#L300-L301](file:///C:/SAGE/sage/pc_control.py#L300-L301)) | **Essential (PowerShell)** |
| **shell** (`shell_run`) | `command: str`, `timeout: int = 60` | Persistent `powershell.exe -Command -` process with stdin pipe and timestamp sentinel delimiter ([pc_control.py#L137-L175](file:///C:/SAGE/sage/pc_control.py#L137-L175)) | Same regex blocklist ([pc_control.py#L387-L390](file:///C:/SAGE/sage/pc_control.py#L387-L390)) | `ActionResult(ok, spoken, details, {stdout})` ([pc_control.py#L397-L398](file:///C:/SAGE/sage/pc_control.py#L397-L398)) | **Essential (PowerShell)** |
| **open_app** | `target: str` | Lookup in `APP_COMMANDS` dictionary or `cmd.exe /c start "" <target>` ([actions.py#L38-L64, L84-L90](file:///C:/SAGE/sage/actions.py#L38-L64)) | Validates non-empty name | `ActionResult(ok, spoken, details)` ([actions.py#L86-L92](file:///C:/SAGE/sage/actions.py#L86-L92)) | **Essential (Open Program)** |
| **click_at** | `x: int, y: int, button: str = "left", clicks: int = 1` | `SetCursorPos`, `mouse_event` (`MOUSEEVENTF_LEFTDOWN/UP` etc.) ([pc_control.py#L521-L540](file:///C:/SAGE/sage/pc_control.py#L521-L540)) | Coordinate validation ([actions.py#L335-L336](file:///C:/SAGE/sage/actions.py#L335-L336)); MCP has screen-corner failsafe ([desktop_mcp.py#L20](file:///C:/SAGE/tools/desktop_mcp.py#L20)) | `ActionResult(ok, spoken, details)` ([pc_control.py#L539](file:///C:/SAGE/sage/pc_control.py#L539)) | **Essential (Mouse)** |
| **move_mouse** | `x: int, y: int` | `user32::SetCursorPos(x, y)` ([pc_control.py#L515-L520](file:///C:/SAGE/sage/pc_control.py#L515-L520)) | Validates x, y present | `ActionResult(ok, spoken, details)` ([pc_control.py#L519](file:///C:/SAGE/sage/pc_control.py#L519)) | **Essential (Mouse)** |
| **scroll** | `amount: int` | `user32::mouse_event(MOUSEEVENTF_WHEEL, 0, 0, amount, 0)` ([pc_control.py#L547](file:///C:/SAGE/sage/pc_control.py#L547)) | None | `ActionResult(ok, spoken, details)` ([pc_control.py#L549](file:///C:/SAGE/sage/pc_control.py#L549)) | **Essential (Mouse)** |
| **type_text** | `text: str, interval: float = 0.0` | `user32::SendInput` with `KEYBDINPUT`, `KEYEVENTF_UNICODE` (0x0004), `wScan = ord(ch)` ([pc_control.py#L551-L592](file:///C:/SAGE/sage/pc_control.py#L551-L592)) | Empty text guard | `ActionResult(ok, spoken, details)` ([pc_control.py#L604](file:///C:/SAGE/sage/pc_control.py#L604)) | **Essential (Keyboard)** |
| **press_keys** | `keys: str` (e.g. `ctrl+s`, `win+d`) | `user32::keybd_event` VK down sequence, then reversed VK up sequence ([pc_control.py#L612-L640](file:///C:/SAGE/sage/pc_control.py#L612-L640)) | VK map validation; rejects unknown keys | `ActionResult(ok, spoken, details)` ([pc_control.py#L641](file:///C:/SAGE/sage/pc_control.py#L641)) | **Essential (Keyboard)** |
| **list_windows** | None | `user32::EnumWindows`, `IsWindowVisible`, `GetWindowTextW`, `GetWindowRect` ([pc_control.py#L648-L677](file:///C:/SAGE/sage/pc_control.py#L648-L677)) | Filters empty titles and hidden windows | `ActionResult(ok, spoken, json_str, {windows: [{hwnd, title, x, y, width, height}]})` ([pc_control.py#L674-L675](file:///C:/SAGE/sage/pc_control.py#L674-L675)) | **Essential (Windows)** |
| **focus_window** | `target: str` | `EnumWindows`, `ShowWindow(hwnd, SW_RESTORE=9)`, `SetForegroundWindow(hwnd)` ([pc_control.py#L679-L707](file:///C:/SAGE/sage/pc_control.py#L679-L707)) | Case-insensitive title match | `ActionResult(ok, spoken, details)` ([pc_control.py#L704](file:///C:/SAGE/sage/pc_control.py#L704)) | **Essential (Windows)** |
| **minimize_all** | None | `keybd_event(VK_LWIN)` + `keybd_event('D')` ([actions.py#L206-L218](file:///C:/SAGE/sage/actions.py#L206-L218)) | None | `ActionResult(ok, spoken)` ([actions.py#L215](file:///C:/SAGE/sage/actions.py#L215)) | **Essential (Windows)** |
| **screen_capture** | `output_path: Optional[str]` | `user32::OpenDesktopW("default")` + `SetThreadDesktop` ([capture.py#L7-L9](file:///C:/SAGE/capture.py#L7-L9)); BitBlt / DXGI desktop duplication | Output path directory ensured | `ActionResult(ok, spoken, details, {path, width, height})` ([pc_control.py#L494-L495](file:///C:/SAGE/sage/pc_control.py#L494-L495)) | **Essential (Screenshot)** |
| **file_read** | `path: str, max_bytes: int = 400000` | Standard filesystem I/O with path resolution (`expandvars`, `expanduser`, `abspath`) ([files.py#L27-L34, L47-L65](file:///C:/SAGE/sage/files.py#L27-L34)) | `MAX_READ = 400KB`; logs to `logs/files.log` ([files.py#L21, L36-L43](file:///C:/SAGE/sage/files.py#L21)) | `ActionResult(ok, spoken, content, {path, size, content})` ([files.py#L59-L60](file:///C:/SAGE/sage/files.py#L59-L60)) | **Essential (Files)** |
| **file_write** | `path: str, content: str, backup: bool = true` | Write file with UTF-8 encoding; creates parent directories ([files.py#L66-L83](file:///C:/SAGE/sage/files.py#L66-L83)) | `MAX_WRITE = 4MB`; automatic `.bak` copy ([files.py#L22, L74](file:///C:/SAGE/sage/files.py#L22)); audit log | `ActionResult(ok, spoken, path, {path, bytes})` ([files.py#L80-L81](file:///C:/SAGE/sage/files.py#L80-L81)) | **Essential (Files)** |
| **file_edit** | `path: str, find: str, replace: str, count: int = 1` | Substring replacement on text content ([files.py#L85-L103](file:///C:/SAGE/sage/files.py#L85-L103)) | Automatic `.bak` backup copy ([files.py#L95](file:///C:/SAGE/sage/files.py#L95)); target string must exist | `ActionResult(ok, spoken, path, {path, replacements})` ([files.py#L100-L101](file:///C:/SAGE/sage/files.py#L100-L101)) | **Essential (Files)** |
| **file_list** | `path: str, pattern: str = "*", recursive: bool = false` | Directory iteration / walk ([files.py#L105-L135](file:///C:/SAGE/sage/files.py#L105-L135)) | Skips system dirs (`$Recycle.Bin`, `AppData`, `node_modules`, etc.); max 200 items ([files.py#L23-L24, L106](file:///C:/SAGE/sage/files.py#L23-L24)) | `ActionResult(ok, spoken, body_str, {path, entries: Vec<String>})` ([files.py#L131-L132](file:///C:/SAGE/sage/files.py#L131-L132)) | **Essential (Files)** |
| **file_search** | `path: str, query: str, pattern: str = "*"` | Regex scan across lines ([files.py#L136-L177](file:///C:/SAGE/sage/files.py#L136-L177)) | Skips files > 1.5MB; max 4000 files scanned; max 60 matches ([files.py#L150, L159, L165](file:///C:/SAGE/sage/files.py#L150)) | `ActionResult(ok, spoken, body_str, {matches: Vec<String>})` ([files.py#L173-L174](file:///C:/SAGE/sage/files.py#L173-L174)) | **Essential (Files)** |
| **clipboard** | `text: Optional[str]` | `OpenClipboard`, `Get/SetClipboardData(CF_UNICODETEXT=13)`, `GlobalAlloc(GMEM_MOVEABLE=2)` ([pc_control.py#L708-L757](file:///C:/SAGE/sage/pc_control.py#L708-L757)) | Safe lock/unlock and Open/Close cleanup | Read: `{text: str}`; Write: `ok` ([pc_control.py#L726, L755](file:///C:/SAGE/sage/pc_control.py#L726)) | **Essential (Clipboard)** |
| **write_script** | `name: str, content: str, language: str = "python"` | File write to `<root>/scripts/<name>.<ext>` ([pc_control.py#L310-L329](file:///C:/SAGE/sage/pc_control.py#L310-L329)) | Path restricted to `scripts/`; sanitized filename | `ActionResult(ok, spoken, details, {path})` ([pc_control.py#L326](file:///C:/SAGE/sage/pc_control.py#L326)) | Extra |
| **run_script** | `path: str, args: List[str]` | `subprocess.run` Python/PowerShell/Batch ([pc_control.py#L330-L364](file:///C:/SAGE/sage/pc_control.py#L330-L364)) | Sandboxed to `scripts/` directory | `ActionResult(ok, spoken, details, {path, stdout, stderr, returncode})` ([pc_control.py#L358-L361](file:///C:/SAGE/sage/pc_control.py#L358-L361)) | Extra |
| **run_python** | `code: str, timeout: int = 60` | Tempfile execution via python executable ([pc_control.py#L365-L382](file:///C:/SAGE/sage/pc_control.py#L365-L382)) | Tempfile unlinked after run | `ActionResult(ok, spoken, details, data)` | Extra |
| **point_screen** | `x: int, y: int, label: str` | Desktop companion avatar positioning / pointing ([orchestrator.py#L1008-L1022](file:///C:/SAGE/sage/orchestrator.py#L1008-L1022)) | Clamped to screen dimensions `0..w, 0..h` ([cortex.py#L204-L215](file:///C:/SAGE/sage/cortex.py#L204-L215)) | `ActionResult(ok, spoken, details, {x, y, label})` ([actions.py#L373-L375](file:///C:/SAGE/sage/actions.py#L373-L375)) | Extra |
| **ocr_screen / ocr_image** | `path: Optional[str]` | RapidOCR ONNX runtime or Tesseract CLI ([ocr.py#L16-L71](file:///C:/SAGE/sage/ocr.py#L16-L71)) | Engine fallback logic | `ActionResult(ok, spoken, text, {text, engine})` ([actions.py#L255, L263](file:///C:/SAGE/sage/actions.py#L255)) | Extra |
| **fetch_url** | `url: str, max_chars: int = 14000` | HTTP GET, HTML tag stripping parser ([pc_control.py#L410-L436](file:///C:/SAGE/sage/pc_control.py#L410-L436)) | Requires `http://` or `https://`; 2MB raw cap | `ActionResult(ok, spoken, body, {url, title, chars})` ([pc_control.py#L432](file:///C:/SAGE/sage/pc_control.py#L432)) | Extra |
| **search_web** | `query: str, limit: int = 5` | DuckDuckGo Lite HTTP scraper ([pc_control.py#L437-L483](file:///C:/SAGE/sage/pc_control.py#L437-L483)) | Headless, no browser process spawned | `ActionResult(ok, spoken, body, {results, query})` ([pc_control.py#L479](file:///C:/SAGE/sage/pc_control.py#L479)) | Extra |
| **web_search** / **open_url** | `query/url: str` | Default browser launch (`webbrowser.open`) ([actions.py#L94-L124](file:///C:/SAGE/sage/actions.py#L94-L124)) | None | `ActionResult(ok, spoken, details)` | Extra |
| **system_status** | None | `psutil` CPU, RAM, Disk telemetry ([actions.py#L125-L161](file:///C:/SAGE/sage/actions.py#L125-L161)) | None | `ActionResult(ok, spoken, details, data)` | Extra |
| **media_control** | `command: str` | Windows multimedia virtual key codes (`VK_VOLUME_MUTE=0xAD`, `VK_VOLUME_UP=0xAF`, etc.) ([actions.py#L21-L36, L162-L190](file:///C:/SAGE/sage/actions.py#L21-L36)) | Command whitelist | `ActionResult(ok, spoken)` | Extra |
| **lock_pc** | None | `user32::LockWorkStation()` ([actions.py#L198-L204](file:///C:/SAGE/sage/actions.py#L198-L204)) | None | `ActionResult(ok, spoken)` | Extra |
| **desktop_mcp tools** | `run_powershell(admin=true)`, `window_controls`, `kill_process`, `power` | UAC `Start-Process RunAs`, UI Automation (`uia`), `psutil.kill`, `shutdown.exe` ([desktop_mcp.py#L120-L206](file:///C:/SAGE/tools/desktop_mcp.py#L120-L206)) | PyAutoGUI failsafe; confirmation on power ops | JSON-RPC responses | Extra |

---

## 2. LLM Tool Invocation Protocol

S.A.G.E. does **not** use native LLM function/tool-calling schemas (e.g. OpenAI/Gemini Tools API). Instead, it enforces structured JSON in the system prompt:

1. **Cortex Single-Turn Protocol** ([sage/cortex.py#L73-L94, L422-L460](file:///C:/SAGE/sage/cortex.py#L73-L94)):
```json
{
  "intent": "DIRECT_ANSWER" | "SYSTEM_ACTION" | "DELEGATE_AGENT",
  "ack": "short spoken ack",
  "reply": "spoken answer",
  "action": "run_command" | "click_at" | "file_read" | ... | "none",
  "action_params": {
    "command": "", "path": "", "query": "", "url": "", "text": "",
    "target": "", "content": "", "script": "", "x": 0, "y": 0, "keys": "",
    "skill": "", "args": {}
  },
  "emotion": "idle" | "pensando" | "feliz" | ...,
  "requires_screen": false,
  "pointer": null | {"x": 0, "y": 0, "label": ""},
  "agent_task": ""
}
```

2. **TaskLoop Multi-Turn Protocol** ([sage/task_loop.py#L50-L62, L124-L135](file:///C:/SAGE/sage/task_loop.py#L50-L62)):
```json
{
  "thought": "reasoning on current evidence",
  "status": "continue" | "done" | "ask",
  "action": "look_screen" | "run_command" | "file_read" | ... | "none",
  "action_params": { "command": "", "path": "", ... },
  "reply": "final response when done or question when ask",
  "emotion": "..."
}
```
*Note*: `status: "ask"` is the LLM-level safety confirmation rule for destructive/irreversible actions ([sage/task_loop.py#L79-L80](file:///C:/SAGE/sage/task_loop.py#L79-L80)).

3. **Desktop MCP Protocol** ([tools/desktop_mcp.py#L23-L236](file:///C:/SAGE/tools/desktop_mcp.py#L23-L236)):
JSON-RPC 2.0 over standard I/O (Model Context Protocol).

---

## 3. Skills Engine & Event Hooks

### Skills Engine (`data/skills/`)
* **Storage** ([sage/skills.py#L47-L135](file:///C:/SAGE/sage/skills.py#L47-L135)):
  - Main file: `data/skills/<clean_name>.json`
  - History versions: `data/skills/_historico/<name>.v<version>.json`
  - Lessons learned: `data/skills/_licoes.json`
  - Schema: `Skill { name, title, description, code, params, language, tests, version, enabled, uses, failures, last_error, created, updated }`.
* **Execution & Sandboxing** ([sage/skills.py#L247-L278](file:///C:/SAGE/sage/skills.py#L247-L278); [sage/skill_runner.py#L28-L201](file:///C:/SAGE/sage/skill_runner.py#L28-L201)):
  - Launched in an isolated child process (`python -m sage.skill_runner`) with 60s timeout.
  - JSON payload provided via `stdin`.
  - Injected runtime `sage` API: `powershell()`, `open()`, `clipboard()`, `set_clipboard()`, `device()`, `http()`, `get_text()`, `get_json()`, `ask()` (Gemini call), `store_get()`, `store_set()`.
  - Process outputs marked line: `@@SAGE_SKILL_RESULT@@{"ok": bool, "result": str, "error": str}`.
  - Auto-repair on failure: catches `SkillError` and invokes `SkillForge.improve()` ([sage/skills.py#L315-L334](file:///C:/SAGE/sage/skills.py#L315-L334)).

### Event Hooks (`data/hooks.json`)
* Stored in `data/hooks.json` ([sage/hooks.py#L26-L71](file:///C:/SAGE/hooks.py#L26-L71)).
* Supported events: `startup`, `prompt`, `reply`, `job_start`, `job_done`, `job_error`, `clone_spawn`, `clone_done`, `clone_fail`, `form_change` ([sage/hooks.py#L28-L39](file:///C:/SAGE/sage/hooks.py#L28-L39)).
* Hook actions: `command` (child process with env `SAGE_EVENT`, `SAGE_TEXT`, `SAGE_JSON`), `webhook` (HTTP POST), `speak` (TTS), `sound` (audio cue) ([sage/hooks.py#L8-L12, L92-L110](file:///C:/SAGE/sage/hooks.py#L8-L12)).

---

## 4. Uncertainties & Porting Risks

1. **Python Skill Runner vs Native Rust Skills**: In Python, skills are arbitrary Python scripts executed via `exec` in a sub-process. In Rust, running dynamic Python requires bundling a Python runtime or migrating dynamic skills to a lightweight embedded engine (e.g. Rhai, Lua, or WASM).
2. **Persistent PowerShell Session**: `PowerShellSession` in `sage/pc_control.py` uses stdin/stdout sentinel matching (`__SAGE_DONE_<timestamp>__`). In Rust, this requires handling asynchronous streams or using Windows ConPTY to avoid deadlocks and buffer overflows.
3. **Screen Capture Performance**: `ImageGrab.grab(all_screens=True)` or Win32 GDI `BitBlt` capture is high-latency across multi-monitor setups. Rust should use the Desktop Duplication API (DXGI) or Windows Graphics Capture API for smooth frame times.
4. **Desktop MCP Compatibility**: `desktop_mcp.py` uses `pywinauto` (UIA) to enumerate controls. In Rust, native UI Automation COM interfaces (`IUIAutomation`) will be needed to replicate `window_controls`.

---

## Handoff

- **Outcome**: Completed architectural reconnaissance and tool specification for PC Control Tools, mapping all tools, safety confirmations, LLM JSON protocols, and skill execution mechanisms.
- **Evidence**: Verified against `sage/pc_control.py`, `sage/actions.py`, `sage/files.py`, `sage/ocr.py`, `capture.py`, `tools/desktop_mcp.py`, `sage/skills.py`, `sage/skill_runner.py`, `sage/hooks.py`, `sage/cortex.py`, `sage/task_loop.py`, and `sage/orchestrator.py`.
- **Files changed**: None (strictly read-only scout).
- **Checks run**: Verified file paths, line references, and syntax structures via direct inspection.
- **Failures**: None.
- **Risks**: Embedding or replacing the Python skill runner in Rust requires an architectural decision on scripting runtime (WASM/Rhai vs external python interpreter).

STATUS: DONE
SUMMARY: Completed full specification of S.A.G.E. PC control tools, Windows Win32 APIs, safety gates, LLM JSON protocols, and skill runner architecture for the Rust port.
