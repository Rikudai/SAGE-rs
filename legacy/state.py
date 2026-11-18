"""
Thread-safe state manager for S.A.G.E. Voice Assistant.
"""
from enum import Enum
import threading
from typing import Callable, List, Optional


class AppStatus(str, Enum):
    IDLE = "IDLE"
    LISTENING = "LISTENING"
    PROCESSING = "PROCESSING"
    SPEAKING = "SPEAKING"
    ERROR = "ERROR"


class SageState:
    def __init__(self, initial_muted: bool = False):
        self._lock = threading.RLock()
        self._status: AppStatus = AppStatus.IDLE
        self._is_muted: bool = initial_muted
        self._last_prompt: str = ""
        self._last_raw_response: str = ""
        self._last_spoken_response: str = ""
        self._current_emotion: str = "idle"
        self._is_speaking: bool = False
        self._last_routing_source: str = "local"
        self._last_action: Optional[str] = None
        self._last_action_result: Optional[str] = None
        self._model_info: dict = {}
        self._activities: list = []

        self._status_listeners: List[Callable[[AppStatus], None]] = []
        self._mute_listeners: List[Callable[[bool], None]] = []
        self._response_listeners: List[Callable[[str, str], None]] = []
        self._emotion_listeners: List[Callable[[str], None]] = []
        self._speaking_listeners: List[Callable[[bool], None]] = []
        self._model_listeners: List[Callable[[dict], None]] = []
        self._activity_listeners: List[Callable[[dict], None]] = []

    @property
    def status(self) -> AppStatus:
        with self._lock:
            return self._status

    @property
    def is_muted(self) -> bool:
        with self._lock:
            return self._is_muted

    @property
    def last_prompt(self) -> str:
        with self._lock:
            return self._last_prompt

    @property
    def last_raw_response(self) -> str:
        with self._lock:
            return self._last_raw_response

    @property
    def last_spoken_response(self) -> str:
        with self._lock:
            return self._last_spoken_response

    @property
    def current_emotion(self) -> str:
        with self._lock:
            return self._current_emotion

    @property
    def is_speaking(self) -> bool:
        with self._lock:
            return self._is_speaking

    @property
    def last_routing_source(self) -> str:
        with self._lock:
            return self._last_routing_source

    @property
    def model_info(self) -> dict:
        with self._lock:
            return dict(self._model_info)

    @property
    def activities(self) -> list:
        with self._lock:
            return list(self._activities)

    def set_status(self, new_status: AppStatus) -> None:
        listeners_to_call = []
        with self._lock:
            if self._status != new_status:
                self._status = new_status
                listeners_to_call = list(self._status_listeners)
        for listener in listeners_to_call:
            try:
                listener(new_status)
            except Exception as e:
                print(f"[SageState] Listener error on status: {e}")

    def set_emotion(self, emotion: str) -> None:
        clean_emotion = emotion.lower().strip() if emotion else "idle"
        listeners_to_call = []
        with self._lock:
            if self._current_emotion != clean_emotion:
                self._current_emotion = clean_emotion
                listeners_to_call = list(self._emotion_listeners)
        for listener in listeners_to_call:
            try:
                listener(clean_emotion)
            except Exception as e:
                print(f"[SageState] Listener error on emotion: {e}")

    def set_speaking(self, is_speaking: bool) -> None:
        listeners_to_call = []
        with self._lock:
            if self._is_speaking != is_speaking:
                self._is_speaking = is_speaking
                listeners_to_call = list(self._speaking_listeners)
        for listener in listeners_to_call:
            try:
                listener(is_speaking)
            except Exception as e:
                print(f"[SageState] Listener error on speaking: {e}")

    def toggle_mute(self) -> bool:
        listeners_to_call = []
        with self._lock:
            self._is_muted = not self._is_muted
            current_mute = self._is_muted
            listeners_to_call = list(self._mute_listeners)
        for listener in listeners_to_call:
            try:
                listener(current_mute)
            except Exception as e:
                print(f"[SageState] Listener error on mute: {e}")
        return current_mute

    def set_mute(self, muted: bool) -> None:
        listeners_to_call = []
        with self._lock:
            if self._is_muted != muted:
                self._is_muted = muted
                listeners_to_call = list(self._mute_listeners)
        for listener in listeners_to_call:
            try:
                listener(muted)
            except Exception as e:
                print(f"[SageState] Listener error on mute: {e}")

    def update_interaction(
        self,
        prompt: str,
        raw_response: str,
        spoken_response: str,
        source: str = "local",
        action: Optional[str] = None,
        action_result: Optional[str] = None
    ) -> None:
        listeners_to_call = []
        with self._lock:
            self._last_prompt = prompt
            self._last_raw_response = raw_response
            self._last_spoken_response = spoken_response
            self._last_routing_source = source
            self._last_action = action
            self._last_action_result = action_result
            listeners_to_call = list(self._response_listeners)
        for listener in listeners_to_call:
            try:
                listener(raw_response, spoken_response)
            except Exception as e:
                print(f"[SageState] Listener error on response: {e}")

    def add_status_listener(self, listener: Callable[[AppStatus], None]) -> None:
        with self._lock:
            if listener not in self._status_listeners:
                self._status_listeners.append(listener)

    def add_mute_listener(self, listener: Callable[[bool], None]) -> None:
        with self._lock:
            if listener not in self._mute_listeners:
                self._mute_listeners.append(listener)

    def add_response_listener(self, listener: Callable[[str, str], None]) -> None:
        with self._lock:
            if listener not in self._response_listeners:
                self._response_listeners.append(listener)

    def add_emotion_listener(self, listener: Callable[[str], None]) -> None:
        with self._lock:
            if listener not in self._emotion_listeners:
                self._emotion_listeners.append(listener)

    def add_speaking_listener(self, listener: Callable[[bool], None]) -> None:
        with self._lock:
            if listener not in self._speaking_listeners:
                self._speaking_listeners.append(listener)

    # ---- brain / activity (visible in the panel) ----
    def set_model_info(self, info: dict) -> None:
        listeners = []
        with self._lock:
            self._model_info.update(info or {})
            listeners = list(self._model_listeners)
        for listener in listeners:
            try:
                listener(dict(self._model_info))
            except Exception as e:
                print(f"[SageState] Listener error on model info: {e}")

    def add_activity(self, kind: str, text: str, level: str = "info") -> None:
        entry = {
            "time": __import__("datetime").datetime.now().strftime("%H:%M:%S"),
            "kind": kind,
            "text": str(text)[:1200],
            "level": level,
        }
        listeners = []
        with self._lock:
            self._activities.append(entry)
            self._activities = self._activities[-240:]
            listeners = list(self._activity_listeners)
        for listener in listeners:
            try:
                listener(entry)
            except Exception as e:
                print(f"[SageState] Listener error on activity: {e}")

    def clear_activities(self) -> None:
        with self._lock:
            self._activities = []

    def add_model_listener(self, listener: Callable[[dict], None]) -> None:
        with self._lock:
            if listener not in self._model_listeners:
                self._model_listeners.append(listener)

    def add_activity_listener(self, listener: Callable[[dict], None]) -> None:
        with self._lock:
            if listener not in self._activity_listeners:
                self._activity_listeners.append(listener)
