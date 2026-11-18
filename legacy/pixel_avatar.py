"""
S.A.G.E. pixel-art avatar builder
=================================
Builds the desktop companion (and its worker clones) from the reference
sprite sheets in ``assets/sprites/rimuru/source``:

* ``sheet_yang.jpg``  - slime body, emotions-friendly frames and the
  slime -> wolf / snake / spider / bat transformations (credits: Yang Kai).
* ``sheet_full.png``  - human, winged and demon-lord forms, the canonical
  "clone" frames used by worker clones, water powers (credits in CREDITS.txt).

Everything is composed at *logical* resolution (one sheet pixel = one
logical pixel), decorated with procedural pixel-art FX (hearts, anger marks,
tears, thinking bubbles, holo-screens...) and upscaled with nearest
neighbour so the art stays crisp. Alpha is binary, which plays perfectly
with the Tk colour-key transparent window.

Outputs
-------
assets/sprites/forms/<form>/<anim>_<i>.png
assets/sprites/clone/<anim>_<i>.png
assets/sage.ico, assets/sage_icon.png

Run ``python -m sage.pixel_avatar --force`` to rebuild.
"""
from __future__ import annotations

import json
import math
import os
import random
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional, Sequence, Tuple

import numpy as np
from PIL import Image, ImageDraw

from sage.sheet_slicer import key_mask, slice_strip

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS_DIR = os.path.join(ROOT_DIR, "assets")
SOURCE_DIR = os.path.join(ASSETS_DIR, "sprites", "rimuru", "source")
FORMS_DIR = os.path.join(ASSETS_DIR, "sprites", "forms")
CLONE_DIR = os.path.join(ASSETS_DIR, "sprites", "clone")
BUILD_STAMP = "pixel-avatar-v10"   # legacy sheet-only builder (see build_all_legacy)

SCALE = 3                 # sheet px -> screen px (sheet forms)
CANVAS = (90, 72)         # canvas of the main companion in sheet px (270x216 on screen)
BASELINE = CANVAS[1] - 3  # feet line
CLONE_CANVAS = (64, 58)
CLONE_SCALE = 2

YANG_BG = [(35, 177, 77), (0, 128, 0)]
FULL_BG = [(255, 127, 39)]

FORMS: Tuple[str, ...] = ("slime", "humano", "alado")
FORM_LABELS = {
    "slime": "Slime",
    "humano": "Forma Humana",
    "alado": "Forma Alada",
}
FORM_ALIASES = {
    "slime": "slime", "gosma": "slime", "gota": "slime", "normal": "slime",
    "humano": "humano", "humana": "humano", "pessoa": "humano", "rimuru": "humano",
    "alado": "alado", "asas": "alado", "anjo": "alado", "voar": "alado",
    # legacy names from the procedural engine
    "orbe": "slime", "sabio": "humano",
}

# Looping states: the pet can stay in any of these (state.set_emotion / pet.update_emotion).
EMOTIONS: Tuple[str, ...] = (
    "idle", "walk", "run", "talk", "feliz", "bravo", "triste", "pensando",
    "alerta", "confuso", "foto", "dormindo", "orquestrando",
    "surpreso", "amor", "rindo", "orgulhoso", "envergonhado", "medo", "curioso",
    "entediado", "determinado", "ouvindo", "comemorando", "tonto", "sabio", "apontar",
    "forjando",
)
# Emotions the language models may pick for a reply.
LLM_EMOTIONS: Tuple[str, ...] = (
    "idle", "feliz", "pensando", "alerta", "confuso", "bravo", "triste", "foto",
    "surpreso", "amor", "rindo", "orgulhoso", "envergonhado", "medo", "curioso",
    "entediado", "determinado", "comemorando",
)
# Fidgets and little actions the pet brain plays on its own (slime only).
ACTIONS: Tuple[str, ...] = (
    "olhar_esq", "olhar_dir", "olhar_cima", "acenar", "espreguicar", "bocejo", "quicar",
    "derreter", "bolha", "predador", "espiar", "danca", "cafune", "arrastado", "pouso",
    "acordar", "piscadela",
)
EXTRA_ANIMS: Tuple[str, ...] = ("teleport", "teleport_out", "teleport_in", "transform", "morph_in", "morph_out",
                                "forjando_in", "forjando_out",
                                # cenas com clones (só o slime): entrada/saída da explicação, maestro
                                # (vários clones ativos) e as reações quando um clone entrega ou falha
                                "orquestrando_in", "orquestrando_out", "maestro", "orgulhoso_clone",
                                "bronca_clone")
CLONE_ANIMS: Tuple[str, ...] = ("spawn", "work", "done", "fail", "vanish")
ALL_ANIMS: Tuple[str, ...] = EMOTIONS + ACTIONS + EXTRA_ANIMS

FRAME_MS = {
    "idle": 70, "walk": 55, "run": 45, "talk": 60, "feliz": 55, "bravo": 55,
    "triste": 85, "pensando": 75, "alerta": 55, "confuso": 70, "foto": 60,
    "dormindo": 110, "orquestrando": 75, "surpreso": 50, "amor": 65, "rindo": 50,
    "orgulhoso": 70, "envergonhado": 75, "medo": 45, "curioso": 70, "entediado": 95,
    "determinado": 50, "ouvindo": 65, "comemorando": 50, "tonto": 65, "sabio": 65, "apontar": 60,
    "forjando": 55, "forjando_in": 55, "forjando_out": 55,
    "orquestrando_in": 50, "orquestrando_out": 50, "maestro": 62, "orgulhoso_clone": 70, "bronca_clone": 62,
    "olhar_esq": 60, "olhar_dir": 60, "olhar_cima": 60, "acenar": 55, "espreguicar": 65,
    "bocejo": 75, "quicar": 45, "derreter": 60, "bolha": 70, "predador": 55, "espiar": 65,
    "danca": 55, "cafune": 60, "arrastado": 50, "pouso": 45, "acordar": 65, "piscadela": 60,
    "teleport": 40, "teleport_out": 40, "teleport_in": 40,
    "transform": 45, "morph_in": 50, "morph_out": 50,
    "spawn": 50, "work": 70, "done": 55, "fail": 70, "vanish": 50,
}

# Palette for FX (pixel-art friendly, no anti-aliasing)
WHITE = (250, 252, 255, 255)
INK = (18, 28, 48, 255)
CYAN = (110, 230, 255, 255)
CYAN_D = (40, 150, 210, 255)
GOLD = (255, 214, 72, 255)
PINK = (255, 120, 170, 255)
RED = (235, 60, 70, 255)
GREEN = (90, 230, 120, 255)
TEAR = (120, 200, 255, 255)
GREY = (170, 180, 200, 255)

Frame = Image.Image


# ==========================================================================
# 1. Source extraction
# ==========================================================================
YANG_STRIPS = {
    "slime_idle": (60, 152, 184, 186),
    "slime_guard": (58, 226, 244, 266),
    "slime_basa": (50, 312, 204, 343),
    "slime_up": (50, 430, 192, 478),
    "slime_air": (33, 712, 559, 756),
    "idle_special": (200, 777, 302, 800),
    "wolf": (32, 500, 852, 552),
    "snake": (36, 575, 370, 613),
    "spider": (30, 640, 558, 692),
    "bat": (36, 936, 1002, 984),
}

# (x, y, w, h[, scale]) boxes on sheet_full.png
FULL_BOXES: Dict[str, List[Tuple]] = {
    "human_in": [(80, 62, 19, 15, 1.5), (113, 53, 12, 24), (139, 33, 26, 42),
                 (173, 27, 31, 49), (215, 27, 31, 49), (258, 29, 31, 49),
                 (295, 28, 31, 49), (338, 28, 31, 49)],
    "human_idle": [(19, 148, 31, 49), (66, 149, 31, 48), (113, 151, 31, 47),
                   (154, 150, 33, 48), (202, 152, 31, 47)],
    "human_jump": [(13, 252, 31, 48), (55, 262, 32, 37), (97, 250, 31, 49),
                   (150, 250, 31, 54), (196, 253, 31, 43), (244, 251, 30, 47),
                   (287, 252, 31, 47)],
    "human_run": [(25, 366, 36, 44), (79, 366, 38, 43), (137, 366, 39, 44),
                  (192, 367, 33, 44), (246, 368, 37, 42), (299, 367, 38, 43)],
    "human_guard": [(18, 473, 34, 48), (72, 473, 31, 55)],
    "alado_in": [(21, 2765, 31, 49), (65, 2764, 31, 51), (115, 2767, 31, 49),
                 (163, 2767, 35, 49), (225, 2769, 35, 49), (273, 2769, 43, 49),
                 (330, 2770, 43, 49)],
    "alado_power": [(567, 2766, 31, 52), (608, 2766, 31, 52), (649, 2765, 31, 52),
                    (690, 2766, 33, 52), (751, 2767, 31, 49)],
    "alado_idle": [(23, 2895, 40, 49), (80, 2896, 38, 49), (134, 2897, 36, 49),
                   (182, 2896, 39, 49), (236, 2896, 36, 49)],
    "alado_fly": [(26, 3003, 43, 43), (89, 3003, 43, 43), (156, 3006, 43, 43),
                  (222, 2999, 36, 54), (460, 3005, 38, 47), (514, 3005, 40, 47),
                  (566, 3005, 40, 47), (621, 3005, 40, 47)],
    "demon_in": [(129, 4377, 15, 10, 2.0), (156, 4377, 15, 10, 2.0),
                 (190, 4343, 42, 51), (252, 4343, 42, 51), (308, 4344, 42, 51),
                 (360, 4344, 42, 51), (422, 4345, 22, 51), (465, 4344, 26, 52),
                 (513, 4346, 28, 52), (563, 4348, 28, 51), (607, 4348, 19, 51)],
    "demon_idle": [(16, 4466, 28, 51), (67, 4466, 28, 50), (122, 4467, 28, 49),
                   (173, 4468, 28, 49)],
    "demon_jump": [(13, 4557, 27, 50), (56, 4551, 30, 54), (105, 4557, 30, 43),
                   (156, 4559, 27, 48), (205, 4561, 27, 47), (255, 4571, 29, 37),
                   (299, 4559, 27, 50)],
    "demon_walk": [(15, 4660, 18, 51), (51, 4660, 27, 51), (94, 4660, 19, 51),
                   (131, 4660, 24, 51), (167, 4660, 30, 51), (213, 4661, 21, 51)],
    "clone_idle": [(62, 2010, 31, 49), (109, 2011, 31, 48), (156, 2013, 31, 47),
                   (197, 2012, 33, 48)],
    "clone_run": [(65, 2104, 36, 44), (119, 2104, 38, 43), (177, 2104, 39, 44),
                  (232, 2105, 33, 44), (286, 2106, 37, 42), (339, 2105, 38, 43)],
    "water_power": [(49, 6861, 31, 25), (100, 6849, 50, 36), (166, 6838, 61, 47)],
    "tornado": [(60, 6913, 43, 44), (128, 6917, 47, 41), (195, 6916, 43, 42),
                (261, 6913, 45, 48)],
}


def _clean_alpha(img: Frame, bg_colors, edge_tol: float = 115.0) -> Frame:
    """Binary alpha + removes JPEG fringe pixels that still look like the key."""
    a = np.asarray(img.convert("RGBA")).copy()
    alpha = a[..., 3] > 127
    rgb = a[..., :3].astype(np.int32)
    near_bg = np.zeros(alpha.shape, dtype=bool)
    for c in bg_colors:
        d = np.sqrt(((rgb - np.array(c)) ** 2).sum(axis=2))
        near_bg |= d < edge_tol
    # edge pixels = opaque pixels touching transparency
    pad = np.pad(alpha, 1, constant_values=False)
    edge = alpha & ~(pad[:-2, 1:-1] & pad[2:, 1:-1] & pad[1:-1, :-2] & pad[1:-1, 2:])
    alpha &= ~(edge & near_bg)
    # drop isolated specks
    pad = np.pad(alpha, 1, constant_values=False)
    neighbours = (pad[:-2, 1:-1].astype(int) + pad[2:, 1:-1] + pad[1:-1, :-2] + pad[1:-1, 2:])
    alpha &= neighbours > 0
    a[..., 3] = np.where(alpha, 255, 0).astype(np.uint8)
    out = Image.fromarray(a, "RGBA")
    bbox = out.getbbox()
    return out.crop(bbox) if bbox else out


def _azure(img: Frame, shift_deg: float = 16.0, sat_boost: float = 1.08) -> Frame:
    """Pulls teal/cyan pixels toward the project's azure blue."""
    rgba = np.asarray(img.convert("RGBA"))
    hsv = np.asarray(img.convert("RGB").convert("HSV")).astype(np.float32)
    h = hsv[..., 0] * 360.0 / 255.0
    sel = (h > 150) & (h < 205) & (hsv[..., 1] > 50)
    h = np.where(sel, h + shift_deg, h)
    hsv[..., 0] = (h % 360) * 255.0 / 360.0
    hsv[..., 1] = np.where(sel, np.clip(hsv[..., 1] * sat_boost, 0, 255), hsv[..., 1])
    rgb = np.asarray(Image.fromarray(hsv.astype(np.uint8), "HSV").convert("RGB"))
    return Image.fromarray(np.dstack([rgb, rgba[..., 3]]), "RGBA")


def _crop_box(sheet: Image.Image, box: Tuple, bg) -> Frame:
    x, y, w, h = box[:4]
    scale = box[4] if len(box) > 4 else 1.0
    region = sheet.crop((x - 1, y - 1, x + w + 1, y + h + 1)).convert("RGB")
    rgb = np.asarray(region)
    mask = key_mask(rgb, bg, 40)
    rgba = np.dstack([rgb, (mask * 255).astype(np.uint8)])
    img = _clean_alpha(Image.fromarray(rgba, "RGBA"), bg)
    if scale != 1.0:
        img = img.resize((max(1, round(img.width * scale)), max(1, round(img.height * scale))),
                         Image.NEAREST)
    return img


# Animated cursor pack (slime_cursors.gif): 6x3 grid, one expression per cell.
GIF_CELL = (224, 274.33)
GIF_SCALE = 3.5          # the art is upscaled ~3.5x in the GIF
GIF_BG = (24, 34, 45)
GIF_CELLS = {             # name: (col, row, keep_detached_top)
    "normal": (0, 0, False),
    "link": (1, 0, False),          # open mouth -> talking
    "help": (2, 0, True),           # sad + floating "?" -> confused
    "working": (3, 0, False),       # "..." dots -> thinking
    "busy": (4, 0, False),          # blue flame aura -> orchestrating clones
    "handwriting": (1, 1, False),   # squinting -> annoyed
    "unavailable": (3, 1, False),   # sad face -> sad
}


# cursor glyphs glued to the body (cell coordinates in the GIF): pencil, "no" sign, arrow
GIF_ERASE = {
    "busy": [(8, 48, 66, 106)],
    "handwriting": [(14, 26, 74, 90)],
    "unavailable": [(12, 10, 90, 72), (12, 72, 58, 84)],
}


def _extract_gif_cells(path: str, max_frames: int = 12) -> Dict[str, List[Frame]]:
    """Cuts each expression cell out of the cursor-pack GIF and re-pixelates it."""
    from PIL import ImageSequence
    from scipy.ndimage import label as _label

    gif = Image.open(path)
    frames = [f.convert("RGB") for f in ImageSequence.Iterator(gif)]
    out: Dict[str, List[Frame]] = {}
    bg = np.array(GIF_BG)
    cw, ch = GIF_CELL
    for name, (col, row, keep_top) in GIF_CELLS.items():
        x0, y0 = int(col * cw), int(row * ch)
        seq: List[Frame] = []
        last = None
        for fr in frames:
            cell = np.asarray(fr.crop((x0, y0, x0 + int(cw), y0 + 228))).astype(int)
            alpha = np.abs(cell - bg).sum(axis=2) > 36
            for (ex0, ey0, ex1, ey1) in GIF_ERASE.get(name, ()):
                alpha[ey0:ey1, ex0:ex1] = False
            img = Image.fromarray(np.dstack([cell.astype(np.uint8),
                                             (alpha * 255).astype(np.uint8)]), "RGBA")
            w, h = int(cw / GIF_SCALE), int(228 / GIF_SCALE)
            small = np.asarray(img.resize((w, h), Image.NEAREST)).copy()
            mask = small[..., 3] > 127
            lab, n = _label(mask, structure=np.ones((3, 3)))
            if n == 0:
                continue
            sizes = [(lab == i).sum() for i in range(1, n + 1)]
            big = int(np.argmax(sizes)) + 1
            by, bx = np.nonzero(lab == big)
            body_top = by.min()
            keep = lab == big
            for i in range(1, n + 1):
                if i == big or sizes[i - 1] < 6:
                    continue
                yy, xx = np.nonzero(lab == i)
                if xx.max() < 12 and yy.max() < 14:          # cursor arrow / hand
                    continue
                if yy.max() < body_top + 2 and not keep_top:   # icons floating above
                    continue
                if name == "working" and yy.min() > by.max() - 2:   # the big "..." under the body
                    continue
                keep |= lab == i
            small[..., 3] = np.where(keep, 255, 0)
            if name in GIF_ERASE:
                # leftovers of the glyph on the body outline (pink/red or grey pixels near the top)
                r, g, b = small[..., 0].astype(int), small[..., 1].astype(int), small[..., 2].astype(int)
                rows = np.arange(small.shape[0])[:, None]
                stray = ((r > 170) & (g < 170) & (b < 200)) | ((abs(r - g) < 12) & (abs(g - b) < 12) & (r > 120) & (r < 215))
                small[..., 3] = np.where(stray & (rows < body_top + 4), 0, small[..., 3])
            sprite = Image.fromarray(small, "RGBA")
            sprite = sprite.crop(sprite.getbbox())
            sig = sprite.tobytes()
            if sig != last:
                seq.append(sprite)
                last = sig
        if len(seq) > max_frames:
            step = len(seq) / float(max_frames)
            seq = [seq[int(i * step)] for i in range(max_frames)]
        if seq:
            out["gif_" + name] = seq
    return out


def extract_sources(source_dir: str = SOURCE_DIR) -> Dict[str, List[Frame]]:
    """Cuts every sequence used by the avatar out of the reference sheets."""
    out: Dict[str, List[Frame]] = {}
    gif_path = os.path.join(source_dir, "slime_cursors.gif")
    if os.path.exists(gif_path):
        try:
            out.update(_extract_gif_cells(gif_path))
        except Exception as e:
            print("[Avatar] GIF do slime ignorado: " + str(e))
    yang_path = os.path.join(source_dir, "sheet_yang.jpg")
    full_path = os.path.join(source_dir, "sheet_full.png")
    if os.path.exists(yang_path):
        yang = Image.open(yang_path).convert("RGB")
        for name, rect in YANG_STRIPS.items():
            frames = slice_strip(yang, rect, YANG_BG, tol=60, merge_gap=0, min_pixels=10)
            out[name] = [_azure(_clean_alpha(f, YANG_BG)) for f in frames]
    if os.path.exists(full_path):
        full = Image.open(full_path).convert("RGB")
        for name, boxes in FULL_BOXES.items():
            out[name] = [_crop_box(full, b, FULL_BG) for b in boxes]
    return out


def sources_available(source_dir: str = SOURCE_DIR) -> bool:
    return (os.path.exists(os.path.join(source_dir, "sheet_yang.jpg"))
            and os.path.exists(os.path.join(source_dir, "sheet_full.png")))


# ==========================================================================
# 2. Pixel FX helpers (logical resolution)
# ==========================================================================
def _blank(size=CANVAS) -> Frame:
    return Image.new("RGBA", size, (0, 0, 0, 0))


def _paste_clip(canvas: Frame, sprite: Frame, x: int, y: int) -> None:
    sx = max(0, -x)
    sy = max(0, -y)
    part = sprite.crop((sx, sy, sprite.width, sprite.height))
    canvas.alpha_composite(part, (max(0, x), max(0, y)))


def _squash(sprite: Frame, sx: float, sy: float) -> Frame:
    w = max(1, int(round(sprite.width * sx)))
    h = max(1, int(round(sprite.height * sy)))
    return sprite.resize((w, h), Image.NEAREST)


def _tint(sprite: Frame, color: Sequence[int], amount: float) -> Frame:
    a = np.asarray(sprite).astype(np.float32)
    c = np.array(color[:3], dtype=np.float32)
    a[..., :3] = a[..., :3] * (1 - amount) + c * amount
    return Image.fromarray(a.clip(0, 255).astype(np.uint8), "RGBA")


def _desaturate(sprite: Frame, amount: float, darken: float = 0.0) -> Frame:
    a = np.asarray(sprite).astype(np.float32)
    lum = (a[..., 0] * 0.3 + a[..., 1] * 0.59 + a[..., 2] * 0.11)[..., None]
    a[..., :3] = a[..., :3] * (1 - amount) + lum * amount
    a[..., :3] *= (1 - darken)
    return Image.fromarray(a.clip(0, 255).astype(np.uint8), "RGBA")


def _outline(sprite: Frame, color=CYAN) -> Frame:
    """1px outline glow around the sprite (used for clones)."""
    alpha = np.asarray(sprite)[..., 3] > 0
    alpha_pad = np.pad(alpha, 1, constant_values=False)
    p2 = np.pad(alpha_pad, 1, constant_values=False)
    ring_full = (p2[:-2, 1:-1] | p2[2:, 1:-1] | p2[1:-1, :-2] | p2[1:-1, 2:]) & ~alpha_pad
    arr = np.zeros((sprite.height + 2, sprite.width + 2, 4), dtype=np.uint8)
    arr[ring_full] = color
    big = Image.fromarray(arr, "RGBA")
    big.alpha_composite(sprite, (1, 1))
    return big


def _dither_fade(sprite: Frame, keep: float, seed: int = 7) -> Frame:
    """Ordered-dither dissolve: keep in [0,1]."""
    bayer = np.array([[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]) / 16.0
    a = np.asarray(sprite).copy()
    h, w = a.shape[:2]
    thr = np.tile(bayer, (h // 4 + 1, w // 4 + 1))[:h, :w]
    a[..., 3] = np.where(thr < keep, a[..., 3], 0)
    return Image.fromarray(a, "RGBA")


def _flash(sprite: Frame, amount: float) -> Frame:
    return _tint(sprite, WHITE, amount)


def _px(d: ImageDraw.ImageDraw, pts, color) -> None:
    for (x, y) in pts:
        d.point((int(x), int(y)), fill=color)


def fx_sparkle(d, x, y, size=1, color=GOLD):
    x, y = int(x), int(y)
    d.point((x, y), fill=WHITE)
    for i in range(1, size + 1):
        _px(d, [(x - i, y), (x + i, y), (x, y - i), (x, y + i)], color)


def fx_heart(d, x, y, color=PINK):
    x, y = int(x), int(y)
    rows = ["01010", "11111", "11111", "01110", "00100"]
    for j, row in enumerate(rows):
        for i, ch in enumerate(row):
            if ch == "1":
                d.point((x + i - 2, y + j - 2), fill=color)
    d.point((x - 1, y - 1), fill=WHITE)


def fx_anger(d, x, y, color=RED):
    x, y = int(x), int(y)
    pts = [(x - 2, y - 1), (x - 1, y - 1), (x + 1, y - 1), (x + 2, y - 1),
           (x - 1, y - 2), (x + 1, y - 2), (x - 1, y + 1), (x + 1, y + 1),
           (x - 2, y + 1), (x + 2, y + 1), (x - 1, y + 2), (x + 1, y + 2)]
    _px(d, pts, color)


def fx_drop(d, x, y, color=TEAR):
    x, y = int(x), int(y)
    _px(d, [(x, y - 2), (x - 1, y - 1), (x, y - 1), (x + 1, y - 1),
            (x - 1, y), (x, y), (x + 1, y), (x, y + 1)], color)
    d.point((x - 1, y - 1), fill=WHITE)


def fx_exclaim(d, x, y, color=GOLD):
    x, y = int(x), int(y)
    d.rectangle((x - 1, y - 6, x + 1, y - 2), fill=INK)
    d.rectangle((x, y - 6, x, y - 3), fill=color)
    d.rectangle((x - 1, y, x + 1, y + 1), fill=INK)
    d.point((x, y), fill=color)


def fx_question(d, x, y, color=CYAN):
    x, y = int(x), int(y)
    rows = ["0110", "1001", "0001", "0010", "0100", "0000", "0100"]
    for j, row in enumerate(rows):
        for i, ch in enumerate(row):
            if ch == "1":
                d.point((x + i - 2, y + j - 6), fill=color)


def fx_bubble(d, x, y, dots: int, color=WHITE):
    """Thought bubble with 0-3 dots."""
    x, y = int(x), int(y)
    d.rounded_rectangle((x - 7, y - 5, x + 7, y + 2), radius=3, fill=color, outline=INK)
    _px(d, [(x - 5, y + 4), (x - 7, y + 6)], color)
    for i in range(dots):
        d.point((x - 3 + i * 3, y - 1), fill=INK)


def fx_zzz(d, x, y, phase: int):
    for i in range(3):
        k = (phase + i) % 3
        ox, oy = x + i * 4, y - i * 4 - (phase % 2)
        if k == 0:
            continue
        _px(d, [(ox, oy), (ox + 1, oy), (ox + 2, oy), (ox + 1, oy + 1),
                (ox, oy + 2), (ox + 1, oy + 2), (ox + 2, oy + 2)], WHITE)


def fx_note(d, x, y, color=CYAN):
    x, y = int(x), int(y)
    _px(d, [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1), (x + 1, y - 1),
            (x + 1, y - 2), (x + 1, y - 3), (x + 2, y - 3), (x + 3, y - 2)], color)


def fx_camera(d, x, y):
    x, y = int(x), int(y)
    d.rectangle((x - 4, y - 2, x + 4, y + 3), fill=GREY, outline=INK)
    d.rectangle((x - 2, y - 4, x + 1, y - 2), fill=GREY, outline=INK)
    d.ellipse((x - 2, y - 1, x + 2, y + 3), fill=INK)
    d.point((x, y + 1), fill=CYAN)


def fx_arrow(d, x, y, direction: int = 1, color=GOLD):
    x, y = int(x), int(y)
    for i in range(6):
        d.point((x + i * direction, y), fill=color)
    _px(d, [(x + 5 * direction - direction, y - 1), (x + 5 * direction - 2 * direction, y - 2),
            (x + 5 * direction - direction, y + 1), (x + 5 * direction - 2 * direction, y + 2)], color)


def fx_gear(d, x, y, phase: int, color=GOLD):
    x, y = int(x), int(y)
    d.ellipse((x - 3, y - 3, x + 3, y + 3), fill=color, outline=INK)
    d.point((x, y), fill=INK)
    for k in range(4):
        ang = (k * math.pi / 2) + phase * math.pi / 8
        tx = x + round(math.cos(ang) * 4)
        ty = y + round(math.sin(ang) * 4)
        d.point((tx, ty), fill=color)


def fx_holo_screen(d, x, y, phase: int):
    """Small holographic screen with scrolling code lines."""
    x, y = int(x), int(y)
    d.rectangle((x - 9, y - 6, x + 9, y + 5), fill=(30, 90, 140, 255), outline=CYAN)
    rng = random.Random(phase * 31 + 7)
    for row in range(4):
        length = rng.randint(4, 14)
        indent = rng.choice((0, 2, 4))
        color = rng.choice((CYAN, GREEN, GOLD, WHITE))
        yy = y - 4 + row * 2 + 1
        d.line((x - 7 + indent, yy, x - 7 + indent + length, yy), fill=color)
    if phase % 2 == 0:
        d.point((x + 7, y + 3), fill=WHITE)


def fx_check(d, x, y, color=GREEN):
    x, y = int(x), int(y)
    d.ellipse((x - 5, y - 5, x + 5, y + 5), fill=(20, 60, 40, 255), outline=color)
    _px(d, [(x - 3, y), (x - 2, y + 1), (x - 1, y + 2), (x, y + 1), (x + 1, y),
            (x + 2, y - 1), (x + 3, y - 2)], color)


def fx_cross(d, x, y, color=RED):
    x, y = int(x), int(y)
    d.ellipse((x - 5, y - 5, x + 5, y + 5), fill=(70, 20, 30, 255), outline=color)
    for i in range(-2, 3):
        _px(d, [(x + i, y + i), (x + i, y - i)], color)


def fx_dust(d, x, y, phase: int):
    for k in range(2):
        r = 1 + (phase + k) % 2
        xx = x - 4 - k * 5 - phase % 3
        d.ellipse((xx - r, y - r, xx + r, y + r), fill=(220, 230, 245, 255))


# ==========================================================================
# 3. Form definitions
# ==========================================================================
def _seq(src: Dict[str, List[Frame]], name: str, idx: Optional[Sequence[int]] = None) -> List[Frame]:
    frames = src.get(name, [])
    if idx is None:
        return list(frames)
    return [frames[i] for i in idx if 0 <= i < len(frames)]


def _ping_pong(frames: List[Frame]) -> List[Frame]:
    if len(frames) <= 2:
        return frames
    return frames + frames[-2:0:-1]


@dataclass
class FormDef:
    key: str
    idle: List[Frame]
    walk: List[Frame]
    run: List[Frame] = field(default_factory=list)
    morph_in: List[Frame] = field(default_factory=list)   # slime -> form
    morph_out: List[Frame] = field(default_factory=list)  # form -> slime
    extra: Dict[str, List[Frame]] = field(default_factory=dict)
    hop: bool = False


def build_form_defs(src: Dict[str, List[Frame]]) -> Dict[str, FormDef]:
    gif_normal = _seq(src, "gif_normal")
    slime_idle = gif_normal or _seq(src, "slime_idle")
    s0 = slime_idle[0] if slime_idle else Image.new("RGBA", (29, 22), CYAN)
    defs: Dict[str, FormDef] = {}

    extra = {
        "swirl": _seq(src, "idle_special"),
        "guard": _seq(src, "slime_guard"),
    }
    if gif_normal:
        # expressive cursor-pack slime: one real expression per emotion
        extra.update({
            "emo_talk": _seq(src, "gif_link"),
            "emo_confuso": _seq(src, "gif_help"),
            "emo_pensando": _seq(src, "gif_working"),
            "emo_orquestrando": _seq(src, "gif_busy"),
            "emo_bravo": _seq(src, "gif_handwriting"),
            "emo_triste": _seq(src, "gif_unavailable"),
            "emo_dormindo": _seq(src, "gif_unavailable"),
        })
    else:
        extra.update({"spin": _seq(src, "slime_air"), "point": _seq(src, "slime_basa"),
                      "reach": _seq(src, "slime_up")})
    defs["slime"] = FormDef("slime", idle=slime_idle if gif_normal else _ping_pong(slime_idle),
                            walk=[s0], hop=True, extra=extra)

    human_in = [s0] + _seq(src, "human_in")
    defs["humano"] = FormDef(
        "humano", idle=_ping_pong(_seq(src, "human_idle")), walk=_seq(src, "human_run"),
        run=_seq(src, "human_run"), morph_in=human_in, morph_out=list(reversed(human_in)),
        extra={"jump": _seq(src, "human_jump"), "guard": _seq(src, "human_guard"),
               # poses sem asas do começo da transformação alada: punho, comemorar, enxugar, apontar
               "poses": _seq(src, "alado_in", range(0, 4)), "mask": _seq(src, "human_in", range(4, 7))},
    )

    alado_in = human_in + _seq(src, "alado_in")
    defs["alado"] = FormDef(
        "alado", idle=_ping_pong(_seq(src, "alado_idle")), walk=_seq(src, "alado_fly"),
        run=_seq(src, "alado_fly"), morph_in=alado_in, morph_out=list(reversed(alado_in)),
        extra={"power": _seq(src, "alado_power"), "flap": _seq(src, "alado_idle", range(0, 4)),
               "fly": _seq(src, "alado_fly"), "point": _seq(src, "alado_in", range(4, 7))},
    )

    return defs


# ==========================================================================
# 4. Animation composition
# ==========================================================================
def _body_box(sprite: Frame, cx: float, base: float, dx=0, dy=0):
    x = int(round(cx - sprite.width / 2 + dx))
    y = int(round(base - sprite.height + dy))
    return x, y, x + sprite.width, y + sprite.height


SHADOW = (6, 18, 34, 255)


def _ground_shadow(canvas: Frame, cx: float, base_y: float, width: int, lift: float) -> None:
    """Dithered pixel-art contact shadow; it shrinks while the body is in the air."""
    shrink = max(0.35, 1.0 - max(0.0, -lift) / 24.0)
    half = max(3, int(width * 0.36 * shrink))
    d = ImageDraw.Draw(canvas)
    y0 = int(base_y)
    for dy_row, frac in ((0, 1.0), (1, 0.78)):
        span = int(half * frac)
        for x in range(int(cx) - span, int(cx) + span + 1):
            if (x + dy_row) % 2 == 0 or abs(x - cx) < span * 0.55:
                d.point((x, y0 + dy_row), fill=SHADOW)


def _compose(sprite: Frame, dx=0.0, dy=0.0, fx: Optional[Callable] = None,
             size=CANVAS, base=None, shadow: bool = True) -> Frame:
    canvas = _blank(size)
    cx = size[0] / 2
    base_y = (size[1] - 3) if base is None else base
    box = _body_box(sprite, cx, base_y, dx, dy)
    if shadow and sprite.width > 4:
        _ground_shadow(canvas, cx + dx, base_y, sprite.width, dy)
    _paste_clip(canvas, sprite, box[0], box[1])
    if fx:
        fx(ImageDraw.Draw(canvas), box)
    return canvas


def _face_y(sprite: Frame) -> int:
    """Row of the dark eye pixels in a slime frame (fallback: 55% height)."""
    a = np.asarray(sprite).astype(int)
    lum = a[..., :3].sum(axis=2)
    alpha = a[..., 3] > 0
    h = sprite.height
    inner = alpha.copy()
    inner[: h // 4] = False
    inner[(h * 3) // 4:] = False
    ys, _ = np.nonzero(inner & (lum < 200))
    return int(np.median(ys)) if len(ys) else int(h * 0.55)


def _loop(frames: List[Frame], n: int) -> List[Frame]:
    if not frames:
        return []
    return [frames[i % len(frames)] for i in range(n)]


def _compose_expression(anim: str, frames: List[Frame]) -> List[Frame]:
    """Real hand-drawn expressions (cursor-pack slime) plus a light FX touch."""
    out: List[Frame] = []
    seq = frames if len(frames) >= 6 else _loop(frames, 8)
    for i, f in enumerate(seq):
        spr, dx, dy = f, 0, 0
        fx: Optional[Callable] = None
        if anim == "talk":
            spr = _squash(f, 1.03, 0.97) if i % 2 else f

            def fx(d, box, i=i):
                hx, hy = box[2] + 2, box[1] + 6
                if i % 4 in (1, 2):
                    _px(d, [(hx, hy), (hx + 1, hy + 1), (hx + 1, hy + 2), (hx, hy + 3)], CYAN)
                if i % 4 == 2:
                    _px(d, [(hx + 3, hy - 1), (hx + 4, hy), (hx + 4, hy + 2), (hx + 4, hy + 3),
                            (hx + 3, hy + 4)], CYAN)
        elif anim == "bravo":
            spr = _tint(f, RED, 0.16 + 0.12 * (i % 2))
            dx = -1 if i % 2 else 1

            def fx(d, box, i=i):
                fx_anger(d, box[2] - 4, box[1] + 2 - (i % 2))
                if i % 4 < 2:
                    d.ellipse((box[0] - 1, box[1] - 4, box[0] + 2, box[1] - 1), fill=GREY)
        elif anim == "triste":
            def fx(d, box, i=i, f=f):
                # the tear rolls down outside the body so it is readable
                fx_drop(d, box[0] - 2, box[1] + f.height // 3 + (i % 6) * 2, (150, 220, 255, 255))
        elif anim == "confuso":
            def fx(d, box, i=i):
                if i % 4 >= 2:
                    fx_question(d, box[2] + 2, box[1] + 6, GOLD)
        elif anim == "dormindo":
            spr = _squash(_desaturate(f, 0.15), 1.06 + 0.02 * (i % 2), 0.9 - 0.02 * (i % 2))

            def fx(d, box, i=i):
                fx_zzz(d, box[2] - 3, box[1] - 2, i)
        elif anim == "pensando":
            def fx(d, box, i=i):
                # small dots above the head: "." -> ".." -> "..." -> "."
                cx = (box[0] + box[2]) // 2
                y = max(1, box[1] - 4)
                count = (i // 2) % 3 + 1          # ".", "..", "...", "." ...
                for k in range(count):
                    x = cx - 4 + k * 4
                    d.rectangle((x, y, x + 1, y + 1), fill=WHITE)
        elif anim == "orquestrando":
            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_gear(d, box[0] - 4, box[1] + 6, i)
                fx_gear(d, box[2] + 4, box[1] + 4, -i, GOLD)
        out.append(_compose(spr, dx=dx, dy=dy, fx=fx))
    return out


def compose_animation(form: FormDef, anim: str) -> List[Frame]:
    idle = form.idle or form.walk
    base = idle[0]
    is_slime = form.key == "slime"
    out: List[Frame] = []

    if anim == "idle":
        frames = idle if len(idle) >= 4 else _loop(idle, 8)
        n = len(frames)
        for i, f in enumerate(frames):
            if is_slime:
                s = math.sin(i / n * 2 * math.pi)
                f = _squash(f, 1 + 0.025 * s, 1 - 0.035 * s)
            out.append(_compose(f, dy=0))
        return out

    if anim in ("walk", "run"):
        if form.hop:
            # squash & stretch hop cycle
            fast = anim == "run"
            plan = [(1.18, 0.80, 0), (1.05, 0.98, -1), (0.90, 1.14, -5), (0.95, 1.06, -8 if fast else -7),
                    (1.0, 1.0, -6 if fast else -5), (1.02, 0.96, -2), (1.22, 0.78, 0), (1.10, 0.90, 0)]
            for i, (sx, sy, dy) in enumerate(plan):
                spr = _squash(base, sx, sy)

                def fx(d, box, i=i):
                    if i in (0, 6, 7):
                        fx_dust(d, box[0] + 2, box[3] - 1, i)
                out.append(_compose(spr, dy=dy, fx=fx))
            return out
        frames = form.run if (anim == "run" and form.run) else form.walk
        return [_compose(f) for f in _loop(frames, max(6, len(frames)))]

    emo = form.extra.get("emo_" + anim)
    if emo:
        return _compose_expression(anim, emo)

    if anim == "talk":
        face_y = _face_y(base)
        for i in range(8):
            f = idle[i % len(idle)]
            pulse = (1.04, 0.95) if i % 2 else (0.98, 1.03)
            spr = _squash(f, *pulse) if is_slime else f

            def fx(d, box, i=i, spr=spr):
                cx = (box[0] + box[2]) // 2
                if is_slime:
                    my = box[1] + int(face_y * spr.height / max(1, base.height)) + 3
                    if i % 2:
                        d.rectangle((cx - 1, my, cx + 1, my + 1), fill=(30, 40, 90, 255))
                        d.point((cx, my + 1), fill=(200, 110, 140, 255))
                    else:
                        d.line((cx - 1, my, cx + 1, my), fill=(30, 40, 90, 255))
                # sound waves beside the head
                hx, hy = box[2] + 2, box[1] + 4
                if i % 4 in (1, 2):
                    _px(d, [(hx, hy), (hx + 1, hy + 1), (hx + 1, hy + 2), (hx, hy + 3)], CYAN)
                if i % 4 == 2:
                    _px(d, [(hx + 3, hy - 1), (hx + 4, hy), (hx + 4, hy + 2), (hx + 4, hy + 3),
                            (hx + 3, hy + 4)], CYAN)
            out.append(_compose(spr, fx=fx))
        return out

    if anim == "feliz":
        if is_slime and form.extra.get("spin"):
            spin = form.extra["spin"]
            hops = [0, -3, -7, -10, -12, -12, -10, -7, -4, -2, 0, 0]
            for i, f in enumerate(spin):
                def fx(d, box, i=i):
                    cx = (box[0] + box[2]) // 2
                    fx_sparkle(d, box[0] - 3, box[1] + 2 + (i % 3), 1 + i % 2)
                    fx_sparkle(d, box[2] + 3, box[1] + 5 - (i % 3), 1 + (i + 1) % 2, CYAN)
                    if i >= 6:
                        fx_heart(d, cx + 6, box[1] - 4 - (i - 6))
                out.append(_compose(f, dy=hops[i % len(hops)], fx=fx))
            return out
        for i in range(8):
            f = idle[i % len(idle)]
            dy = -abs(round(math.sin(i / 8 * 2 * math.pi) * 5))

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_heart(d, cx + 8, box[1] - 3 - i % 3)
                fx_sparkle(d, box[0] - 3, box[1] + 4, 1 + i % 2)
            out.append(_compose(f, dy=dy, fx=fx))
        return out

    if anim == "bravo":
        for i in range(8):
            f = idle[i % len(idle)]
            spr = _tint(f, RED, 0.18 + 0.12 * (i % 2))
            if is_slime:
                spr = _squash(spr, 1.06, 0.94)

            def fx(d, box, i=i):
                fx_anger(d, box[2] - 2, box[1] + 1 - (i % 2))
                if i % 4 < 2:
                    d.ellipse((box[0] - 2, box[1] - 5, box[0] + 1, box[1] - 2), fill=GREY)
            out.append(_compose(spr, dx=(-1 if i % 2 else 1), fx=fx))
        return out

    if anim == "triste":
        for i in range(8):
            f = idle[(i // 2) % len(idle)]
            spr = _desaturate(f, 0.45, 0.12)
            if is_slime:
                spr = _squash(spr, 1.12, 0.84)

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_drop(d, box[0] + spr.width // 3, box[1] + spr.height // 2 + i, TEAR)
                d.ellipse((cx - 7, box[1] - 9, cx + 7, box[1] - 4), fill=GREY, outline=INK)
                if i % 2 == 0:
                    d.point((cx - 3 + i % 3, box[1] - 2), fill=TEAR)
            out.append(_compose(spr, fx=fx))
        return out

    if anim == "pensando":
        for i in range(8):
            f = idle[(i // 2) % len(idle)]

            def fx(d, box, i=i):
                fx_bubble(d, box[2] + 4, box[1] - 4, dots=(i // 2) % 4)
            out.append(_compose(f, dx=(1 if (i // 2) % 2 else 0), fx=fx))
        return out

    if anim == "alerta":
        plan = [(0.9, 1.15, -3), (0.92, 1.12, -5), (1.0, 1.0, -2), (1.05, 0.96, 0)] * 2
        for i, (sx, sy, dy) in enumerate(plan):
            spr = _squash(base, sx, sy) if is_slime else idle[i % len(idle)]

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_exclaim(d, cx + 1, box[1] - 4 - (2 if i < 2 else 0))
            out.append(_compose(spr, dy=dy if is_slime else (-2 if i < 2 else 0), fx=fx))
        return out

    if anim == "confuso":
        for i in range(8):
            f = idle[i % len(idle)]
            spr = f.rotate((6 if (i // 2) % 2 else -6), resample=Image.NEAREST, expand=True)

            def fx(d, box, i=i):
                fx_question(d, box[2] + 1, box[1] + 1 - i % 2)
                if i >= 4:
                    fx_question(d, box[0] - 3, box[1] + 3, GOLD)
            out.append(_compose(spr, fx=fx))
        return out

    if anim == "foto":
        for i in range(8):
            f = idle[i % len(idle)]
            spr = _flash(f, 0.85) if i in (3, 4) else f

            def fx(d, box, i=i):
                fx_camera(d, box[2] + 5, box[1] + 2)
                if i in (3, 4):
                    for k in range(4):
                        fx_sparkle(d, box[2] + 5 + (k - 1.5) * 4, box[1] - 4 + (k % 2) * 3, 2, WHITE)
            out.append(_compose(spr, fx=fx))
        return out

    if anim == "apontar":
        point = form.extra.get("point") if is_slime else None
        if point:
            seq = point + [point[-1]] * 5
            for i, f in enumerate(seq):
                def fx(d, box, i=i):
                    if i >= 2:
                        fx_sparkle(d, box[2] + 2, box[1] + 5, 1 + i % 2)
                out.append(_compose(f, dx=6, fx=fx))
            return out
        for i in range(8):
            f = idle[i % len(idle)]

            def fx(d, box, i=i):
                fx_arrow(d, box[2] + 2 + i % 3, box[1] + 10, 1)
            out.append(_compose(f, fx=fx))
        return out

    if anim == "dormindo":
        for i in range(6):
            f = base
            spr = _squash(_desaturate(f, 0.2), 1.12 + 0.03 * (i % 2), 0.82 - 0.03 * (i % 2)) \
                if is_slime else _desaturate(f, 0.2)

            def fx(d, box, i=i):
                fx_zzz(d, box[2] - 2, box[1] - 3, i)
            out.append(_compose(spr, fx=fx))
        return out

    if anim == "orquestrando":
        swirl = form.extra.get("swirl") or []
        for i in range(8):
            f = idle[i % len(idle)]

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                top = box[1] - 2
                if swirl:
                    s = swirl[i % len(swirl)]
                    _paste_clip(d._image, s, int(cx - s.width / 2), max(0, top - s.height))
                    top -= s.height
                fx_gear(d, cx - 8, max(4, top - 3), i)
                fx_gear(d, cx + 8, max(4, top - 5), -i, CYAN)
                fx_note(d, box[2] + 3, box[1] + 2 - i % 3)
            out.append(_compose(f, fx=fx))
        return out

    if anim == "teleport":
        steps = [1.0, 0.85, 0.65, 0.45, 0.28, 0.12, 0.0, 0.12, 0.3, 0.55, 0.8, 1.0]
        for i, k in enumerate(steps):
            spr = _dither_fade(_flash(base, 1 - k), k) if k < 1 else base

            def fx(d, box, i=i, k=k):
                cx = (box[0] + box[2]) // 2
                for j in range(4):
                    yy = box[3] - ((i * 5 + j * 9) % 36)
                    fx_sparkle(d, cx - 8 + j * 5, yy, 1, CYAN if j % 2 else WHITE)
            out.append(_compose(spr, dy=-round((1 - k) * 4), fx=fx))
        return out

    if anim == "transform":
        for i in range(10):
            k = abs(5 - i) / 5.0
            spr = _flash(base, 1 - k)

            def fx(d, box, i=i):
                cx, cy = (box[0] + box[2]) // 2, (box[1] + box[3]) // 2
                r = 4 + i * 2
                for j in range(8):
                    ang = j * math.pi / 4 + i * 0.3
                    fx_sparkle(d, cx + math.cos(ang) * r, cy + math.sin(ang) * r * 0.7,
                               1, CYAN if j % 2 else GOLD)
            out.append(_compose(spr, fx=fx))
        return out

    if anim in ("morph_in", "morph_out"):
        seq = form.morph_in if anim == "morph_in" else form.morph_out
        if not seq:
            seq = [base]
        n = len(seq)
        for i, f in enumerate(seq):
            edge = (i == 0) or (i == n - 1)
            spr = f if edge else _flash(f, 0.15)

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                for j in range(3):
                    fx_sparkle(d, cx - 10 + j * 10, box[3] - 3 - ((i * 4 + j * 7) % 20),
                               1, CYAN if (i + j) % 2 else WHITE)
            out.append(_compose(spr, fx=fx))
        return out

    return [_compose(base)]


# ==========================================================================
# 5. Clones (worker agents)
# ==========================================================================
def _clone_tint(f: Frame) -> Frame:
    """Clones are a slightly paler, glassier copy of S.A.G.E. with a cyan rim."""
    return _outline(_tint(f, (190, 235, 255), 0.18), CYAN)


def compose_clone_animation(src: Dict[str, List[Frame]], anim: str) -> List[Frame]:
    size = CLONE_CANVAS
    normal = _seq(src, "gif_normal")
    working = _seq(src, "gif_working")
    sad = _seq(src, "gif_unavailable")
    if not normal:                                   # fallback: old human clone art
        normal = _seq(src, "clone_idle") or [Image.new("RGBA", (20, 20), CYAN)]
    # the "..." dots of the working face are drawn by us, smaller, above the head
    work_frames = [_clone_tint(f) for f in (working or normal)]
    idle = [_clone_tint(f) for f in normal]
    out: List[Frame] = []

    if anim == "spawn":
        # a droplet falls, splashes into a puddle and grows into a clone
        base = idle[0]
        plan = [(0.18, 0.25, -26), (0.2, 0.3, -14), (0.22, 0.32, -4), (0.55, 0.22, 0),
                (0.8, 0.45, 0), (1.12, 0.78, 0), (0.94, 1.12, -3), (1.0, 1.0, 0)]
        for i, (sx, sy, dy) in enumerate(plan):
            spr = _squash(base, sx, sy)
            if i < 6:
                spr = _flash(spr, 0.25)

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                if i == 3:
                    for k in (-1, 1):
                        fx_drop(d, cx + k * 9, box[3] - 5, CYAN)
                if i >= 5:
                    fx_sparkle(d, box[2] + 2, box[1] + 2, 1, WHITE)
            out.append(_compose(spr, dy=dy, fx=fx, size=size))
        return out

    if anim == "work":
        for i in range(12):
            f = work_frames[i % len(work_frames)]

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                count = (i // 2) % 3 + 1
                for k in range(count):
                    x = cx - 4 + k * 4
                    d.rectangle((x, max(1, box[1] - 4), x + 1, max(2, box[1] - 3)), fill=WHITE)
                fx_holo_screen(d, box[2] + 1, box[3] - 8, i)
            out.append(_compose(f, dx=-6, fx=fx, size=size))
        return out

    if anim == "done":
        for i in range(8):
            f = idle[i % len(idle)]
            hop = [0, -3, -5, -3, 0, 0, 0, 0][i]

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_check(d, cx, max(6, box[1] - 7))
            out.append(_compose(f, dy=hop, fx=fx, size=size))
        return out

    if anim == "fail":
        frames = [_clone_tint(f) for f in sad] or idle
        for i in range(8):
            f = _desaturate(frames[i % len(frames)], 0.35)

            def fx(d, box, i=i):
                cx = (box[0] + box[2]) // 2
                fx_cross(d, cx, max(6, box[1] - 7))
            out.append(_compose(f, dx=(-1 if i % 2 else 1), fx=fx, size=size))
        return out

    if anim == "vanish":
        last = idle[0]
        for k in (1.0, 0.85, 0.65, 0.45, 0.28, 0.12, 0.0):
            spr = _squash(last, 1 + (1 - k) * 0.3, max(0.2, k)) if k > 0 else _blank((1, 1))
            if 0 < k < 1:
                spr = _dither_fade(_flash(spr, 1 - k), k)

            def fx(d, box, k=k):
                cx = (box[0] + box[2]) // 2
                t = 1 - k
                for j in range(5):
                    fx_drop(d, cx - 10 + j * 5, box[3] - 6 - t * 26 - (j % 2) * 4, CYAN)
            out.append(_compose(spr, fx=fx, size=size, shadow=k > 0.3))
        return out

    return [_compose(idle[0], size=size)]


# ==========================================================================
# 6. Build / IO
# ==========================================================================
def _upscale(img: Frame, scale: int) -> Frame:
    return img.resize((img.width * scale, img.height * scale), Image.NEAREST)


def _user_locked(directory: str, anim: str) -> bool:
    """Animations edited in the Sprite Studio are never overwritten by a rebuild."""
    try:
        with open(os.path.join(ASSETS_DIR, "sprites", "sprite_meta.json"), encoding="utf-8") as f:
            meta = json.load(f)
    except Exception:
        return False
    form = "clone" if os.path.abspath(directory) == os.path.abspath(CLONE_DIR) else os.path.basename(directory)
    return bool((meta.get(form + "/" + anim) or {}).get("locked"))


def _write(frames: List[Frame], directory: str, anim: str, scale: int) -> int:
    os.makedirs(directory, exist_ok=True)
    if _user_locked(directory, anim):
        return 0
    # remove stale frames of this animation
    idx = 0
    while True:
        stale = os.path.join(directory, anim + "_" + str(idx) + ".png")
        if not os.path.exists(stale):
            break
        os.remove(stale)
        idx += 1
    for i, f in enumerate(frames):
        _upscale(f, scale).save(os.path.join(directory, anim + "_" + str(i) + ".png"), "PNG")
    return len(frames)


def build_icon(src: Dict[str, List[Frame]], assets_dir: str = ASSETS_DIR) -> Optional[str]:
    frames = src.get("slime_idle") or []
    if not frames:
        return None
    s = frames[0]
    scale = max(1, 220 // max(s.width, s.height))
    big = _upscale(s, scale)
    canvas = Image.new("RGBA", (256, 256), (0, 0, 0, 0))
    canvas.alpha_composite(big, ((256 - big.width) // 2, (256 - big.height) // 2 + 10))
    os.makedirs(assets_dir, exist_ok=True)
    png = os.path.join(assets_dir, "sage_icon.png")
    ico = os.path.join(assets_dir, "sage.ico")
    canvas.save(png, "PNG")
    canvas.save(ico, format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48),
                                           (64, 64), (128, 128), (256, 256)])
    return ico


def build_all(force: bool = False, verbose: bool = True,
              forms_dir: str = FORMS_DIR, clone_dir: str = CLONE_DIR,
              source_dir: str = SOURCE_DIR, icons: bool = True, **kw) -> Dict[str, int]:
    """Builds every animation with the sprite generator (sage.sprite_gen).

    The slime and the clones are procedural; the other forms reuse the bodies
    cut from the reference sheets by this module.
    """
    from sage import sprite_gen
    return sprite_gen.build_all(force=force, verbose=verbose, forms_dir=forms_dir, clone_dir=clone_dir,
                                source_dir=source_dir, icons=icons, **kw)


def build_all_legacy(force: bool = False, verbose: bool = True,
              forms_dir: str = FORMS_DIR, clone_dir: str = CLONE_DIR,
              source_dir: str = SOURCE_DIR, icons: bool = True) -> Dict[str, int]:
    stamp = os.path.join(forms_dir, ".build_stamp")
    if not force and os.path.exists(stamp):
        try:
            if open(stamp, encoding="utf-8").read().strip() == BUILD_STAMP:
                return {}
        except Exception:
            pass
    src = extract_sources(source_dir)
    if not src:
        if verbose:
            print("[Avatar] Sheets de origem não encontradas em " + source_dir)
        return {}
    defs = build_form_defs(src)
    stats: Dict[str, int] = {}
    for key, form in defs.items():
        directory = os.path.join(forms_dir, key)
        total = 0
        for anim in EMOTIONS + EXTRA_ANIMS:
            total += _write(compose_animation(form, anim), directory, anim, SCALE)
        stats[key] = total
        if verbose:
            print("[Avatar] " + key + ": " + str(total) + " frames")
    total = 0
    for anim in CLONE_ANIMS:
        total += _write(compose_clone_animation(src, anim), clone_dir, anim, CLONE_SCALE)
    stats["clone"] = total
    if icons:
        build_icon(src)
    os.makedirs(forms_dir, exist_ok=True)
    with open(stamp, "w", encoding="utf-8") as f:
        f.write(BUILD_STAMP)
    return stats


def animation_paths(form: str, anim: str, forms_dir: str = FORMS_DIR) -> List[str]:
    directory = clone_dir_or_form(form, forms_dir)
    paths = []
    for idx in range(200):
        path = os.path.join(directory, anim + "_" + str(idx) + ".png")
        if not os.path.exists(path):
            break
        paths.append(path)
    return paths


def clone_dir_or_form(form: str, forms_dir: str = FORMS_DIR) -> str:
    if form == "clone":
        return CLONE_DIR
    return os.path.join(forms_dir, form)


def resolve_form(name: str) -> Optional[str]:
    key = (name or "").lower().strip()
    if key in FORMS:
        return key
    return FORM_ALIASES.get(key)


def render_showcase(path: str, forms_dir: str = FORMS_DIR) -> str:
    """Contact sheet: every form x a few emotions (for docs / visual QA)."""
    anims = ("idle", "talk", "feliz", "bravo", "triste", "pensando", "alerta", "amor", "determinado",
             "sabio", "dormindo")
    tiles = []
    for form in FORMS:
        row = []
        for anim in anims:
            paths = animation_paths(form, anim, forms_dir)
            if paths:
                row.append(Image.open(paths[len(paths) // 3]).convert("RGBA"))
        if row:
            tiles.append(row)
    if not tiles:
        return ""
    tw, th = tiles[0][0].size
    sheet = Image.new("RGBA", (tw * len(anims), th * len(tiles)), (24, 28, 44, 255))
    for r, row in enumerate(tiles):
        for c, img in enumerate(row):
            sheet.alpha_composite(img, (c * tw, r * th))
    sheet.save(path)
    return path


if __name__ == "__main__":
    import sys
    print(build_all(force="--force" in sys.argv))
    out = os.path.join(ASSETS_DIR, "showcase", "pixel_forms.png")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    print(render_showcase(out))
