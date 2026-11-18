"""
Procedural sound design for S.A.G.E..

Generates a small, cohesive UI soundscape (warm synthetic chimes, soft
whooshes and tactile clicks) as 44.1 kHz stereo WAV files, then plays them
through pygame's mixer. No external sample packs are required.

Cues
----
ready           soft rising chord when the assistant boots
listen_start    airy two-note rise (microphone open)
listen_stop     gentle descending confirmation
thinking        subtle pulse while planning
success         bright bell arpeggio
error           soft detuned thud (never harsh)
teleport        whoosh + sparkle (pet teleports across the screen)
transform       shimmering morph chord
click           tiny tactile click
screenshot      camera shutter
"""
from __future__ import annotations

import math
import os
import wave
from typing import Dict, Optional

import numpy as np

SR = 44100
DURATIONS = {
    "ready": 1.05,
    "listen_start": 0.30,
    "listen_stop": 0.32,
    "thinking": 0.45,
    "success": 0.85,
    "error": 0.55,
    "teleport": 0.62,
    "transform": 0.90,
    "click": 0.09,
    "screenshot": 0.28,
}


# --------------------------------------------------------------------------
# DSP helpers
# --------------------------------------------------------------------------
def _t(n: int) -> np.ndarray:
    return np.arange(n, dtype=np.float64) / SR


def _env(n: int, attack: float, decay: float, curve: float = 2.0) -> np.ndarray:
    t = _t(n)
    a = np.clip(t / max(1e-4, attack), 0, 1)
    d = np.exp(-np.clip(t - attack, 0, None) / max(1e-4, decay))
    return (a ** 0.7) * (d ** (1.0 / curve))


def _bell(freq: float, n: int, detune: float = 0.0, harmonics=(1.0, 2.0, 3.0, 4.2),
          amps=(1.0, 0.45, 0.22, 0.10), decay: float = 0.35) -> np.ndarray:
    t = _t(n)
    out = np.zeros(n)
    for h, a in zip(harmonics, amps):
        out += a * np.sin(2 * np.pi * freq * h * (1 + detune) * t + (h * 0.7))
    return out * _env(n, 0.008, decay, curve=2.2) / max(1.0, sum(amps) * 0.6)


def _sweep(f0: float, f1: float, n: int, shape: float = 1.0) -> np.ndarray:
    t = _t(n)
    k = (t / max(1e-4, t[-1])) ** shape
    phase = 2 * np.pi * (f0 * t + (f1 - f0) * k * t / 2.0)
    return np.sin(phase)


def _noise(n: int, seed: int = 7) -> np.ndarray:
    rng = np.random.default_rng(seed)
    return rng.standard_normal(n)


def _lowpass(x: np.ndarray, cutoff: float) -> np.ndarray:
    """One-pole low-pass (cutoff in Hz)."""
    a = math.exp(-2 * math.pi * cutoff / SR)
    y = np.zeros_like(x)
    acc = 0.0
    for i, v in enumerate(x):
        acc = (1 - a) * v + a * acc
        y[i] = acc
    return y


def _reverb(x: np.ndarray, decay: float = 0.32, mix: float = 0.22) -> np.ndarray:
    """Cheap but musical Schroeder-ish reverb using comb delays."""
    out = x.copy()
    for delay_ms, gain in ((37, 0.30), (53, 0.24), (71, 0.18), (97, 0.13)):
        d = int(SR * delay_ms / 1000)
        buf = np.zeros_like(x)
        buf[d:] = x[:-d] * gain * decay
        out += buf
    return out * (1 - mix) + out * mix


def _stereo(x: np.ndarray, width: float = 0.12) -> np.ndarray:
    """Creates subtle stereo width with a tiny delay on the right channel."""
    d = int(SR * width / 1000)
    left = x
    right = np.copy(x)
    if d > 0:
        right[d:] = x[:-d]
    return np.stack([left, right], axis=1)


def _normalize(x: np.ndarray, peak: float = 0.72) -> np.ndarray:
    m = float(np.max(np.abs(x))) or 1.0
    return x / m * peak


def _pad(x: np.ndarray, total: int) -> np.ndarray:
    if len(x) >= total:
        return x[:total]
    return np.concatenate([x, np.zeros(total - len(x))])


# --------------------------------------------------------------------------
# Individual cues
# --------------------------------------------------------------------------
def _cue_ready(n: int) -> np.ndarray:
    a = _bell(220.0, n, decay=0.55, harmonics=(1, 2, 3), amps=(1, 0.5, 0.25))
    b = _bell(329.63, n, decay=0.45) * 0.7
    c = _bell(440.0, n, decay=0.40) * 0.45
    shimmer = _sweep(1200, 2400, n, shape=0.7) * _env(n, 0.15, 0.35) * 0.06
    return a + b + c + shimmer


def _cue_listen_start(n: int) -> np.ndarray:
    x = _sweep(660, 1320, n, shape=1.6) * _env(n, 0.01, 0.12)
    air = _lowpass(_noise(n), 6000) * _env(n, 0.02, 0.10) * 0.10
    return x * 0.55 + air


def _cue_listen_stop(n: int) -> np.ndarray:
    x = _sweep(880, 440, n, shape=1.3) * _env(n, 0.008, 0.10)
    return x * 0.55


def _cue_thinking(n: int) -> np.ndarray:
    t = _t(n)
    pulse = (np.sin(2 * np.pi * 3.0 * t) > 0.6).astype(float) * _env(n, 0.02, 0.30)
    tone = np.sin(2 * np.pi * 520 * t) * 0.25 + np.sin(2 * np.pi * 780 * t) * 0.12
    return pulse * tone


def _cue_success(n: int) -> np.ndarray:
    out = np.zeros(n)
    notes = [(523.25, 0.0), (659.25, 0.10), (783.99, 0.20)]
    for freq, delay in notes:
        d = int(delay * SR)
        seg = _bell(freq, n - d, decay=0.35)
        out[d:] += seg * 0.6
    return out


def _cue_error(n: int) -> np.ndarray:
    t = _t(n)
    low = np.sin(2 * np.pi * 138 * t) * np.exp(-t / 0.16)
    detuned = np.sin(2 * np.pi * 146 * t) * np.exp(-t / 0.14) * 0.5
    thump = _lowpass(_noise(n), 400) * np.exp(-t / 0.05) * 0.25
    return low * 0.5 + detuned + thump


def _cue_teleport(n: int) -> np.ndarray:
    whoosh = _lowpass(_noise(n, seed=3), 2600) * _env(n, 0.05, 0.16) * 0.5
    # travelling shimmer
    shimmer = _sweep(700, 2600, n, shape=0.8) * _env(n, 0.06, 0.12) * 0.35
    sparkle = _bell(1760, n, decay=0.18, harmonics=(1, 2), amps=(1, 0.3)) * 0.22
    return whoosh + shimmer + sparkle


def _cue_transform(n: int) -> np.ndarray:
    t = _t(n)
    chord = (np.sin(2 * np.pi * 392 * t) + np.sin(2 * np.pi * 494 * t) * 0.8 +
             np.sin(2 * np.pi * 587 * t) * 0.7)
    trem = 0.65 + 0.35 * np.sin(2 * np.pi * 7 * t)
    sweep = _sweep(300, 1600, n, shape=0.7) * 0.18
    return (chord * trem * _env(n, 0.18, 0.5, 1.6) + sweep) * 0.5


def _cue_click(n: int) -> np.ndarray:
    t = _t(n)
    click = _lowpass(_noise(n, seed=11), 5200) * np.exp(-t / 0.004) * 0.9
    body = np.sin(2 * np.pi * 1800 * t) * np.exp(-t / 0.006) * 0.25
    return click + body


def _cue_screenshot(n: int) -> np.ndarray:
    t = _t(n)
    shutter = _lowpass(_noise(n, seed=5), 7000) * (np.exp(-t / 0.012) + np.exp(-np.clip(t - 0.12, 0, None) / 0.012))
    click = np.sin(2 * np.pi * 2400 * t) * np.exp(-t / 0.005) * 0.2
    return shutter * 0.55 + click


_BUILDERS = {
    "ready": _cue_ready,
    "listen_start": _cue_listen_start,
    "listen_stop": _cue_listen_stop,
    "thinking": _cue_thinking,
    "success": _cue_success,
    "error": _cue_error,
    "teleport": _cue_teleport,
    "transform": _cue_transform,
    "click": _cue_click,
    "screenshot": _cue_screenshot,
}


def synthesize(name: str) -> np.ndarray:
    """Returns a normalized stereo float array for the given cue."""
    dur = DURATIONS.get(name, 0.3)
    n = int(SR * dur)
    mono = _BUILDERS[name](n)
    mono = _reverb(mono, mix=0.18)
    mono = _normalize(_pad(mono, n), peak=0.70)
    return _stereo(mono, width=0.10)


def write_cue(name: str, path: str, gain: float = 0.85) -> str:
    data = synthesize(name) * gain
    pcm = np.clip(data, -1.0, 1.0)
    pcm = (pcm * 32767).astype("<i2")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with wave.open(path, "wb") as wf:
        wf.setnchannels(2)
        wf.setsampwidth(2)
        wf.setframerate(SR)
        wf.writeframes(pcm.tobytes())
    return path


def ensure_sfx(directory: str) -> Dict[str, str]:
    """Generates any missing cue files and returns {cue: path}."""
    os.makedirs(directory, exist_ok=True)
    paths: Dict[str, str] = {}
    for name in _BUILDERS:
        path = os.path.join(directory, f"{name}.wav")
        if not os.path.exists(path) or os.path.getsize(path) < 256:
            try:
                write_cue(name, path)
            except Exception as e:
                print(f"[SFX] failed to synthesize {name}: {e}")
                continue
        paths[name] = path
    return paths
