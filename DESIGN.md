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
   is a bug.
2. **One accent colour.** Electric Blue (`--c-accent`) marks the primary action of
   a view. Never use it decoratively and never add a second chromatic accent.
3. **No elevation.** `--shadow-*` resolve to `none`. Layering is z-index, opacity
   and the frosted `--c-toolbar` surface — nothing else.
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

## Design tokens

All tokens live in `src/App.css`. Nothing else may define colour/radius/motion.

> **Known exception:** `UpdatePanel.css` still carries its own literal `--t-*`
> palette — a near-copy of the values below, with slightly different dark-mode
> borders. Treat it as debt: new work aliases `--c-*` instead.

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
| `--c-accent` | `#3E6AE1` | `#5B7EF0` | **Primary CTA only** |
| `--c-accent-hover` | `#3457C4` | `#6F8DF3` | Primary CTA hover |
| `--c-accent-soft` | `rgba(62,106,225,.10)` | `rgba(91,126,240,.16)` | Focus ring, selected/active tint |
| `--c-danger` / `--c-danger-soft` | `#C0392B` / `rgba(192,57,43,.10)` | `#E5655C` / `rgba(229,101,92,.14)` | Errors, destructive actions |
| `--c-success` / `--c-success-soft` | `#1F8A3B` / `rgba(31,138,59,.10)` | `#3BB55C` / `rgba(59,181,92,.14)` | Success, running/done state |
| `--c-warning` / `--c-warning-soft` | `#B07500` / `rgba(176,117,0,.10)` | `#E0A33C` / `rgba(224,163,60,.14)` | Warnings, elevated risk |

Semantic colours are functional (status, risk level, attachment kind) and are the
only sanctioned exception to "one accent". Never use them decoratively.

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

`.send-btn`, `.btn-primary`, `.settings-btn-primary`, `.confirm-dialog-button.confirm`

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

### Overlays and popups

- Modal backdrop: `rgba(128,128,128,.65)` + `backdrop-filter: blur(8px)`.
- Popup / dialog surface: `--c-surface` + 1px `--c-border-i` + 4px radius, **no shadow**.
- Frosted bars: `background: var(--c-toolbar)` + `backdrop-filter: blur(20px)`.

### Chat transcript

This is the one place with a deliberate asymmetry:

- **Assistant turn is transparent.** Its text sits directly on the canvas and is
  separated by whitespace — not boxed. This is the closest analogue to the
  product-copy-on-white discipline of the source language.
- **User turn is the only card**: `--c-surface-2` fill, 4px radius,
  `padding: 10px 12px`, right-aligned, `max-width: 80%`.
- Reasoning, tool-call groups, code blocks and tables use `--c-surface-2` with
  4px radius and `--c-border-s` internals.
- Streaming states may animate a gradient sweep across a label, but must respect
  `prefers-reduced-motion`.

---

## Layout and density

**Spacing scale: 4px base** — use `2 / 4 / 6 / 8 / 10 / 12 / 14 / 16 / 18 / 20`.
Nothing in between.

| Zone | Rules |
|---|---|
| Chat transcript (`.messages`) | `padding: 18px 16px 20px`, `gap: 16px` |
| Chat input area | `padding: 10px 12px 12px`, `gap: 8px`; control row `gap: 6px` |
| Sidebar | `width: 400px`, 1px right hairline |
| Panel header / footer | `padding: 10px 12px`, 1px hairline |
| Panel list | `padding: 10px 12px`, `gap: 6px` |
| Panel row | `padding: 10px 12px` |
| Settings nav | `180px` column, item `min-height: 28px`, `font-size: 13px` |
| Settings content | `padding: 14px 16px 20px`, `gap: 14px` |
| Settings section | `gap: 10px`, `padding-bottom: 14px`, 1px hairline |
| Settings section body | `gap: 8px`, `max-width: 760px` |

### The compact zone

Settings, the chat input row, and the sidebar panels all share **34px controls**.
Settings additionally drops to **13px** text. This is what keeps the app feeling
like one instrument rather than three glued together — do not let a new form
re-introduce 40px inputs.

### Whitespace philosophy

Whitespace is the frame, not leftover space. Prefer removing a border or a fill
over adding padding to compensate. If a region feels crowded, the fix is almost
always fewer elements, not more spacing.

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
  are the only sanctioned uses).
- Don't use `scale()` / `translate()` on hover or press.
- Don't use `text-transform: uppercase` or negative letter-spacing.
- Don't use radius above 4px, and never a `border-radius: 50%` on a button.
- Don't add a second chromatic accent colour.
- Don't declare `--t-*` / `--ts-*` aliases as `var(--t-…)` of themselves — a CSS
  custom property cannot self-reference and resolves to nothing.
- Don't duplicate a primitive into a component stylesheet; that is how the panels
  drifted apart before.
- Don't ship a UI change without looking at it in both themes.

---

## File map

| File | Owns |
|---|---|
| `src/App.css` | **All tokens**, app shell, toolbar, transcript, input row, dialogs, markdown editor, scrollbars, shared primitives |
| `src/components/ChatMessage.css` | Transcript turns, reasoning, code blocks, tables (`--ts-*` are aliases of `--c-*`) |
| `src/components/ToolCallGroup.css` | Tool-call cards and streaming states |
| `src/components/TodoList.css` | Todo rows |
| `src/components/{Skills,Mcp,Tools,Agents,History}Panel.css` | Panel-specific layout only — chrome comes from the shared primitives |
| `src/components/SettingsPanel.css` | Settings page (`--t-*` are aliases of `--c-*`) |
| `src/components/ModelSelect.css` | Model picker trigger + portal popup |
| `src/components/{Update,Monitor,AgentMission,Profile}Panel.css` | Feature surfaces, self-contained |

### Theme plumbing

`data-theme` on `<html>` is set by `applyTheme()` in `src/App.tsx` from
`config.theme` (`auto` / `light` / `dark`). Everything must work through the token
layer — no component should branch on the theme itself except where a literal is
unavoidable (currently: the toggle knob in dark mode, and the monitor terminal).

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
