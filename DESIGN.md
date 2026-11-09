# AI Chat — Design System

> **This file is the source of truth for every visual change in this repository.**
> Read it before touching a `.css` file or writing JSX that carries styling.
> A UI change that breaks the rules below is a regression even if it renders
> correctly on your machine.

The system is built for a **dense, keyboard-driven, information-heavy desktop
tool**: a monochrome-plus-one palette, a 4px radius, flat surfaces and a single
motion curve. Everything below is written against this codebase — the tokens in
`src/App.css` are the implementation, this document is the contract.

---

## Non-negotiables

1. **Never hardcode a colour, radius, shadow, duration or easing.** Use the tokens
   declared in `src/App.css`. A literal hex outside `:root` / `[data-theme="dark"]`
   / the palette blocks is a bug.
2. **One accent colour.** Electric Blue (`--c-accent`) marks the primary action of
   a view. Never use it decoratively and never add a second chromatic accent.
   A palette may re-hue it, but it stays the only accent of that view.
3. **No elevation.** `--shadow-*` resolve to `none`. Layering is z-index, opacity
   and the frosted `--c-toolbar` surface — nothing else. The **one** exception is
   the functional scroll-edge shade (`--shadow-scroll-top` /
   `--shadow-scroll-bottom`): it never lifts a surface, it only says “the
   transcript continues past this bar”, and it is toggled by scroll position.
4. **4px radius on interactive elements.** No pills, no large radii, no circles
   (except the deliberate dot/spinner indicators).
5. **One motion curve.** `--t-dur` + `--t-ease` for every state change. Colour and
   border transitions only — no `scale`, no `translate` on hover.
6. **Hairlines separate, not boxes.** 1px `--c-border` for structure, 1px
   `--c-border-i` for interactive outlines.
7. **Reuse the shared primitives.** `.close-btn`, `.btn-primary`, `.btn-secondary`,
   `.inline-edit-btn`, `.field-title-row` are defined **once** in `src/App.css`.
   Never re-declare them in a component stylesheet.
8. **Sentence case everywhere.** No `text-transform: uppercase` in UI chrome.
9. **Both themes, always.** Every new surface must be checked in light *and* dark.
10. **Density over decoration.** When in doubt, remove padding, remove a border, or
    remove a colour — do not add one.

---

## Two dimensions: mode and palette

The interface has **two independent colour dimensions**, both set on `<html>`:

| Attribute | Values | Meaning |
|---|---|---|
| `data-theme` | `light` / `dark` | Light vs. dark resolution. `auto` follows the OS and resolves to one of the two. |
| `data-palette` | `default` / `sakura` / `cream` | The colour scheme: which hues the surfaces, the accent and the semantic states wear. |

Both are written by `applyTheme()` in `src/App.tsx` from `config.theme` and
`config.palette`. Every palette therefore needs a **light *and* a dark token
set** — the two dimensions are orthogonal, so `cream` + `dark` must look as
finished as `cream` + `light`.

| Palette | Light | Dark | Character |
|---|---|---|---|
| `default` | Pure White canvas, Electric Blue accent | Carbon Dark canvas, lifted Electric Blue | The monochrome-plus-one baseline. |
| `sakura` (樱花粉) | 草莓牛奶 `#FFDDE9` canvas, `#FFF7FA` panels, 樱花粉 `#FF8FB1` accent with `#8E2450` as its legible twin, 薄荷奶绿 `#DFF5EA` code, 薰衣草紫 `#EDE4FF` reasoning — the pastel-pink group of Color Hunt (`#FFCED8` / `#FFDDDD` / `#FFF4DE` / `#FFE8C4`), cooled into a strawberry-milk scheme | Deep plum canvas `#2B1A23`, `#37212C` panels, lifted `#FFA6C2` accent | Cute. Soft pastel pink, deliberately the one palette that is not paper-white. |
| `cream` (乳白) | The adjacent warm-white group — 蚌肉白 `#F9F1DB` canvas, 乳白 `#F9F4DC` panels, 奶油白 `#FFF8E7` alternate surface, 粉白 `#FBF2E3` secondary — washed with oklab gradients instead of flat fills | Deep umber canvas, the same tones as warm greys | Paper. The accent stays Electric Blue: this palette is about the neutrals. |

### Rules for a palette block

- A palette block defines the **complete** token set. It never inherits from
  another palette, and it never inherits a token it forgot — otherwise a missing
  line leaks the neighbouring scheme's colour into the UI.
- `[data-palette="x"]` holds the light set; `[data-theme="dark"][data-palette="x"]`
  overrides it for dark mode. Both blocks sit **after** `[data-theme="dark"]` in
  `src/App.css`, so the dark+palette selector (specificity 0-2-0) wins.
- A pastel accent cannot carry a label. That is what `--c-accent-text` is for:
  the same hue, darkened, used for **text, icons, hairlines and focus rings**.
  Fills (buttons, chips, toggles) keep `--c-accent` and take `--c-on-accent` as
  their label colour.
- The Cream palette is the sanctioned place for gradients on a surface: its
  `--c-bg-image` / `--c-code-image` / `--c-reason-image` are oklab washes of its
  own adjacent group. Everywhere else, surfaces stay flat.

---

## Design tokens

All tokens live in `src/App.css`. Nothing else may define colour/radius/motion.

> **Known exception:** none. `UpdatePanel.css` used to carry its own literal
> `--t-*` palette; it now aliases the global tokens like every other panel, so
> it follows the active palette.

### Surfaces

| Token | Light | Dark | Role |
|---|---|---|---|
| `--c-bg` | `#FFFFFF` | `#171A20` | Page canvas |
| `--c-surface` | `#FFFFFF` | `#1E2127` | Panels, popups, cards, inputs |
| `--c-surface-2` | `#F4F4F4` | `#23262D` | Alternate surface: user turn, code, reasoning, tool calls |
| `--c-hover` | `rgba(23,26,32,.04)` | `rgba(255,255,255,.06)` | Hover fill |
| `--c-active` | `rgba(23,26,32,.07)` | `rgba(255,255,255,.10)` | Pressed fill |
| `--c-toolbar` | `rgba(255,255,255,.75)` | `rgba(23,26,32,.75)` | Frosted bars: toolbar, input area, sidebar, panel headers |

### Text hierarchy

| Token | Light | Dark | Role |
|---|---|---|---|
| `--c-text` | `#171A20` | `#F4F4F4` | Headings, nav, item names |
| `--c-body` | `#393C41` | `#D0D1D2` | Body copy, secondary buttons |
| `--c-muted` | `#5C5E62` | `#8E8E93` | Meta, descriptions, icons at rest |
| `--c-placeholder` | `#8E8E8E` | `#6B6E73` | Placeholders, disabled text |

### Hairlines

| Token | Light | Dark | Role |
|---|---|---|---|
| `--c-border` | `#EEEEEE` | `rgba(255,255,255,.08)` | Structural separators (header/footer rules, section rules) |
| `--c-border-i` | `#D0D1D2` | `rgba(255,255,255,.16)` | Input, card and row outlines |
| `--c-border-s` | `#F4F4F4` | `rgba(255,255,255,.05)` | Faintest hairline: code blocks, table cell internals |

### Accent and semantic states

| Token | Light | Dark | Role |
|---|---|---|---|
| `--c-accent` | `#3E6AE1` | `#5B7EF0` | **Primary CTA only** — the fill of the primary action |
| `--c-accent-hover` | `#3457C4` | `#6F8DF3` | Primary CTA hover |
| `--c-accent-soft` | `rgba(62,106,225,.10)` | `rgba(91,126,240,.16)` | Focus ring, selected/active tint |
| `--c-accent-text` | = `--c-accent` | = `--c-accent` | The accent as **text / icon / hairline / focus ring**. A palette with a pastel accent re-hues this darker twin so it stays legible. |
| `--c-on-accent` | `#FFFFFF` | `#FFFFFF` | The label colour that sits **on** an accent fill (dark on a pastel accent) |
| `--c-danger` / `--c-danger-soft` | `#C0392B` / `rgba(192,57,43,.10)` | `#E5655C` / `rgba(229,101,92,.14)` | Errors, destructive actions |
| `--c-success` / `--c-success-soft` | `#1F8A3B` / `rgba(31,138,59,.10)` | `#3BB55C` / `rgba(59,181,92,.14)` | Success, running/done state |
| `--c-warning` / `--c-warning-soft` | `#B07500` / `rgba(176,117,0,.10)` | `#E0A33C` / `rgba(224,163,60,.14)` | Warnings, elevated risk |

Semantic colours are functional (status, risk level, attachment kind) and are the
only sanctioned exception to "one accent". Never use them decoratively.

> **Split rule:** a rule that paints an accent **fill** uses `--c-accent`; a rule
> that paints an accent **line or label** uses `--c-accent-text`. Mixing them up
> is what makes a pastel palette unreadable.

### Surface washes

| Token | Default | Role |
|---|---|---|
| `--c-bg-image` | `none` | Optional wash over the canvas (`body`, `.chat-area`) |
| `--c-code-image` | `none` | Optional wash over code blocks |
| `--c-reason-image` | `none` | Optional wash over reasoning / quote surfaces |

`none` everywhere but in the Cream palette, which fills them with the oklab
gradients of its adjacent group. A rule that uses them must set
`background-color` first and `background-image` after — the `background`
shorthand resets the image.

### Code and reasoning surfaces

`--c-code` and `--c-reason` both resolve to the alternate surface (`#F4F4F4` /
`rgba(255,255,255,.06)` / `.04`); `--c-reason-b` is the matching border.

### Elevation — deliberately absent

```css
--shadow-sm: none;
--shadow-md: none;
--shadow-lg: none;
--shadow-panel: none;
```

The names are kept as no-ops so legacy rules still parse.

**Scroll-edge shade — the one sanctioned shadow.** Two extra tokens exist beside
the no-ops, for the transcript's top and bottom boundary only:

```css
--c-scroll-shade: rgba(0, 0, 0, 0.18);            /* light; dark: 0.55 black */
--shadow-scroll-top: 0 12px 18px -12px var(--c-scroll-shade);
--shadow-scroll-bottom: 0 -12px 18px -12px var(--c-scroll-shade);
```

They are applied by `.toolbar.is-scrolled` / `.input-area.is-scrolled`, toggled
from the transcript's scroll handler whenever content is hidden past that bar —
the shade is a scroll cue, not depth. Never reuse them on anything else, and
never give a panel or popup a shadow of its own.

> **Gotcha:** `box-shadow: 0 1px 3px rgba(...), var(--shadow-sm)` is **invalid CSS**
> once the token is `none` — the whole declaration is dropped silently. Never mix a
> literal shadow with a `--shadow-*` token. Focus rings are written as a single
> ring: `box-shadow: 0 0 0 3px var(--c-accent-soft)`.

### Radii

```css
--r-sm: 4px;  --r-md: 4px;  --r-lg: 4px;  --r-xl: 4px;  --r-pill: 4px;
```

`--r-pill` is a **historical name**, not a pill — it is 4px so old chip styles
square off with everything else. New code should write `4px` or `var(--r-sm)`.

### Motion

```css
--t-dur: 0.33s;
--t-ease: cubic-bezier(0.5, 0, 0, 0.75);
```

Use as `transition: background-color var(--t-dur) var(--t-ease)`. The only
sanctioned deviation is `box-shadow 0.25s var(--t-ease)` on the inset focus/active
ring of primary buttons.

### Legacy aliases that must stay

`--c-text-secondary` (→ Pewter), `--c-bg-secondary` (→ alternate surface). They are
referenced by `ModelSelect.css` and `ProfilePanel.css`; removing them silently
breaks those rules.

---

## Typography

- **Family:** `RazerF5` first, then system UI, then CJK fallbacks. Set once on
  `:root`; inherit it. Only code surfaces switch to a monospace stack.
- **Base size:** `14px` on `:root`.
- **Letter-spacing: `normal` everywhere.** No negative tracking, no tracking on
  labels. This is the single most visible departure from typical "tech brand"
  typography and it is intentional.
- **Weights: `400` and `500` only.** No `600`, no `700`, no `300`.

| Role | Size | Weight | Line height |
|---|---|---|---|
| Empty-state headline | 17px | 500 | 1.4 |
| Panel / dialog / section title | 15–17px | 500 | 1.3 |
| Nav, toolbar, buttons, field labels, item names | 14px | 500 | 1.2 |
| Chat body, prose | 14px | 400 | 1.65 |
| Settings controls & labels (compact zone) | 13px | 400 / 500 | 1.5 |
| Descriptions, meta, hints, badges | 12px | 400 | 1.5 |
| Code | 13px | 400 | 1.6 |

---

## Components

### Primary CTA — the only place Electric Blue is allowed

`.send-btn`, `.btn-primary`, `.settings-btn-primary`

```css
background: var(--c-accent);
color: #FFFFFF;
border: 3px solid transparent;   /* reserves the ring's space */
border-radius: 4px;
font-weight: 500;
transition: background-color var(--t-dur) var(--t-ease),
            border-color var(--t-dur) var(--t-ease),
            box-shadow 0.25s var(--t-ease);
```

- Hover → `--c-accent-hover`. Nothing else moves.
- Active / `:focus-visible` → `box-shadow: rgba(0,0,0,.2) 0 0 0 2px inset`.
- Disabled → `opacity: .4–.5`, `cursor: not-allowed`, no shadow.
- Heights: 34px in the chat input row and Settings, 36px in sidebar panels.
- One primary CTA per view. If two actions compete, one becomes secondary.

### Secondary button

White fill, `--c-body` label, 1px `--c-border-i` outline, 4px radius. Hover →
`--c-surface-2` fill and `--c-muted` outline.

### Tinted action

`.settings-btn-tinted` — the secondary action that reloads state (fetching the
model list). `--c-accent-soft` fill, `--c-accent-text` label, no outline. Hover
→ `--c-accent` fill and `--c-on-accent` label. The soft tint never replaces the
primary CTA; both may not appear in the same row.

### Nav / toolbar button

Transparent, 14px/500, `--c-text`, 4px radius, `min-height: 32px`, `padding: 4px 12px`.
Hover and `.active` both use `--c-hover` — active state is a fill, never a colour
change or an underline.

### Icon button

`.close-btn` and the panel action buttons: transparent, `--c-muted` glyph,
4px radius. Hover → `--c-hover` fill + `--c-text`. Destructive variant →
`--c-danger-soft` fill + `--c-danger` glyph.

### Inputs, textareas, selects

```css
height: 34px;                    /* textarea: auto + 8px 11px padding */
border: 1px solid var(--c-border-i);
border-radius: 4px;
background: var(--c-surface);
font-size: 14px;                 /* 13px in the compact Settings zone */
transition: border-color var(--t-dur) var(--t-ease);
```

Focus is a **border-colour change to `--c-accent`** — no glow, no ring, no shadow.
Placeholder uses `--c-placeholder`. Selects use the inline SVG chevron in
`--c-placeholder` grey, never the native arrow.

### Toggle switch

36×20 track, `border-radius: 4px`; 14×14 knob, `border-radius: 2px`,
`translateX(16px)` when checked. Off → `--c-border-i`. On → `--c-accent`.
Square corners are deliberate: a switch is a precision control here, not a pill.

### Cards, rows, list items

```css
background: var(--c-surface);
border: 1px solid var(--c-border-i);
border-radius: 4px;
padding: 10px 12px;
```

Hover deepens the outline to `--c-muted`. **No shadow, no lift, no scale.** A
disabled row is `opacity: .55`.

### Provenance badge

A hairline marker that records where a row came from — e.g. an MCP server the
assistant built itself in self-evolution mode (`.mcp-self-badge`):

```css
display: inline-block;
margin-left: 6px;
padding: 0 4px;
border: 1px solid var(--c-border);
border-radius: 4px;
font-size: 11px;
font-weight: 400;
line-height: 15px;
color: var(--c-muted);
```

Metadata is neutral by construction: never a pill, never a fill, never the
accent colour. A badge that is not a state must not compete with the one accent.

### Overlays and popups

- Modal backdrop: `rgba(128,128,128,.65)` + `backdrop-filter: blur(8px)`.
- Popup / dialog surface: `--c-surface` + 1px `--c-border-i` + 4px radius, **no shadow**.
  A popup that floats *inside* the composer uses `--c-bg-secondary` instead, so it
  separates from the frosted input bar without elevation.
- Frosted bars: `background: var(--c-toolbar)` + `backdrop-filter: blur(20px)`.

#### Composer mention popups (`@file`, `/skill`)

One surface, two triggers, one behaviour. `components/FileMentionPopup.css` owns
the frame — header, rows, active fill, empty state — and both
`components/FileMentionPopup.tsx` and `components/SlashMentionPopup.tsx` render
those `.file-mention-*` classes. `SlashMentionPopup.css` may only add the skill
glyph colour (`--c-accent-text`) and the left-to-right description row; it must
never restate the frame.

- Sits `bottom: calc(100% + 6px)` inside `.input-row`, full input width,
  `max-height: 240px` with its own scroll.
- **Never takes focus.** A row accepts on `mousedown` + `preventDefault`, so the
  caret stays in the textarea and the popup is drivable by keyboard alone.
- Keyboard contract, identical for both: `↑` / `↓` move, `Enter` / `Tab` accept,
  `Esc` dismisses. Only the active row wears the `--c-accent-soft` fill.
- Row: 4px radius, transparent at rest, `--c-hover` on hover; name `--c-text`
  500, meta (path or description) `--c-muted` 11px, truncated, `margin-left: auto`.
- **At most one popup is open** — the token grammars are mutually exclusive
  (`@` needs a word boundary and no whitespace/`/`; `/` needs a word boundary and
  no whitespace/`@`/`#`), so no stacking rule is needed.
- The `@` picker materialises attachments on send; the `/` picker leaves a plain
  `/skill-name` token in the body for the model to resolve with `use_skill`.

#### Chat notices (warning / error)

Warnings and errors never enter the transcript. A bar inside the message flow
pushed the conversation around as it appeared and then stayed on screen until the
next request; they now float over the chat window instead. `components/ChatNotice.tsx`
renders the `.chat-notice` rules from `src/App.css` inside `.chat-notices` — a
zero-height anchor strip in `App.tsx` directly above the transcript (below the
toolbar and its optional bars), so a notice needs no measured offset and
reserves no space.

- **Placement:** absolute, `top: 8px; left: 12px`, `width: min(420px, calc(100% - 24px))`
  — the chat window's top-left corner, never over the toolbar's controls.
- **Surface:** `--c-surface` + 1px `--c-border-i` + 4px radius, **no shadow**, with
  a 2px semantic rule on the left (`--c-danger` for errors, `--c-warning` for
  warnings) and the glyph in the same colour. Body is `--c-body` 13px / 1.55.
- **One dismiss target:** `.chat-notice-body` (glyph + text) is the `<button>` —
  click, `Enter` or `Space` closes the notice. Long text clamps to three lines
  with an ellipsis; the full message rides along as the accessible name and in
  `title`.
- **Timing:** leaves on click and on its own after **5 s** — `opacity` → 0 on
  `--t-dur` / `--t-ease`, then unmounts. One notice at a time; a new one replaces
  the old and restarts the dwell. `prefers-reduced-motion` collapses the fade to
  a single frame, it does not remove the disappearance.
- **At most one action**, as a compact `.inline-edit-btn` at the end of the bar
  (today: "Continue" for a turn that died half-way). A notice that carries an
  action **does not** fade on its own — a button disappearing from under the
  pointer that is reaching for it is worse than a bar that waits to be read.
- Nothing about a failure is rendered in the transcript: no error row, no
  failure bubble. The turn stays on disk with its `status: "error"` for history
  and for the retry path, but the user-facing report is the notice alone.

### Chat transcript

This is the one place with a deliberate asymmetry:

- **Assistant turn is transparent.** Its text sits directly on the canvas and is
  separated by whitespace — not boxed. This is the closest analogue to the
  product-copy-on-white discipline of the source language.
- **User turn is the only card**: `--c-surface-2` fill, 4px radius,
  `padding: 10px 12px`, right-aligned, `max-width: 80%`.
- Reasoning, tool-call groups, code blocks and tables use `--c-surface-2` with
  4px radius and `--c-border-s` internals.
- **Mid-turn injections fold at three lines.** What the user pushed into a
  running turn renders under the reply (`.message-injection`); anything past
  three lines is clamped with a `Show more` / `Show less` action built from the
  `.inline-edit-btn` primitive at message density (24px). The fold is *measured*
  after paint (`scrollHeight > clientHeight`), so a note that fits never shows a
  dead toggle.
- Streaming states may animate a gradient sweep across a label, but must respect
  `prefers-reduced-motion`.

### Interaction monitor

The log dialog opened from Settings → Runtime & Debug. Chrome is the shared
dialog recipe — `--c-toolbar` header with one hairline, `--c-surface` content,
4px radius, no elevation — and it carries **no literal colour**: the dark box it
used to be was the last hardcoded surface in the system.

- **Panes (2/3 of the height).** List column `35%` / `min-width: 260px` with a
  1px right hairline; the detail pane fills the rest. Rows: hover `--c-hover`,
  selected `--c-accent-soft` plus a 2px `--c-accent-text` left border.
- **Type badge is metadata, not state**: hairline `--c-border` + `--c-muted`,
  4px. Only error rows (`llm_error` / `tool_error` / `mcp_call`) borrow
  `--c-danger`. Never a filled pill.
- **Session report (`flex: 0 0 33.3333%`)** fills exactly the bottom third of
  the dialog, whatever its height (`min-height: 132px` keeps it usable on short
  windows). Seven tiles — total tokens, cache hit, cache miss, context used,
  requests, thinking per round, tool calls — each `--c-surface` + hairline:
  label 11px `--c-muted`, value 15px/500 with tabular numerals, detail 11px.
  Hit / miss / context add a 2px track (`--c-accent`, `--c-danger` from 85 %
  context), the same weight the composer's usage bar uses. The ranking column
  beside them lists the busiest tools (≤ 8 rows, failures in `--c-danger`) and
  scrolls on its own; below 900px the report stacks under the tiles.
- **Data** comes from `get_session_stats` (`src-tauri/src/db.rs`), aggregated in
  SQL: tokens / cache / context from `api_requests`, tool traffic from
  `interaction_log` (`mcp_call` is the *failure* row, `mcp_response` the success
  one). The context ratio is `context_tokens / context_window`, with the window
  resolved from `config.model_context_lengths` / `config.model_metadata` for the
  newest request's model — unknown renders `—` and a hint, never a fake 0 %.
- **Thinking per round** is `reasoning_ms` — the wait from sending the request
  to the first *content* token, recorded only when the model streamed reasoning
  first (`StreamTiming` in `llm_complete.rs`). A round without a thinking phase
  stores NULL, so the average is never diluted by non-thinking models and the
  tile reads `—` instead of `0.0s`. `first_token_ms` (any first delta) is its
  detail line; both ride along in the `llm_api` interaction metadata.
- **Contrast picks** (measured over all six palette × theme combinations, 11px
  text ≥ 4.5:1): the accent wash of a selected row costs ~1.5 steps from meta
  text, so `.selected` steps its badge / actor / time / preview up to
  `--c-body`; the cache chip is a *figure*, not a state, so it is neutral
  (`--c-surface-2` + `--c-border`, `--c-body`) — `--c-accent-text` on
  `--c-accent-soft` sinks to 3.1:1 in five of six combinations; the error card
  sits on `--c-surface` (not the tinted `--c-surface-2`) because `--c-danger`
  on the tint lands at 4.35:1 in cream dark; and the maintenance status keeps
  its `--c-success` tone on a dot, with the message itself in `--c-body`
  (`--c-success` as text is 3.9:1 on the light toolbars).

---

## Layout and density

**Spacing scale: 4px base** — use `2 / 4 / 6 / 8 / 10 / 12 / 14 / 16 / 18 / 20`.
Nothing in between.

| Zone | Rules |
|---|---|
| Chat transcript (`.messages`) | `padding: 18px 16px 20px`, `gap: 16px` |
| Chat input area | `padding: 10px 12px 12px`, `gap: 8px`; control row `gap: 6px` |
| Sidebar | `width: 400px`; 1px right hairline drawn by `.sidebar::after`, starting `--panel-header-h` down so it never crosses the header band |
| Panel header | `.panel-header` (shared, `App.css`): `min-height: var(--panel-header-h)` = 49px so its hairline lands on the chat toolbar's, `padding: 6px 12px`, frosted `--c-toolbar`, 1px hairline, title + one `faAnglesLeft` collapse chevron and nothing else |
| Panel footer | `padding: 10px 12px`, 1px hairline |
| Panel list | `padding: 10px 12px`, `gap: 6px` |
| Panel row | `padding: 10px 12px` |
| Settings nav | `180px` column, item `min-height: 28px`, `font-size: 13px` |
| Settings content | `padding: 14px 16px 20px`, `gap: 14px` |
| Settings section | `gap: 10px`, `padding-bottom: 14px`, 1px hairline |
| Settings section body | `gap: 8px`, `max-width: 760px` |

### Context-usage bar

The 2px bar pinned to the input area's top border has two states:

- **Streaming, usage unknown** — a soft `--c-accent` gradient dash sweeps across
  the track, so a long think shows activity before the provider reports token
  usage. This is one of the two sanctioned gradients (the other is a streaming
  label sweep).
- **Usage known / idle** — the request's context ratio as a flat `--c-accent`
  fill, switching to `--c-danger` at 90 % and above.

`prefers-reduced-motion` replaces the sweep with a dim static line. The bar is
absolutely positioned over the input area's top border, so it never causes
layout shift.

### Mention progress strip

A PDF mention (`@paper.pdf#2`) is converted to Markdown *before* the turn
starts, so the input area grows one status row while it runs:

- one muted 12px row (`--c-muted`): spinner + "converting `paper.pdf`…", the
  page counter in 11px `--c-placeholder` with tabular numerals, then the same
  2px `--c-accent` track the context bar uses (no gradient, no fill);
- it is **in flow**, unlike the usage bar: it shares `.injection-bar`'s recipe,
  so a transient row enters and leaves the stack the same way and needs no
  reserved height;
- `prefers-reduced-motion` drops the width transition, the row is `role="status"`
  so a screen reader hears the page change, and the composer clears it in a
  `finally` — a failed conversion never leaves it behind.

### The compact zone

Settings, the chat input row, and the sidebar panels all share **34px controls**.
Settings additionally drops to **13px** text. This is what keeps the app feeling
like one instrument rather than three glued together — do not let a new form
re-introduce 40px inputs.

### Scrollbars

Only the transcript and the panel lists draw a bar, and only while they are
scrolling (`.scrolling-active`). Everything else that scrolls hides the bar:
the composer (`.chat-input`) grows to its `max-height` and then scrolls
invisibly — a bar inside a 34px field sits on top of the tail of every full
line — and the sidebar divider/scroll treatment must not re-introduce one.

### Whitespace philosophy

Whitespace is the frame, not leftover space. Prefer removing a border or a fill
over adding padding to compensate. If a region feels crowded, the fix is almost
always fewer elements, not more spacing.

---

## Window chrome

The window is frameless (`decorations: false` in `tauri.conf.json`); the titlebar
is ours. It is the only chrome above the app and it follows the same rules as
every other surface — no shadow, one hairline, one accent.

| Part | Rule |
|---|---|
| Bar | `--titlebar-h` (32px), `--c-toolbar` + `blur(20px)`, 1px `--c-border` bottom, `user-select: none` |
| Drag region | The bar **and** its filler carry `data-tauri-drag-region` (the attribute does not inherit); buttons never do |
| Window buttons | 34×28, 4px radius, `--c-muted` glyph at 12px; hover `--c-hover` + `--c-text`; active `--c-active` |
| Close button | Destructive variant: hover is `--c-danger-soft` fill + `--c-danger` glyph |
| App menu (`⋯`) | `--c-surface`, 1px `--c-border-i`, 4px radius, no shadow; rows 32px, 13px/400, hover `--c-hover` |
| Workspace label | 12px `--c-muted`, ellipsized; 1px `--c-danger` underline at a 2px offset marks the active workspace |
| Placement | Windows/Linux: minimize / maximize / close on the right; macOS: close / minimize / maximize on the left |

The titlebar is `position: fixed` at `z-index: 2000`, above every overlay, so the
window controls stay reachable while Settings or the Markdown editor is open.
Fullscreen pages start at `top: var(--titlebar-h)`; the sidebar starts below it
too. True modal dialogs (9999 layer) still cover the titlebar on purpose.

The sidebar panel header (`.panel-header`) repeats the titlebar's frosted
`--c-toolbar` and the chat toolbar's height, so the whole top of the window —
titlebar, panel header, chat toolbar — reads as one glass band divided only by
the hairline under the panel header. That shared height is what lands the
hairline on the toolbar's own row, so the top of the window shows one
horizontal line rather than two lines 7px apart. The header carries the title
and the collapse control and nothing else.

The 1px divider between the sidebar and the chat area is drawn by
`.sidebar::after` and starts `--panel-header-h` below the top of the panel: in
the header's own band it is deliberately absent, because a vertical line there
would cut the glass band in two right next to the chevron and make the header read as
a board glued to the left edge.

---

## Do's and Don'ts

### Do

- Reach for a token first; if none fits, **add a token** to `src/App.css` rather
  than a literal.
- Keep one accent per view and let hierarchy come from the four text greys.
- Use 1px hairlines for separation and whitespace for grouping.
- Keep every state change on `--t-dur` / `--t-ease` and limit it to colour/border.
- Square off toggles, badges and chips at 4px.
- Reuse `.btn-primary` / `.btn-secondary` / `.close-btn` instead of writing new ones.
- Check dark mode and `prefers-reduced-motion` before calling a UI change done.
- Verify with `npx tsc --noEmit && npx vite build`.

### Don't

- Don't add a box-shadow for depth. Layering is z-index, opacity, frosted glass.
- Don't introduce a gradient on a UI surface (progress bars and streaming sweeps
  are the only sanctioned uses; the Cream palette's own oklab washes are the one
  deliberate exception, and they live in its `--c-*-image` tokens).
- Don't use `scale()` / `translate()` on hover or press.
- Don't use `text-transform: uppercase` or negative letter-spacing.
- Don't use radius above 4px, and never a `border-radius: 50%` on a button.
- Don't add a second chromatic accent colour.
- Don't paint an accent-coloured label with `--c-accent` — that is `--c-accent-text`'s
  job, and a pastel accent makes it unreadable.
- Don't declare `--t-*` / `--ts-*` aliases as `var(--t-…)` of themselves — a CSS
  custom property cannot self-reference and resolves to nothing.
- Don't duplicate a primitive into a component stylesheet; that is how the panels
  drifted apart before.
- Don't ship a UI change without looking at it in both themes.

---

## File map

| File | Owns |
|---|---|
| `src/App.css` | **All tokens**, app shell, toolbar, **titlebar / window chrome**, transcript, **floating chat notices**, input row, dialogs, markdown editor, scrollbars, shared primitives |
| `src/components/ChatMessage.css` | Transcript turns, reasoning, code blocks, tables (`--ts-*` are aliases of `--c-*`) |
| `src/components/ToolCallGroup.css` | Tool-call cards and streaming states |
| `src/components/TodoList.css` | Todo rows |
| `src/components/FileMentionPopup.css` | Composer mention popup — one surface shared by the `@file` and `/skill` pickers; `SlashMentionPopup.css` only adds the glyph colour and the left-to-right description |
| `src/components/{Skills,Mcp,Tools,Agents,History}Panel.css` | Panel-specific layout only — chrome comes from the shared primitives |
| `src/components/SettingsPanel.css` | Settings page (`--t-*` are aliases of `--c-*`) |
| `src/components/ModelSelect.css` | Model picker trigger + portal popup |
| `src/components/TitleBar.tsx` | Custom window chrome: drag region, window controls, `⋯` app menu (styles in `App.css`) |
| `src/components/{Update,Monitor,AgentMission,Profile}Panel.css` | Feature surfaces, self-contained — `MonitorPanel.css` also owns the two log panes and the session-report strip |

### Theme plumbing

`data-theme` and `data-palette` on `<html>` are set by `applyTheme()` in
`src/App.tsx` from `config.theme` (`auto` / `light` / `dark`) and
`config.palette` (`default` / `sakura` / `cream`). Everything must work through the
token layer — no component should branch on the theme or the palette itself
except where a literal is unavoidable (currently: the toggle knob in dark mode).

The Settings picker previews a change immediately but persists it with the
form's **Save Changes**; `config.palette` rides along with `config.theme` in
the same save.

### Verification

```sh
npx tsc --noEmit && npx vite build
```

Do **not** use `npm run build` for a style check — it runs `sync-version:git-tag`
first and aborts without a git tag.

Undefined-token audit (should print nothing):

```sh
grep -rho -- '--[a-zA-Z0-9-]*:' src --include='*.css' | sed 's/:$//' | sort -u > /tmp/defined.txt
grep -rho -- 'var(--[a-zA-Z0-9-]*' src --include='*.css' | sed 's/var(//' | sort -u > /tmp/used.txt
comm -13 /tmp/defined.txt /tmp/used.txt
```
