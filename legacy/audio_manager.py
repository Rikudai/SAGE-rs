"""
Audio Manager for S.A.G.E. Voice Assistant:
- Microphone capture with speech_recognition
- Wake word stripping
- Fish Audio TTS (default, Iron Man S.A.G.E. voice) & Edge-TTS fallback (pt-BR-AntonioNeural)
- Interruptible playback with pygame.mixer
- Acoustic audio cues with winsound
"""
import asyncio
import json
import os
import re

from sage.text_processing import pronounce_name
import tempfile
import threading
import time
import urllib.error
import urllib.request
from typing import List, Optional, Callable

import edge_tts
import numpy as np
import pygame
import speech_recognition as sr
import winsound

from sage import sfx

# pythonw não tem console: print() some. Todo passo do push-to-talk vai também para este arquivo.
PTT_LOG_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "logs", "ptt.log")


def ptt_log(msg: str) -> None:
    """Appends a timestamped line to logs/ptt.log. Never raises."""
    try:
        os.makedirs(os.path.dirname(PTT_LOG_PATH), exist_ok=True)
        with open(PTT_LOG_PATH, "a", encoding="utf-8") as f:
            f.write(time.strftime("[%Y-%m-%d %H:%M:%S] ") + msg + "\n")
    except Exception:
        pass


# Segurar a tecla precisa durar pelo menos isso para a soltura encerrar a captura.
PTT_MIN_HOLD_S = 0.25

FISH_AUDIO_TTS_URL = "https://api.fish.audio/v1/tts"
DEFAULT_FISH_API_KEY = os.environ.get("FISH_AUDIO_API_KEY", "")  # real key lives in config.json
DEFAULT_FISH_REFERENCE_ID = "a5b93aeddcc948c19ea04f0afe9d178c"
DEFAULT_FISH_MODEL = "s2.1-pro-free"


def _wake_pattern(wake_word: str) -> str:
    """Builds a tolerant pattern: S.A.G.E., s a g e and sage all match."""
    letters = [re.escape(ch) for ch in wake_word if ch not in " .-"]
    separator = r"[\s.\-]*"
    return r"^\s*" + separator.join(letters) + r"[\s,.:;!?\-]*"


# Como o nome é falado ("Seije"), e como o reconhecedor de voz costuma escrever isso.
SPOKEN_WAKE_WORDS = ["seije", "ei seije", "seige", "ei seige", "seje", "ei seje", "sei je", "seiji", "ei seiji"]


def clean_wake_word(text: str, wake_words: List[str]) -> str:
    """Strips leading wake words (e.g. S.A.G.E., Ei S.A.G.E., Seije) from user prompt."""
    if not text:
        return ""
    cleaned = text.strip()
    for ww in sorted(set(wake_words) | set(SPOKEN_WAKE_WORDS), key=len, reverse=True):
        pattern = _wake_pattern(ww)
        if re.search(pattern, cleaned, re.IGNORECASE):
            cleaned = re.sub(pattern, "", cleaned, flags=re.IGNORECASE).strip()
            break
    return cleaned if cleaned else text.strip()


ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_SFX_DIR = os.path.join(ROOT_DIR, "assets", "sfx")


class AudioManager:
    def __init__(self, sfx_dir: Optional[str] = None, enable_sfx: bool = True):
        self._recognizer = sr.Recognizer()
        self._recognizer.dynamic_energy_threshold = True
        self._recognizer.energy_threshold = 300
        self._recognizer.pause_threshold = 1.0

        # Initialize pygame mixer for audio playback
        try:
            if not pygame.mixer.get_init():
                pygame.mixer.init()
                try:
                    pygame.mixer.set_num_channels(16)
                except Exception:
                    pass
        except Exception as e:
            print(f"[AudioManager] Warning initializing pygame.mixer: {e}")

        self.sfx_dir = sfx_dir or DEFAULT_SFX_DIR
        self.enable_sfx = enable_sfx
        self._sfx_paths: Optional[dict] = None
        self._sfx_sounds: dict = {}
        self._sfx_channel: Optional[pygame.mixer.Channel] = None

        self._current_audio_file: Optional[str] = None
        self._stop_requested = threading.Event()
        self._mixer_lock = threading.RLock()
        self._closed = False

    def close(self) -> None:
        """Shuts down audio mixer cleanly (serialized against cue threads)."""
        self._closed = True
        with self._mixer_lock:
            self.stop_tts()
            try:
                if pygame.mixer.get_init():
                    pygame.mixer.quit()
            except Exception:
                pass

    def _prepare_sfx(self) -> None:
        """Generates/loads the synthesized cue bank once."""
        if self._sfx_paths is not None:
            return
        self._sfx_paths = {}
        if not self.enable_sfx:
            return
        try:
            self._sfx_paths = sfx.ensure_sfx(self.sfx_dir)
        except Exception as e:
            print(f"[AudioManager] SFX generation warning: {e}")
            self._sfx_paths = {}

    def play_sfx(self, name: str) -> bool:
        """Plays the sound mapped to an event (see sage.sound_bank / web UI)."""
        if self._closed or not self.enable_sfx:
            return False
        try:
            from sage import sound_bank
            resolved = sound_bank.resolve(name)
        except Exception:
            resolved = None
        if resolved is None:
            return False
        path, volume = resolved
        with self._mixer_lock:
            if self._closed:
                return False
            try:
                if not pygame.mixer.get_init():
                    return False
                sound = self._sfx_sounds.get(path)
                if sound is None:
                    sound = pygame.mixer.Sound(path)
                    self._sfx_sounds[path] = sound
                channel = pygame.mixer.find_channel(True)
                if channel is None:
                    return False
                channel.set_volume(max(0.0, min(1.0, volume)))
                channel.play(sound)
                return True
            except Exception:
                return False

    def reload_sfx(self) -> None:
        """Drops cached sounds after the user changes the sound settings."""
        with self._mixer_lock:
            self._sfx_sounds.clear()

    def play_file(self, path: str, volume: float = 0.8) -> bool:
        """Preview of any library file (used by the web UI)."""
        with self._mixer_lock:
            try:
                if not pygame.mixer.get_init():
                    return False
                snd = pygame.mixer.Sound(path)
                ch = pygame.mixer.find_channel(True)
                ch.set_volume(max(0.0, min(1.0, volume)))
                ch.play(snd)
                return True
            except Exception:
                return False

    def play_audio_cue(self, cue_type: str) -> None:
        """Plays a brief non-blocking audio cue to guide the user."""
        def _play():
            self.play_sfx(cue_type)
        threading.Thread(target=_play, daemon=True).start()

    def record_and_transcribe(
        self,
        language: str = "pt-BR",
        timeout: int = 6,
        phrase_time_limit: int = 120,
        audio_cues: bool = True,
        is_key_pressed_callback: Optional[Callable[[], bool]] = None
    ) -> Optional[str]:
        """
        Listens to microphone input and converts speech to text using Google Speech API.
        If is_key_pressed_callback is provided, keeps recording as long as the key is held down.
        """
        if audio_cues:
            self.play_audio_cue("listen_start")

        try:
            if is_key_pressed_callback:
                import pyaudio
                import numpy as np

                p = pyaudio.PyAudio()
                sample_rate = 16000
                chunk_size = 1024
                try:
                    stream = p.open(
                        format=pyaudio.paInt16,
                        channels=1,
                        rate=sample_rate,
                        input=True,
                        frames_per_buffer=chunk_size
                    )
                except Exception as e:
                    ptt_log(f"ERRO abrindo microfone (pyaudio): {e!r}")
                    p.terminate()
                    return None
                frames = []
                start_time = time.time()
                last_voice_time = time.time()
                key_was_held = False
                held_since = 0.0
                peak_rms = 0.0

                while time.time() - start_time < phrase_time_limit:
                    data = stream.read(chunk_size, exception_on_overflow=False)
                    frames.append(data)
                    chunk_arr = np.frombuffer(data, dtype=np.int16)
                    chunk_rms = float(np.sqrt(np.mean(chunk_arr.astype(np.float32) ** 2))) if len(chunk_arr) > 0 else 0.0
                    peak_rms = max(peak_rms, chunk_rms)

                    # Check if push-to-talk key is currently held
                    if is_key_pressed_callback():
                        if not key_was_held:
                            held_since = time.time()
                        key_was_held = True
                        last_voice_time = time.time()
                    elif key_was_held:
                        if time.time() - held_since >= PTT_MIN_HOLD_S:
                            # User really held the key and released it: complete recording now!
                            break
                        # Released almost at once (hook glitch or a quick tap): don't end the
                        # capture on that; fall back to tap mode, which ends on silence.
                        key_was_held = False
                        last_voice_time = time.time()
                        ptt_log("soltura antes de 250 ms: tratada como toque")
                    else:
                        # Tap mode: check voice activity via RMS energy
                        if chunk_rms > 250:
                            last_voice_time = time.time()
                        elif time.time() - last_voice_time > 2.5:
                            # Silence gap detected in tap mode
                            break

                    time.sleep(0.005)

                stream.stop_stream()
                stream.close()
                p.terminate()
                ptt_log(f"captura: {len(frames)} quadros, {time.time() - start_time:.2f}s, "
                        f"modo={'segurar' if key_was_held else 'toque'}, pico_rms={peak_rms:.0f}")

                if not frames:
                    ptt_log("captura vazia (nenhum quadro lido)")
                    return None

                raw_audio = b"".join(frames)
                audio_data = sr.AudioData(raw_audio, sample_rate, 2)

            else:
                with sr.Microphone() as source:
                    # Extended ambient noise and pause threshold
                    self._recognizer.adjust_for_ambient_noise(source, duration=0.3)
                    self._recognizer.pause_threshold = 2.5
                    audio_data = self._recognizer.listen(
                        source,
                        timeout=timeout,
                        phrase_time_limit=phrase_time_limit
                    )

            if audio_cues:
                self.play_audio_cue("listen_stop")

            transcription = self._recognizer.recognize_google(audio_data, language=language)
            return transcription.strip() if transcription else None

        except sr.WaitTimeoutError:
            ptt_log("STT: timeout sem fala")
            return None
        except sr.UnknownValueError:
            ptt_log("STT: áudio sem fala reconhecível")
            return None
        except Exception as e:
            ptt_log(f"ERRO no push-to-talk (captura/STT): {e!r}")
            print(f"[AudioManager] STT Error: {e}")
            if audio_cues:
                self.play_audio_cue("error")
            return None

    def stop_tts(self) -> None:
        """Stops any currently playing audio immediately."""
        self._stop_requested.set()
        try:
            if pygame.mixer.get_init():
                pygame.mixer.music.stop()
        except Exception as e:
            print(f"[AudioManager] Error stopping music: {e}")

    def _synthesize_fish_audio(
        self,
        text: str,
        output_path: str,
        api_key: str = DEFAULT_FISH_API_KEY,
        reference_id: str = DEFAULT_FISH_REFERENCE_ID,
        model: str = DEFAULT_FISH_MODEL
    ) -> bool:
        """Synthesizes speech via Fish Audio API and writes MP3 bytes to output_path."""
        payload = json.dumps({
            "text": text,
            "reference_id": reference_id,
            "format": "mp3"
        }).encode("utf-8")

        req = urllib.request.Request(
            FISH_AUDIO_TTS_URL,
            data=payload,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
                "User-Agent": "S.A.G.E./2.0",
                "model": model
            },
            method="POST"
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                audio_bytes = resp.read()
                if not audio_bytes:
                    return False
                with open(output_path, "wb") as f:
                    f.write(audio_bytes)
                return True
        except Exception as e:
            print(f"[AudioManager] Fish Audio TTS warning (falling back to edge-tts): {e}")
            return False

    def _normalized_wav(self, source_path: str):
        """Peak-normalizes quiet TTS output to a temporary WAV (Fish audio can be low)."""
        try:
            import wave

            if not pygame.mixer.get_init():
                pygame.mixer.init()
            sound = pygame.mixer.Sound(source_path)
            arr = pygame.sndarray.array(sound)
            if arr.size == 0:
                return None
            peak = int(np.max(np.abs(arr.astype(np.int32))))
            target = int(32767 * 0.92)
            if peak <= 0 or peak >= target * 0.97:
                return None
            scaled = (arr.astype(np.float32) * (target / float(peak))).astype(np.int16)
            fd, out_path = tempfile.mkstemp(suffix=".wav")
            os.close(fd)
            with wave.open(out_path, "wb") as wf:
                wf.setnchannels(2 if scaled.ndim > 1 else 1)
                wf.setsampwidth(2)
                wf.setframerate(44100)
                wf.writeframes(scaled.tobytes())
            return out_path
        except Exception:
            return None

    def speak(
        self,
        text: str,
        voice: str = "pt-BR-FranciscaNeural",
        rate: str = "+0%",
        volume: str = "+0%",
        pitch: str = "+0Hz",
        on_start: Optional[Callable[[], None]] = None,
        on_finish: Optional[Callable[[], None]] = None,
        tts_provider: str = "edge_tts",
        fish_api_key: Optional[str] = None,
        fish_reference_id: Optional[str] = None,
        fish_model: str = DEFAULT_FISH_MODEL
    ) -> bool:
        """
        Synthesizes text using Fish Audio (or edge-tts) and plays it with pygame.mixer.
        Returns True if played to completion, False if interrupted or error.
        """
        if not text or not text.strip():
            return False
        text = pronounce_name(text)        # escrito S.A.G.E., falado "Seije"

        self.stop_tts()
        self._stop_requested.clear()

        temp_path = None
        try:
            with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as tf:
                temp_path = tf.name

            synthesized = False
            if tts_provider in ("fish_audio", "fish"):
                synthesized = self._synthesize_fish_audio(
                    text=text,
                    output_path=temp_path,
                    api_key=fish_api_key or DEFAULT_FISH_API_KEY,
                    reference_id=fish_reference_id or DEFAULT_FISH_REFERENCE_ID,
                    model=fish_model or DEFAULT_FISH_MODEL
                )

            if not synthesized:
                # Generate TTS audio with edge-tts
                async def _synthesize():
                    comm = edge_tts.Communicate(
                        text=text,
                        voice=voice,
                        rate=rate,
                        volume=volume,
                        pitch=pitch
                    )
                    await comm.save(temp_path)

                asyncio.run(_synthesize())

            if self._stop_requested.is_set():
                return False

            if on_start:
                on_start()

            if not pygame.mixer.get_init():
                pygame.mixer.init()

            # Normalize quiet voices (Fish Audio) to a consistent loudness
            play_path = temp_path
            normalized_path = self._normalized_wav(temp_path)
            if normalized_path:
                play_path = normalized_path

            pygame.mixer.music.load(play_path)
            pygame.mixer.music.play()

            while pygame.mixer.music.get_busy():
                if self._stop_requested.is_set():
                    pygame.mixer.music.stop()
                    break
                time.sleep(0.08)

            completed = not self._stop_requested.is_set()
            return completed

        except Exception as e:
            print(f"[AudioManager] TTS Playback error: {e}")
            return False
        finally:
            if on_finish:
                on_finish()
            # Clean up temp file safely after unhooking from mixer
            if temp_path:
                try:
                    pygame.mixer.music.unload()
                except Exception:
                    pass
                for target_file in (temp_path, locals().get("normalized_path")):
                    if not target_file:
                        continue
                    for _ in range(3):
                        try:
                            if os.path.exists(target_file):
                                os.remove(target_file)
                            break
                        except Exception:
                            time.sleep(0.05)
