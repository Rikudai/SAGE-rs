# Port Specification: S.A.G.E. Slime Avatar (Rust / egui)

## 1. Sprite Asset Architecture: Pre-rendered vs. Procedural
* **Final Animations on Disk**: Pre-baked and fully available on disk as sequenced 32-bit RGBA PNG files. A Rust app **can play them directly** without procedural rendering ([sprite_overlay.py:52-63](file:///C:/SAGE/sage/sprite_overlay.py#L52-L63), [sprite_overlay.py:279-370](file:///C:/SAGE/sage/sprite_overlay.py#L279-L370)).
* **Procedural Pipeline**: Python generates frames via `sage.sprite_gen` ([sprite_gen.py:1-21](file:///C:/SAGE/sage/sprite_gen.py#L1-L21)) and `sage.slime_rig` ([slime_rig.py:4-17](file:///C:/SAGE/sage/slime_rig.py#L4-L17)) using supersampled vector math and NumPy. Porting the procedural rig to Rust is **hard / unnecessary** because frames are cached and validated by `.build_stamp` (`sprite-gen-v3|gel=0.90`) ([sprite_gen.py:2011-2015](file:///C:/SAGE/sage/sprite_gen.py#L2011-L2015)).
* **Disk Structure & Frame Dimensions**:
  * `assets/sprites/forms/<form>/<anim>_<index>.png`: Main companion forms (`slime`, `humano`, `alado`). Logical canvas `(135, 108)` scaled 2x yields **270x216 px** ([slime_rig.py:27-28](file:///C:/SAGE/sage/slime_rig.py#L27-L28), [sprite_gen.py:39-41](file:///C:/SAGE/sage/sprite_gen.py#L39-L41), [pixel_avatar.py:47-48](file:///C:/SAGE/sage/pixel_avatar.py#L47-L48)).
  * `assets/sprites/clone/<anim>_<index>.png`: Multi-agent worker clones (`spawn`, `work`, `done`, `fail`, `vanish`). Logical canvas `(64, 58)` scaled 2x yields **128x116 px** ([sprite_gen.py:42-44](file:///C:/SAGE/sage/sprite_gen.py#L42-L44), [sprite_overlay.py:650](file:///C:/SAGE/sage/sprite_overlay.py#L650)).
  * `assets/pointer/pointer_<index>.png`: Pulsing screen target, 24 frames at **320x320 px** ([character_assets.py:28-29](file:///C:/SAGE/sage/character_assets.py#L28-L29)).
  * `assets/sprites/`: Static 64x64/260x260 fallback sprites (`idle.png`, `idle_talk.png`) ([pet_window_qt.py:64-82](file:///C:/SAGE/sage/pet_window_qt.py#L64-L82)).
  * `assets/sprites/rimuru/source/`: Raw sprite sheets (`sheet_yang.jpg`, `sheet_full.png`, `slime_cursors.gif`) ([pixel_avatar.py:42](file:///C:/SAGE/sage/pixel_avatar.py#L42)).
  * Manifests: `assets/sprites/forms/.build_stamp` (version stamp). `assets/sprites/sprite_meta.json` provides optional custom timing overrides ([pixel_avatar.py:1109-1114](file:///C:/SAGE/sage/pixel_avatar.py#L1109-L1114), [sprite_overlay.py:978-984](file:///C:/SAGE/sage/sprite_overlay.py#L978-L984)).

---

## 2. Animation States, Frame Timing & Triggers
* **Frame Timing**:
  * Default frame durations range from 45ms to 120ms (~8.3 to 22.2 FPS) defined in `FRAME_MS` ([pixel_avatar.py:99-113](file:///C:/SAGE/sage/pixel_avatar.py#L99-L113)) and `ANIM_FRAME_MS` ([sprite_overlay.py:73-77](file:///C:/SAGE/sage/sprite_overlay.py#L73-L77), [sprite_overlay.py:985-988](file:///C:/SAGE/sage/sprite_overlay.py#L985-L988)).
  * Common frame delays: `idle` (70ms), `walk` (55ms), `run` (45ms), `talk` (60ms), `pensando` (75ms), `dormindo` (110ms), `orquestrando` (75ms), `teleport` (40ms), `clone` (50–70ms).
* **State Taxonomy & Triggers**:
  * **Idle**: Baseline loop when no speech, locomotion, or busy state is active ([sprite_overlay.py:1692](file:///C:/SAGE/sage/sprite_overlay.py#L1692)).
  * **Talking**: Triggered by `state.set_speaking(True)`. Plays `talk` and toggles `mouth_open` boolean every tick step ([sprite_overlay.py:1098-1111](file:///C:/SAGE/sage/sprite_overlay.py#L1098-L1111), [sprite_overlay.py:1674-1676](file:///C:/SAGE/sage/sprite_overlay.py#L1674-L1676)).
  * **Listening**: Triggered by `AppStatus.LISTENING`. Companion leans in with cupped ear (`ouvindo`) ([orchestrator.py:202-203](file:///C:/SAGE/sage/orchestrator.py#L202-L203), [sprite_overlay.py:1113-1125](file:///C:/SAGE/sage/sprite_overlay.py#L1113-L1125)).
  * **Thinking**: Triggered by `AppStatus.PROCESSING`. Plays `pensando`; if `deep_thinking=True` (Claude/deep mode), switches pose to `sabio` ([orchestrator.py:540](file:///C:/SAGE/sage/orchestrator.py#L540), [orchestrator.py:848-849](file:///C:/SAGE/sage/orchestrator.py#L848-L849), [sprite_overlay.py:1688-1689](file:///C:/SAGE/sage/sprite_overlay.py#L1688-L1689)).
  * **Clones**: Swarm jobs trigger `orquestrando` (or `maestro` when >= 2 clones active) ([orchestrator.py:1238](file:///C:/SAGE/sage/orchestrator.py#L1238), [sprite_overlay.py:1250-1260](file:///C:/SAGE/sage/sprite_overlay.py#L1250-L1260), [sprite_overlay.py:1690-1691](file:///C:/SAGE/sage/sprite_overlay.py#L1690-L1691)). Each worker clone runs in a secondary window cycling `spawn` -> `work` -> `done`/`fail` -> `vanish` ([sprite_overlay.py:446-636](file:///C:/SAGE/sage/sprite_overlay.py#L446-L636)). Companion reacts with `orgulhoso_clone` or `bronca_clone` ([sprite_overlay.py:1287-1300](file:///C:/SAGE/sage/sprite_overlay.py#L1287-L1300)).
  * **Transformations**: Triggered by voice/tray command or `PetBrain` whim ([orchestrator.py:898-906](file:///C:/SAGE/sage/orchestrator.py#L898-L906), [pet_brain.py:324-343](file:///C:/SAGE/sage/pet_brain.py#L324-L343)). Executes transition sequence: `current -> morph_out -> load target -> morph_in` ([sprite_overlay.py:1198-1217](file:///C:/SAGE/sage/sprite_overlay.py#L1198-L1217)).
  * **Emotions**: Set via LLM response intent or `PetBrain` ([state.py:103-114](file:///C:/SAGE/sage/state.py#L103-L114), [orchestrator.py:564](file:///C:/SAGE/sage/orchestrator.py#L564)). 28 emotions total (`feliz`, `bravo`, `triste`, `confuso`, `alerta`, `amor`, etc.) ([pixel_avatar.py:71-77](file:///C:/SAGE/sage/pixel_avatar.py#L71-L77)). Non-busy emotions auto-relax back to `idle` after 6–12 seconds ([pet_brain.py:49-51](file:///C:/SAGE/sage/pet_brain.py#L49-L51), [sprite_overlay.py:1127-1138](file:///C:/SAGE/sage/sprite_overlay.py#L1127-L1138)).
  * **Locomotion & Dragging**: Wanders inside `leash_radius` (default 260px) playing `walk` (speed 2.6) or `run` (speed 4.6), horizontally mirrored if walking left ([sprite_overlay.py:828](file:///C:/SAGE/sage/sprite_overlay.py#L828), [sprite_overlay.py:1465-1475](file:///C:/SAGE/sage/sprite_overlay.py#L1465-L1475), [sprite_overlay.py:1703-1707](file:///C:/SAGE/sage/sprite_overlay.py#L1703-L1707)). Mouse drag switches state to `arrastado`, and mouse release drops into `pouso` ([sprite_overlay.py:1679-1680](file:///C:/SAGE/sage/sprite_overlay.py#L1679-L1680), [sprite_overlay.py:1748-1751](file:///C:/SAGE/sage/sprite_overlay.py#L1748-L1751)).

---

## 3. Window Behavior & Input Handling
* **Window Attributes**: Frameless (`overrideredirect`), always on top (`-topmost`), tool window ([sprite_overlay.py:867-868](file:///C:/SAGE/sage/sprite_overlay.py#L867-L868), [pet_window_qt.py:39-43](file:///C:/SAGE/sage/pet_window_qt.py#L39-L43)).
* **Transparency**: 32-bit premultiplied ARGB layered surface via Win32 `UpdateLayeredWindow` ([sprite_overlay.py:877-879](file:///C:/SAGE/sage/sprite_overlay.py#L877-L879)) with fallback to color-key (`#041A34`) ([sprite_overlay.py:869-884](file:///C:/SAGE/sage/sprite_overlay.py#L869-L884)).
* **Click-through**:
  * Main Companion: **NOT** click-through. It intercepts mouse events for drag and petting ([sprite_overlay.py:874](file:///C:/SAGE/sage/sprite_overlay.py#L874), [sprite_overlay.py:888-891](file:///C:/SAGE/sage/sprite_overlay.py#L888-L891)).
  * Screen Pointer Overlay (`PointerOverlay`): **IS** click-through via `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE` ([sprite_overlay.py:141-158](file:///C:/SAGE/sage/sprite_overlay.py#L141-L158)).
* **Drag & Position**: Left-click drag moves window and updates anchor/home position on release ([sprite_overlay.py:1712-1751](file:///C:/SAGE/sage/sprite_overlay.py#L1712-L1751)). Initial position sits at bottom-right corner: `screen_w - win_w - 40`, `screen_h - win_h - 60` ([sprite_overlay.py:860-863](file:///C:/SAGE/sage/sprite_overlay.py#L860-L863)).
* **Speech Bubble**: **None exists** on the floating pet. Spoken responses are audio TTS only; visual text was delegated to the Web HUD / thinking overlay ([orchestrator.py:209-214](file:///C:/SAGE/sage/orchestrator.py#L209-L214)).
* **Click Menu**:
  * Left-click: Drag ([sprite_overlay.py:1712-1715](file:///C:/SAGE/sage/sprite_overlay.py#L1712-L1715)).
  * Double-click: Petting interaction -> fires `petted` event, reacts with `feliz` for 2.4s ([sprite_overlay.py:1752-1756](file:///C:/SAGE/sage/sprite_overlay.py#L1752-L1756)).
  * Context menu: Bound to the **Windows System Tray**, not the pet window ([ui.py:887-920](file:///C:/SAGE/sage/ui.py#L887-L920)).

---

## 4. Brain / Pet Communication & Events
* **Brain to Pet** (`PetBrain.tick` actions executed by `DesktopPetWindow._do_brain_action`) ([pet_brain.py:204-385](file:///C:/SAGE/sage/pet_brain.py#L204-L385), [sprite_overlay.py:1532-1570](file:///C:/SAGE/sage/sprite_overlay.py#L1532-L1570)):
  * `{"type": "relax"}`: Resets mood to `idle`.
  * `{"type": "anim", "name": str, "loops": int}`: 17 spontaneous fidgets (`acenar`, `bocejo`, `quicar`, `danca`, `bolha`, `derreter`, `espiar`, `piscadela`, `predador`, `olhar_esq`, `olhar_dir`, `olhar_cima`, etc.).
  * `{"type": "emotion", "name": str, "seconds": float}`: Temporary emotion reaction.
  * `{"type": "walk", "run": bool}`: Autonomous wander inside leash.
  * `{"type": "form", "name": str, "hold": int, "roam": bool}`: Spontaneous form change.
  * `{"type": "sleep"}`: Triggers yawn and enters `dormindo` state.
  * `{"type": "wake", "then": str}`: Wakes up when user returns, waves (`acenar`) or stretches (`espreguicar`).
* **Pet / Orchestrator to Brain** (`PetBrain.on_event`) ([pet_brain.py:141-171](file:///C:/SAGE/sage/pet_brain.py#L141-L171), [sprite_overlay.py:1149-1153](file:///C:/SAGE/sage/sprite_overlay.py#L1149-L1153)):
  * `"petted"`: Increases affection (+0.04) and joy (+0.15); saved to `data/pet_mood.json`.
  * `"drag_start"`: Boosts curiosity (+0.1), reduces boredom (-0.2).
  * `"drop"`: Increments joy (+0.03).
  * `"task_done"`: Swarm completion (+0.2 joy, -0.4 boredom).
  * `"task_fail"`: Swarm failure (-0.2 joy).
  * `"interaction"`: User prompt received (-0.35 boredom, +0.1 curiosity).
  * `"emotion:<name>"`: Shifts emotional valence.

---

## 5. Relevant Configuration Keys
Read from `config.json` without secret values ([config.json:301-316](file:///C:/SAGE/config.json#L301-L316)):
* `enable_desktop_pet`: bool (master toggle)
* `sprites_dir`: string path to sprite directory
* `pet_size`: int (display size)
* `enable_pet_walking`: bool
* `pet_form`: string default form (`slime`, `humano`, `alado`)
* `pet_leash_radius`: float (pixels)
* `pet_pointer_seconds`: float
* `pet_auto_form_seconds`: int
* `pet_form_hold_seconds`: int
* `pet_sleep_after_seconds`: int
* `pet_opacity`: float (0.1–1.0)
* `pet_gel_opacity`: float (0.2–1.0)
* `pet_scale`: float (0.5–2.0)
* `pet_personality`: string preset (`brincalhao`, `calmo`, `curioso`, `dorminhoco`, `serio`)
* `pet_activity`: float (0.0–1.0)
* `pet_free_will`: bool

---

## Uncertainties
1. **egui Per-Pixel Alpha Hit-Testing**: egui running on `winit` handles frameless transparent windows, but passing mouse clicks through transparent alpha regions directly to underlying Windows desktop applications requires Win32 custom hit testing (`WM_NCHITTEST` returning `HTTRANSPARENT` on zero-alpha pixels) or shaped windows.
2. **Sub-Window Management**: Python spawned independent top-level Tk windows for each worker clone and pointer overlay. In egui/eframe, managing multiple transparent native windows requires multi-viewport support or native OS child window handles.

---

## Handoff
* **Outcome**: Completed architectural scouting and delivered concise port specification for the SLIME AVATAR unit.
* **Evidence**: Verified all animation frames on disk under `assets/sprites/forms/` and `assets/sprites/clone/`. Identified exact canvas resolutions (270x216 companion, 128x116 clones), animation timings, state transition machines, window behavior, and event protocols.
* **Files Changed**: None (read-only scout).
* **Checks Run**: File readings and image verifications via `read_file` under `C:/SAGE`. No terminal commands executed. No secrets revealed.
* **Failures**: None.
* **Risks**: None for code; implementation risk in Rust centers on multi-window transparent hit-testing in egui.

STATUS: DONE
SUMMARY: The slime avatar animations exist on disk as pre-baked PNG sequences (270x216 companion, 128x116 clones) ready for an egui player, requiring no procedural porting.
