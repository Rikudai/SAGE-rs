"""
S.A.G.E. - Desktop Companion Overlay
==================================
Always-on-top borderless window that renders the animated character.

Features
--------
* Sprites from the generator (sage.sprite_gen): a procedural slime with a face
  rig, plus the sheet forms humano e alado
  with a fluid silhouette morph between them.
* Per-pixel alpha (sage.layered): translucent gel, soft shadow, configurable
  whole-pet opacity. Falls back to a colour-key canvas when unavailable.
* A brain of its own (sage.pet_brain): mood, personality, reactions to the
  mouse and to the user's presence, spontaneous fidgets, walks and forms.
* Worker clones: one small window per delegated agent (spawn -> work -> vanish).
* A "leash": the character only wanders inside a configurable radius around
  the spot where the user dropped it.
* Screen-pointing mode: teleports next to a target pixel and points at it,
  then returns home.
* Drag & drop (it dangles, then lands), double-click petting.
"""
from __future__ import annotations

import ctypes
import math
import os
import random
import sys
import threading
import time
import tkinter as tk
from typing import Callable, Dict, List, Optional

from PIL import Image, ImageTk

from sage import clone_layout, layered, pixel_avatar, sprite_art
from sage.pet_brain import PetBrain
from sage.character_assets import (
    FORMS_DIR,
    POINTER_DIR,
    animation_paths,
    ensure_assets,
    form_dir,
    load_pointer_file_paths,
)

EMOTIONS = list(pixel_avatar.EMOTIONS)
FORMS = list(pixel_avatar.FORMS)
ALL_ANIMS = tuple(pixel_avatar.ALL_ANIMS)


def ensure_avatar_assets(verbose: bool = False) -> None:
    """Builds the pixel avatar from the source sheets if frames are missing."""
    try:
        if pixel_avatar.sources_available():
            pixel_avatar.build_all(force=False, verbose=verbose)
            return
    except Exception as e:
        print("[Pet] Falha ao construir avatar pixel: " + str(e))
    try:
        ensure_assets(verbose=verbose)
    except Exception:
        pass

def _photo(img: Image.Image):
    """ImageTk wrapper that degrades to a PIL image when Tk is unavailable."""
    try:
        return ImageTk.PhotoImage(img)
    except Exception:
        return img


ANIM_FRAME_MS = {
    "idle": 120, "walk": 90, "run": 70, "talk": 95, "feliz": 95, "bravo": 110,
    "triste": 130, "pensando": 150, "alerta": 110, "confuso": 120, "foto": 80,
    "apontar": 110, "teleport": 45, "transform": 55,
}


# --------------------------------------------------------------------------
# One-time compatibility sprites (used by tests / legacy integrations)
# --------------------------------------------------------------------------
def ensure_placeholder_sprites(sprites_dir: str, size: int = 64) -> None:
    """Generates the legacy static sprite set using the new art engine."""
    os.makedirs(sprites_dir, exist_ok=True)
    slime_sub = os.path.join(sprites_dir, "slime")
    os.makedirs(slime_sub, exist_ok=True)

    for emotion in list(sprite_art.EMOTIONS) + ["apontar"]:
        for is_talk in (False, True):
            suffix = "_talk" if is_talk else ""
            filepath = os.path.join(sprites_dir, emotion + suffix + ".png")
            if not os.path.exists(filepath):
                anim = "talk" if is_talk else emotion
                frame = sprite_art.render_frame("slime", anim, 1 if is_talk else 0,
                                                sprite_art.FRAME_COUNTS.get(anim, 8), size=size)
                frame.save(filepath, "PNG")

        frame_0 = os.path.join(slime_sub, emotion + "_0.png")
        if not os.path.exists(frame_0):
            frame = sprite_art.render_frame("slime", emotion, 0,
                                            sprite_art.FRAME_COUNTS.get(emotion, 8), size=size)
            frame.save(frame_0, "PNG")


# --------------------------------------------------------------------------
# Pointer overlay (click-through marker)
# --------------------------------------------------------------------------
class PointerOverlay:
    """Pulsing target ring that calls the user's attention to a screen point."""

    def __init__(self, root: tk.Tk, size: int = 320):
        self.root = root
        self.size = size
        self._frames: List[ImageTk.PhotoImage] = []
        self._paths: List[str] = []
        self._job: Optional[str] = None
        self._idx = 0

        self.window = tk.Toplevel(root)
        self.window.overrideredirect(True)
        self.window.attributes("-topmost", True)
        trans = "#06101F"
        self._trans_color = trans
        self.window.configure(bg=trans)
        try:
            self.window.wm_attributes("-transparentcolor", trans)
        except Exception:
            pass

        self.canvas = tk.Canvas(self.window, width=size, height=size,
                                bg=trans, highlightthickness=0, bd=0)
        self.canvas.pack(fill=tk.BOTH, expand=True)
        self._item = self.canvas.create_image(0, 0, anchor=tk.NW)
        self._label_photo = None
        self._label_item = None
        self.window.withdraw()
        self.window.update_idletasks()
        self._make_click_through()

    def _make_click_through(self) -> None:
        """Adds WS_EX_TRANSPARENT without breaking Tk's color-key layered window."""
        if not sys.platform.startswith("win"):
            return
        try:
            hwnd = int(self.window.winfo_id())
            user32 = ctypes.windll.user32
            GWL_EXSTYLE = -20
            style = user32.GetWindowLongW(hwnd, GWL_EXSTYLE)
            style |= 0x00000020    # WS_EX_TRANSPARENT (mouse passes through)
            style |= 0x08000000    # WS_EX_NOACTIVATE
            style |= 0x00000080    # WS_EX_TOOLWINDOW
            user32.SetWindowLongW(hwnd, GWL_EXSTYLE, style)
            # re-apply the color key: changing ex-style can drop the layer attrs
            self.window.wm_attributes("-transparentcolor", self._trans_color)
        except Exception:
            pass

    def _load(self) -> None:
        paths = load_pointer_file_paths()
        self._frames = []
        for path in paths:
            frame = Image.open(path).convert("RGBA").resize(
                (self.size, self.size), Image.Resampling.LANCZOS)
            self._frames.append(_photo(frame))

    def _set_label(self, label: Optional[str], ring_cx: Optional[float] = None,
                   ring_cy: Optional[float] = None) -> None:
        if self._label_item is not None:
            try:
                self.canvas.delete(self._label_item)
            except Exception:
                pass
            self._label_item = None
        self._label_photo = None
        if not label:
            return
        try:
            pill = sprite_art.render_label_pill(label, font_px=max(15, self.size // 14),
                                                max_width=int(self.size * 0.94))
            self._label_photo = _photo(pill)
            cx = self.size / 2 if ring_cx is None else ring_cx
            cy = self.size * 0.80 if ring_cy is None else ring_cy
            label_y = cy + self.size * 0.34
            if label_y + pill.height / 2 > self.size - 4:
                label_y = cy - self.size * 0.33
            label_y = max(pill.height / 2 + 4, min(self.size - pill.height / 2 - 4, label_y))
            label_x = max(pill.width / 2 + 4, min(self.size - pill.width / 2 - 4, cx))
            self._label_item = self.canvas.create_image(label_x, label_y,
                                                         image=self._label_photo,
                                                         anchor=tk.CENTER)
        except Exception:
            pass

    def show_at(self, x: int, y: int, label: Optional[str] = None) -> None:
        if not self._frames:
            self._load()
        try:
            screen_w = int(self.root.winfo_screenwidth())
            screen_h = int(self.root.winfo_screenheight())
        except Exception:
            screen_w, screen_h = 1920, 1080
        half = self.size // 2
        # keep the window fully on screen while the ring stays exactly on target
        win_x = int(max(0, min(screen_w - self.size, x - half)))
        win_y = int(max(0, min(screen_h - self.size, y - half)))
        self.window.geometry(str(self.size) + "x" + str(self.size) + "+" +
                             str(win_x) + "+" + str(win_y))
        ring_cx = float(x - win_x)
        ring_cy = float(y - win_y)
        try:
            self.canvas.coords(self._item, ring_cx - half, ring_cy - half)
        except Exception:
            pass
        self._set_label(label, ring_cx, ring_cy)
        self.window.deiconify()
        self.window.lift()
        self._make_click_through()
        self._idx = 0
        self._animate()

    def _animate(self) -> None:
        if not self._frames:
            return
        try:
            self.canvas.itemconfig(self._item, image=self._frames[self._idx % len(self._frames)])
            self._idx += 1
            self._job = self.root.after(55, self._animate)
        except Exception:
            pass

    def hide(self) -> None:
        if self._job:
            try:
                self.root.after_cancel(self._job)
            except Exception:
                pass
            self._job = None
        try:
            self.window.withdraw()
        except Exception:
            pass

    def destroy(self) -> None:
        self.hide()
        try:
            self.window.destroy()
        except Exception:
            pass


# --------------------------------------------------------------------------
# Frame loading (lazy, per form)
# --------------------------------------------------------------------------
def blend_frames(a, b, k: float):
    """Crossfade between two layered frames ``(premultiplied BGRA, (x, y))``.

    The frames may be cropped to different boxes; the result covers both. ``k`` = 0 gives
    ``a``, 1 gives ``b``. Premultiplied pixels blend linearly, so no halo appears.
    """
    import numpy as np
    k = max(0.0, min(1.0, float(k)))
    (pa, (ax, ay)), (pb, (bx, by)) = a, b
    x0, y0 = min(ax, bx), min(ay, by)
    x1 = max(ax + pa.shape[1], bx + pb.shape[1])
    y1 = max(ay + pa.shape[0], by + pb.shape[0])
    acc = np.zeros((y1 - y0, x1 - x0, 4), dtype=np.float32)
    acc[ay - y0:ay - y0 + pa.shape[0], ax - x0:ax - x0 + pa.shape[1]] += pa.astype(np.float32) * (1.0 - k)
    acc[by - y0:by - y0 + pb.shape[0], bx - x0:bx - x0 + pb.shape[1]] += pb.astype(np.float32) * k
    return (acc + 0.5).clip(0, 255).astype(np.uint8), (x0, y0)


def _binary_alpha(img: Image.Image) -> Image.Image:
    """Colour-key windows cannot blend: snap alpha to on/off (fallback mode only)."""
    r, g, b, a = img.split()
    return Image.merge("RGBA", (r, g, b, a.point(lambda v: 255 if v >= 110 else 0)))


class FrameStore:
    """Animations of one form, decoded on first use.

    In layered mode a frame is ``(premultiplied BGRA array, (x, y))`` cropped to the
    animation's bounding box; in fallback mode it is a Tk PhotoImage. Only a handful
    of animations stay decoded (LRU), the common ones are pinned.
    """

    PINNED = ("idle", "talk", "walk", "pensando", "ouvindo")
    KEEP = 12

    def __init__(self, form: str, layered_mode: bool, scale: float = 1.0):
        self.form = form
        self.layered = layered_mode
        self.scale = float(scale)
        directory = pixel_avatar.clone_dir_or_form(form)
        self._names = [a for a in ALL_ANIMS if os.path.exists(os.path.join(directory, a + "_0.png"))]
        self._cache: Dict[tuple, List] = {}
        self._order: List[tuple] = []
        self._lock = threading.RLock()

    def __contains__(self, anim) -> bool:
        return anim in self._names

    def __bool__(self) -> bool:
        return bool(self._names)

    def __iter__(self):
        return iter(self._names)

    def keys(self):
        return list(self._names)

    def __getitem__(self, anim: str) -> List:
        if anim not in self._names:
            raise KeyError(anim)
        return self._frames(anim, False)

    def get(self, anim: str, default=None):
        return self._frames(anim, False) if anim in self._names else default

    def flipped(self, anim: str) -> List:
        return self._frames(anim, True) if anim in self._names else []

    def _frames(self, anim: str, flip: bool) -> List:
        key = (anim, flip)
        with self._lock:
            frames = self._cache.get(key)
            if frames is not None:
                if key in self._order:
                    self._order.remove(key)
                    self._order.append(key)
                return frames
        frames = self._decode(anim, flip)
        with self._lock:
            self._cache[key] = frames
            self._order.append(key)
            loose = [k for k in self._order if k[0] not in self.PINNED]
            while len(loose) > self.KEEP:
                old = loose.pop(0)
                self._order.remove(old)
                self._cache.pop(old, None)
        return frames

    def _decode(self, anim: str, flip: bool) -> List:
        imgs = []
        for p in pixel_avatar.animation_paths(self.form, anim):
            try:
                with Image.open(p) as im:
                    img = im.convert("RGBA")
            except Exception:
                continue
            if abs(self.scale - 1.0) > 0.01:
                img = img.resize((max(1, int(round(img.width * self.scale))),
                                  max(1, int(round(img.height * self.scale)))), Image.NEAREST)
            if flip:
                img = img.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
            imgs.append(img)
        if not imgs:
            return []
        if not self.layered:
            return [_photo(_binary_alpha(i)) for i in imgs]
        # crop the whole animation to one shared box (keeps memory small)
        x0, y0, x1, y1 = imgs[0].width, imgs[0].height, 0, 0
        for im in imgs:
            box = im.getbbox()
            if box:
                x0, y0, x1, y1 = min(x0, box[0]), min(y0, box[1]), max(x1, box[2]), max(y1, box[3])
        if x1 <= x0 or y1 <= y0:
            x0, y0, x1, y1 = 0, 0, 1, 1
        return [(layered.premultiply(im.crop((x0, y0, x1, y1))), (x0, y0)) for im in imgs]

    def preload(self, anims) -> None:
        for a in anims:
            try:
                self.get(a)
            except Exception:
                pass


def _user_idle_seconds() -> float:
    """Seconds since the last keyboard/mouse input anywhere in Windows."""
    if not sys.platform.startswith("win"):
        return 0.0
    try:
        class LASTINPUTINFO(ctypes.Structure):
            _fields_ = [("cbSize", ctypes.c_uint), ("dwTime", ctypes.c_uint)]
        info = LASTINPUTINFO()
        info.cbSize = ctypes.sizeof(info)
        if not ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info)):
            return 0.0
        tick = ctypes.windll.kernel32.GetTickCount()
        return max(0.0, ((tick - info.dwTime) & 0xFFFFFFFF) / 1000.0)
    except Exception:
        return 0.0


# --------------------------------------------------------------------------
# Worker clones (visual side of the multi-agent brain)
# --------------------------------------------------------------------------
CLONE_TRANS = "#05162B"


def _make_click_through(window, trans_color: str, click_through: bool = True) -> None:
    if not sys.platform.startswith("win"):
        return
    try:
        hwnd = int(window.winfo_id())
        user32 = ctypes.windll.user32
        style = user32.GetWindowLongW(hwnd, -20)
        style |= 0x08000000 | 0x00000080
        if click_through:
            style |= 0x00000020
        user32.SetWindowLongW(hwnd, -20, style)
        window.wm_attributes("-transparentcolor", trans_color)
    except Exception:
        pass


def _label_image(text: str, width: int, height: int) -> Image.Image:
    """Name tag under a clone (drawn by us so it can live in a per-pixel alpha window)."""
    from PIL import ImageDraw, ImageFont
    img = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    if not text:
        return img
    d = ImageDraw.Draw(img)
    font = None
    for name in ("consolab.ttf", "consola.ttf", "seguisb.ttf"):
        try:
            font = ImageFont.truetype(name, 11)
            break
        except Exception:
            continue
    if font is None:
        font = ImageFont.load_default()
    box = d.textbbox((0, 0), text, font=font)
    tw, th = box[2] - box[0], box[3] - box[1]
    x0 = max(1, (width - tw) // 2 - 6)
    x1 = min(width - 2, x0 + tw + 12)
    d.rounded_rectangle((x0, 2, x1, height - 3), radius=6, fill=(14, 36, 64, 225), outline=(110, 230, 255, 255))
    d.text(((x0 + x1) // 2 - tw // 2 - box[0], (height - th) // 2 - box[1] - 1), text, fill=(223, 247, 255, 255), font=font)
    return img


DRAG_THRESHOLD_SQ = 5 * 5   # movimento menor que 5 px ainda é clique, não arrasto


class CloneWindow:
    """One worker clone: spawns, works with a holo-screen, then vanishes.

    Dá para arrastar com o botão esquerdo: a janela segue o mouse, fica presa
    na área visível dos monitores e, ao soltar, avisa ``on_dropped`` (para
    salvar a posição do role).
    """

    LABEL_H = 22

    def __init__(self, root, clone_id: str, label: str, x: int, y: int,
                 frames: Dict[str, List], frame_size=(128, 116), images: Optional[Dict[str, List]] = None,
                 opacity: float = 1.0, role: str = "",
                 screens_fn: Optional[Callable[[], list]] = None,
                 on_dropped: Optional[Callable[[str, int, int], None]] = None):
        self.root = root
        self.clone_id = clone_id
        self.label = label
        self.role = role or ""
        self.frames = frames
        self.images = images or {}
        self.opacity = opacity
        self.state = "spawn"
        self._idx = 0
        self._job = None
        self._on_gone: Optional[Callable] = None
        self._alive = True
        self._composed: Dict[tuple, object] = {}
        self._screens_fn = screens_fn or (lambda: [])
        self._on_dropped = on_dropped
        self._drag: Optional[tuple] = None
        self._drag_moved = False
        self._drag_screens: list = []
        w, h = frame_size
        self.frame_w, self.frame_h = w, h
        self.width = max(w, 150)
        self.height = h + self.LABEL_H
        self.x, self.y = int(x), int(y)
        self.window = tk.Toplevel(root)
        self.window.overrideredirect(True)
        self.window.attributes("-topmost", True)
        self.window.configure(bg=CLONE_TRANS, cursor="fleur")
        self.window.geometry("%dx%d+%d+%d" % (self.width, self.height, x, y))
        self._surface = layered.LayeredSurface.create(self.window, self.width, self.height) \
            if self.images else None
        self.canvas = None
        if self._surface is None:
            try:
                self.window.wm_attributes("-transparentcolor", CLONE_TRANS)
            except Exception:
                pass
            self.canvas = tk.Canvas(self.window, width=self.width, height=self.height,
                                    bg=CLONE_TRANS, highlightthickness=0, bd=0, cursor="fleur")
            self.canvas.pack(fill=tk.BOTH, expand=True)
            self._sprite = self.canvas.create_image(self.width // 2, h, anchor=tk.S)
            self._label_bg = self.canvas.create_rectangle(0, 0, 0, 0, fill="#0E2440", outline="#6EE6FF")
            self._label_txt = self.canvas.create_text(self.width // 2, h + self.LABEL_H // 2,
                                                      text="", fill="#DFF7FF",
                                                      font=("Consolas", 8, "bold"))
        self.set_label(label)
        if self._surface is None:
            self.window.update_idletasks()
            _make_click_through(self.window, CLONE_TRANS, click_through=False)
        self.window.bind("<ButtonPress-1>", self._on_press)
        self.window.bind("<B1-Motion>", self._on_motion)
        self.window.bind("<ButtonRelease-1>", self._on_release)
        self._tick()

    def _on_press(self, event) -> None:
        self._drag = (event.x_root, event.y_root, self.window.winfo_x(), self.window.winfo_y())
        self._drag_moved = False
        try:
            self._drag_screens = list(self._screens_fn())
        except Exception:
            self._drag_screens = []

    def _on_motion(self, event) -> None:
        if self._drag is None:
            return
        sx, sy, ox, oy = self._drag
        dx, dy = event.x_root - sx, event.y_root - sy
        if not self._drag_moved:
            if dx * dx + dy * dy <= DRAG_THRESHOLD_SQ:
                return
            self._drag_moved = True
        x, y = clone_layout.clamp_to_screens(ox + dx, oy + dy, self.width, self.height, self._drag_screens)
        self.x, self.y = x, y
        try:
            self.window.geometry("+%d+%d" % (x, y))
        except Exception:
            pass

    def _on_release(self, event) -> None:
        moved = self._drag_moved
        self._drag = None
        self._drag_moved = False
        if not moved:
            return  # clique simples: nada muda
        x, y = self.x, self.y
        if self._on_dropped:
            try:
                self._on_dropped(self.clone_id, x, y)
            except Exception:
                pass

    def set_label(self, text: str) -> None:
        self.label = (text or "")[:26]
        self._composed.clear()
        self._label_img = None
        if self.canvas is None:
            return
        try:
            self.canvas.itemconfig(self._label_txt, text=self.label)
            bbox = self.canvas.bbox(self._label_txt)
            if bbox:
                self.canvas.coords(self._label_bg, bbox[0] - 5, bbox[1] - 2, bbox[2] + 5, bbox[3] + 2)
        except Exception:
            pass

    def set_state(self, state: str, on_gone: Optional[Callable] = None) -> None:
        if state not in self.frames:
            return
        self.state = state
        self._idx = 0
        if on_gone:
            self._on_gone = on_gone

    def _layered_frame(self, state: str, idx: int):
        key = (state, idx)
        px = self._composed.get(key)
        if px is None:
            if self._label_img is None:
                self._label_img = _label_image(self.label, self.width, self.LABEL_H)
            canvas = Image.new("RGBA", (self.width, self.height), (0, 0, 0, 0))
            canvas.alpha_composite(self.images[state][idx], ((self.width - self.frame_w) // 2, 0))
            if state != "vanish":
                canvas.alpha_composite(self._label_img, (0, self.frame_h))
            px = layered.premultiply(canvas)
            self._composed[key] = px
        return px

    def _tick(self) -> None:
        if not self._alive:
            return
        frames = self.frames.get(self.state) or self.frames.get("work") or []
        if frames:
            if self.state in ("spawn", "done", "fail", "vanish") and self._idx >= len(frames):
                if self.state == "spawn":
                    self.state, self._idx = "work", 0
                elif self.state in ("done", "fail"):
                    self.state, self._idx = "vanish", 0
                else:
                    self.destroy()
                    return
                frames = self.frames.get(self.state) or frames
            try:
                i = self._idx % len(frames)
                if self._surface is not None:
                    self._surface.show(self._layered_frame(self.state, i), self.opacity)
                else:
                    self.canvas.itemconfig(self._sprite, image=frames[i])
            except Exception:
                pass
            self._idx += 1
        ms = pixel_avatar.FRAME_MS.get(self.state, 110)
        try:
            self._job = self.root.after(ms, self._tick)
        except Exception:
            pass

    def destroy(self) -> None:
        self._alive = False
        try:
            if self._job:
                self.root.after_cancel(self._job)
        except Exception:
            pass
        if self._surface is not None:
            self._surface.close()
            self._surface = None
        try:
            self.window.destroy()
        except Exception:
            pass
        cb, self._on_gone = self._on_gone, None
        if cb:
            try:
                cb(self.clone_id)
            except Exception:
                pass


class CloneSquad:
    """Spawns and arranges worker clones around the main companion."""

    MAX_VISIBLE = 6

    def __init__(self, root, anchor: Callable[[], tuple],
                 positions: Optional[clone_layout.ClonePositionStore] = None):
        self.root = root
        self.anchor = anchor
        self.clones: Dict[str, CloneWindow] = {}
        self._frames: Optional[Dict[str, List]] = None
        self._images: Dict[str, List] = {}
        self._frame_size = (128, 116)
        self.opacity = 1.0
        self.layered = isinstance(root, tk.Misc) and sys.platform.startswith("win")
        self.positions = positions if positions is not None else clone_layout.ClonePositionStore()

    def _load(self) -> Dict[str, List]:
        if self._frames is None:
            frames: Dict[str, List] = {}
            images: Dict[str, List] = {}
            for anim in pixel_avatar.CLONE_ANIMS:
                paths = pixel_avatar.animation_paths("clone", anim)
                imgs = [Image.open(p).convert("RGBA") for p in paths]
                if imgs:
                    self._frame_size = imgs[0].size
                images[anim] = imgs
                frames[anim] = imgs if self.layered else [_photo(_binary_alpha(i)) for i in imgs]
            self._frames = frames
            self._images = images if self.layered else {}
        return self._frames

    def _screens(self) -> list:
        """Área de trabalho de cada monitor (sem barra de tarefas)."""
        try:
            sw, sh = int(self.root.winfo_screenwidth()), int(self.root.winfo_screenheight())
        except Exception:
            sw, sh = 1920, 1080
        return clone_layout.monitor_work_areas(fallback=(0, 0, sw, sh))

    def _queue_area(self, screens: list) -> tuple:
        """Área de trabalho do monitor onde está o avatar principal (ou o primeiro)."""
        ax, ay, aw, ah = self.anchor()
        cx, cy = ax + aw / 2.0, ay + ah / 2.0
        for r in screens:
            if r[0] <= cx < r[2] and r[1] <= cy < r[3]:
                return r
        return screens[0]

    def _place(self, role: str, size: tuple) -> tuple:
        """Posição de um clone novo: a salva para o role (se ainda cabe e está livre),
        senão a primeira célula livre da fila no canto, longe do avatar e dos clones vivos."""
        w, h = size
        screens = self._screens()
        live = [(c.x, c.y, c.x + c.width, c.y + c.height) for c in self.clones.values()]
        saved = self.positions.get(role)
        if saved and clone_layout.fits_on_screens(saved[0], saved[1], w, h, screens) \
                and not any(clone_layout.intersects((saved[0], saved[1], saved[0] + w, saved[1] + h), r)
                            for r in live):
            return saved
        ax, ay, aw, ah = self.anchor()
        avoid = [(ax, ay, ax + aw, ay + ah)] + live
        return clone_layout.queue_position(self._queue_area(screens), (w, h), avoid=avoid)

    def _on_dropped(self, clone_id: str, x: int, y: int) -> None:
        clone = self.clones.get(clone_id)
        if clone is not None:
            self.positions.set(clone.role, x, y)

    def spawn(self, clone_id: str, label: str, role: str = "") -> None:
        frames = self._load()
        if not frames.get("work") or clone_id in self.clones:
            return
        if len(self.clones) >= self.MAX_VISIBLE:
            return
        used = {getattr(c, "slot", -1) for c in self.clones.values()}
        slot = next(i for i in range(self.MAX_VISIBLE) if i not in used)
        w, h = max(self._frame_size[0], 150), self._frame_size[1] + CloneWindow.LABEL_H
        x, y = self._place(role, (w, h))
        clone = CloneWindow(self.root, clone_id, label, x, y, frames, self._frame_size,
                            images=self._images, opacity=self.opacity, role=role,
                            screens_fn=self._screens, on_dropped=self._on_dropped)
        clone.slot = slot
        self.clones[clone_id] = clone

    def update(self, clone_id: str, label: Optional[str] = None, state: Optional[str] = None) -> None:
        clone = self.clones.get(clone_id)
        if not clone:
            return
        if label:
            clone.set_label(label)
        if state:
            clone.set_state(state)

    def dismiss(self, clone_id: str, success: bool = True,
                on_gone: Optional[Callable[[str], None]] = None) -> None:
        clone = self.clones.get(clone_id)
        if not clone:
            if on_gone:
                on_gone(clone_id)
            return

        def _gone(cid):
            self.clones.pop(cid, None)
            if on_gone:
                on_gone(cid)
        clone.set_state("done" if success else "fail", on_gone=_gone)

    def clear(self) -> None:
        for clone in list(self.clones.values()):
            clone.destroy()
        self.clones.clear()


# --------------------------------------------------------------------------
# Main companion window
# --------------------------------------------------------------------------
class DesktopPetWindow:
    """Floating pixel-art slime companion: walks, emotes, transforms, clones.

    Rendering uses a per-pixel alpha window when possible (translucent gel, soft
    shadow, live ``opacity``); otherwise it falls back to a colour-key canvas.
    A ``PetBrain`` decides what it does when nobody is asking for anything.
    """

    # Forms whose animation changes are crossfaded (the bodies come from sheets, so a hard cut
    # between two poses reads as a flicker). Morphs and teleports are transitions already.
    CROSSFADE_FORMS = ("humano", "alado")
    CROSSFADE_MS = 140.0
    NO_CROSSFADE = ("morph_in", "morph_out", "teleport", "teleport_in", "teleport_out", "transform",
                    "forjando", "forjando_in", "forjando_out",       # as emendas da forja já são quadro a quadro
                    "orquestrando", "orquestrando_in", "orquestrando_out", "maestro", "orgulhoso_clone",
                    "bronca_clone")
    CLONE_REACTIONS = ("orgulhoso_clone", "bronca_clone")

    FLIP_ANIMS = ("walk", "run", "idle", "talk", "alerta", "apontar", "ouvindo", "arrastado", "pouso",
                  "curioso", "quicar")

    def __init__(
        self,
        root: tk.Tk,
        sprites_dir: str,
        size: int = 96,
        enable_walking: bool = True,
        leash_radius: float = 260.0,
        form: str = "slime",
        pointer_seconds: float = 9.0,
        auto_form_seconds: int = 0,
        on_teleport_sound: Optional[Callable[[], None]] = None,
        on_transform_sound: Optional[Callable[[], None]] = None,
        sleep_after_seconds: int = 600,
        form_hold_seconds: int = 45,
        opacity: float = 1.0,
        scale: float = 1.0,
        personality: str = "brincalhao",
        activity: float = 0.6,
        free_will: bool = True,
        brain_enabled: bool = True,
        on_relax: Optional[Callable[[], None]] = None,
    ):
        self.root = root
        self.sprites_dir = sprites_dir
        self.size = size
        self.enable_walking = enable_walking
        self.leash_radius = float(leash_radius)
        self.pointer_seconds = float(pointer_seconds)
        self.auto_form_seconds = int(auto_form_seconds or 0)
        self.sleep_after_seconds = int(sleep_after_seconds or 0)
        self.form_hold_seconds = int(form_hold_seconds or 0)
        self.on_teleport_sound = on_teleport_sound
        self.on_transform_sound = on_transform_sound
        self.on_relax = on_relax
        self.opacity = max(0.1, min(1.0, float(opacity or 1.0)))
        self.scale = max(0.5, min(2.0, float(scale or 1.0)))

        self.current_emotion = "idle"
        self.current_form = pixel_avatar.resolve_form(form) or "slime"
        self.is_speaking = False
        self.is_listening = False
        self.deep_thinking = False
        self.mouth_open = False
        self.is_visible = True
        self.facing_right = True
        self._last_activity = time.time()
        self._emotion_since = time.time()

        self.current_x = self.current_y = 0.0
        self.target_x = self.target_y = 0.0
        self.anchor_x = self.anchor_y = 0.0
        self.is_walking = False
        self.walk_speed = 2.6

        self._drag_start_x = self._drag_start_y = 0
        self._drag_origin_x = self._drag_origin_y = 0.0
        self._is_dragging = False
        self._drag_moved = False

        self._frame_index = 0
        self._frame_accum = 0.0
        self._last_tick = 0.0
        self._frames: FrameStore = None          # type: ignore
        self._form_cache: Dict[str, FrameStore] = {}
        self._animation_frames: FrameStore = None  # type: ignore
        self._oneshot: Optional[dict] = None
        self._shown_key = None                   # (form, animation) on screen
        self._shown_frame = None
        self._fade: Optional[dict] = None
        self._sequence_token = 0
        self._pointer: Optional[PointerOverlay] = None
        self._morphing = False
        self._pointing = False
        self._form_return_job = None
        self._surface = None
        self._last_brain = 0.0

        ensure_avatar_assets()
        self._load_ms_overrides()
        self._layered_wanted = isinstance(root, tk.Misc) and sys.platform.startswith("win")
        base_w, base_h = self._frame_size()
        self.win_w, self.win_h = int(round(base_w * self.scale)), int(round(base_h * self.scale))
        self.size = self.win_w

        screen_w, screen_h = self._screen_size()
        self.current_x = float(max(0, screen_w - self.win_w - 40))
        self.current_y = float(max(0, screen_h - self.win_h - 60))
        self.anchor_x, self.anchor_y = self.current_x, self.current_y
        self.target_x, self.target_y = self.current_x, self.current_y

        self.window = tk.Toplevel(root)
        self.window.overrideredirect(True)
        self.window.attributes("-topmost", True)
        TRANS = "#041A34"
        self.window.config(bg=TRANS)
        self.window.geometry("%dx%d+%d+%d" % (self.win_w, self.win_h,
                                              int(self.current_x), int(self.current_y)))
        self.canvas = tk.Canvas(self.window, width=self.win_w, height=self.win_h,
                                bg=TRANS, highlightthickness=0, cursor="hand2")
        self.canvas.pack(fill=tk.BOTH, expand=True)
        self._sprite_item = self.canvas.create_image(0, 0, anchor=tk.NW)
        if self._layered_wanted:
            self._surface = layered.LayeredSurface.create(self.window, self.win_w, self.win_h)
        if self._surface is None:
            try:
                self.window.wm_attributes("-transparentcolor", TRANS)
                self.window.attributes("-alpha", self.opacity)
            except Exception:
                pass

        self._load_form(self.current_form)

        self.canvas.bind("<Button-1>", self._on_drag_start)
        self.canvas.bind("<B1-Motion>", self._on_drag_motion)
        self.canvas.bind("<ButtonRelease-1>", self._on_drag_end)
        self.canvas.bind("<Double-Button-1>", self._on_double_click)

        self.clones = CloneSquad(root, anchor=lambda: (int(self.current_x), int(self.current_y),
                                                       self.win_w, self.win_h))
        self.clones.opacity = self.opacity

        self.brain: Optional[PetBrain] = None
        if brain_enabled:
            try:
                kw = {} if self._layered_wanted else {"mood_path": None}   # tests never touch data/
                self.brain = PetBrain(personality=personality, activity=activity, free_will=free_will,
                                      allow_walking=enable_walking, **kw)
            except Exception as e:
                print("[Pet] Cérebro do pet indisponível: " + str(e))

        self._render_current()
        self._last_tick = time.time()
        self._tick_loop()

        if self.enable_walking and self.brain is None:
            self.root.after(random.randint(9000, 16000), self._schedule_wander)
        if self.auto_form_seconds > 0 and (self.brain is None or not self.brain.free_will):
            self.root.after(self.auto_form_seconds * 1000, self._auto_morph_tick)

    # ------------------------------------------------------------------
    # Platform helpers
    # ------------------------------------------------------------------
    def _frame_size(self):
        paths = pixel_avatar.animation_paths(self.current_form, "idle")
        if paths:
            try:
                with Image.open(paths[0]) as img:
                    return img.size
            except Exception:
                pass
        return (pixel_avatar.CANVAS[0] * pixel_avatar.SCALE,
                pixel_avatar.CANVAS[1] * pixel_avatar.SCALE)

    def _screen_size(self):
        try:
            return int(self.root.winfo_screenwidth()), int(self.root.winfo_screenheight())
        except Exception:
            return 1920, 1080

    def _work_area(self):
        w, h = self._screen_size()
        return 0, 20, w - self.win_w, h - self.win_h - 40

    def _clamp_screen(self, x: float, y: float):
        left, top, right, bottom = self._work_area()
        return max(left, min(right, x)), max(top, min(bottom, y))

    def _clamp_to_leash(self, x: float, y: float):
        dx, dy = x - self.anchor_x, y - self.anchor_y
        dist = math.hypot(dx, dy)
        if dist > self.leash_radius and dist > 0:
            k = self.leash_radius / dist
            x, y = self.anchor_x + dx * k, self.anchor_y + dy * k
        return self._clamp_screen(x, y)

    # ------------------------------------------------------------------
    # Frame loading
    # ------------------------------------------------------------------
    def _load_form(self, form: str) -> None:
        store = self._form_cache.get(form)
        if store is None:
            store = FrameStore(form, self._surface is not None, self.scale)
            self._form_cache[form] = store
            if self._surface is not None:
                threading.Thread(target=store.preload, daemon=True,
                                 args=(("idle", "talk", "walk", "pensando", "ouvindo", "arrastado", "pouso"),)).start()
        self._frames = store
        self._animation_frames = store
        self.current_form = form

    def reload_frames(self) -> None:
        """Re-reads every frame from disk (after edits in the Sprite Studio or a rebuild)."""
        self._form_cache.clear()
        self._load_ms_overrides()
        try:
            self.clones._frames = None
        except Exception:
            pass
        self._load_form(self.current_form)
        self._render_current()

    def _load_ms_overrides(self) -> None:
        try:
            from sage.sprite_studio import load_meta
            meta = load_meta()
        except Exception:
            meta = {}
        self._ms_overrides = {k: int(v["ms"]) for k, v in meta.items() if isinstance(v, dict) and v.get("ms")}

    def _anim_ms(self, anim: str) -> int:
        override = getattr(self, "_ms_overrides", {}).get(self.current_form + "/" + anim)
        return override or pixel_avatar.FRAME_MS.get(anim, ANIM_FRAME_MS.get(anim, 110))

    def _flipped(self, anim: str) -> List:
        return self._frames.flipped(anim)

    def _present(self, anim: str, frame) -> None:
        """Shows a frame of ``anim``; a change of animation is crossfaded on the sheet forms."""
        key = (self.current_form, anim)
        prev_key, prev = self._shown_key, self._shown_frame
        self._shown_key, self._shown_frame = key, frame
        if self._surface is None or not isinstance(frame, tuple):
            self._fade = None
            self._show(frame)
            return
        if (prev_key is not None and key != prev_key and prev is not None and isinstance(prev, tuple)
                and key[0] == prev_key[0] and key[0] in self.CROSSFADE_FORMS
                and anim not in self.NO_CROSSFADE and prev_key[1] not in self.NO_CROSSFADE):
            self._fade = {"from": prev, "t0": time.time()}
        fade = self._fade
        if fade:
            k = (time.time() - fade["t0"]) * 1000.0 / self.CROSSFADE_MS
            if k < 1.0:
                try:
                    self._show(blend_frames(fade["from"], frame, k * k * (3 - 2 * k)))
                    return
                except Exception:
                    pass
            self._fade = None
        self._show(frame)

    def _show(self, frame) -> None:
        """Puts one decoded frame on screen."""
        try:
            if self._surface is not None:
                self._surface.show(frame[0], self.opacity, frame[1])
            else:
                self.canvas.itemconfig(self._sprite_item, image=frame)
        except Exception:
            pass

    # ------------------------------------------------------------------
    # Look & personality (live settings)
    # ------------------------------------------------------------------
    def set_opacity(self, opacity: float) -> None:
        """Whole-pet transparency, 0.1 (ghost) .. 1.0 (solid). Applies immediately."""
        self.opacity = max(0.1, min(1.0, float(opacity)))
        self.clones.opacity = self.opacity
        for clone in list(self.clones.clones.values()):
            clone.opacity = self.opacity

        def _apply():
            if self._surface is None:
                try:
                    self.window.attributes("-alpha", self.opacity)
                except Exception:
                    pass
            self._render_current()
        self._ui(_apply)

    def apply_settings(self, config) -> None:
        """Re-reads the avatar options from the config object (called after saving settings)."""
        self.set_opacity(getattr(config, "pet_opacity", self.opacity))
        self.enable_walking = bool(getattr(config, "enable_pet_walking", self.enable_walking))
        self.leash_radius = float(getattr(config, "pet_leash_radius", self.leash_radius))
        self.sleep_after_seconds = int(getattr(config, "pet_sleep_after_seconds", self.sleep_after_seconds) or 0)
        self.form_hold_seconds = int(getattr(config, "pet_form_hold_seconds", self.form_hold_seconds) or 0)
        if self.brain:
            self.brain.set_personality(getattr(config, "pet_personality", self.brain.personality))
            self.brain.activity = max(0.0, min(1.0, float(getattr(config, "pet_activity", self.brain.activity))))
            self.brain.free_will = bool(getattr(config, "pet_free_will", self.brain.free_will))
            self.brain.allow_walking = self.enable_walking

    def mood(self) -> Dict:
        return self.brain.snapshot() if self.brain else {}

    # ------------------------------------------------------------------
    # Emotions / speech
    # ------------------------------------------------------------------
    def update_emotion(self, emotion: str) -> None:
        clean = (emotion or "idle").lower().strip()
        if clean not in ALL_ANIMS:
            clean = "idle"
        self._last_activity = time.time()
        if clean != self.current_emotion:
            previous = self.current_emotion
            self.current_emotion = clean
            self._emotion_since = time.time()
            self._frame_index = 0
            self._frame_accum = 0.0
            self._ui(lambda: self._play_transition(previous, clean))
            if clean not in ("idle", "walk", "run"):
                self.is_walking = False
                self._cancel_soft()
            if self.brain:
                if clean != "dormindo":
                    self.brain.asleep = False
                self.brain.on_event("emotion:" + clean)
        self._ui(self._render_current)

    def _play_transition(self, previous: str, new: str) -> None:
        """Estados com entrada e saída próprias (``<anim>_in`` / ``<anim>_out``), como a forja: a bigorna
        surge e o martelo é puxado antes do laço; ao sair, o martelo é guardado e a bigorna some."""
        try:
            if new + "_in" in self._frames:
                self._play_once(new + "_in", self._anim_ms(new + "_in"))
            elif previous + "_out" in self._frames:
                self._play_once(previous + "_out", self._anim_ms(previous + "_out"))
        except Exception:
            pass

    def set_speaking(self, is_speaking: bool) -> None:
        self.is_speaking = is_speaking
        self._last_activity = time.time()
        if not is_speaking:
            self.mouth_open = False
            self._emotion_since = time.time()
        else:
            self.is_walking = False
            self._cancel_soft()
            if self.current_emotion == "dormindo":
                self.current_emotion = "idle"
            if self.brain:
                self.brain.asleep = False
                self.brain.on_event("interaction")
        self._ui(self._render_current)

    def set_listening(self, listening: bool) -> None:
        """Leans in with a cupped "ear" while the microphone is open."""
        self.is_listening = bool(listening)
        self._last_activity = time.time()
        if listening:
            self.is_walking = False
            self._cancel_soft()
            if self.current_emotion == "dormindo":
                self.current_emotion = "idle"
            if self.brain:
                self.brain.asleep = False
                self.brain.on_event("interaction")
        self._ui(self._render_current)

    def react(self, emotion: str, seconds: float = 2.5) -> None:
        """Short emotional reaction that falls back to idle on its own."""
        self.update_emotion(emotion)

        def _back():
            if self.current_emotion == emotion and not self.is_speaking:
                self.update_emotion("idle")
        try:
            self.root.after(int(seconds * 1000), _back)
        except Exception:
            pass

    def play(self, anim: str, loops: int = 1, on_done: Optional[Callable] = None) -> bool:
        """Plays an action animation once (a fidget). Anything important interrupts it."""
        if anim not in self._frames:
            if on_done:
                on_done()
            return False
        self.is_walking = False
        self._play_once(anim, self._anim_ms(anim), on_done=on_done, loops=loops, soft=True)
        return True

    def notify(self, event: str) -> None:
        """Tells the brain something happened (task_done, task_fail, interaction...)."""
        if self.brain:
            self.brain.on_event(event)

    # ------------------------------------------------------------------
    # Forms (real morph chain: current -> slime -> target)
    # ------------------------------------------------------------------
    @property
    def forms(self) -> List[str]:
        return list(FORMS)

    def set_form(self, form: str, animate: bool = True, hold_seconds: Optional[int] = None,
                 on_ready: Optional[Callable] = None) -> None:
        if not self._is_main_thread():
            self.root.after(0, lambda: self.set_form(form, animate, hold_seconds, on_ready))
            return
        target = pixel_avatar.resolve_form(form) or "slime"
        if target == self.current_form or self._morphing:
            return
        self._last_activity = time.time()
        self.is_walking = False
        self._cancel_soft()
        if self.brain:
            self.brain.asleep = False
        if self._form_return_job:
            try:
                self.root.after_cancel(self._form_return_job)
            except Exception:
                pass
            self._form_return_job = None

        def _finish():
            self._morphing = False
            self.current_emotion = "idle"
            self._emotion_since = time.time()
            self._frame_index = 0
            self._render_current()
            hold = self.form_hold_seconds if hold_seconds is None else hold_seconds
            if target != "slime" and hold and hold > 0:
                self._form_return_job = self.root.after(int(hold * 1000), self._return_to_slime)
            if on_ready:
                on_ready()

        if not animate:
            self._load_form(target)
            _finish()
            return

        self._morphing = True
        if self.on_transform_sound:
            try:
                self.on_transform_sound()
            except Exception:
                pass

        def _morph_in():
            self._load_form(target)
            if target == "slime":
                self._play_once("pouso", self._anim_ms("pouso"), on_done=_finish) if "pouso" in self._frames else _finish()
                return
            self._play_once("morph_in", self._anim_ms("morph_in"), on_done=_finish)

        if self.current_form != "slime":
            self._play_once("morph_out", self._anim_ms("morph_out"),
                            on_done=lambda: (self._load_form("slime"), _morph_in()))
        else:
            _morph_in()

    def _return_to_slime(self) -> None:
        self._form_return_job = None
        if self.current_form == "slime":
            return
        if self.is_speaking or self._oneshot or self._pointing or self.clones.clones:
            self._form_return_job = self.root.after(4000, self._return_to_slime)
            return
        self.set_form("slime", animate=True)

    def next_form(self) -> str:
        idx = (FORMS.index(self.current_form) + 1) % len(FORMS)
        self.set_form(FORMS[idx], animate=True)
        return FORMS[idx]

    def _auto_morph_tick(self) -> None:
        try:
            if (self.is_visible and not self.is_speaking and not self.is_walking
                    and not self._oneshot and self.current_emotion == "idle"
                    and not self._is_dragging and self.current_form == "slime"
                    and not self.clones.clones):
                self.set_form(random.choice([f for f in FORMS if f != "slime"]), animate=True,
                              hold_seconds=20)
        except Exception:
            pass
        try:
            self.root.after(self.auto_form_seconds * 1000, self._auto_morph_tick)
        except Exception:
            pass

    # ------------------------------------------------------------------
    # Clones (multi-agent visualisation)
    # ------------------------------------------------------------------
    def spawn_clone(self, clone_id: str, label: str, role: str = "") -> None:
        def _do():
            self._cancel_soft()
            self.clones.spawn(clone_id, label, role=role)
            if self.current_emotion in ("idle", "pensando"):
                previous = self.current_emotion
                self.current_emotion = "orquestrando"
                self._emotion_since = time.time()
                self._frame_index = 0
                self._play_transition(previous, "orquestrando")
        self._ui(_do)

    def update_clone(self, clone_id: str, label: Optional[str] = None,
                     state: Optional[str] = None) -> None:
        self._ui(lambda: self.clones.update(clone_id, label, state))

    def dismiss_clone(self, clone_id: str, success: bool = True) -> None:
        def _finish():
            if self.clones.clones or self.current_emotion != "orquestrando":
                return
            self.current_emotion = "comemorando" if (success and "comemorando" in self._frames) else \
                ("feliz" if success else "pensando")
            self._emotion_since = time.time()
            self._frame_index = 0
            self._play_transition("orquestrando", self.current_emotion)
            self.notify("task_done" if success else "task_fail")

        def _gone(_cid):
            if self.clones.clones or self.current_emotion != "orquestrando":
                return
            shot = self._oneshot
            if shot and shot.get("name") in self.CLONE_REACTIONS:
                shot["on_done"] = _finish            # deixa a reação terminar antes de sair da cena
            else:
                _finish()

        def _do():
            self._react_to_clone(success)
            self.clones.dismiss(clone_id, success, on_gone=_gone)
        self._ui(_do)

    def _react_to_clone(self, success: bool) -> None:
        """Cafuné no clone que entregou, bronca no que falhou (só com a cena de clones no ar)."""
        anim = "orgulhoso_clone" if success else "bronca_clone"
        shot = self._oneshot
        busy = shot and not shot.get("soft") and shot.get("name") not in self.CLONE_REACTIONS
        if (anim not in self._frames or self.current_emotion != "orquestrando" or self.is_speaking
                or self._is_dragging or self._morphing or busy):
            return
        self._play_once(anim, self._anim_ms(anim))

    # ------------------------------------------------------------------
    # Teleport + pointer
    # ------------------------------------------------------------------
    def _teleport(self, move: Callable, on_done: Optional[Callable] = None) -> None:
        """Dissolve -> ``move()`` -> reappear."""
        if self.on_teleport_sound:
            try:
                self.on_teleport_sound()
            except Exception:
                pass
        if "teleport_out" in self._frames and "teleport_in" in self._frames:
            def _arrived():
                move()
                self._play_once("teleport_in", self._anim_ms("teleport_in"), on_done=on_done)
            self._play_once("teleport_out", self._anim_ms("teleport_out"), on_done=_arrived)
        else:
            def _arrived_legacy():
                move()
                if on_done:
                    on_done()
            self._play_once("teleport", 38, on_done=_arrived_legacy)

    def vanish(self, on_done: Optional[Callable] = None) -> None:
        """Teleports out for good (used when the app closes): dissolve, then hide."""
        if not self._is_main_thread():
            try:
                self.root.after(0, lambda: self.vanish(on_done))
            except Exception:
                if on_done:
                    on_done()
            return
        self._sequence_token += 1
        self._leaving = True
        if self.brain:
            try:
                self.brain.save()
            except Exception:
                pass
        self.brain = None                     # no more fidgets / walks
        self.is_walking = False
        self._pointing = False
        self._is_dragging = False
        try:
            self.clones.clear()
        except Exception:
            pass
        self._hide_pointer()

        def _gone():
            self.hide()
            if on_done:
                on_done()
        if not self.is_visible:
            _gone()
            return
        if self.on_teleport_sound:
            try:
                self.on_teleport_sound()
            except Exception:
                pass
        anim = "teleport_out" if "teleport_out" in self._frames else "teleport"
        self._play_once(anim, self._anim_ms(anim) if anim == "teleport_out" else 38, on_done=_gone)

    def point_at(self, x: float, y: float, label: Optional[str] = None,
                 hold_seconds: Optional[float] = None) -> None:
        if not self._is_main_thread():
            self.root.after(0, lambda: self.point_at(x, y, label, hold_seconds))
            return
        self._sequence_token += 1
        token = self._sequence_token
        self.is_walking = False
        self._is_dragging = False
        self._cancel_soft()
        self._pointing = True
        if self.brain:
            self.brain.asleep = False

        def _move():
            if token == self._sequence_token:
                self._move_near_point(x, y)
                self._show_pointer(x, y, label)

        def _arrive():
            if token != self._sequence_token:
                return
            self.current_emotion = "apontar" if "apontar" in self._frames else "alerta"
            self._emotion_since = time.time()
            self._frame_index = 0
            self._render_current()
            hold = self.pointer_seconds if hold_seconds is None else hold_seconds
            self.root.after(int(hold * 1000), lambda: self._return_home(token))

        self._teleport(_move, _arrive)

    def _move_near_point(self, x: float, y: float) -> None:
        # the body is bottom-centred in the window; stand right next to the target,
        # vertically centred on it, so the slime itself marks the spot
        body_half = self.win_w * 0.27
        gap = 10
        pet_x = x - self.win_w / 2 - body_half - gap
        self.facing_right = True
        if pet_x < 0:
            pet_x = x - self.win_w / 2 + body_half + gap
            self.facing_right = False
        pet_y = y - self.win_h * 0.80
        pet_x, pet_y = self._clamp_screen(pet_x, pet_y)
        self.current_x, self.current_y = float(pet_x), float(pet_y)
        self.target_x, self.target_y = self.current_x, self.current_y
        self._set_geometry()

    def _show_pointer(self, x: float, y: float, label: Optional[str]) -> None:
        """No on-screen marker any more: the slime stands right beside the target."""
        self.pointer_label = label or ""

    def _hide_pointer(self) -> None:
        if self._pointer:
            self._pointer.hide()

    def _return_home(self, token: int) -> None:
        if token != self._sequence_token:
            return
        self._hide_pointer()

        def _move():
            if token != self._sequence_token:
                return
            self.current_x, self.current_y = self.anchor_x, self.anchor_y
            self.target_x, self.target_y = self.anchor_x, self.anchor_y
            self.facing_right = True
            self._set_geometry()
            self.current_emotion = "idle"
            self._emotion_since = time.time()
            self._frame_index = 0

        def _done():
            self._pointing = False
            self._render_current()

        self._teleport(_move, _done)

    def teleport_home(self) -> None:
        self._sequence_token += 1
        self._pointing = False
        self._hide_pointer()
        self.current_x, self.current_y = self.anchor_x, self.anchor_y
        self.target_x, self.target_y = self.anchor_x, self.anchor_y
        self._set_geometry()
        self.current_emotion = "idle"
        self._render_current()

    def set_home(self, x: Optional[float] = None, y: Optional[float] = None) -> None:
        x = self.current_x if x is None else x
        y = self.current_y if y is None else y
        self.anchor_x, self.anchor_y = self._clamp_screen(float(x), float(y))

    # ------------------------------------------------------------------
    # Locomotion
    # ------------------------------------------------------------------
    def _set_geometry(self) -> None:
        try:
            self.window.geometry("+%d+%d" % (int(self.current_x), int(self.current_y)))
        except Exception:
            pass

    def _start_wander(self, run: bool = False) -> None:
        angle = random.uniform(0, 2 * math.pi)
        radius = random.uniform(self.leash_radius * 0.25, self.leash_radius)
        nx = self.anchor_x + math.cos(angle) * radius
        ny = self.anchor_y + math.sin(angle) * radius * 0.35
        nx, ny = self._clamp_to_leash(nx, ny)
        self.target_x, self.target_y = float(nx), float(ny)
        self.is_walking = True
        self.walk_speed = 4.6 if run else 2.6
        self.current_emotion = "walk"
        self._frame_index = 0
        self._frame_accum = 0.0

    def _schedule_wander(self) -> None:
        try:
            if (self.enable_walking and not self.is_speaking and not self._is_dragging
                    and not self._oneshot and not self._morphing and not self.clones.clones
                    and self.current_emotion in ("idle", "walk", "run")):
                if random.random() < 0.7:
                    self._start_wander(run=random.random() < 0.2)
        except Exception:
            pass
        try:
            self.root.after(random.randint(9000, 18000), self._schedule_wander)
        except Exception:
            pass

    # ------------------------------------------------------------------
    # Brain (autonomous behaviour)
    # ------------------------------------------------------------------
    def _cursor_pos(self):
        try:
            x, y = self.root.winfo_pointerx(), self.root.winfo_pointery()
            if isinstance(x, int) and isinstance(y, int):
                return x, y
        except Exception:
            pass
        return None

    def _brain_tick(self, now: float) -> None:
        if self.brain is None or now - self._last_brain < 0.2:
            return
        self._last_brain = now
        asleep = self.current_emotion == "dormindo"
        free = (self.is_visible and not self.is_speaking and not self.is_listening and not self._is_dragging
                and not self._morphing and not self._pointing and not self.is_walking
                and not self.clones.clones and self.current_emotion in ("idle", "dormindo"))
        self.brain.asleep = asleep
        ctx = {
            "cursor": self._cursor_pos(),
            "pet": (self.current_x + self.win_w / 2.0, self.current_y + self.win_h * 0.74),
            "free": free,
            "user_idle": _user_idle_seconds(),
            "pet_idle": now - self._last_activity,
            "sleep_after": self.sleep_after_seconds,
            "hour": time.localtime(now).tm_hour,
            "form": self.current_form,
            "emotion": self.current_emotion,
            "emotion_age": now - self._emotion_since,
            "speaking": self.is_speaking or self.is_listening or bool(self.clones.clones) or self._pointing,
            "actions": self._frames,
            "forms": FORMS,
        }
        action = self.brain.tick(now, ctx)
        if action:
            self._do_brain_action(action)

    def _do_brain_action(self, action: Dict) -> None:
        kind = action.get("type")
        if kind == "relax":
            if self.on_relax:
                try:
                    self.on_relax()            # lets the app state settle too
                except Exception:
                    pass
            if self.current_emotion != "idle":
                self.update_emotion("idle")
        elif kind == "anim":
            name = action.get("name", "")
            if name in ("olhar_esq", "olhar_dir"):
                self.facing_right = True            # these already look the right way
            self.play(name, int(action.get("loops", 1)))
        elif kind == "emotion":
            self.react(action.get("name", "idle"), float(action.get("seconds", 3)))
        elif kind == "walk":
            if self.enable_walking:
                self._start_wander(run=bool(action.get("run")))
        elif kind == "form":
            roam = bool(action.get("roam"))
            self.set_form(action.get("name", "slime"), animate=True, hold_seconds=int(action.get("hold", 30)),
                          on_ready=(lambda: self._start_wander(run=True)) if (roam and self.enable_walking) else None)
        elif kind == "sleep":
            def _sleep():
                self.current_emotion = "dormindo"
                self._emotion_since = time.time()
                self._frame_index = 0
            if not self.play("bocejo", 1, on_done=_sleep):
                pass
        elif kind == "wake":
            then = action.get("then")
            self.current_emotion = "idle"
            self._emotion_since = time.time()
            self._last_activity = time.time()
            self._frame_index = 0
            self.play("acordar", 1, on_done=(lambda: self.play(then)) if then else None)

    # ------------------------------------------------------------------
    # Animation loop
    # ------------------------------------------------------------------
    def _play_once(self, animation: str, frame_ms: int, on_done: Optional[Callable] = None,
                   loops: int = 1, soft: bool = False) -> None:
        frames = self._frames.get(animation) or self._frames.get("idle") or []
        if not frames:
            if on_done:
                on_done()
            return
        pool = frames
        if soft and not self.facing_right and animation in self.FLIP_ANIMS:
            pool = self._flipped(animation) or frames
        self._oneshot = {"name": animation, "frames": frames, "pool": pool, "idx": 0, "ms": frame_ms, "acc": 0.0,
                         "on_done": on_done, "loops": max(1, int(loops)), "soft": soft}

    def _cancel_soft(self) -> None:
        """Drops a fidget in progress so something important can show right away."""
        if self._oneshot and self._oneshot.get("soft"):
            self._oneshot = None

    def _tick_loop(self) -> None:
        now = time.time()
        dt_ms = max(10.0, (now - self._last_tick) * 1000.0)
        self._last_tick = now
        try:
            self._brain_tick(now)
            if self._oneshot:
                self._advance_oneshot(dt_ms)
            else:
                if self.brain is None:
                    self._maybe_sleep(now)
                self._advance_locomotion()
                self._advance_animation(dt_ms)
                self._render_current()
        except Exception:
            pass
        try:
            self.root.after(33, self._tick_loop)
        except Exception:
            pass

    def _maybe_sleep(self, now: float) -> None:
        if (self.sleep_after_seconds and self.current_emotion == "idle" and not self.is_speaking
                and not self.is_walking and not self.clones.clones
                and now - self._last_activity > self.sleep_after_seconds
                and "dormindo" in self._frames):
            self.current_emotion = "dormindo"
            self._frame_index = 0

    def _advance_oneshot(self, dt_ms: float) -> None:
        data = self._oneshot
        data["acc"] += dt_ms
        steps = int(data["acc"] // data["ms"])
        if steps:
            data["acc"] -= steps * data["ms"]
            data["idx"] += steps
        pool = data["pool"]
        total = len(pool)
        if data["idx"] >= total and data["loops"] > 1:
            data["loops"] -= 1
            data["idx"] = data["idx"] % total
        if data["idx"] >= total:
            self._oneshot = None
            self._present(data.get("name", ""), pool[-1])
            cb = data["on_done"]
            if cb:
                cb()
            if not self._oneshot:
                self._frame_index = 0
                self._render_current()
            return
        self._present(data.get("name", ""), pool[data["idx"]])

    def _advance_locomotion(self) -> None:
        if not (self.is_walking and not self._is_dragging and not self.is_speaking):
            return
        dx = self.target_x - self.current_x
        dy = self.target_y - self.current_y
        dist = math.hypot(dx, dy)
        if dist > self.walk_speed:
            self.current_x += dx / dist * self.walk_speed
            self.current_y += dy / dist * self.walk_speed
            if abs(dx) > 1:
                self.facing_right = dx >= 0
            self._set_geometry()
        else:
            self.current_x, self.current_y = self.target_x, self.target_y
            self.is_walking = False
            if self.current_emotion in ("walk", "run"):
                self.current_emotion = "idle"
                self._emotion_since = time.time()
                self._frame_index = 0
            self._set_geometry()

    def _advance_animation(self, dt_ms: float) -> None:
        anim = self._active_animation()
        speed = self._anim_ms(anim)
        self._frame_accum += dt_ms
        if self._frame_accum >= speed:
            steps = max(1, int(self._frame_accum // speed))
            self._frame_accum -= steps * speed
            self._frame_index += steps
            if self.is_speaking:
                self.mouth_open = not self.mouth_open

    def _active_animation(self) -> str:
        frames = self._frames
        if self._is_dragging and self._drag_moved and "arrastado" in frames:
            return "arrastado"
        if self.is_speaking and "talk" in frames:
            return "talk"
        if self.is_listening and "ouvindo" in frames:
            return "ouvindo"
        if self.is_walking:
            return "run" if self.walk_speed > 4.0 and "run" in frames else "walk"
        emo = self.current_emotion
        if emo == "pensando" and self.deep_thinking and "sabio" in frames:
            return "sabio"
        if emo == "orquestrando" and "maestro" in frames and len(self.clones.clones) >= 2:
            return "maestro"                     # vários clones ao mesmo tempo: ele rege o grupo
        return emo if emo in frames else "idle"

    def _render_current(self) -> None:
        if self._oneshot:
            return
        anim = self._active_animation()
        frames = self._frames.get(anim) or self._frames.get("idle") or []
        if not frames:
            return
        pool = frames
        # sprites face right; mirror when walking/pointing left
        if not self.facing_right and anim in self.FLIP_ANIMS:
            flipped = self._flipped(anim)
            if flipped:
                pool = flipped
        self._present(anim, pool[self._frame_index % len(pool)])

    # ------------------------------------------------------------------
    # Interaction
    # ------------------------------------------------------------------
    def _on_drag_start(self, event) -> None:
        self._is_dragging = True
        self._drag_moved = False
        self.is_walking = False
        self._last_activity = time.time()
        self._sequence_token += 1
        self._pointing = False
        self._cancel_soft()
        self._hide_pointer()
        self._drag_start_x, self._drag_start_y = event.x_root, event.y_root
        self._drag_origin_x, self._drag_origin_y = self.current_x, self.current_y
        if self.current_emotion in ("dormindo", "walk", "run"):
            self.current_emotion = "idle"
            self._emotion_since = time.time()
        if self.brain:
            self.brain.asleep = False

    def _on_drag_motion(self, event) -> None:
        self.current_x = float(self._drag_origin_x + event.x_root - self._drag_start_x)
        self.current_y = float(self._drag_origin_y + event.y_root - self._drag_start_y)
        self.target_x, self.target_y = self.current_x, self.current_y
        try:
            moved = abs(event.x_root - self._drag_start_x) + abs(event.y_root - self._drag_start_y)
            if not self._drag_moved and moved > 8:
                self._drag_moved = True
                self._frame_index = 0
                self.notify("drag_start")
        except Exception:
            pass
        self._set_geometry()

    def _on_drag_end(self, event) -> None:
        moved = self._drag_moved
        self._is_dragging = False
        self._drag_moved = False
        self.set_home(self.current_x, self.current_y)
        if moved:
            self.notify("drop")
            self.play("pouso")

    def _on_double_click(self, event) -> None:
        self.is_walking = False
        self.notify("petted")
        self.react("feliz", 2.4)

    def toggle_visibility(self) -> None:
        self.is_visible = not self.is_visible
        try:
            if self.is_visible:
                self.window.deiconify()
            else:
                self._hide_pointer()
                self.window.withdraw()
        except Exception:
            pass

    def show(self) -> None:
        self.is_visible = True
        try:
            self.window.deiconify()
        except Exception:
            pass

    def hide(self) -> None:
        self.is_visible = False
        self._hide_pointer()
        try:
            self.window.withdraw()
        except Exception:
            pass

    def destroy(self) -> None:
        self._sequence_token += 1
        if self.brain:
            self.brain.save()
        try:
            self.clones.clear()
        except Exception:
            pass
        if self._pointer:
            self._pointer.destroy()
        if self._surface is not None:
            self._surface.close()
            self._surface = None
        try:
            self.window.destroy()
        except Exception:
            pass

    # ------------------------------------------------------------------
    # Thread marshalling
    # ------------------------------------------------------------------
    @staticmethod
    def _is_main_thread() -> bool:
        return threading.current_thread() is threading.main_thread()

    def _ui(self, fn: Callable) -> None:
        if not self._is_main_thread():
            try:
                self.root.after(0, fn)
            except Exception:
                pass
            return
        fn()
