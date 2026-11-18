"""
Antigravity CLI Client wrapper.
Executes prompts through agy.exe in headless print mode with session persistence
and real-time line-by-line output streaming.
"""
import queue
import subprocess
import sys
import threading
import time
from typing import Optional, Callable

from sage.text_processing import clean_cli_response


class AgyClient:
    def __init__(self, executable_path: str, timeout_seconds: int = 300):
        self.executable_path = executable_path
        self.timeout_seconds = timeout_seconds
        self._has_started_session = False
        self._force_new_session = False

    def reset_session(self) -> None:
        """Resets CLI conversation state so next turn starts with clean context."""
        self._has_started_session = False
        self._force_new_session = True

    def run_prompt(
        self,
        prompt: str,
        conversation_id: Optional[str] = None,
        on_stream_line: Optional[Callable[[str], None]] = None
    ) -> str:
        """
        Executes a prompt against agy.exe with session persistence and real-time streaming.
        Calls on_stream_line(line) for every line output by the CLI as it arrives.
        Applies clean_cli_response to filter internal diagnostic noise for the final return value.
        """
        if not prompt or not prompt.strip():
            return "Nenhum comando fornecido."

        cmd = [self.executable_path, "--dangerously-skip-permissions"]

        if self._force_new_session:
            cmd.append("--new-project")
            self._force_new_session = False
        elif self._has_started_session:
            cmd.append("--continue")

        cmd.extend(["-p", prompt.strip()])

        creationflags = 0
        if sys.platform == "win32":
            creationflags = subprocess.CREATE_NO_WINDOW

        try:
            process = subprocess.Popen(
                cmd,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                bufsize=1,
                creationflags=creationflags
            )
        except FileNotFoundError:
            return f"Executável do Antigravity CLI não encontrado no caminho: {self.executable_path}"
        except Exception as e:
            return f"Erro inesperado ao consultar Antigravity: {str(e)}"

        # Queue-based reader thread to safely stream output without blocking on readline
        output_queue: queue.Queue[str] = queue.Queue()

        def _reader():
            try:
                for line in iter(process.stdout.readline, ""):
                    output_queue.put(line)
            except Exception:
                pass
            finally:
                try:
                    process.stdout.close()
                except Exception:
                    pass

        reader_thread = threading.Thread(target=_reader, daemon=True)
        reader_thread.start()

        lines = []
        deadline = time.time() + self.timeout_seconds
        timed_out = False

        while True:
            poll_code = process.poll()
            try:
                remaining = deadline - time.time()
                if remaining <= 0:
                    timed_out = True
                    break
                timeout_chunk = min(0.1, remaining)
                line = output_queue.get(timeout=timeout_chunk)
                lines.append(line)
                if on_stream_line:
                    try:
                        on_stream_line(line)
                    except Exception:
                        pass
            except queue.Empty:
                if poll_code is not None and output_queue.empty():
                    break
                if time.time() > deadline:
                    timed_out = True
                    break

        if timed_out:
            try:
                process.terminate()
                process.wait(timeout=2)
            except Exception:
                try:
                    process.kill()
                except Exception:
                    pass
            return "Tempo limite de execução atingido ao consultar o Antigravity."

        self._has_started_session = True
        full_output = "".join(lines).strip()

        if process.returncode == 0:
            cleaned = clean_cli_response(full_output)
            return cleaned if cleaned else "Comando executado com sucesso."
        else:
            cleaned_err = clean_cli_response(full_output)
            if cleaned_err and cleaned_err != "Ação concluída com sucesso.":
                return f"Erro ao executar Antigravity CLI: {cleaned_err}"
            return "Comando executado."
