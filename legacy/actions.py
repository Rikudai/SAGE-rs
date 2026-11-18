"""
Action Orchestrator for S.A.G.E.:
Executes local PC actions in milliseconds without requiring cloud or heavy agent calls.
"""
import ctypes
import os
import subprocess
import urllib.parse
import webbrowser
from typing import Dict, Any, Optional

import psutil

from sage.result import ActionResult
from sage.pc_control import PCController
from sage.files import FileTools
from sage import ocr as ocr_module


# Virtual key codes for Windows multimedia keys
VK_VOLUME_MUTE = 0xAD
VK_VOLUME_DOWN = 0xAE
VK_VOLUME_UP = 0xAF
VK_MEDIA_NEXT_TRACK = 0xB0
VK_MEDIA_PREV_TRACK = 0xB1
VK_MEDIA_PLAY_PAUSE = 0xB3
VK_LWIN = 0x5B
KEYEVENTF_KEYUP = 0x0002


def _send_key(vk_code: int):
    """Sends a key press and release event to Windows."""
    user32 = ctypes.windll.user32
    user32.keybd_event(vk_code, 0, 0, 0)
    user32.keybd_event(vk_code, 0, KEYEVENTF_KEYUP, 0)


APP_COMMANDS = {
    "chrome": ["cmd.exe", "/c", "start", "chrome"],
    "google chrome": ["cmd.exe", "/c", "start", "chrome"],
    "navegador": ["cmd.exe", "/c", "start", "chrome"],
    "edge": ["cmd.exe", "/c", "start", "msedge"],
    "msedge": ["cmd.exe", "/c", "start", "msedge"],
    "brave": ["cmd.exe", "/c", "start", "brave"],
    "code": ["cmd.exe", "/c", "start", "code"],
    "vscode": ["cmd.exe", "/c", "start", "code"],
    "vs code": ["cmd.exe", "/c", "start", "code"],
    "visual studio code": ["cmd.exe", "/c", "start", "code"],
    "notepad": ["notepad.exe"],
    "bloco de notas": ["notepad.exe"],
    "calc": ["calc.exe"],
    "calculadora": ["calc.exe"],
    "explorer": ["explorer.exe"],
    "arquivos": ["explorer.exe"],
    "meus arquivos": ["explorer.exe"],
    "taskmgr": ["taskmgr.exe"],
    "gerenciador de tarefas": ["taskmgr.exe"],
    "terminal": ["cmd.exe", "/c", "start", "powershell"],
    "powershell": ["cmd.exe", "/c", "start", "powershell"],
    "prompt": ["cmd.exe", "/c", "start", "cmd"],
    "discord": ["cmd.exe", "/c", "start", "discord:"],
    "steam": ["cmd.exe", "/c", "start", "steam:"],
    "spotify": ["cmd.exe", "/c", "start", "spotify:"]
}


class ActionOrchestrator:
    def __init__(self, screen_path: str = "C:/SAGE/screen.png",
                 enable_shell: bool = True, shell_timeout: int = 120):
        self.screen_path = screen_path
        self.pc = PCController(screen_path=screen_path, allow_shell=enable_shell,
                               default_timeout=shell_timeout)
        self.files = FileTools()

    def open_app(self, app_name: str) -> ActionResult:
        """Opens common system applications by name."""
        if not app_name:
            return ActionResult(False, "Nenhum aplicativo especificado.", "App name is empty")
        
        target = app_name.lower().strip()
        cmd = APP_COMMANDS.get(target)

        try:
            if cmd:
                subprocess.Popen(cmd, shell=False)
                return ActionResult(True, f"Abrindo {app_name}.", f"Executed: {cmd}")
            else:
                # Try generic launch
                subprocess.Popen(["cmd.exe", "/c", "start", "", target], shell=False)
                return ActionResult(True, f"Iniciando {app_name}.", f"Executed start for {target}")
        except Exception as e:
            return ActionResult(False, f"Não consegui abrir {app_name}.", str(e))

    def web_search(self, query: str, engine: str = "google") -> ActionResult:
        """Searches the web via default browser."""
        if not query:
            return ActionResult(False, "Nenhum termo de pesquisa fornecido.")
        
        encoded = urllib.parse.quote(query)
        if "youtube" in engine.lower() or "video" in engine.lower():
            url = f"https://www.youtube.com/results?search_query={encoded}"
            speech = f"Pesquisando {query} no YouTube."
        else:
            url = f"https://www.google.com/search?q={encoded}"
            speech = f"Pesquisando {query} no Google."

        try:
            webbrowser.open(url)
            return ActionResult(True, speech, f"Opened URL: {url}")
        except Exception as e:
            return ActionResult(False, "Erro ao abrir navegador.", str(e))

    def open_url(self, url: str) -> ActionResult:
        """Opens a direct URL in browser."""
        if not url:
            return ActionResult(False, "URL não informada.")
        if not url.startswith("http://") and not url.startswith("https://"):
            url = "https://" + url
        try:
            webbrowser.open(url)
            return ActionResult(True, "Abrindo link no navegador.", f"URL: {url}")
        except Exception as e:
            return ActionResult(False, "Erro ao abrir link.", str(e))

    def get_system_status(self) -> ActionResult:
        """Gathers real-time hardware telemetry: CPU, RAM, GPU, Disk."""
        try:
            cpu_pct = psutil.cpu_percent(interval=0.1)
            mem = psutil.virtual_memory()
            disk = psutil.disk_usage("C:\\")

            ram_used_gb = mem.used / (1024 ** 3)
            ram_free_gb = mem.available / (1024 ** 3)
            ram_total_gb = mem.total / (1024 ** 3)
            disk_free_gb = disk.free / (1024 ** 3)

            spoken = (
                f"Sua CPU está em {cpu_pct:.0f}%, e você tem "
                f"{ram_free_gb:.1f} GB de memória RAM livre de {ram_total_gb:.0f} GB. "
                f"A GPU Vega 7 com 4 GB de VRAM dedicada está ativa."
            )

            details = (
                f"📊 STATUS DO SISTEMA:\n"
                f"• CPU (Ryzen 5 5600GT): {cpu_pct:.1f}% de uso\n"
                f"• RAM: {mem.percent}% em uso ({ram_used_gb:.1f} GB / {ram_total_gb:.1f} GB) | Livre: {ram_free_gb:.1f} GB\n"
                f"• GPU: AMD Radeon Graphics (Vega 7) | VRAM Dedicada: 4.0 GB\n"
                f"• Disco C: {disk.percent}% em uso ({disk_free_gb:.1f} GB livres)"
            )

            data = {
                "cpu_percent": cpu_pct,
                "ram_percent": mem.percent,
                "ram_free_gb": ram_free_gb,
                "ram_total_gb": ram_total_gb,
                "disk_free_gb": disk_free_gb
            }
            return ActionResult(True, spoken, details, data)
        except Exception as e:
            return ActionResult(False, "Não consegui obter o status do sistema.", str(e))

    def media_control(self, command: str) -> ActionResult:
        """Controls system volume and media playback."""
        cmd = command.lower().strip()
        try:
            if cmd in ("mute", "mudo", "mutar", "desmutar"):
                _send_key(VK_VOLUME_MUTE)
                return ActionResult(True, "Áudio mutado ou desmutado.")
            elif cmd in ("volume_up", "aumentar_volume", "aumentar volume", "mais som", "sobe volume"):
                for _ in range(5):
                    _send_key(VK_VOLUME_UP)
                return ActionResult(True, "Volume aumentado.")
            elif cmd in ("volume_down", "diminuir_volume", "diminuir volume", "menos som", "baixa volume"):
                for _ in range(5):
                    _send_key(VK_VOLUME_DOWN)
                return ActionResult(True, "Volume diminuído.")
            elif cmd in ("play", "pause", "play_pause", "pausar", "despausar", "tocar"):
                _send_key(VK_MEDIA_PLAY_PAUSE)
                return ActionResult(True, "Reprodução alternada.")
            elif cmd in ("next", "proxima", "próxima", "passar"):
                _send_key(VK_MEDIA_NEXT_TRACK)
                return ActionResult(True, "Próxima faixa.")
            elif cmd in ("prev", "anterior", "voltar"):
                _send_key(VK_MEDIA_PREV_TRACK)
                return ActionResult(True, "Faixa anterior.")
            else:
                return ActionResult(False, f"Comando de mídia não reconhecido: {command}")
        except Exception as e:
            return ActionResult(False, "Erro no controle de mídia.", str(e))

    def capture_screen(self, output_path: Optional[str] = None) -> ActionResult:
        """Captures the full screen and saves it to an image file."""
        result = self.pc.screenshot(output_path or self.screen_path)
        if result.success:
            result.spoken_summary = "Tela capturada com sucesso."
        return result

    def lock_pc(self) -> ActionResult:
        """Locks the Windows workstation immediately."""
        try:
            ctypes.windll.user32.LockWorkStation()
            return ActionResult(True, "Computador bloqueado.")
        except Exception as e:
            return ActionResult(False, "Falha ao bloquear o computador.", str(e))

    def minimize_all(self) -> ActionResult:
        """Minimizes all windows to show desktop."""
        try:
            user32 = ctypes.windll.user32
            # Windows + D
            user32.keybd_event(VK_LWIN, 0, 0, 0)
            user32.keybd_event(ord('D'), 0, 0, 0)
            user32.keybd_event(ord('D'), 0, KEYEVENTF_KEYUP, 0)
            user32.keybd_event(VK_LWIN, 0, KEYEVENTF_KEYUP, 0)
            return ActionResult(True, "Janelas minimizadas.")
        except Exception as e:
            return ActionResult(False, "Falha ao minimizar janelas.", str(e))

    def execute_action(self, action_name: str, params: Optional[Dict[str, Any]] = None) -> ActionResult:
        """Main dispatcher for action orchestrator."""
        params = params or {}
        act = action_name.lower().strip() if action_name else ""

        if act in ("open_app", "abrir_app", "abrir_aplicativo", "abrir"):
            target = params.get("target") or params.get("app") or params.get("name") or ""
            return self.open_app(str(target))

        elif act in ("web_search", "pesquisa_web", "pesquisar", "busca"):
            query = params.get("query") or params.get("termo") or params.get("text") or ""
            engine = params.get("engine") or params.get("target") or "google"
            return self.web_search(str(query), engine=str(engine))

        elif act in ("open_url", "abrir_link", "link"):
            url = params.get("url") or params.get("link") or ""
            return self.open_url(str(url))

        elif act in ("system_status", "status_sistema", "status", "hardware", "cpu_ram"):
            return self.get_system_status()

        elif act in ("media_control", "controle_midia", "volume", "musica"):
            cmd = params.get("command") or params.get("cmd") or params.get("action") or "play_pause"
            return self.media_control(str(cmd))

        elif act in ("screen_capture", "screen_vision", "capturar_tela", "print"):
            return self.capture_screen()

        elif act in ("ocr_screen", "ocr_tela", "ler_tela"):
            shot = self.capture_screen()
            if not shot.success:
                return shot
            text = ocr_module.ocr_image(self.screen_path)
            if text is None:
                return ActionResult(False, "OCR indisponível no momento.",
                                    "Instale rapidocr_onnxruntime ou use a visão local.")
            return ActionResult(True, "Texto extraído da tela.", text or "sem texto",
                                {"text": text, "engine": ocr_module.engine_name()})

        elif act in ("ocr_image", "ocr_imagem", "ocr_arquivo"):
            path = params.get("path") or params.get("target") or params.get("image") or ""
            text = ocr_module.ocr_image(str(path))
            if text is None:
                return ActionResult(False, "OCR indisponível ou imagem inválida.", str(path))
            return ActionResult(True, "Texto extraído.", text or "sem texto",
                                {"text": text, "engine": ocr_module.engine_name()})

        elif act in ("file_read", "ler_arquivo", "read_file"):
            path = params.get("path") or params.get("target") or params.get("file") or ""
            return self.files.read(str(path))

        elif act in ("file_write", "escrever_arquivo", "write_file"):
            path = params.get("path") or params.get("target") or params.get("file") or ""
            content = params.get("content") or params.get("text") or ""
            return self.files.write(str(path), str(content))

        elif act in ("file_edit", "editar_arquivo", "edit_file"):
            path = params.get("path") or params.get("target") or ""
            find = params.get("find") or params.get("old") or ""
            replace = params.get("replace") or params.get("new") or ""
            return self.files.edit(str(path), str(find), str(replace))

        elif act in ("file_list", "listar_arquivos", "list_files"):
            path = params.get("path") or params.get("target") or "."
            return self.files.list_dir(str(path), str(params.get("pattern") or "*"),
                                       bool(params.get("recursive", False)))

        elif act in ("file_search", "buscar_arquivos", "search_files"):
            path = params.get("path") or params.get("target") or "."
            query = params.get("query") or params.get("text") or ""
            return self.files.search(str(path), str(query), str(params.get("pattern") or "*"))

        elif act in ("fetch_url", "ler_url", "http_get"):
            url = params.get("url") or params.get("target") or ""
            return self.pc.fetch_url(str(url))

        elif act in ("search_web", "pesquisa_texto", "web_research"):
            query = params.get("query") or params.get("text") or ""
            return self.pc.web_search_text(str(query))

        elif act in ("shell", "terminal", "shell_run"):
            command = params.get("command") or params.get("cmd") or params.get("text") or ""
            return self.pc.shell_run(str(command))

        elif act in ("lock_pc", "bloquear_tela", "bloquear"):
            return self.lock_pc()

        elif act in ("minimize_all", "minimizar_tudo", "area_de_trabalho"):
            return self.minimize_all()

        # ---- full PC control (shell, scripts, mouse, keyboard, windows) ----
        elif act in ("run_command", "executar_comando", "shell", "executar", "cmd"):
            command = params.get("command") or params.get("cmd") or params.get("text") or ""
            shell = params.get("shell") or "powershell"
            return self.pc.run_command(str(command), shell=str(shell))

        elif act in ("write_script", "criar_script", "salvar_script"):
            name = params.get("script") or params.get("name") or params.get("filename") or "auxiliar"
            content = params.get("text") or params.get("content") or params.get("code") or ""
            language = params.get("language") or "python"
            return self.pc.write_script(str(name), str(content), str(language))

        elif act in ("run_script", "rodar_script", "executar_script"):
            script = params.get("script") or params.get("path") or params.get("name") or ""
            args = params.get("args") or []
            return self.pc.run_script(str(script), list(args) if isinstance(args, list) else [str(args)])

        elif act in ("run_python", "executar_python"):
            code = params.get("text") or params.get("code") or ""
            return self.pc.run_python(str(code))

        elif act in ("click_at", "click", "clicar", "clique"):
            x = params.get("x")
            y = params.get("y")
            button = params.get("button") or "left"
            clicks = int(params.get("clicks") or 1)
            if x is None or y is None:
                return ActionResult(False, "Preciso das coordenadas x e y para clicar.")
            return self.pc.click(int(x), int(y), str(button), clicks)

        elif act in ("type_text", "digitar", "escrever"):
            text = params.get("text") or params.get("content") or ""
            return self.pc.type_text(str(text))

        elif act in ("press_keys", "atalho", "hotkey"):
            keys = params.get("keys") or params.get("key") or params.get("combo") or ""
            return self.pc.press_keys(str(keys))

        elif act in ("move_mouse", "mover_mouse"):
            if params.get("x") is None or params.get("y") is None:
                return ActionResult(False, "Preciso das coordenadas x e y.")
            return self.pc.move_mouse(int(params["x"]), int(params["y"]))

        elif act in ("scroll", "rolar"):
            return self.pc.scroll(int(params.get("amount") or -3))

        elif act in ("list_windows", "janelas", "listar_janelas"):
            return self.pc.list_windows()

        elif act in ("focus_window", "focar_janela"):
            title = params.get("target") or params.get("title") or ""
            return self.pc.focus_window(str(title))

        elif act in ("clipboard", "copiar", "area_transferencia"):
            text = params.get("text")
            if text is None:
                return self.pc.get_clipboard()
            return self.pc.set_clipboard(str(text))

        elif act in ("point_screen", "apontar", "pointer"):
            x = params.get("x")
            y = params.get("y")
            if x is None or y is None:
                return ActionResult(False, "Preciso das coordenadas para apontar.")
            return ActionResult(True, "Apontando na tela.",
                                f"Alvo em ({x}, {y})", {"x": int(x), "y": int(y),
                                                        "label": params.get("label") or ""})

        else:
            return ActionResult(False, "Ação não suportada.", f"Unknown action: {action_name}")
