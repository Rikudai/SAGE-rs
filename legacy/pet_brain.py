"""
S.A.G.E. pet brain - gives the desktop companion a life of its own.

Pure decision logic (no Tk, no drawing): the pet window feeds it a small
context every tick and executes the action it returns. Everything is local
and free: no model calls.

What drives the behaviour
-------------------------
* **Mood** - energy, joy, boredom, curiosity and affection drift with time
  and react to events (petting, being dragged, finished tasks, errors...).
* **Personality** - a preset (``pet_personality``) weights the repertoire:
  brincalhao, calmo, curioso, dorminhoco, serio.
* **Senses** - where the mouse is and how it moves (looks at it, startles
  when it zips past, enjoys being rubbed), how long the user has been away
  (dozes off, wakes up and waves when they return) and the time of day.
* **Free will** - with ``pet_free_will`` on it also wanders and changes form
  on its own, picking a form that suits the mood.

Affection is remembered between sessions in ``data/pet_mood.json``.
"""
from __future__ import annotations

import json
import math
import os
import random
import time
from typing import Dict, List, Optional, Tuple

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MOOD_PATH = os.path.join(ROOT_DIR, "data", "pet_mood.json")

# trait weights: playful, calm, curious, sleepy, affectionate, skittish
PERSONALITIES: Dict[str, Dict[str, float]] = {
    "brincalhao": {"playful": 1.0, "calm": 0.3, "curious": 0.7, "sleepy": 0.3, "loving": 0.8, "skittish": 0.6},
    "calmo":      {"playful": 0.35, "calm": 1.0, "curious": 0.5, "sleepy": 0.5, "loving": 0.6, "skittish": 0.2},
    "curioso":    {"playful": 0.6, "calm": 0.4, "curious": 1.0, "sleepy": 0.3, "loving": 0.5, "skittish": 0.5},
    "dorminhoco": {"playful": 0.3, "calm": 0.8, "curious": 0.3, "sleepy": 1.0, "loving": 0.6, "skittish": 0.2},
    "serio":      {"playful": 0.15, "calm": 0.9, "curious": 0.6, "sleepy": 0.3, "loving": 0.3, "skittish": 0.1},
}
PERSONALITY_LABELS = {
    "brincalhao": "Brincalhão", "calmo": "Calmo", "curioso": "Curioso", "dorminhoco": "Dorminhoco", "serio": "Sério",
}

# Emotions that are "work states": the brain never interrupts or relaxes them.
BUSY_EMOTIONS = ("pensando", "orquestrando", "foto", "ouvindo", "sabio", "apontar", "determinado")
# Reply emotions fade back to idle after a while (seconds).
RELAX_AFTER = {"feliz": 9, "bravo": 10, "triste": 12, "alerta": 8, "confuso": 9, "surpreso": 6, "amor": 9,
               "rindo": 7, "orgulhoso": 9, "envergonhado": 9, "medo": 8, "curioso": 9, "entediado": 12,
               "comemorando": 6, "tonto": 6}

FORM_MOODS = {            # which mood each form suits
    "humano": "calm", "alado": "proud",
}


def _clamp(v: float, lo: float = 0.0, hi: float = 1.0) -> float:
    return max(lo, min(hi, v))


class PetBrain:
    def __init__(self, personality: str = "brincalhao", activity: float = 0.6, free_will: bool = True,
                 allow_walking: bool = True, mood_path: Optional[str] = MOOD_PATH,
                 rng: Optional[random.Random] = None, now: Optional[float] = None):
        self.rng = rng or random.Random()
        self.mood_path = mood_path
        self.set_personality(personality)
        self.activity = _clamp(float(activity), 0.0, 1.0)
        self.free_will = bool(free_will)
        self.allow_walking = bool(allow_walking)
        now = time.time() if now is None else now
        self.energy = 0.8
        self.joy = 0.6
        self.boredom = 0.2
        self.curiosity = 0.4
        self.affection = 0.3
        self.pets_total = 0
        self._load()
        self._last_tick = now
        self._next_action = now + self.rng.uniform(4, 9)
        self._next_look = now + 3
        self._next_form = now + self.rng.uniform(150, 300)
        self._cooldown: Dict[str, float] = {}
        self._cursor: List[Tuple[float, float, float]] = []      # (t, x, y)
        self._user_was_away = False
        self._recent: List[str] = []
        self._last_save = now
        self._greeted = False
        self.asleep = False

    # ------------------------------------------------------------ setup
    def set_personality(self, name: str) -> None:
        self.personality = name if name in PERSONALITIES else "brincalhao"
        self.traits = dict(PERSONALITIES[self.personality])

    def _load(self) -> None:
        if not self.mood_path:
            return
        try:
            with open(self.mood_path, encoding="utf-8") as f:
                data = json.load(f)
            self.affection = _clamp(float(data.get("affection", self.affection)))
            self.pets_total = int(data.get("pets_total", 0))
        except Exception:
            pass

    def save(self) -> None:
        if not self.mood_path:
            return
        try:
            os.makedirs(os.path.dirname(self.mood_path), exist_ok=True)
            with open(self.mood_path, "w", encoding="utf-8") as f:
                json.dump({"affection": round(self.affection, 3), "pets_total": self.pets_total,
                           "saved_at": time.strftime("%Y-%m-%d %H:%M:%S")}, f)
        except Exception:
            pass

    def snapshot(self) -> Dict:
        return {"personality": self.personality, "energy": round(self.energy, 2), "joy": round(self.joy, 2),
                "boredom": round(self.boredom, 2), "curiosity": round(self.curiosity, 2),
                "affection": round(self.affection, 2), "asleep": self.asleep, "pets_total": self.pets_total,
                "mood": self.mood_label()}

    def mood_label(self) -> str:
        if self.asleep:
            return "dormindo"
        if self.energy < 0.25:
            return "sonolento"
        if self.joy > 0.75:
            return "radiante"
        if self.boredom > 0.7:
            return "entediado"
        if self.joy < 0.3:
            return "pra baixo"
        if self.curiosity > 0.7:
            return "curioso"
        return "tranquilo"

    # ------------------------------------------------------------ events
    def on_event(self, name: str, now: Optional[float] = None) -> Optional[Dict]:
        """Things that happen to the pet. May return an immediate action."""
        now = time.time() if now is None else now
        if name == "petted":
            self.affection = _clamp(self.affection + 0.04)
            self.joy = _clamp(self.joy + 0.15)
            self.boredom = _clamp(self.boredom - 0.3)
            self.pets_total += 1
            self.save()
        elif name == "drag_start":
            self.curiosity = _clamp(self.curiosity + 0.1)
            self.boredom = _clamp(self.boredom - 0.2)
        elif name == "drop":
            self.joy = _clamp(self.joy + 0.03)
        elif name == "task_done":
            self.joy = _clamp(self.joy + 0.2)
            self.boredom = _clamp(self.boredom - 0.4)
        elif name == "task_fail":
            self.joy = _clamp(self.joy - 0.2)
        elif name == "interaction":             # the user talked to it
            self.boredom = _clamp(self.boredom - 0.35)
            self.curiosity = _clamp(self.curiosity + 0.1)
            self.energy = _clamp(self.energy + 0.05)
            self._next_action = max(self._next_action, now + 6)
        elif name.startswith("emotion:"):
            emo = name.split(":", 1)[1]
            delta = {"feliz": 0.1, "comemorando": 0.2, "amor": 0.15, "rindo": 0.15, "orgulhoso": 0.1,
                     "triste": -0.15, "bravo": -0.1, "medo": -0.1}.get(emo, 0.0)
            self.joy = _clamp(self.joy + delta)
        return None

    # ------------------------------------------------------------ helpers
    def _ready(self, key: str, now: float) -> bool:
        return now >= self._cooldown.get(key, 0.0)

    def _cool(self, key: str, now: float, seconds: float) -> None:
        self._cooldown[key] = now + seconds

    def _interval(self) -> float:
        base = 24.0 - 19.0 * self.activity                 # 24 s (lazy) .. 5 s (hyper)
        return base * self.rng.uniform(0.6, 1.5) * (1.4 if self.energy < 0.3 else 1.0)

    def _pick(self, options: List[Tuple[str, float]]) -> Optional[str]:
        opts = [(n, w * (0.25 if n in self._recent[-3:] else 1.0)) for n, w in options if w > 0.001]
        total = sum(w for _, w in opts)
        if total <= 0:
            return None
        r = self.rng.uniform(0, total)
        for n, w in opts:
            r -= w
            if r <= 0:
                return n
        return opts[-1][0]

    def _remember(self, name: str) -> None:
        self._recent.append(name)
        self._recent = self._recent[-6:]

    @staticmethod
    def is_night(hour: int) -> bool:
        return hour >= 23 or hour < 6

    # ------------------------------------------------------------ main loop
    def tick(self, now: float, ctx: Dict) -> Optional[Dict]:
        """Returns the next action, or None.

        ctx keys: cursor (x, y) | None, pet (cx, cy), free (bool: nothing else is going on),
        user_idle (s), hour (0-23), form, emotion, emotion_age (s), speaking (bool),
        actions (set of animation names available for the current form), forms (list).
        """
        dt = max(0.0, min(5.0, now - self._last_tick))
        self._last_tick = now
        t = self.traits
        hour = int(ctx.get("hour", 12))
        night = self.is_night(hour)

        # mood drift
        if self.asleep:
            self.energy = _clamp(self.energy + dt / 420.0)
            self.boredom = _clamp(self.boredom - dt / 200.0)
        else:
            drain = (1.0 / 2700.0) * (1.8 if night else 1.0) * (0.7 + 0.8 * t["sleepy"])
            self.energy = _clamp(self.energy - dt * drain)
            self.boredom = _clamp(self.boredom + dt / (160.0 - 90.0 * t["playful"]))
        self.joy += (0.55 - self.joy) * min(1.0, dt / 240.0)
        self.curiosity = _clamp(self.curiosity - dt / 150.0 + dt / 400.0 * t["curious"])
        if now - self._last_save > 300:
            self._last_save = now
            self.save()

        # cursor memory
        cur = ctx.get("cursor")
        if cur:
            self._cursor.append((now, float(cur[0]), float(cur[1])))
        self._cursor = [c for c in self._cursor if now - c[0] <= 1.6][-80:]

        user_idle = float(ctx.get("user_idle", 0.0))
        actions = ctx.get("actions") or ()
        emotion = ctx.get("emotion", "idle")

        def can(name: str) -> bool:
            return name in actions

        # reply emotions fade back to idle on their own
        if (not ctx.get("speaking") and emotion in RELAX_AFTER and not self.asleep
                and float(ctx.get("emotion_age", 0.0)) > RELAX_AFTER[emotion]):
            return {"type": "relax"}

        if not ctx.get("free"):
            return None

        # ---- sleeping -------------------------------------------------
        if self.asleep:
            if user_idle < 1.5 and self._user_was_away and self.energy > 0.35:
                self.asleep = False
                self._user_was_away = False
                self._next_action = now + 5
                self._cool("greet", now, 600)
                return {"type": "wake", "then": "acenar" if t["loving"] > 0.4 else None}
            if user_idle >= 30:
                self._user_was_away = True
            if self.energy >= 0.98 and user_idle < 120:
                self.asleep = False
                self._next_action = now + 4
                return {"type": "wake", "then": "espreguicar"}
            return None

        # ---- the user went away / came back -----------------------------
        if user_idle >= 120:
            self._user_was_away = True
        elif self._user_was_away and user_idle < 1.5:
            self._user_was_away = False
            if self._ready("greet", now):
                self._cool("greet", now, 600)
                self.joy = _clamp(self.joy + 0.1)
                self._next_action = now + 6
                return {"type": "anim", "name": "acenar"} if can("acenar") else {"type": "emotion", "name": "feliz", "seconds": 3}

        sleep_after = float(ctx.get("sleep_after", 600) or 0)
        sleepy = self.energy < 0.18 or (sleep_after and user_idle > sleep_after * (0.5 if night else 1.0)
                                        and float(ctx.get("pet_idle", 0.0)) > sleep_after * 0.5)
        if sleepy and ctx.get("form", "slime") == "slime":
            self.asleep = True
            return {"type": "sleep"}

        # ---- reacting to the mouse --------------------------------------
        pet = ctx.get("pet")
        if cur and pet and len(self._cursor) >= 3:
            dx, dy = cur[0] - pet[0], cur[1] - pet[1]
            dist = math.hypot(dx, dy)
            path = sum(math.hypot(b[1] - a[1], b[2] - a[2]) for a, b in zip(self._cursor, self._cursor[1:]))
            span = max(0.05, self._cursor[-1][0] - self._cursor[0][0])
            speed = path / span
            near_all = all(math.hypot(c[1] - pet[0], c[2] - pet[1]) < 95 for c in self._cursor)
            if near_all and path > 260 and span > 0.7 and self._ready("pet", now):
                self._cool("pet", now, 5)
                self._cool("look", now, 6)
                self.on_event("petted", now)
                self._next_action = now + 5
                return {"type": "anim", "name": "cafune", "loops": 2} if can("cafune") else \
                    {"type": "emotion", "name": "amor", "seconds": 3}
            if (speed > 2600 and dist < 150 and self._ready("startle", now)
                    and self.rng.random() < 0.35 + 0.5 * t["skittish"]):
                self._cool("startle", now, 25)
                self._cool("look", now, 5)
                self._next_action = now + 5
                return {"type": "emotion", "name": "surpreso", "seconds": 1.6}
            if dist < 340 and speed > 40 and now >= self._next_look and self._ready("look", now):
                self._next_look = now + self.rng.uniform(3.5, 9.0) / (0.5 + t["curious"])
                self.curiosity = _clamp(self.curiosity + 0.05)
                if dist < 70:
                    return {"type": "anim", "name": "piscadela"} if (can("piscadela") and self.rng.random() < 0.3 * t["loving"]) else None
                if abs(dy) > abs(dx) * 1.3 and dy < 0 and can("olhar_cima"):
                    return {"type": "anim", "name": "olhar_cima"}
                name = "olhar_dir" if dx > 0 else "olhar_esq"
                if can(name):
                    return {"type": "anim", "name": name}

        if now < self._next_action:
            return None
        self._next_action = now + self._interval()

        # ---- changing form on a whim ---------------------------------------
        if (self.free_will and ctx.get("form", "slime") == "slime" and now >= self._next_form
                and self.boredom > 0.45 and self.energy > 0.4):
            forms = [f for f in (ctx.get("forms") or []) if f != "slime"]
            if forms:
                if self.energy > 0.7 and t["playful"] > 0.5:
                    want = "energetic"
                elif self.joy > 0.7:
                    want = "proud"
                elif self.boredom > 0.75:
                    want = "mischief"
                else:
                    want = "calm"
                pool = [f for f in forms if FORM_MOODS.get(f) == want] or forms
                form = self.rng.choice(pool)
                self._next_form = now + self.rng.uniform(240, 600) / (0.4 + self.activity)
                self.boredom = _clamp(self.boredom - 0.5)
                self._next_action = now + 8
                return {"type": "form", "name": form, "hold": self.rng.randint(22, 45),
                        "roam": want == "energetic"}

        # ---- pick something to do -----------------------------------------
        e, j, b, c, a = self.energy, self.joy, self.boredom, self.curiosity, self.affection
        if ctx.get("form", "slime") != "slime":
            choice = self._pick([("walk", 2.0 * e if self.allow_walking else 0), ("run", 1.5 * e * t["playful"] if self.allow_walking else 0),
                                 ("emo:feliz", 0.6 * j), ("emo:orgulhoso", 0.5 * j), ("emo:curioso", 0.6 * c), ("none", 1.0)])
        else:
            tired = 1.0 - e
            choice = self._pick([
                ("bocejo", (1.6 * tired + (0.8 if night else 0)) * (0.5 + t["sleepy"]) if can("bocejo") else 0),
                ("espreguicar", 0.5 + 0.8 * tired if can("espreguicar") else 0),
                ("quicar", 1.4 * e * t["playful"] if can("quicar") else 0),
                ("danca", 1.3 * j * e * t["playful"] if can("danca") else 0),
                ("bolha", 0.5 + 1.2 * b * (0.4 + t["playful"]) if can("bolha") else 0),
                ("derreter", 0.9 * b * (0.4 + t["calm"]) + 0.6 * tired if can("derreter") else 0),
                ("espiar", 1.1 * c * (0.4 + t["playful"]) if can("espiar") else 0),
                ("piscadela", 0.9 * a * t["loving"] if can("piscadela") else 0),
                ("predador", 0.5 * e * t["playful"] if can("predador") else 0),
                ("acenar", 0.5 * a * t["loving"] if (can("acenar") and user_idle < 20) else 0),
                ("olhar_esq", 0.5 * c if can("olhar_esq") else 0),
                ("olhar_dir", 0.5 * c if can("olhar_dir") else 0),
                ("olhar_cima", 0.4 * c if can("olhar_cima") else 0),
                ("emo:entediado", 1.5 * max(0.0, b - 0.55) * (1.2 - t["playful"] * 0.5)),
                ("emo:curioso", 0.8 * c * t["curious"]),
                ("emo:feliz", 0.6 * max(0.0, j - 0.6)),
                ("emo:orgulhoso", 0.5 * max(0.0, j - 0.7)),
                ("walk", (1.6 * e + 0.8 * b) * (0.4 + t["playful"]) if self.allow_walking else 0),
                ("run", 0.9 * e * e * t["playful"] if self.allow_walking else 0),
                ("none", 0.8 + 1.5 * t["calm"]),
            ])
        if not choice or choice == "none":
            return None
        self._remember(choice)
        self.boredom = _clamp(self.boredom - 0.18)
        if choice in ("walk", "run"):
            self.energy = _clamp(self.energy - (0.01 if choice == "walk" else 0.025))
            return {"type": "walk", "run": choice == "run"}
        if choice.startswith("emo:"):
            return {"type": "emotion", "name": choice[4:], "seconds": self.rng.uniform(3.5, 6.0)}
        if choice in ("quicar", "danca", "predador"):
            self.energy = _clamp(self.energy - 0.02)
        loops = 2 if choice in ("danca",) else 1
        return {"type": "anim", "name": choice, "loops": loops}
