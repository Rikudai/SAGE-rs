# Port Specification: Persona & Prompts (Rust / native egui)

Port specification for the `PERSONA & PROMPTS` subsystem from Python to Rust.

---

## 1. Named Prompt Blocks in `sage/persona.py`

| Block Constant | Purpose | Placeholders & Fill Method | File & Line |
| :--- | :--- | :--- | :--- |
| `IDENTITY` | Core identity (blue slime technical partner), demeanor, conversational boundaries, response styling. | `{name_rule}`: filled dynamically by `_name_rule(user_name)` | [`persona.py:16-39`](file:///C:/SAGE/sage/persona.py#L16-L39) |
| `THINKING` | 6-step internal pre-response reasoning protocol (resolve refs, scope, act, verify, recover, conclude). | *None* | [`persona.py:44-54`](file:///C:/SAGE/sage/persona.py#L44-L54) |
| `SCOPES` | Taxonomy of request scopes mapping user intent to specific models/clones. | *None* | [`persona.py:59-77`](file:///C:/SAGE/sage/persona.py#L59-L77) |
| `PLANNING` | Rules for task breakdown (by deliverable, $\le 4$ tasks, parallel execution, self-contained specs). | *None* | [`persona.py:82-95`](file:///C:/SAGE/sage/persona.py#L82-L95) |
| `AUTONOMY` | Boundaries between autonomous execution (low-risk/reversible) vs. user confirmation prompts. | *None* | [`persona.py:100-112`](file:///C:/SAGE/sage/persona.py#L100-L112) |
| `NAME` | Immutable lore explaining the S.A.G.E. acronym, Rimuru tribute, and "Seije" pronunciation. | *None* | [`persona.py:117-126`](file:///C:/SAGE/sage/persona.py#L117-L126) |
| `WINDOWS_RULES` | Mandatory Windows 11 / PowerShell requirements and forbidden bash syntax equivalents. | *None* | [`persona.py:131-153`](file:///C:/SAGE/sage/persona.py#L131-L153) |
| `EMOTIONS` | 18 allowed avatar emotion triggers driving visual state transitions. | *None* | [`persona.py:158-166`](file:///C:/SAGE/sage/persona.py#L158-L166) |

*Note*: `BLOCK_NAMES` ([`persona.py:213`](file:///C:/SAGE/sage/persona.py#L213)) exposes `IDENTITY`, `THINKING`, `SCOPES`, `PLANNING`, `AUTONOMY`, and `EMOTIONS` for user override via `data/persona.json` ([`persona.py:212, 233-259`](file:///C:/SAGE/sage/persona.py#L212)). `NAME` and `WINDOWS_RULES` are fixed and cannot be edited.

---

## 2. User-Name Rule

Defined in `_name_rule(user_name: Optional[str]) -> str` ([`persona.py:169-173`](file:///C:/SAGE/sage/persona.py#L169-L173)):
- **With Name**: Returns `"Chame-o pelo nome, {name}, de vez em quando (não em toda frase)."`
- **Empty / None**: Returns `"Não invente um nome para ele."`
- Inserted into `IDENTITY` at line 22 via `core(user_name)` ([`persona.py:176-178`](file:///C:/SAGE/sage/persona.py#L176-L178)). When saving custom overrides, if `{name_rule}` is omitted, `\n{name_rule}\n` is automatically re-appended ([`persona.py:247-248`](file:///C:/SAGE/sage/persona.py#L247-L248)).

---

## 3. Final System Prompt Assembly by Role

1. **Router**:
   - Assembled as `PERSONA + "\n" + AUTONOMY + WINDOWS_RULES + ROUTER_RULES` ([`cortex.py:133`](file:///C:/SAGE/sage/cortex.py#L133), [`persona.py:273`](file:///C:/SAGE/sage/persona.py#L273)).
   - Runtime additions: `{screen_w}` and `{screen_h}` interpolated into `ROUTER_RULES`, followed by `EVOLUTION_RULES` ([`cortex.py:48-61`](file:///C:/SAGE/sage/cortex.py#L48-L61)) and dynamically loaded skills list from `SKILL_CATALOG()` ([`cortex.py:349-357`](file:///C:/SAGE/sage/cortex.py#L349-L357)).
2. **Chat**:
   - Assembled as `PERSONA + "\n\n" + style_text` ([`cortex.py:477`](file:///C:/SAGE/sage/cortex.py#L477)).
   - `style_text`: selects `CHAT_STYLE_LONG` ([`cortex.py:139-142`](file:///C:/SAGE/sage/cortex.py#L139-L142)) if the profile requires `"detailed"` or prompt triggers explanation regex (`explique|detalh|passo a passo|...`); otherwise selects `CHAT_STYLE_SHORT` ([`cortex.py:135-138`](file:///C:/SAGE/sage/cortex.py#L135-L138)).
3. **Brain**:
   - Assembled by `persona.brain(user_name)` ([`persona.py:186-191`](file:///C:/SAGE/sage/persona.py#L186-L191)): combines `core(user_name)`, `THINKING`, `SCOPES`, `PLANNING`, `AUTONOMY`, and `"Você é o CÉREBRO PRINCIPAL: decide, cria clones, revisa as entregas e fala com o usuário."`. Bound in swarm as `BRAIN_PERSONA` ([`swarm.py:899, 1334`](file:///C:/SAGE/sage/swarm.py#L899)).
4. **Clone**:
   - Base template `ROLE_BASE` ([`swarm.py:549-555`](file:///C:/SAGE/sage/swarm.py#L549-L555)): concise worker persona + `WINDOWS_RULES`.
   - Tool agents extend this with `AGENT_BASE` ([`swarm.py:557-566`](file:///C:/SAGE/sage/swarm.py#L557-L566)) specifying autonomous execution, MCP servers, and Git Bash / PowerShell guidelines.
   - Per-role instructions appended via `ROLES[key].system` ([`swarm.py:568-732`](file:///C:/SAGE/sage/swarm.py#L568-L732)). Generic override builder provided in `persona.clone(role_text)` ([`persona.py:194-202`](file:///C:/SAGE/sage/persona.py#L194-L202)).

---

## 4. Canned Phrases Categories (`sage/phrases.py`)

- **`WELCOME`** ([`phrases.py:17-48`](file:///C:/SAGE/sage/phrases.py#L17-L48)): Keyed by time of day:
  - `"manha"` (05:00–11:59), `"tarde"` (12:00–17:59), `"noite"` (18:00–23:59), `"madrugada"` (00:00–04:59), and `"qualquer"` (combined with active period).
- **`FAREWELL`** ([`phrases.py:50-59`](file:///C:/SAGE/sage/phrases.py#L50-L59)): 8 departure phrases.
- **`CLONE_OK`** ([`phrases.py:61-69`](file:///C:/SAGE/sage/phrases.py#L61-L69)): 7 lines taking credit for clone success; supports `{tarefa}` placeholder.
- **`CLONE_FAIL`** ([`phrases.py:71-78`](file:///C:/SAGE/sage/phrases.py#L71-L78)): 6 lines blaming the clone copy; supports `{tarefa}` placeholder.
- **Selection logic** ([`phrases.py:83-111`](file:///C:/SAGE/sage/phrases.py#L83-L111)): `pick()` filters `{tarefa}` if empty, uses a recent history ring buffer (`maxlen <= 3`) to avoid immediate repeats, and replaces `{tarefa}` with `(tarefa or "a tarefa")`.

---

## 5. Voice Profiles & Pre-TTS Processing

- **Profiles** (`sage/voice_profiles.py`):
  - `VoiceProfile` struct with Fish Audio cloud IDs and Edge-TTS offline fallbacks ([`voice_profiles.py:14-26`](file:///C:/SAGE/sage/voice_profiles.py#L14-L26)).
  - Profiles: `"rimuru"` (default, Thalita / rate +6% / pitch +10Hz), `"sabio"` (Francisca / rate +3% / pitch +2Hz), `"classica"` (Antonio / rate +0% / pitch -2Hz) ([`voice_profiles.py:29-65`](file:///C:/SAGE/sage/voice_profiles.py#L29-L65)).
- **Text Post-Processing** (`sage/text_processing.py`):
  - `pronounce_name(text)` ([`text_processing.py:154-170`](file:///C:/SAGE/sage/text_processing.py#L154-L170)): Converts `S.A.G.E.`, `SAGE`, or `Sage` to `"Seije"`. Handles trailing period (kept as sentence closer only if followed by uppercase/end of text). Excludes file extensions like `.py` and `.json`.
  - `simplify_url_for_speech(url)` ([`text_processing.py:81-113`](file:///C:/SAGE/sage/text_processing.py#L81-L113)): Uses `FRIENDLY_DOMAINS` ([`text_processing.py:11-35`](file:///C:/SAGE/sage/text_processing.py#L11-L35)) or domain basenames to pronounce domain names comfortably.
  - `sanitize_for_speech(text, max_sentences=4)` ([`text_processing.py:172-249`](file:///C:/SAGE/sage/text_processing.py#L172-L249)):
    1. Replaces code blocks with panel redirect message.
    2. Simplifies URLs.
    3. Verbalizes Windows paths (`"na pasta do sistema, arquivo..."`).
    4. Replaces git commit hashes with `"código identificador"`.
    5. Strips backticks, markdown headers (`#`), bold/italic (`**`, `*`, `__`, `_`, `~~`), bullets/quotes (`>`, `-`, `1.`), and HTML tags (`<...>`).
    6. Truncates to `max_sentences` with panel overflow note.

---

## 6. Intent Guard Rules (`sage/intent_guard.py`)

- **`fix_transcription`** ([`intent_guard.py:18-27, 51-55`](file:///C:/SAGE/sage/intent_guard.py#L18-L27)): Regex replacement for misheard technical terms ("Beach Hub" -> "GitHub", "pai ton" -> "Python", etc.).
- **`strip_misheard_wake`** ([`intent_guard.py:28-30, 58-69`](file:///C:/SAGE/sage/intent_guard.py#L28-L30)): Strips leading misheard wake words (`WAKE_MISHEARS`, e.g. "CG", "ceagê", "seige").
- **`looks_like_task`** ([`intent_guard.py:31-39, 71-77`](file:///C:/SAGE/sage/intent_guard.py#L31-L39)): Identifies actionable requests vs chit-chat. Triggers if query has `"tarefa complexa"`, $\ge 2$ verbs from `TASK_VERBS`, $\ge 1$ verb with $\ge 14$ words, or total word count $\ge 40$.
- **`is_empty_promise`** ([`intent_guard.py:40-44, 80-82`](file:///C:/SAGE/sage/intent_guard.py#L40-L44)): Identifies non-execution promises (`vou fazer`, `deixa comigo`) under 400 characters.
- **`needs_screen_context`** ([`intent_guard.py:86-97, 101-106`](file:///C:/SAGE/sage/intent_guard.py#L86-L97)): Checks for screen references (`_SEEING` phrases or demonstrative pronouns + nouns not in `_TALK_NOUNS`).
- **`mentions_screenshot`** ([`intent_guard.py:98, 109-110`](file:///C:/SAGE/sage/intent_guard.py#L98)): Detects explicit screenshot words (`print`, `captura`, `screenshot`, etc.).

---

## 7. Plain Data vs. Logic Classification

- **Plain Data (Static `&str` constants, static structs, embedded JSON/YAML)**:
  - All prompt block bodies (`IDENTITY`, `THINKING`, etc. in [`persona.py`](file:///C:/SAGE/sage/persona.py)).
  - `WELCOME`, `FAREWELL`, `CLONE_OK`, `CLONE_FAIL` arrays in [`phrases.py`](file:///C:/SAGE/sage/phrases.py).
  - `PROFILES` catalog in [`voice_profiles.py`](file:///C:/SAGE/sage/voice_profiles.py).
  - `FRIENDLY_DOMAINS` map in [`text_processing.py`](file:///C:/SAGE/sage/text_processing.py).
  - Constant tuples `TRANSCRIPTION_FIXES`, `WAKE_MISHEARS`, `TASK_VERBS`, `_TALK_NOUNS`, `_SEEING` in [`intent_guard.py`](file:///C:/SAGE/sage/intent_guard.py).
- **Logic (Rust modules / functions / regex engine)**:
  - Prompt interpolation, overrides loading/saving in `persona.rs`.
  - Time-of-day resolution and anti-repetition ring buffer in `phrases.rs`.
  - Regex replacement pipelines in `text_processing.rs` (Unicode-aware regex crate).
  - Unicode NFKD normalization (`unicode-normalization` crate) and matching heuristics in `intent_guard.rs`.

---

## Uncertainties

1. **User Overrides Persistence Format**: `persona.py:212` uses JSON (`data/persona.json`). In native egui Rust, whether to retain JSON or transition to TOML/Ron for config consistency.
2. **Dynamic Skill Catalog Injection**: `cortex.py:353` calls `SKILL_CATALOG()` dynamically; the Rust implementation will need an equivalent trait or read-lock to the skill repository.

---

## Handoff

- **Outcome**: Completed scout analysis and produced Markdown specification for porting `PERSONA & PROMPTS` to Rust.
- **Evidence**: Verified against `sage/persona.py`, `sage/phrases.py`, `sage/voice_profiles.py`, `sage/text_processing.py`, `sage/intent_guard.py`, `sage/cortex.py`, and `sage/swarm.py`.
- **Files Changed**: None (read-only scout mode).
- **Checks Run**: Read and cross-referenced all prompt definitions, role assemblies, and rule lists.
- **Failures**: None.
- **Risks**: Regex porting (`text_processing` and `intent_guard`) requires Rust regex crate with support for Unicode case-folding and lookaround patterns (or manual refactoring of negative lookaheads used in `pronounce_name`).

STATUS: DONE
SUMMARY: Delivered concise port specification for S.A.G.E. persona, prompt assemblies, phrase categories, text sanitization, and intent guard rules with file:line citations.
