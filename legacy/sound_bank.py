"""
S.A.G.E. sound bank
===================
Maps assistant *events* (ready, listening, clone spawned, error...) to sound
files the user can change in the web UI.

Library (assets/sfx/library/):
* ``kenney/``  - Kenney "Interface Sounds" (CC0): water drops, glass chimes,
                 plucks, confirmations...
* ``slime/``   - soft slime sounds synthesised here (bloops, bubbles, squish,
                 sparkles) - smooth envelopes, no harsh beeps.
* ``custom/``  - files uploaded by the user.

Configuration: data/sound_config.json -> {"master": 0.8, "enabled": true,
"events": {"ready": {"file": "kenney/confirmation_002.wav", "volume": 0.8,
"enabled": true}, ...}}
"""
from __future__ import annotations

import json
import math
import os
import re
import threading
import wave
from typing import Dict, List, Optional

import numpy as np

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIBRARY_DIR = os.path.join(ROOT_DIR, "assets", "sfx", "library")
CONFIG_PATH = os.path.join(ROOT_DIR, "data", "sound_config.json")
RATE = 44100

EVENTS = {
    "ready": "S.A.G.E. iniciou",
    "listen_start": "Começou a ouvir",
    "listen_stop": "Parou de ouvir",
    "queued": "Mensagem entrou na fila",
    "thinking": "Pensando / executando",
    "success": "Ação concluída",
    "error": "Erro",
    "notify": "Aviso / trabalho pronto",
    "question": "Pergunta para você",
    "transform": "Transformação",
    "teleport": "Teleporte",
    "clone_spawn": "Clone surgiu",
    "clone_done": "Clone entregou",
    "screenshot": "Print da tela",
    "click": "Clique",
}

DEFAULTS = {
    "ready": ("slime/sparkle_rise.wav", 0.7),
    "listen_start": ("kenney/drop_002.wav", 0.8),
    "listen_stop": ("kenney/drop_003.wav", 0.7),
    "queued": ("slime/bloop_small.wav", 0.6),
    "thinking": ("kenney/pluck_001.wav", 0.5),
    "success": ("kenney/confirmation_002.wav", 0.7),
    "error": ("kenney/error_004.wav", 0.7),
    "notify": ("kenney/glass_002.wav", 0.75),
    "question": ("kenney/question_001.wav", 0.75),
    "transform": ("slime/squish_morph.wav", 0.8),
    "teleport": ("kenney/maximize_006.wav", 0.7),
    "clone_spawn": ("slime/bloop_up.wav", 0.75),
    "clone_done": ("slime/bubbles_pop.wav", 0.7),
    "screenshot": ("kenney/click_003.wav", 0.7),
    "click": ("kenney/click_002.wav", 0.5),
}

_lock = threading.RLock()


# ==========================================================================
# Synthesised slime sounds
# ==========================================================================
def _env(n: int, attack: float = 0.006, release: float = 0.5) -> np.ndarray:
    t = np.arange(n) / RATE
    a = np.clip(t / max(attack, 1e-4), 0, 1)
    r = np.exp(-t / max(release, 1e-4))
    return a * r


def _lowpass(x: np.ndarray, cutoff: float) -> np.ndarray:
    rc = 1.0 / (2 * math.pi * cutoff)
    alpha = (1 / RATE) / (rc + 1 / RATE)
    y = np.zeros_like(x)
    acc = 0.0
    for i, v in enumerate(x):
        acc += alpha * (v - acc)
        y[i] = acc
    return y


def _bloop(f0: float, f1: float, dur: float, release: float = 0.06, harm: float = 0.25) -> np.ndarray:
    n = int(RATE * dur)
    t = np.arange(n) / RATE
    k = np.log(f1 / f0) / dur
    phase = 2 * math.pi * f0 * (np.exp(k * t) - 1) / k
    tone = np.sin(phase) + harm * np.sin(2 * phase + 0.4)
    return tone * _env(n, 0.004, release)


def _place(buf: np.ndarray, snd: np.ndarray, at: float, gain: float = 1.0) -> None:
    i = int(at * RATE)
    j = min(len(buf), i + len(snd))
    buf[i:j] += snd[: j - i] * gain


def _reverb(x: np.ndarray, mix: float = 0.22) -> np.ndarray:
    out = x.copy()
    for delay, g in ((0.031, 0.5), (0.047, 0.35), (0.073, 0.22), (0.109, 0.12)):
        d = int(delay * RATE)
        out[d:] += x[:-d] * g * mix * 2
    return out


def _normalise(x: np.ndarray, peak: float = 0.85) -> np.ndarray:
    m = float(np.max(np.abs(x))) or 1.0
    return x / m * peak


def _save(path: str, mono: np.ndarray) -> None:
    mono = _normalise(_reverb(mono))
    # soft fade-out to avoid clicks
    fade = min(len(mono), int(0.01 * RATE))
    mono[-fade:] *= np.linspace(1, 0, fade)
    data = (mono * 32767).astype(np.int16)
    stereo = np.column_stack([data, data]).ravel()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(stereo.tobytes())


def synth_slime_library(force: bool = False) -> List[str]:
    out_dir = os.path.join(LIBRARY_DIR, "slime")
    rng = np.random.default_rng(7)
    recipes = {}

    def bloop_up():
        b = np.zeros(int(RATE * 0.32))
        _place(b, _bloop(260, 820, 0.16, 0.07), 0.0)
        _place(b, _bloop(520, 1240, 0.10, 0.05, 0.1), 0.05, 0.35)
        return b

    def bloop_down():
        b = np.zeros(int(RATE * 0.32))
        _place(b, _bloop(900, 280, 0.18, 0.08), 0.0)
        return b

    def bloop_small():
        b = np.zeros(int(RATE * 0.2))
        _place(b, _bloop(480, 980, 0.07, 0.04, 0.12), 0.0, 0.8)
        return b

    def bubbles_pop():
        b = np.zeros(int(RATE * 0.55))
        for k, (a, f) in enumerate(((0.0, 520), (0.07, 700), (0.15, 610), (0.24, 880), (0.31, 1040))):
            _place(b, _bloop(f * 0.7, f * 1.6, 0.06, 0.035, 0.1), a, 0.9 - k * 0.1)
        return b

    def squish_morph():
        n = int(RATE * 0.75)
        t = np.arange(n) / RATE
        noise = rng.standard_normal(n)
        wobble = 1 + 0.45 * np.sin(2 * math.pi * 9 * t)
        body = _lowpass(noise, 900) * wobble * _env(n, 0.03, 0.22) * 0.9
        b = body.copy()
        _place(b, _bloop(180, 520, 0.35, 0.18, 0.3), 0.05, 0.9)
        # sparkle tail
        for k in range(6):
            f = 1800 + k * 340
            ping = np.sin(2 * math.pi * f * np.arange(int(RATE * 0.25)) / RATE) * _env(int(RATE * 0.25), 0.002, 0.07)
            _place(b, ping, 0.32 + k * 0.05, 0.18)
        return b

    def sparkle_rise():
        b = np.zeros(int(RATE * 0.9))
        notes = [523.25, 659.25, 783.99, 1046.5]   # C major arpeggio
        for k, f in enumerate(notes):
            n = int(RATE * 0.5)
            t = np.arange(n) / RATE
            tone = (np.sin(2 * math.pi * f * t) + 0.3 * np.sin(2 * math.pi * f * 2 * t)) * _env(n, 0.004, 0.18)
            _place(b, tone, k * 0.085, 0.55)
        _place(b, _bloop(300, 700, 0.12, 0.06), 0.0, 0.5)
        return b

    def squish_soft():
        n = int(RATE * 0.35)
        noise = rng.standard_normal(n)
        b = _lowpass(noise, 600) * _env(n, 0.01, 0.08)
        _place(b, _bloop(220, 380, 0.2, 0.08), 0.0, 0.6)
        return b

    recipes.update({
        "bloop_up.wav": bloop_up, "bloop_down.wav": bloop_down, "bloop_small.wav": bloop_small,
        "bubbles_pop.wav": bubbles_pop, "squish_morph.wav": squish_morph,
        "sparkle_rise.wav": sparkle_rise, "squish_soft.wav": squish_soft,
    })
    written = []
    for name, fn in recipes.items():
        path = os.path.join(out_dir, name)
        if force or not os.path.exists(path):
            _save(path, fn())
            written.append(path)
    return written


# ==========================================================================
# Configuration
# ==========================================================================
def default_config() -> Dict:
    return {"master": 0.8, "enabled": True,
            "events": {k: {"file": f, "volume": v, "enabled": True} for k, (f, v) in DEFAULTS.items()}}


def load_config() -> Dict:
    cfg = default_config()
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as f:
            user = json.load(f) or {}
        cfg["master"] = float(user.get("master", cfg["master"]))
        cfg["enabled"] = bool(user.get("enabled", cfg["enabled"]))
        for k, v in (user.get("events") or {}).items():
            if k in cfg["events"] and isinstance(v, dict):
                cfg["events"][k].update({kk: v[kk] for kk in ("file", "volume", "enabled") if kk in v})
    except Exception:
        pass
    return cfg


def save_config(cfg: Dict) -> Dict:
    clean = default_config()
    clean["master"] = max(0.0, min(1.0, float(cfg.get("master", 0.8))))
    clean["enabled"] = bool(cfg.get("enabled", True))
    for k, v in (cfg.get("events") or {}).items():
        if k not in clean["events"] or not isinstance(v, dict):
            continue
        entry = clean["events"][k]
        if "file" in v and (v["file"] == "" or library_path(str(v["file"]))):
            entry["file"] = str(v["file"])
        if "volume" in v:
            entry["volume"] = max(0.0, min(1.0, float(v["volume"])))
        if "enabled" in v:
            entry["enabled"] = bool(v["enabled"])
    with _lock:
        os.makedirs(os.path.dirname(CONFIG_PATH), exist_ok=True)
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(clean, f, indent=2, ensure_ascii=False)
    return clean


def library_path(rel: str) -> Optional[str]:
    """Resolves a library-relative file safely (no path traversal)."""
    rel = (rel or "").replace("\\", "/").strip("/")
    if not rel or ".." in rel.split("/"):
        return None
    path = os.path.abspath(os.path.join(LIBRARY_DIR, rel))
    if not path.startswith(os.path.abspath(LIBRARY_DIR)) or not os.path.isfile(path):
        return None
    return path


def library() -> List[Dict]:
    out = []
    if not os.path.isdir(LIBRARY_DIR):
        return out
    for group in sorted(os.listdir(LIBRARY_DIR)):
        gdir = os.path.join(LIBRARY_DIR, group)
        if not os.path.isdir(gdir):
            continue
        for name in sorted(os.listdir(gdir)):
            if name.lower().endswith((".wav", ".ogg", ".mp3")):
                out.append({"file": group + "/" + name, "group": group, "name": name})
    return out


def save_upload(filename: str, raw: bytes) -> str:
    base = re.sub(r"[^a-zA-Z0-9_.-]", "_", os.path.basename(filename or "som.wav"))[:60]
    if not base.lower().endswith((".wav", ".ogg", ".mp3")):
        raise ValueError("use .wav, .ogg ou .mp3")
    if len(raw) > 5 * 1024 * 1024:
        raise ValueError("arquivo grande demais (máx. 5 MB)")
    out_dir = os.path.join(LIBRARY_DIR, "custom")
    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, base), "wb") as f:
        f.write(raw)
    return "custom/" + base


def resolve(event: str) -> Optional[tuple]:
    """(absolute path, volume) for an event, or None when muted/disabled."""
    cfg = load_config()
    if not cfg.get("enabled", True):
        return None
    entry = cfg["events"].get(event)
    if not entry or not entry.get("enabled", True) or not entry.get("file"):
        return None
    path = library_path(entry["file"])
    if not path:
        return None
    return path, float(entry.get("volume", 0.8)) * float(cfg.get("master", 0.8))


def ensure_library() -> None:
    synth_slime_library(force=False)
