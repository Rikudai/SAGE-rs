"""
Pausa a mídia enquanto o S.A.G.E. fala (ou ouve)
================================================
Antes de falar, ele pausa a música ou o vídeo que estiver tocando no PC; quando termina, retoma
**só o que ele mesmo pausou**. Se nada estava tocando, não mexe em nada (nunca dá "play" sozinho).

Fala e escuta pedem a mídia com :meth:`MediaFocus.hold` e devolvem com :meth:`MediaFocus.release`.
Enquanto alguém segura, a mídia fica pausada; quando o último solta, ela volta depois de uma
pequena folga (``grace``), para não piscar entre "ouvi você" → "pensando" → "respondendo".

No Windows a mídia é controlada pelos controles de mídia do sistema (os mesmos da tecla de
volume / tela de bloqueio): Spotify, YouTube no navegador, VLC, Netflix, players do Windows...
Primeiro pela biblioteca ``winrt`` (se instalada) e, sem ela, pelo PowerShell do Windows.
"""
from __future__ import annotations

import json
import queue
import subprocess
import sys
import threading
from typing import Callable, List, Optional, Set

_NO_WINDOW = 0x08000000 if sys.platform == "win32" else 0


class MediaBackend:
    """Pausa o que está tocando e devolve um identificador por sessão pausada."""

    def pause_playing(self) -> List[str]:
        return []

    def resume(self, sessions: List[str]) -> None:
        pass


# --------------------------------------------------------------------------
# Windows: controles de mídia do sistema (GlobalSystemMediaTransportControlsSessionManager)
# --------------------------------------------------------------------------
class WinRtBackend(MediaBackend):
    """Pela biblioteca ``winrt`` (pip install winrt-Windows.Media.Control winrt-Windows.Foundation)."""

    def __init__(self):
        from winrt.windows.media.control import (                                   # noqa: F401
            GlobalSystemMediaTransportControlsSessionManager as Manager,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status)
        self._manager, self._status = Manager, Status

    def _run(self, coro):
        import asyncio
        return asyncio.run(coro)

    def pause_playing(self) -> List[str]:
        async def go():
            manager = await self._manager.request_async()
            paused = []
            for session in manager.get_sessions():
                info = session.get_playback_info()
                if info is not None and info.playback_status == self._status.PLAYING:
                    if await session.try_pause_async():
                        paused.append(session.source_app_user_model_id)
            return paused
        return self._run(go())

    def resume(self, sessions: List[str]) -> None:
        wanted = set(sessions)

        async def go():
            manager = await self._manager.request_async()
            for session in manager.get_sessions():
                if session.source_app_user_model_id in wanted:
                    await session.try_play_async()
        self._run(go())


_PS_PRELUDE = r"""
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await($op, [Type]$type) {
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    $null = $task.Wait(5000)
    $task.Result
}
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$mgr = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) `
             ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
"""

_PS_PAUSE = _PS_PRELUDE + r"""
$paused = @()
foreach ($s in $mgr.GetSessions()) {
    $info = $s.GetPlaybackInfo()
    if ($info -and $info.PlaybackStatus -eq 'Playing') {
        if (Await ($s.TryPauseAsync()) ([bool])) { $paused += $s.SourceAppUserModelId }
    }
}
ConvertTo-Json -Compress -InputObject @($paused)
"""

_PS_RESUME = _PS_PRELUDE + r"""
$wanted = ConvertFrom-Json $env:SAGE_MEDIA_SESSIONS
foreach ($s in $mgr.GetSessions()) {
    if ($wanted -contains $s.SourceAppUserModelId) { $null = Await ($s.TryPlayAsync()) ([bool]) }
}
"""


class PowerShellBackend(MediaBackend):
    """Pelo Windows PowerShell 5.1 (o ``powershell.exe`` que vem no Windows; o pwsh 7 não carrega WinRT)."""

    def __init__(self, timeout: float = 15.0):
        self.timeout = timeout

    def _ps(self, script: str, env_sessions: Optional[List[str]] = None) -> str:
        import os
        env = dict(os.environ)
        if env_sessions is not None:
            env["SAGE_MEDIA_SESSIONS"] = json.dumps(env_sessions)
        proc = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                               "-Command", script], capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=self.timeout, env=env, creationflags=_NO_WINDOW)
        if proc.returncode != 0:
            raise RuntimeError((proc.stderr or proc.stdout or "PowerShell falhou").strip()[:300])
        return proc.stdout.strip()

    def pause_playing(self) -> List[str]:
        out = self._ps(_PS_PAUSE)
        line = (out.splitlines() or ["[]"])[-1]
        data = json.loads(line) if line else []
        if isinstance(data, str):
            data = [data]
        return [str(x) for x in data or []]

    def resume(self, sessions: List[str]) -> None:
        if sessions:
            self._ps(_PS_RESUME, sessions)


def default_backend() -> MediaBackend:
    if sys.platform != "win32":
        return MediaBackend()
    try:
        return WinRtBackend()
    except Exception:
        return PowerShellBackend()


# --------------------------------------------------------------------------
# quem segura a mídia
# --------------------------------------------------------------------------
class MediaFocus:
    """Contagem de quem precisa de silêncio (fala, escuta); pausa no primeiro, retoma depois do último."""

    def __init__(self, backend: Optional[MediaBackend] = None, grace: float = 1.2,
                 enabled: Callable[[], bool] = lambda: True, log: Callable[[str], None] = print):
        self.backend = backend or default_backend()
        self.grace = grace
        self.enabled = enabled
        self.log = log
        self._holders: Set[str] = set()
        self._paused: List[str] = []
        self._lock = threading.Lock()
        self._jobs: "queue.Queue" = queue.Queue()
        self._timer: Optional[threading.Timer] = None
        self._generation = 0
        threading.Thread(target=self._worker, daemon=True, name="MediaFocus").start()

    # ---------------------------------------------------------------- API
    def hold(self, who: str) -> None:
        """``who`` precisa de silêncio (a primeira chamada pausa o que estiver tocando)."""
        with self._lock:
            self._generation += 1
            self._cancel_timer()
            first = not self._holders
            self._holders.add(who)
        if first and self.enabled():
            self._jobs.put(("pause", None))

    def release(self, who: str, grace: Optional[float] = None) -> None:
        """``who`` terminou; quando ninguém mais segura, a mídia volta depois de ``grace`` segundos
        (padrão: o do construtor). Uma folga maior cobre o tempo de pensar entre ouvir e responder."""
        with self._lock:
            if who not in self._holders:
                return
            self._holders.discard(who)
            if self._holders:
                return
            gen = self._generation
            self._cancel_timer()
            wait = self.grace if grace is None else grace
            self._timer = threading.Timer(wait, self._resume_if_still_free, args=(gen,))
            self._timer.daemon = True
            self._timer.start()

    @property
    def holding(self) -> bool:
        with self._lock:
            return bool(self._holders)

    def wait_idle(self, timeout: float = 10.0) -> None:
        """Espera as pausas/retomadas pendentes (usado nos testes e ao encerrar)."""
        done = threading.Event()
        self._jobs.put(("mark", done))
        done.wait(timeout)

    # ---------------------------------------------------------------- interno
    def _cancel_timer(self) -> None:
        if self._timer is not None:
            self._timer.cancel()
            self._timer = None

    def _resume_if_still_free(self, gen: int) -> None:
        with self._lock:
            if self._holders or gen != self._generation:
                return
            self._timer = None
        self._jobs.put(("resume", None))

    def _worker(self) -> None:
        while True:
            kind, arg = self._jobs.get()
            try:
                if kind == "mark":
                    arg.set()
                elif kind == "pause":
                    if self._paused:                      # já está pausado por nós
                        continue
                    paused = self.backend.pause_playing()
                    self._paused = list(paused)
                    if paused:
                        self.log("[Mídia] Pausei para falar: " + ", ".join(paused))
                elif kind == "resume":
                    sessions, self._paused = self._paused, []
                    if sessions:
                        self.backend.resume(sessions)
                        self.log("[Mídia] Retomei: " + ", ".join(sessions))
            except Exception as e:
                self.log("[Mídia] Não consegui controlar a mídia: " + str(e)[:200])
