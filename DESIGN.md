# Rabbit Chat Studio — Design System

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
   and the frosted `--glass-backdrop` surface — nothing else. There are exactly **two** exceptions and neither lifts a surface:
   the functional scroll-edge shade (`--shadow-scroll-top` /
   `--shadow-scroll-bottom`): it never lifts a surface, it only says “the
   transcript continues past this bar”, and it is toggled by scroll position. The other is the specular inner edge (`--glass-edge`): a 1px highlight along the **window's** top edge that casts nothing outward; a band tucked under another band gets none, because there it reads as a stray rule. A drop shadow, a glow or a second ring is still a bug.
4. **4px radius on interactive elements.** No pills, no large radii, no circles
   (except the deliberate dot/spinner indicators).
5. **One motion curve.** `--t-dur` + `--t-ease` for every state change. Colour and
   border transitions only — no `scale`, no `translate` on hover. The **one** spring in the system is the side panel's entrance (`SIDEBAR_SPRING` in `src/App.tsx`): a 20px glide at stiffness 320 / damping 32, because a fixed curve cannot give a 400px frosted panel its settle. It is disabled under `prefers-reduced-motion`.
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
| `--c-toolbar` | `rgba(255,255,255,.62)` | `rgba(23,26,32,.62)` | Frosted bars: toolbar, input area, sidebar, panel headers |

#### Frosted glass (`--glass-*`)

The blur is a third, independent dimension, next to `data-theme` and
`data-palette` (see “Theming” below). Every translucent surface reads the same
tokens, so the strength is one global setting instead of a per-component
decision, and `off` is a supported look rather than a broken one.

| Token | Standard | Role |
|---|---|---|
| `--glass-blur` | `20px` | Blur of the frosted bars: titlebar, toolbar, input area, popups |
| `--glass-blur-panel` | `calc(var(--glass-blur) * 1.5)` → `30px` | Blur of the side panel and the panel headers — the same frost one step deeper, because their whole backdrop is 6% of an already blurred transcript. Derived, so `off` still zeroes it |
| `--glass-blur-scrim` | `8px` | Blur of modal backdrops — a smaller radius keeps a full-window scrim cheap |
| `--glass-saturate` | `180%` | Saturation boost paired with the blur on **every** glass surface, so a tinted band lifts the colour behind it instead of greying it out. `off` sets `100%` |
| `--glass-saturate-panel` | `220%` | The side panel's own boost: at 6% read-through an extra radius has little luminance to work on, so the frost reads through colour instead. `off` sets `100%` |
| `--glass-backdrop` | `var(--c-toolbar)` | The fill the glass sits on |
| `--glass-transcript` | `var(--c-bg)` | Fill of the reading column. The canvas at every glass level — a translucent sheet over the canvas it already is cannot be seen, it can only flatten Cream's wash — and `color-mix(in srgb, var(--c-bg) 94%, transparent)` under `data-vibrancy`, where the desktop is behind it |
| `--glass-transcript-image` | `var(--c-bg-image)` | The palette wash under the transcript; `none` while the sheet is translucent |
| `--glass-canvas` | `transparent` | Canvas of a panel mounted in the glass sidebar |
| `--glass-edge` | `inset 0 1px 0 rgba(255,255,255,.55)` | The specular 1px inner highlight, window top edge only (dark: `.07`) |
| `--c-window` | `color-mix(in srgb, var(--c-surface) 40%, transparent)` | Window-frame tint, only defined under `html[data-vibrancy]` (see *Window vibrancy*) |

The level lives in `AppConfig.glass_effect`, is written to `<html>` as
`data-glass` (`applyGlassEffect()` in `src/App.tsx`) and selects a block in
`App.css` placed **after** the palette blocks (same specificity as `:root`,
later wins):

| `data-glass` | `--glass-blur` | `--glass-blur-panel` | `--glass-blur-scrim` | Note |
|---|---|---|---|---|
| `off` | `0px` | `0px` | `0px` | Fill swaps to `--c-surface`, saturate to `100%`: with no blur a `.62`-alpha bar would show the transcript through it |
| `subtle` | `12px` | `18px` | `6px` | |
| `standard` | `20px` | `30px` | `8px` | `:root` value, used when the config has no entry |
| `strong` | `32px` | `48px` | `14px` | |

`--glass-blur-panel` is the one rate that is **derived** instead of declared by
the level blocks — `calc(var(--glass-blur) * 1.5)` — because it is not a third
kind of glass but the same frost one step deeper, for the one surface whose
whole backdrop is 6% of an already blurred transcript (see *Control fills*).
Deriving it keeps `off` honest (0px × 1.5 is still 0) and stops the ladder from
opening a hole between `standard` and `strong`.

The transcript is the one surface that does **not** follow this ladder. Its
backdrop is either the canvas it already paints or — under `data-vibrancy` — the
desktop, so a level-scaled tint would be invisible in the first case and cost
the palette its canvas in the second (see *Chat transcript*). Its fill follows
`data-vibrancy` only.

#### Window vibrancy (`data-vibrancy`)

A webview can only blur its own pixels, so the desktop seen **through** the
window is blurred by the compositor's **Acrylic** backdrop (`window-vibrancy`,
applied by `apply_window_backdrop` in `src-tauri/src/lib.rs`): the Windows 11
system backdrop `DWMSBT_TRANSIENTWINDOW` on 11, the v1809 accent blur on
Windows 10. Acrylic, not Mica, on purpose — Mica only samples the wallpaper, so
against a plain background the frame reads as the flat pane it was before,
while Acrylic blurs what is actually behind the window. `apply_mica` still runs
first for its side effect: it is the only call that sets
`DWMWA_USE_IMMERSIVE_DARK_MODE`, so the backdrop follows the *app* theme and
not the system one; the Acrylic call then replaces the Mica backdrop.

The same `glass_effect` level drives it — `set_window_vibrancy` runs once at
startup and again from Settings → Appearance, and writes
`<html data-vibrancy="acrylic|mica">` **only** when the native call succeeded.
Without the attribute every vibrancy rule is inert and the app keeps its opaque
canvas, because a transparent window with no backdrop would show a raw,
unblurred desktop. `off` clears the backdrop.

The frame is the thinnest glass: `html[data-vibrancy]` points `--glass-backdrop`
at `--c-window`, so the titlebar, sidebar, toolbar, input area and panel headers
let the desktop through, while `.chat-area` drops its canvas. The transcript is
glass too, but it cannot take `--c-window`: it holds a wall of 13px text over
whatever the user has on their desktop, so it takes a floor of its own —
`--glass-transcript: color-mix(in srgb, var(--c-bg) 94%, transparent)`, the
density of the Settings sheet — and no `backdrop-filter`, because the
compositor has already blurred what is behind it. Measured over the worst
backdrops (black behind the light palettes, white behind the dark ones), 94%
keeps `--c-text` ≥ 9.3:1, `--c-body` ≥ 7.2:1 and `--c-muted` ≥ 4.5:1 across all
six palette × theme pairs; at 90% `--c-muted` already falls to 4.0:1 on the dark
palettes. The alpha does not follow the level ladder — it is a readability
floor, not decoration.

| `data-glass` | `--c-window` | Note |
|---|---|---|
| `subtle` | `color-mix(in srgb, var(--c-surface) 24%, transparent)` | Thin tint, the desktop reads clearly |
| `standard` | `color-mix(in srgb, var(--c-surface) 40%, transparent)` | The value on `html[data-vibrancy]` itself |
| `strong` | `color-mix(in srgb, var(--c-surface) 58%, transparent)` | Densest tint; the desktop is a hint |

The alphas stay **well under** the opaque `--c-toolbar` (`.62`): the backdrop
already blurs, so a tint as dense as the old bar fill composites back to the
same flat pane and the glass looks switched off. The native backdrop only adds
the blur (the Windows 11 system backdrop is tinted by the OS, the Windows 10
path gets a low-alpha tint), so the density per level lives entirely in
`--c-window` and both backdrops land on the same contrast.

#### Control fills (`--c-field` / `--c-btn` / `--c-sheet` / `--c-panel` / `--c-popup`)

A control painted `--c-surface` on a translucent band is a solid block on
glass. These five tokens are the fix, and they are opaque by default, so every
platform without a backdrop keeps the opaque look:

| Token | Opaque | Under `data-vibrancy` | Used by |
|---|---|---|---|
| `--c-field` | `var(--c-surface)` | `var(--c-window)` | inputs, textareas, selects, segmented and palette options, timer / injection chips |
| `--c-btn` | `var(--c-surface-2)` | `color-mix(in srgb, var(--c-surface) 55%, transparent)` | tonal buttons, list rows, nav items, inner confirmation cards |
| `--c-sheet` | `var(--c-surface)` | `color-mix(in srgb, var(--c-surface) 94%, transparent)` | page canvas that replaces the transcript (Settings, prompt editor) |
| `--c-panel` | `var(--glass-backdrop)` | `color-mix(in srgb, var(--c-bg) 94%, transparent)` | the side panel shell, its headers and the bands mounted inside it |
| `--c-popup` | `var(--c-surface)` | `color-mix(in srgb, var(--c-surface) 84%, transparent)` | floating cards over readable text (model picker, `@` popup, confirmation panel) |

`--c-field` resolving to the band tint is deliberate: a field then reads as part
of the glass and only its hairline defines it — the same relationship the opaque
design already had, where a field was filled with the page colour. `--c-btn`
stays one step denser than the band, so a tonal fill still reads as a patch
rather than as a hairline. Popups keep most of their body and pick up
`backdrop-filter: blur(var(--glass-blur-scrim))` under vibrancy: they open over
the reading column, so CSS *can* blur what is behind them even though it can
never blur the desktop. Hover fills on a glass surface use `--c-active`, which is
alpha-based in both themes and therefore never turns into a solid block.

The two large surfaces settle their density differently, because they cover
different things. A **page** (`--c-sheet`) carries 13px labels on every row and
replaces the transcript, so it stays deliberately near-opaque: at the window
tint the chat behind it read through as text and its hairlines cut the surface
into bands of different colour — the Settings header sat over the chat toolbar
while the rest sat over the transcript. At 94% nothing behind reads, and the
blur that stays on keeps the surface frosted instead of flat.

The **side panel** (`--c-panel`) is the opposite case: it floats over the
reading column, so the frost behind it is the effect — but it still carries 13px
session titles, so it lets 6% of the backdrop through and no more: 94% of the
canvas, one value for both themes and no dark override (`--c-bg` is already
theme-aware). Those 6% are what `blur(var(--glass-blur-panel))
saturate(var(--glass-saturate-panel))` filters — 30px / 220% at `standard`,
against 20px / 180% for a band — and they are enough for the panel to sit *on*
the conversation instead of on the bare canvas. The extra radius and the higher
saturation are what make 6% read as frost rather than as tint: at that alpha
there is little luminance for a radius to work on, but the ghost's own colour
still comes through stronger.

The density is also what makes the panel legible, which is the real argument:
a `--c-muted` title holds 4.6–6.0:1 where a blurred glyph of the transcript
passes beneath it, against 1.9–3.1:1 at two thirds and 4.3–5.5:1 at 92% — the
step below this one, and already under the 4.5:1 floor the palettes promise on
the dark default. Mixed from `--c-bg` and not `--c-surface`, so a panel's cards
(opaque `--c-surface`) still sit a shade above its shell, exactly as the opaque
design did.

Never write a blur radius, a `saturate()` or a `--c-toolbar` fill at a call site.
The whole material is these four declarations and nothing else:

```css
background: var(--glass-backdrop);   /* or --c-window under data-vibrancy */
-webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturate));
box-shadow: var(--glass-edge);
```

`.sidebar` and `.panel-header` are the single sanctioned variation: they read
`--glass-blur-panel` / `--glass-saturate-panel` in place of the two band values,
because they are the only surfaces that float over another surface instead of
sitting on the canvas.

Glass only reads when something sits **behind** it, so a panel that lives in the
sidebar (`.skills-panel`, `.tools-panel`, `.mcp-panel`, `.history-panel`,
`.agents-panel`) paints `--glass-canvas` and not a canvas colour. The sidebar's
own `--glass-backdrop` + blur then stays visible and the transcript underneath
is faintly readable — the sidebar is a `position: fixed` overlay, so the
message column really does continue beneath it. Cards inside the panel keep
`--c-surface`: only the pane is translucent, never the content.

#### Window-less backdrop (`data-glass-fallback`)

A window backdrop is the one part of the material the app cannot ask for
itself: `window-vibrancy` has Windows (DWM) and macOS (`NSVisualEffectView`)
backends only, `apply_window_backdrop` in `src-tauri/src/lib.rs` is
`#[cfg(target_os = "windows")]`, and on Linux the blur behind a window belongs
to the WM — KWin and Hyprland each have their own, GNOME/Mutter has none at
all. So every non-Windows platform fell back to the opaque canvas: the frame
bands still ran `backdrop-filter`, but the only thing behind them was a flat
`--c-bg`, and blurring a flat colour is a no-op. The sidebar was the single
exception — it is a `position: fixed` overlay, so the transcript really does
continue beneath it and its blur is visible on any platform.

`applyWindowVibrancy()` writes `<html data-glass-fallback>` when
`set_window_vibrancy` reports no backdrop (Linux, macOS, a Windows build without
Mica/Acrylic). The attribute is **not** a second material: it matches the same
tint block as `data-vibrancy`, so `--c-window`, `--c-field`, `--c-btn`,
`--c-sheet`, `--c-panel`, `--c-popup`, `--glass-transcript` and the level ladder
are identical. Only the origin of the backdrop changes — the app paints it:

| Token | Light | Dark | Role |
|---|---|---|---|
| `--glass-ambient` | `none` | `none` | The wash itself; only `[data-glass-fallback]` fills it |
| `--c-ambient` | `color-mix(in srgb, var(--c-text) 10%, transparent)` | `color-mix(in srgb, var(--c-text) 6%, transparent)` | The ink the wash is built from |

The wash is two oversized radial blobs whose peaks sit **inside** the frame
bands — top-right behind the toolbar, bottom-left behind the input area — and
fade out before they reach the middle of the reading column. `--glass-ambient`
is painted on `body::before` (`position: fixed`, `inset: 0`, `z-index: -1`), so
it is pinned to the window, above the canvas colour and below every in-flow
surface. Strength is where this had to be measured rather than chosen:

| | Rendered band at the blob peak (vs the flat canvas) | `--c-muted` on the band |
|---|---|---|
| light, 10% ink | `255 → 243` (12/255 darker) | 5.8 Default · 5.0 Sakura · 4.7 Cream |
| dark, 6% ink | `26 → 36` (10/255 lighter — a 40% lift on a dark band) | 4.7 Default · 5.9 Sakura · 6.2 Cream |

That ceiling is the honest limit of a *painted* backdrop: the bands carry
`--c-muted` labels, and the ink can only move the canvas as far as `--c-muted`
can afford — Cream's light and the default dark are within 0.2 of the 4.5 floor
at these alphas, so 10% / 6% is the end of the runway. What it deliberately
does *not* buy: the reading column stays flat (10% of the ink behind the 94%
fill is ~1/255, which is the whole point of the transcript floor), and the bands
blur the wash, never text. A backdrop that reads as strongly as Windows' Acrylic
over a photo wallpaper is not reachable this way — it needs content behind the
bands (the sidebar already has it: the transcript scrolls beneath it).

Rules that follow:

- **The window stays opaque.** `html` / `body` keep their `--c-bg`; only
  `[data-vibrancy]` may drop the page background, because only then is there a
  desktop behind it. `.chat-area` drops its canvas in both cases, so the wash is
  what the transcript's tint sits on (`.messages` still paints
  `--glass-transcript`).
- **Neutral only.** `--c-ambient` is mixed from `--c-text`, so each palette
  washes its own ink over its own canvas and no palette block declares a colour.
  Never build the wash from `--c-accent`: the accent is for the primary action,
  never for decoration.
- **`off` is excluded** (`:not([data-glass="off"])`) — no blur means no glass,
  and a backdrop nothing sits on is dead paint.
- **Contrast is kept.** Measured over the densest blob, across all six palette ×
  theme pairs: `--c-text` 10.5–17.2:1, `--c-body` 8.2–12:1 and `--c-muted`
  5.1–7:1 on the transcript; `--c-text` 10.2–15.5:1 and `--c-muted` 4.7–6.2:1 on
  a `--c-window` band. The `--c-muted` floor (4.5:1) holds everywhere, so the
  fallback never weakens a palette's guarantee.
- The scrim blur on `.file-mention-popup`, `.dropdown-popup`,
  `.settings-page` and `.prompt-editor-overlay` is kept: those open over the
  transcript, which is opaque enough to blur.
- **The blur stays honest.** The bands blur the wash, never text: the transcript
  scrolls inside `.messages`, not under the toolbar, so the frame's glass is
  translucency over a soft backdrop — not the text-frosting the sidebar gets.
  Making the bars frost the transcript is a layout change (the column would have
  to scroll beneath them), not a token one.

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
| `--c-bg-image` | `none` | Optional wash over the canvas (`body`, `.chat-area`, `.messages`) |
| `--c-code-image` | `none` | Optional wash over code blocks |
| `--c-reason-image` | `none` | Optional wash over reasoning / quote surfaces |
| `--glass-ambient` | `none` | Stand-in backdrop painted on `body::before` where no window backdrop exists (`data-glass-fallback`) |

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
--t-dur: 0.18s;
--t-ease: cubic-bezier(0.25, 1, 0.5, 1);
```

Use as `transition: background-color var(--t-dur) var(--t-ease)`. 180ms is the top
of the 120–200ms band the interaction spec asks for; anything longer reads as lag
in a keyboard-driven tool. The curve is a fast-out / slow-in ease (Out Quad). The
only sanctioned deviation is `box-shadow 0.25s var(--t-ease)` on the inset
focus/active ring of primary buttons.

### Legacy aliases that must stay

`--c-text-secondary` (→ Pewter), `--c-bg-secondary` (→ alternate surface). They are
referenced by `ModelSelect.css`; removing them silently
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

### Disclosure row

`.section-toggle` folds a compact settings block (Tools → auto-accept, Sub
Agents → orchestration settings): a full-width transparent button with a
14px/500 `--c-text` label on the left and an 11px `--c-muted` chevron on the
right, plus `aria-expanded` and a 1px `--c-accent-text` focus outline. The body
is rendered only while expanded. A collapsed section must never hide its master
control — the primary switch of the section stays outside the fold, so the
collapsed state still tells the user whether the feature is on.

### Inputs, textareas, selects

```css
height: 34px;                    /* textarea: auto + 8px 11px padding */
border: 1px solid var(--c-border-i);
border-radius: 4px;
background: var(--c-surface);
font-size: 14px;                 /* 13px in the compact Settings zone */
transition: border-color var(--t-dur) var(--t-ease);
```

Focus is a **border-colour change to `--c-accent-text`** — no glow, no ring, no shadow.
Placeholder uses `--c-placeholder`.

A `<select>` is never a native one. The list it opens is drawn by the platform —
GTK on Linux, a menu on macOS, its own popup on Windows — so the OS, not this
document, would author the control's loudest state. Every picker goes through
`components/Dropdown.tsx` instead; the platform-drawn `option` pair left `App.css`
with the last native select.

### Dropdowns

One primitive: a field-shaped trigger plus a `Portal`-mounted listbox, classes
only in `App.css` (`.dropdown-trigger*`, `.dropdown-chevron`, `.dropdown-popup`,
`.dropdown-list`, `.dropdown-option*`, `.dropdown-scrollbar*`). A component sheet
may add a width cap, a leading glyph or a row's own badges — never a frame.

| Part | Spec |
|---|---|
| Trigger | The field anatomy above: 34px, 1px `--c-border-i`, 4px radius, `--c-field` fill, 13px in the compact zone. **No hover fill** — a closed dropdown is a field, not a button |
| Trigger, focus / open | `border-color: var(--c-accent-text)`. No ring, no glow, no elevation; `aria-expanded` carries the same tint |
| Arrow | `.dropdown-chevron`: 10×6 masked glyph tinted by `currentColor` — `--c-muted` at rest, `--c-text` when the trigger is hovered, `--c-accent-text` while open or keyboard-focused. It flips 180° on `--t-dur`/`--t-ease` and holds still under `prefers-reduced-motion` |
| Popup | `--c-popup` surface, 1px `--c-border-i`, 4px, 4px inner padding, 6px gap to the trigger, 12px viewport margin, and it flips upwards when the room below is short |
| Row | `min-height: 28px`, `padding: 4px 8px`, 4px radius, 13px. One active fill: `--c-accent-soft` — the pointer and the keyboard move the *same* row |
| Row, selected | `--c-accent-text` label at `font-weight: 500`, never 600 |
| Scroll | The native bar is hidden and a 2px `--c-muted` indicator at 45% opacity is drawn in the DOM (WebKit ignores `::-webkit-scrollbar`); it turns `--c-accent-text` while the popup is hovered |

A trigger is exactly as wide as its call site says and no wider: it shows the
field's value, so it never grows at the expense of a neighbouring input. A list
whose values run long — absolute paths, model ids — gets a **width floor on the
popup** instead (`minPopupWidth`, as the model picker does for long ids); the
popup is clamped to the viewport and grows towards the room.

The titlebar workspace indicator is the one picker that is **not** this
primitive: its trigger is the label that marks the active workspace (12px, red
hairline), and replacing it with a 34px field would turn the window's identity
line into a form control. The list it opens is therefore a **menu** — the app
menu's frame and rows — not a field's listbox. Both are single-select popups
with one active fill; what changes is the chrome they hang from.

The arrow is a **mask, not a picture**. `--icon-chevron` holds the shape only —
10×6, a shallow 35° chevron, 1.25px stroke, butt caps, miter join — so the token
carries no ink and one definition serves every theme, palette and glass level;
the primitive paints it with `currentColor`. A painted data URI would freeze a
grey into the token, need a second definition per theme, and still lose the
hover/open tint — which is why the shape sits at `linecap: butt` next to the
field's 1px hairline rather than shouting over it.

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

#### Side-panel list rows

The five sidebar lists (`.history-list`, `.skills-list`, `.tools-list`,
`.mcp-list`, `.agents-list`) share one flat row language. The panel's own frost
is the surface, so a sidebar row is *not* a card — the rule above describes
cards elsewhere (popups, mission rows, forms):

```css
/* the list */
padding: 4px 8px 10px;
display: flex;
flex-direction: column;

/* a row */
position: relative;
padding: 9px 10px;
border-radius: var(--r-sm);
background: transparent;

/* the separator: one inset hairline between neighbours */
.history-item + .history-item::before {
  content: "";
  position: absolute;
  top: 0;
  left: 10px;
  right: 10px;
  height: 1px;
  background: var(--c-border-s);
}
```

- Hover and keyboard focus fill the row with `--c-hover`; there is no outline,
  no lift and no scale. The hairline stays — it is the list's structure, not a
  hover state.
- Titles are 14px/500 `--c-text`; metadata is 12px `--c-muted` (timestamps in
  `--c-placeholder`, tabular numerals). No negative tracking.
- A switched-off row is `opacity: .55`; its switch and actions stay reachable.
- The only accent in a list is the current History session: `.history-selection`
  (`--c-accent-soft` fill + 2px `--c-accent` leading rule) is drawn once behind
  the rows so it travels between sessions instead of blinking on each row. The
  History filter tabs use the same travelling-fill idea, and the selected
  auto-accept row in the Tools footer uses `--c-accent-soft`.
- `.toggle-switch` (36×20, `--c-knob`) is the only switch in a row; a panel
  stylesheet must not declare a local toggle.
- Row actions are 24×24 transparent icon buttons in `--c-muted`; hover →
  `--c-hover` + `--c-text`, destructive → `--c-danger-soft` + `--c-danger`.

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

- Modal backdrop: `rgba(128,128,128,.65)` + `backdrop-filter: blur(var(--glass-blur-scrim))`.
- Popup / dialog surface: `--c-surface` + 1px `--c-border-i` + 4px radius, **no shadow**.
  A popup that floats *inside* the composer uses `--c-bg-secondary` instead, so it
  separates from the frosted input bar without elevation.
- Frosted bars: `background: var(--glass-backdrop)` + `backdrop-filter: blur(var(--glass-blur))`
  (`saturate(var(--glass-saturate))` added where the header tints its backdrop).
  Both tokens follow Settings → Appearance → Frosted glass.

#### Composer mention popups (`@file`, `/skill`, `+agent`)

One surface, three triggers, one behaviour. `components/FileMentionPopup.css` owns
the frame — header, rows, active fill, empty state — and
`components/FileMentionPopup.tsx`, `components/SlashMentionPopup.tsx` and
`components/AgentMentionPopup.tsx` render those `.file-mention-*` classes.
`SlashMentionPopup.css` may only add the skill glyph colour
(`--c-accent-text`) and the left-to-right description row, and
`AgentMentionPopup.css` the same two deltas plus the emoji badge box; neither
may restate the frame.

- Sits `bottom: calc(100% + 6px)` inside `.input-row`, full input width,
  `max-height: 240px` with its own scroll.
- **Never takes focus.** A row accepts on `mousedown` + `preventDefault`, so the
  caret stays in the textarea and the popup is drivable by keyboard alone.
- Keyboard contract, identical for all three: `↑` / `↓` move, `Enter` / `Tab`
  accept, `Esc` dismisses. Only the active row wears the `--c-accent-soft` fill.
- Row: 4px radius, transparent at rest, `--c-hover` on hover; name `--c-text`
  500, meta (path or description) `--c-muted` 11px, truncated, `margin-left: auto`.
- **At most one popup is open** — the token grammars are mutually exclusive
  (`@` needs a word boundary and no whitespace/`/`; `/` needs a word boundary and
  no whitespace/`@`/`#`; `+` needs to open the line and the line may only hold
  `+token` words), so no stacking rule is needed.
- The `@` picker materialises attachments on send; the `/` picker leaves a plain
  `/skill-name` token in the body for the model to resolve with `use_skill`; the
  `+` picker leaves a `+agent-token` the backend resolves into a session-scoped
  system-prompt snapshot (only in a conversation with no history yet).

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

#### Empty-state nap face (evolution maintenance)

The smiley above the empty-state greeting is an action while
`self_evolution_mode` is on. It is a real `<button>` around the same SVG (the
hopping animation is unchanged); the decorative hint/bubble layers sit beside
it and are `aria-hidden`, while the button's `aria-label` carries the action.
When the mode is off the same face is rendered as a plain decorative `<div>`:
no hint, no hover treatment, no click handler and no focus target.

- **Idle:** hovering or focusing shows a 12px speech bubble — `--c-surface`,
  1px `--c-border-i`, 4px radius and a rotated 8px square tail pointing at the
  face — "点击让我小憩一下", fading in on `--t-dur` / `--t-ease` (opacity only,
  no movement). The face keeps hopping.
- **Running (the `rabbit-hop--sleeping` modifier):** the shuttle parks at
  `translateX(36px)` (centred in its 120px lane), the hop is replaced by a slow
  `scale` breathing loop, the gaze stops and each eye squashes to
  `scaleY(0.12)` — a closed line. The hint bubble moves above the nap bubble
  and reads "正在小憩，整理技能与记忆… · 双击可终止（不建议）".
- **Status block:** while the pass runs, the greeting's two lines are replaced
  by a two-line live status: a 17px/500 `--c-text` title ("正在整理技能与记忆…")
  and a 14px `--c-muted` detail ("Dreaming Scene N/M", where N is the current
  round and M the executed tool-call count). The phase from
  `evolution-maintenance-state` (`inventory` / `files` / `skills` / `memory` /
  `wrap_up`) is kept as the detail line's `title` tooltip. Round and tool-call
  counts use tabular figures so changing numbers never shift the line.
- **Result bubble:** a completed pass replaces the greeting with a clickable
  speech bubble in the same slot: `--c-surface`, 1px `--c-border-i`, 4px
  radius, a rotated 8px tail pointing up at the face, 13px `--c-body` text
  (the model's report, `pre-wrap`), a 13px/500 `--c-text` title and an 11px
  `--c-muted` "点击关闭" line. It is capped at 420px wide / `min(260px, 45vh)`
  tall with its own scroll area (the tail sits on the outer wrapper so scrolling
  cannot clip it). Clicking anywhere in the bubble dismisses it and restores the
  greeting; starting a new pass also clears it. It fades in on `--t-dur` /
  `--t-ease` (opacity only) and is static under `prefers-reduced-motion`. No
  extra colours, no elevation.
- **Bubble:** 100×46px, `--c-surface` + 1px `--c-border-i`, 4px radius, no
  shadow; a rotated 8px square forms the tail. Inside, one horizontal row of
  random glyphs (`--c-muted`, 12px) scrolls left on a 4.2s linear loop; the
  glyph list is rendered twice and the track translates `-50%`, so the loop is
  seamless. Three `z` glyphs (13/11/9px) drift up to the right of the bubble.
- **Cancel:** two clicks inside a 320ms window on the napping face stop the
  pass; a keyboard activation (`detail === 0`) stops it immediately. A
  double-click on the *hopping* face only starts the pass (the first click's
  start is deliberately not treated as a cancel).
- **Done / stopped / error:** the terminal event removes the bubble and
  restores the hop. An error also surfaces through the normal `ChatNotice`
  warning. Nothing is added to the transcript.
- **Tokens only:** face `--c-muted`, eyes `--c-bg`, bubble/hint `--c-surface`
  with `--c-border-i`; no accent, no literal hex, no elevation.
- **`prefers-reduced-motion: reduce`:** every loop stops — the face is parked
  and breathing-less, eyes stay closed, the ticker is static and the `z`s are
  visible at their offset positions.

### Chat transcript

This is the one place with a deliberate asymmetry:

- **The transcript is the palette canvas, and glass under the desktop.**
  `.messages` paints `background-color: var(--glass-transcript)` then
  `background-image: var(--glass-transcript-image)`. Both resolve to the canvas
  (`--c-bg` + `--c-bg-image`, the pair `.chat-area` paints) at every glass
  level, so a palette with a canvas wash (Cream) runs behind the transcript and
  behind both frosted bars instead of stopping at the toolbar hairline; under
  `data-vibrancy` the fill becomes a 94% sheet of the canvas and lets the
  desktop tint the column. The longhands are required: the `background`
  shorthand resets the image. The transcript draws it *itself* rather than
  inheriting it from `.chat-area`, which is what keeps the column solid where
  that one drops its canvas.
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
- **Turn cost sits to the right of the reply's action buttons.** A settled turn
  shows one muted line (`.message-usage-meta`): duration, total tokens, and —
  only when the provider reported prompt caching — the turn's hit rate,
  separated by ` · `. 12px `--c-muted`, tabular numerals, no fill, no border,
  no badge: it is a figure, not a state. A turn whose endpoint reported no
  usage shows nothing at all rather than inventing a zero.
- Streaming states may animate a gradient sweep across a label, but must respect
  `prefers-reduced-motion`.

### Figures

The assistant can draw. A reply may carry a 2D figure (an inline `<svg>`) or a
3D one (a `<carve>` scene), and both arrive in the transcript as the same card:

- **One frame for both kinds**: `--c-surface-2` fill, `1px --c-border-s`, 4px
  radius, no elevation — the recipe a code block and a table already use. A
  figure is content, not a floating panel.
- **Two surfaces, one step.** The drawing sits on `--c-surface` (the sheet) and
  the caption bar on the tinted `--c-surface-2`; the colour step *is* the
  separator, so the card carries no second hairline. The same sheet colour is
  what a saved PNG and a 3D canvas are filled with, so the file matches the
  screen.
- **The drawing owns the width.** The viewport is full-bleed inside the frame
  with 10px padding; a 2D drawing scales by its `viewBox`, a 3D canvas keeps a
  16:10 viewport. Neither scrolls sideways.
- **A drawing is capped, never stretched.** A 2D figure taller than `420px`
  letterboxes inside the viewport (`preserveAspectRatio` centres it), so a
  hand-written `viewBox` with an extreme aspect ratio cannot grow the card to
  millions of pixels; the 3D canvas keeps its fixed 16:10 and needs no cap.
- **The caption bar** holds the title at 12px `--c-muted` (ellipsised) and the
  save action. Actions are revealed on hover *and* on `:focus-within` — the
  code-block toolbar rule, so the card is usable without a pointer.
- **Saving is a state, not a toast**: `--c-success` text with the
  workspace-relative path when the file lands (and that line is the way to the
  folder), `--c-danger` when it fails, and the button itself reads `Saving…`.
- **Colour comes from tokens, never from the payload.** A `<carve>` scene names
  `accent` / `success` / `warning` / `danger` / `text` / `muted`; the renderer
  resolves them from the live element, so a palette switch repaints the figure.
  A 2D figure is written in `var(--c-*)` and `currentColor` and keeps them.
- **A figure that cannot be drawn shows its source** in the code surface instead
  of an empty frame; one that is still streaming says `Drawing…` in `--c-muted`.
- **Motion**: no auto-rotation. Orbiting is pointer-driven only, so a reply is
  static unless the user touches it.

#### Dynamic figures (`html-fig`)

A drawing that has to *move* — a physics sketch that animates, a plot the user
can drag, a canvas loop — is a small document rather than a picture. A reply may
carry one as a fenced `html-fig` block or an inline `<html-fig>…</html-fig>`,
and it is the same card as the other two kinds. What differs is where it runs:
the fragment never touches the transcript's own document. It is assembled into a
complete page (`src/utils/htmlFigure.ts`) and handed to a sandboxed frame.

- **The frame is a separate origin.** `sandbox="allow-scripts"` *without*
  `allow-same-origin`: the origin is opaque, so `parent.document` throws,
  `localStorage` throws, no app cookie is readable, and the frame cannot
  navigate the window. `allow-scripts` is the only permission — no popups, no
  forms, no modals, no top navigation.
- **The network is shut by policy — except navigation.** The wrapper carries
  `default-src 'none'`, with inline `style` / `script` open (that *is* what a
  dynamic figure is) and `img-src data: blob:`. Measured on WebKitGTK, fourteen
  channels tried (`fetch`, `XHR`, beacon, `WebSocket`, `EventSource`, `img`,
  `script`, stylesheet, `@import`, `@font-face`, `url()`, media, child frame):
  zero requests reached a listening server, and a saved file still works offline.
  The app document itself ships without a CSP, so this is the second wall rather
  than a copy of the first. `navigate-to` never shipped, though, so a fragment
  that assigns `location` *does* get a request out — the URL is the payload. The
  sandbox still applies to the document that arrives; the card counts the frame's
  loads and closes the figure on the second one instead of drawing an outside
  page inside a reply.
- **Execution is tiered by what the fragment can do, consented for script.**
  Markup, CSS and SVG animation cannot block the thread they share with the
  transcript, so a script-less fragment starts on the settle commit and the
  reader sees the animation without a click. A fragment that *could* run script
  (`<script>`, an `on…` attribute, a `javascript:` URL — `htmlFigure.hasInlineScript`)
  waits behind a poster (`Play dynamic figure` / `Contains script · runs
  sandboxed · no network`), because a sandboxed frame shares the web process
  with the transcript in **both** engines: one `while (true) {}` freezes the
  whole window, and no watchdog can end it because the watchdog runs on the
  frozen thread (measured — a 2 s loop left a 2041 ms hole in the parent's
  50 ms timer on WebKitGTK and 2022 ms on Chromium). The poster offers a second
  button, `Play automatically this session`: it records consent in
  `sessionStorage` — surviving a reload, dying with the app, unreadable and
  unwritable by the frame (opaque origin) — and every gate that is still up
  lifts at that reply's next rebuild. Never auto-run script without that record,
  and never "improve" the tiers into autoplay for everything.
- **The wrapper is authored here, the fragment is not.** The fragment is parsed
  once and its nodes are *imported* into the wrapper, never concatenated into a
  template string — a `</style>` inside the model's CSS would reopen the
  wrapper's markup. `base` / `link` / `meta` / `title` from the fragment's head
  are dropped; its scripts move into the body, after the reporter, so one that
  throws cannot cost the card its height.
- **Tokens cross the boundary as a message.** Custom properties do not inherit
  through a frame element, so the sheet is baked into the frame's `:root` when
  it is built and *re-sent* on a theme or palette switch: the animation keeps its
  place instead of restarting.
- **The card measures the frame, not the reverse.** The viewport is
  `padding: 0` — the frame *is* the sheet — and a reporter inside the wrapper
  measures `body`'s border box and posts it; the card clamps it to `120–560px`.
  Measuring `documentElement` would echo the height the card just set and the
  message would be a loop.
- **It plays when the reply settles.** Chromium reloads a frame whenever the
  element is re-inserted, and a commit replaces the transcript subtree, so a
  frame mounted mid-stream would restart on every token. The card waits for the
  commit nothing follows; until then it reads `Plays when the reply finishes` in
  `--c-muted` with the accent glyph, never an empty box.
- **Saving writes `.html`, not PNG.** An animation has no single frame and a
  cross-origin frame cannot be captured, so the save writes a self-contained
  document to `<workspace>/figures/<slug>.html` with the tokens baked in — the
  figure opens by double-clicking it.
- **A fragment over the cap shows its source** (`htmlTooLarge` in the status
  line) instead of running, exactly as an undrawable `svg` does.

#### Mermaid diagrams

With `enhanced_figures` off (Settings → Advanced) the prompt asks for a fenced
`mermaid` block instead of hand-written `svg` / `carve` / `html-fig`, and the
transcript renders those diagrams too — as the same card.

- **Same frame, no save action.** The card is `.chat-figure` *plus*
  `.chat-mermaid`: frame, caption bar, `Drawing…` and the invalid state are the
  figure card's. There is nothing to save — the SVG is generated by the
  renderer, not authored by the model.
- **Mermaid bakes its colours into the SVG**, so it is themed where it is
  rendered: `themeVariables` come from the live element's tokens and the card
  re-renders on a theme or palette switch. CSS cannot repaint it.
- **A graph over ~2.5× the card width keeps its natural width** and the card
  scrolls sideways; below that it scales like a drawn figure. Squeezing a
  15-node flow into the card turned it into a 14px strip.
- **The SVG is pruned before insertion** (`pruneMermaidSvg`): `script`,
  `foreignObject`, `on*` handlers and external references go. Mermaid's own
  `<style>` stays — it is scoped to that diagram's generated id.
- **The export re-renders in the print palette.** The PDF/HTML export inlines
  the drawn SVG; since the colours are baked, exporting a dark transcript would
  otherwise put dark blocks on a white page, so the export renders each diagram
  again with the print tokens.

### Markdown editor

Titlebar `⋯` → "Markdown edit…" opens a two-column overlay over the chat:
the source on the left, the rendered preview (`react-markdown` + KaTeX + GFM)
on the right.

- **Header actions are secondary.** `toolbar-btn`, right-aligned before
  Cancel; Electric Blue stays on "Save" (`send-btn`) alone. "Export PDF" leads
  the group and carries an icon — it is a different kind of action from the two
  text buttons beside it.
- **A PDF leaves the app as a file, and it is always light.** The preview is
  cloned and printed in the print palette (`--c-text` on `#ffffff`, hairlines,
  A4 with 16 mm margins), never in the screen's colours — a dark transcript
  must not become a dark sheet. Bars and scrollbars the preview shows on screen
  (the table's horizontal scroll, for one) become plain page flow.
- **The file manager is the confirmation.** A finished export reveals the file
  in Explorer/Finder; floating notices stay reserved for warnings and errors, so
  a success never borrows their colour. A cancelled save dialog does nothing at
  all. The transcript's export button takes the same route (`utils/pdfExport`),
  so a reply also leaves as a file — same dialog, same palette, same reveal.

### Interaction monitor

The log dialog opened from Settings → Runtime & Debug. Chrome is the shared
dialog recipe — `--glass-backdrop` header with one hairline, `--c-surface` content,
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
- **Read after the turn, never during it.** The log is read when the dialog
  opens, again when a turn ends and on demand (`Refresh` in the controls row) —
  not on a timer: a running turn is still writing those rows, so it would read a
  half-written session while holding the writer's connection. While a turn runs
  the list pane says so in one line and the report third stays hidden. Each row
  carries a 150-character preview per side, cut in SQL (`substr`), so a
  tool-heavy session transfers kilobytes instead of its whole payload.
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
| Panel header | `.panel-header` (shared, `App.css`): `min-height: var(--panel-header-h)` = 49px so its hairline lands on the chat toolbar's, `padding: 6px 12px`, frosted `--glass-backdrop`, 1px hairline, title + one `faAnglesLeft` collapse chevron and nothing else |
| Panel footer | `padding: 10px 12px`, 1px hairline |
| Panel list | `padding: 4px 8px 10px`; the rows sit flush, separated by a 1px inset hairline (no `gap`) |
| Panel row | `padding: 9px 10px`, `gap: 4px`, transparent on the panel's frost, `--r-sm`; hover/focus-within `--c-hover` |
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
| Bar | `--titlebar-h` (32px), `--glass-backdrop` + `blur(var(--glass-blur))`, 1px `--c-border` bottom, `user-select: none` |
| Drag region | The bar **and** its filler carry `data-tauri-drag-region` (the attribute does not inherit); buttons never do |
| Window buttons | 34×28, 4px radius, `--c-muted` glyph at 12px; hover `--c-hover` + `--c-text`; active `--c-active` |
| Close button | Destructive variant: hover is `--c-danger-soft` fill + `--c-danger` glyph |
| App menu (`⋯`) | `--c-surface`, 1px `--c-border-i`, 4px radius, no shadow; rows 32px, 13px/400, hover `--c-hover` |
| Workspace indicator | 12px `--c-muted`, ellipsized; 1px `--c-danger` underline at a 2px offset marks the active workspace |
| Workspace indicator, control | 22px tall, `0 6px` padding, 4px radius; hover / open / focus is a colour change only — `--c-hover` fill, `--c-text` label, no ring, no size change; the underline stays (it marks, it does not interact). 55% opacity for an inert row |
| Workspace switcher popup | The app menu's frame and rows (32px, 13px), anchored left under the indicator; `min-width: 260px`, `max-width: min(480px, calc(100vw - 96px))`, `max-height` with the native bar hidden (it is a menu, not a panel list). A row is the **whole** folder name — the name is the identity and never truncates — then the 11px `--c-muted` parent path, which is the only part allowed to ellipsize, plus an 11px note: `--c-muted` for the current workspace, `--c-warning` for a folder that is gone. Choosing a row closes every sidebar before the swap, so no panel repaints against the workspace being replaced |
| Placement | Windows/Linux: minimize / maximize / close on the right; macOS: close / minimize / maximize on the left |

The titlebar is `position: fixed` at `z-index: 2000`, above every overlay, so the
window controls stay reachable while Settings or the Markdown editor is open.
Fullscreen pages start at `top: var(--titlebar-h)`; the sidebar starts below it
too. True modal dialogs (9999 layer) still cover the titlebar on purpose.

The sidebar panel header (`.panel-header`) repeats the titlebar's frosted
`--glass-backdrop` and the chat toolbar's height, so the whole top of the window —
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

## PATCH v1.0 — Liquid Glass material & interaction spec

An external spec ("AI 客户端界面设计与交互规范 v1.0") was folded into this
contract. It describes a macOS Liquid Glass client; this app is a Tauri desktop
tool built around a 4px hairline layout, so the patch is applied **through the
token system**: its *material* moved, its *geometry* did not. Everything below is
implemented with a token, or listed as a deliberate refusal.

### The three principles, in this codebase

| Principle | Implementation |
|---|---|
| 通透与层级 — translucency carries the hierarchy | `--glass-backdrop` + `blur(--glass-blur)` + `saturate(--glass-saturate)` + `--glass-edge`, scaled globally by `data-glass`, with the DWM backdrop under `data-vibrancy`. A coloured block where a tinted surface would do is a regression. |
| 克制与聚焦 — the conversation is the interface | One accent (`--c-accent`); the transcript stays the canvas `--c-bg` (it is glass only where a desktop is behind it); the chrome stays neutral. |
| 细腻的光影 — specular detail | `--glass-edge` (1px inner top highlight), `--c-border` hairlines, and the two sanctioned shades: the scroll edge and `--shadow-side`. Nothing else. |

### Material (spec §2)

| Spec says | Here |
|---|---|
| `blur(24px) saturate(180%)` | `blur(--glass-blur)` — `20px` standard, `12` subtle, `32` strong, `0` off — always paired with `saturate(--glass-saturate)` (`180%`, `100%` when off). The side panel and panel headers read the derived pair `--glass-blur-panel` / `--glass-saturate-panel` (`30px` / `220%` at standard) instead — the one sanctioned variation |
| `border: 1px solid rgba(255,255,255,.4)` | `--glass-edge` inner highlight (window top edge only), on top of the `--c-border` hairline |
| `box-shadow: 0 8px 32px rgba(0,0,0,.08)` | **refused** — non-negotiable 3, no elevation. The one shading in the system is `--shadow-side`, which separates the floating side panel from the transcript it covers and casts on nothing else |
| `border-radius: 16px` panels, `24px` pills | **refused** — non-negotiable 4. The 4px geometry is what keeps the app from reading as a web page imitating macOS |
| focus glow `0.08 → 0.18` on the input pill | **refused for the composer**: it holds focus while typing, so a permanent glow would be the loudest object on screen. The glow stays on the primary CTA ring (`--c-accent-soft`, `0.10 → 0.18` on hover) |

### Interaction (spec §3–§5)

Already satisfied — no change was needed:

- **Header**: the window drag region (`--titlebar-h`), 1.5px `currentColor`
  strokes, `--c-hover` fill on `--t-dur`.
- **Composer**: `@` reference, attachments, model picker, thinking level and the
  accent send button, all on the frosted `--glass-backdrop`.
- **Reasoning** is a collapsible low-contrast block (`--c-reason`, `--c-muted`
  copy); **code** blocks carry their own surface, language label and copy button.
- **Motion**: `--t-dur` `180ms`, `--t-ease` `cubic-bezier(.25, 1, .5, 1)`, colour
  and border only.

Refused or deferred:

- **Spring physics** for anything other than the side panel — one curve is a non-negotiable. Height
  transitions use `--t-ease`, and `prefers-reduced-motion` still collapses them.
- **`scale(1.02)` on the send button** — non-negotiable 5 keeps hover to colour;
  a scaling button in a 34px row shifts the optical weight of its neighbours.
- **Liquid radii / floating pill geometry** — that is a *mode*: it would have to
  swap `--r-*` wholesale. Pills next to 4px fields is worse than neither.
- **A second chromatic accent** (Aurora Blue / Fluid Violet as a new hue) — the
  palette system already re-hues the single accent; a parallel one breaks rule 2.

---

## PATCH v1.1 — side panel (history, skills, tools, agents, MCP)

The side panel is the one surface that floats **over** the transcript, so it is
also the one place where the glass has to show something: the conversation
blurring past behind it is the effect, not an accident.

| Spec says | Here |
|---|---|
| `rgba(255,255,255,.72)` light, `rgba(30,30,30,.65)` dark | `--c-panel` = `color-mix(in srgb, var(--c-bg) 94%, transparent)` — the `.72 / .65` pair first became `75% / 65%`, then `66.667%` for both themes, then 95%, and settled on 94% (6% read-through) once the panel was specified as frosted rather than see-through; 92% is the next step down and it already drops a `--c-muted` title to 4.3:1 on the dark default (two thirds: 1.9–3.1:1), see *Control fills*. The frost went the other way — the panel moved to `--glass-blur-panel` / `--glass-saturate-panel` (30px / 220% at `standard`) instead of taking more backdrop. Mixed from the canvas, not the surface, so the panel's cards (opaque `--c-surface`) still sit a shade above the shell |
| `blur(28px) saturate(190%)` | `blur(var(--glass-blur)) saturate(var(--glass-saturate))` — the level scales it `12 / 20 / 32px`, so nothing hard-codes a radius. The panel's frost is the derived `--glass-blur-panel` (`18 / 30 / 48px`) with `--glass-saturate-panel` |
| `border-right: 1px solid rgba(255,255,255,.35)` | the `.sidebar::after` hairline stays the structural edge; there is no lit edge on the panel — its top is tucked under the titlebar — only the `--shadow-side` shade |
| `box-shadow: 10px 0 30px rgba(0,0,0,.04)` | `--shadow-side` = `10px 0 30px var(--c-panel-shade)` (`rgba(0,0,0,.05)` light, `.32` dark), applied **only** to `.sidebar` |
| `border-radius: 10px` rows, `rgba(var(--primary-color-rgb), .08)` fills | rows keep `--r-sm` and use `--c-accent-soft` / `--c-hover`; there is no `--primary-color-rgb`, and no second way to spell the accent |
| card rows: `--c-surface` + 1px outline | **refused for sidebar lists** — the panel's frost is the surface. Rows are transparent, there is no outline and no elevation; one inset `--c-border-s` hairline separates neighbours (see *Side-panel list rows*) |
| active row = soft fill + theme indicator | `.history-selection` is `--c-accent-soft` with a `2px --c-accent` leading bar and **no** outline. The filter tabs' travelling indicator lost its accent outline for the same reason: a box around the label read as a form field |
| hover `rgba(0,0,0,.03)` | `--c-hover`; the separator hairline stays, because it is the list's structure rather than a hover state. Only the two hairlines around the selected History row are hidden, so the travelling fill reads as one continuous row |
| title Medium, meta smaller / lower contrast | `.history-preview` `500` at `14px`; `.history-meta` and `.history-created` stay `400` at `12px` in `--c-muted` / `--c-placeholder` |
| gradient on the selected row | **refused** — gradients are palette-only (Cream washes); a translucent fill does the same job without a second colour system |

---

## PATCH v1.2 — panel motion: spring physics

`framer-motion` (`motion.aside` + `AnimatePresence`) drives the side panel's
entrance and exit. It is the **only** animation in the app that is not
`--t-dur` + `--t-ease`.

```ts
// src/App.tsx
const SIDEBAR_SPRING = { type: "spring", stiffness: 320, damping: 32 };
const SIDEBAR_SHOWN = { opacity: 1, x: 0, pointerEvents: "auto" };
const SIDEBAR_HIDDEN = { opacity: 0, x: -20, pointerEvents: "none" };
```

| Spec says | Here, and why |
|---|---|
| `animate={{ width: isOpen ? 280 : 0 }}` | **refused for width** — the panel keeps its fixed `400px`. Animating the width of a frosted surface re-samples the backdrop and reflows every row on each frame, which is the stutter the spec's own §3 warns about |
| `x: isOpen ? 0 : -20` | adopted verbatim as `SIDEBAR_HIDDEN.x`: a 20px glide plus the fade is what makes the spring felt without moving the whole surface |
| `stiffness: 320, damping: 32` | adopted verbatim. No second spring exists anywhere, so the app still has one motion language |
| `overflow: hidden` + `white-space: nowrap` | already true, and moot: nothing resizes, so no text can reflow mid-animation (`.sidebar { overflow: hidden }`, rows ellipsise) |
| `translateZ(0)` / `backface-visibility: hidden` | `.sidebar` carries `will-change: transform` + `backface-visibility: hidden`; the inline `transform` framer writes would overwrite a literal `translateZ(0)` |
| a leaving panel must not eat clicks | `SIDEBAR_HIDDEN.pointerEvents: "none"` |
| `prefers-reduced-motion` | `useReducedMotion()` collapses the spring to a 0ms swap; mount and unmount still run |

`--t-dur-sidebar` and the `.sidebar-opening` / `.sidebar-closing` crossfade were
removed with it. `AnimatePresence` owns the exit, and `sidebar` state drops to
`null` the moment it closes — Escape, the window title and chat-click handling
never see a half-open panel.

---

## PATCH v1.3 — the transcript sits on the palette canvas

`.messages` carried `background: var(--c-bg)`, the shorthand, which resets
`background-image` to `none`. On the Cream palette — the one palette whose
`--c-bg-image` is an oklab wash — the toolbar and the input area read as a
gradient frosted band while the transcript underneath them went flat: the wash
was painted by `.chat-area` and then covered by its own child. The transcript
now paints the same canvas pair, colour first, image second.

| Spec says | Here, and why |
|---|---|
| the transcript is opaque | **kept.** It stays opaque: the Cream wash is built from opaque canvas colours, so neither the reading contrast nor the scroll cost moves, and `data-vibrancy` still gets a solid sheet |
| the transcript is the same canvas as the chrome | **the fix.** Two longhands on `.messages`, no new token and no new gradient — it consumes the palette's existing `--c-bg-image`, which is the sanctioned surface wash |
| the frame is glass, so the transcript should be glass too | **refused.** `--glass-backdrop` resolves to `--c-surface` at `data-glass="off"` (the transcript would become a panel) and to `--c-window` under `data-vibrancy` (the transcript would show the desktop, lose its contrast guarantee and re-composite on every scroll frame). It would cost per-level `--c-transcript` tokens and a reversal of "the transcript stays a solid sheet" |
| `.messages` could just inherit `.chat-area` | **refused.** Under `data-vibrancy` `.chat-area` is transparent, so an inherited canvas would leak the desktop into the transcript |

---

## PATCH v1.4 — the transcript is glass

v1.3 refused a glass transcript. The refusal is reversed in the one place glass
can exist — `data-vibrancy`, where the desktop is behind the column — and kept
everywhere else, because a translucent sheet over the canvas it already is
cannot be seen: it can only flatten Cream's wash, and pulling the fill toward
the bars' white would cost each palette its canvas (Sakura would lose the pink
the palette is named for). Glass is a *material* the column shares; the colour
it shares is the canvas.

| v1.3 said | Here, and why |
|---|---|
| `--glass-backdrop` resolves to `--c-surface` at `off`, so the transcript would become a panel | **accepted.** The transcript does not use `--glass-backdrop`. Its fill is `--glass-transcript`, which is `--c-bg` at every glass level, so `off` — and every level without a desktop behind it — is byte-identical to v1.3 |
| under vibrancy `--glass-backdrop` is `--c-window`, and the column would lose its contrast guarantee | **accepted, answered with a floor.** The column carries its own alpha: 94% of `--c-bg`, the density of the Settings sheet. Measured over a black desktop (light palettes) and a white one (dark palettes): `--c-text` ≥ 9.3:1, `--c-body` ≥ 7.2:1, `--c-muted` ≥ 4.5:1 across all six palette × theme pairs. At 90% the dark palettes' meta text lands at 4.0:1 |
| it would re-composite on every scroll frame | **not applicable.** The column paints no `backdrop-filter`. Without vibrancy the only thing behind it is the flat canvas; under vibrancy the compositor has already blurred the desktop, so a CSS blur would filter a filtered image. A translucent fill scrolls like any other background |
| simply copy the bars' fill, which is what “adapt the message background to the toolbar” reads as | **refused.** `--glass-backdrop` is a gradient in Cream and a near-white in Sakura: copying it turns the column into another band and buries the palette's canvas. The transcript borrows the material and the frame's tint family, not its density |
| follow the `data-glass` ladder like `--c-window` does | **refused.** The floor is a readability constraint, not decoration: 24–58% under a wall of 13px text is what v1.3 was right about. The transcript is the densest glass in the app and stays there at every level |

---

## PATCH v1.5 — the reply can draw

The transcript renders markdown with `html: false`, and that is not negotiable:
raw HTML is the injection vector the whole renderer exists to avoid. So a figure
never travels as HTML. The source is scanned *before* markdown-it runs, each
figure is cut out and replaced by an invisible slot token, and the card is
mounted into the committed subtree afterwards. Everything else follows from that
one decision.

| Tempting | Here, and why |
|---|---|
| enable raw HTML so a `<svg>` just works | **refused.** `html: false` is the only thing between a model reply and a script host. The slot-token path reaches the same result while keeping every element the model wrote out of the document until it has been sanitised |
| make `<carve>` a `<canvas>` the model draws into with script | **refused, same rule.** Executing model-authored JavaScript in the transcript is raw HTML with extra steps. `<carve>` is a declarative scene — JSON primitives, transforms, token colour names — drawn by three.js. A fragment that genuinely has to *run* is a different feature with a different boundary, not a change to `<carve>`: see PATCH v1.6, where the frame's opaque origin — not a sanitiser — is what keeps it out of the app |
| one `WebGLRenderer` per figure | **refused.** A renderer per card exhausts the browser's ~16 context limit after a few scenes, and every rebuild leaks one. One shared off-screen renderer blits each scene into the figure's own 2D canvas: unbounded figures, one context, and the canvas is already a PNG source |
| let the card rebuild with the transcript | **refused.** The subtree is replaced on every streamed token; a rebuilt card would drop its pixels and the orbit angle the user just dragged. Cards are cached by source hash and *moved* back into the new tree |
| keep `<style>` inside the sanitised SVG — drawings lean on it | **refused.** Inside an inline SVG those rules style the whole document, so a reply could restyle app UI (hide the confirm dialog's buttons, fake one) with selectors no text screen can tell apart from drawing styles. The element is dropped; drawings carry their look in presentation attributes, which is what the prompt asks for anyway. The *root's* `style` attribute is dropped too — the root is the one element that generates a CSS box, so `position: fixed` there would lift the drawing out over the window — and the viewport gets `contain: layout paint` as the belt to that braces |
| save the PNG at 1× | **refused.** A chart pasted into a document has to be sharp: 2× backing store, capped so a huge figure cannot allocate a 100-megapixel canvas, with the computed styles inlined so the file does not depend on this app's CSS variables |
| encode the PNG in Rust | **not possible here, and not needed.** There is no `image` / `resvg` dependency and the pixels only exist in the webview; the backend validates the payload (PNG magic, 12 MiB cap, whitelisted slug) and writes `<workspace>/figures/<slug>.png` — not `.data/`, because the file belongs to the user |

---

## PATCH v1.6 — a figure can move

v1.5 refused to execute model-authored JavaScript, and that refusal still stands
for one document: **this** document. A drawing that animates needs script, so
the escape is not a weaker parser but a stronger boundary — the fragment runs in
a frame with its own, opaque origin. "Raw HTML with extra steps" was the right
call while the only alternative was the transcript's own scope; a separate
origin is a different thing.

| Tempting | Here, and why |
|---|---|
| run the fragment inline, sanitised like an SVG | **refused.** A document-wide allowlist for arbitrary HTML + CSS is a filter, not a boundary: one missed `style` rule restyles the app, one missed attribute is a handler. The frame's origin is the boundary, and a filter cannot replace it |
| `sandbox="allow-scripts allow-same-origin"` so the parent can inject the document, read pixels, or resize cheaply | **refused, and it is the classic escape.** Those two flags together mean "same origin" — the frame could reach `parent.document`, the app's storage and its cookies, which is exactly what the sandbox exists to prevent. The frame stays opaque; communication is `postMessage` in both directions, and `event.source` is checked |
| give the frame a network-capable CSP so a fragment can load a CDN library | **refused.** `default-src 'none'` is what makes a saved file safe to open and a reply unable to exfiltrate. Charts draw with inline `<canvas>` / `<svg>`, not with a download |
| capture the frame to PNG for the save button and the PDF | **not possible.** A cross-origin frame cannot be drawn into a canvas, and an animation has no single frame worth keeping. The save writes the *document* (`<slug>.html`), tokens baked in, and the PDF keeps the frame |
| rewrite `srcdoc` on a theme switch | **refused.** A rewrite reloads the document and restarts the animation. The tokens are re-sent as one message and written to `:root` in place |
| measure `documentElement` for the height | **refused.** The root element fills the frame viewport, so it would report the height the card just set; the reporter measures `body`'s border box instead |
| mount the frame as soon as its source is complete | **refused.** Chromium reloads a frame on re-insertion and a commit replaces the subtree, so a frame in a still-streaming reply restarts on every token. It becomes *available* on the settle commit (the one nothing follows) and the card says so until then |
| start the animation on its own — the reader asked for a drawing, after all | **refused.** On WebKitGTK the frame shares the web process with the transcript: a 2 s `while` froze the parent's 50 ms timer for 2.041 s, so a `while (true) {}` would take the window down. The poster makes execution the reader's click, which needs no setting and no trust in the model — *revised by PATCH v1.7: script-less fragments auto-run; only script waits for the click* |
| let a watchdog stop a runaway fragment | **refused, it cannot work.** The watchdog runs on the thread the fragment blocks, so it fires only after the loop ends — and it never ends. A timer that fires 2 s late cannot interrupt anything; only *not starting* the script is a real boundary |
| read `default-src 'none'` as "a fragment cannot reach the network" | **half true, so stated as such.** It refuses every request the frame itself makes (fourteen channels measured, zero hits), but navigation is not a request in CSP's sense: `location = 'https://…?d=' + payload` leaves. The card counts frame loads and closes the figure on the second one |
| trust the serialiser to keep a fragment's `</style>` inside its element | **not reachable, and still guarded.** A literal closing tag ends the element during the first parse, and raw-text elements do not decode entities (`&lt;/style&gt;` stays text — checked), so no parsed text node can hold one. `defuseRawText` remains as the belt for the day this module writes text into a `<style>` itself |
| let a saved file drop the CSP because it is "just a local file" | **refused.** The file is the same fragment. It keeps the policy, so double-clicking it cannot phone home either |

PATCH v1.7 revises one of those rows — *start the animation on its own* — after
measuring both engines and splitting the question it answered too coarsely:

## PATCH v1.7 — auto-run what cannot hang the window

The v1.6 gate asked the same question of every dynamic figure, which made CSS
animation — the common case — cost a click it did not need. The measure that
decided it: a sandboxed frame shares the web process with the transcript on
WebKitGTK **and** on Chromium (a 2 s `while` in the frame left a 2 s hole in the
parent's 50 ms timer on both), so the dangerous thing is not "a dynamic figure"
but *script on that thread*. Markup, CSS and SVG animation cannot block it.

| Tempting | Here, and why |
|---|---|
| auto-run every figure, because the reader asked for a drawing | **refused, now for a precise reason.** A fragment that runs script shares the thread with the transcript in both engines; the gate stays for exactly that case |
| auto-run only the fragments that have no script (`htmlFigure.hasInlineScript` — `<script>` elements, `on…` attributes, `javascript:` URLs) | **this is the tier.** No script, no blocking: CSS animation starts on the settle commit, no click. A fragment that cannot be parsed counts as scripted and takes the gate |
| keep asking per figure, forever | **refused.** The poster's second button, `Play automatically this session`, records consent in `sessionStorage` (dies with the app, unreadable by the frame) and lifts every standing gate at that reply's next rebuild |
| keep consent in `localStorage` so it outlives the restart | **refused.** "May this app run model-authored script" is a per-session answer; `sessionStorage` asks it again every launch |
| run a script figure's code in a Web Worker to make autoplay safe | **not a transparent upgrade.** The fragment's script touches the DOM; a DOM proxy over `postMessage` is a library-sized shim the model cannot be relied on to write to. OffscreenCanvas covers only canvas figures. Revisit only as an explicit *flavour*, never as a silent rewrite |
| isolate the frame in its own process (multi-webview) | **unavailable.** Tauri's multi-webview is `unstable` and not supported on Linux, which is the engine in question; and the same thread is shared on Chromium too |
| reset a load counter only when the frame document starts | **wrong — the move also reloads.** A cached card is re-inserted on rebuild and the reload fires a load that is not the fragment navigating away, so a settled CSS figure got closed as "navigated". `beforeMount` zeroes the counter before the move, and only a card still in the `gate` state may auto-lift — a card the guard blocked stays blocked |

---

## PATCH v1.8 — the workspace indicator is the switcher

The titlebar already named the active workspace, and the only way to change it
was Settings → Workspace, behind a Save. Making the label itself the entry
point removes a round trip without adding a second place to *look* for the
workspace: the thing that names it is the thing that changes it.

| Tempting | Here, and why |
|---|---|
| swap the label for a field-shaped `Dropdown` trigger | **refused.** The trigger would become a 34px bordered control sitting next to the window title, and the one line that marks *which* workspace this window is would stop being a label. The indicator keeps its typography and hairline; only a hover / open fill distinguishes a control from a mark |
| let the list keep the "stage it, then Save" rule | **refused.** A shortcut that still needs a second confirmation is the thing it was meant to replace. Settings stays staged (it is a form) and the titlebar applies immediately — two affordances, two contracts, both labelled as such |
| apply the switch by re-sending the whole `AppConfig` from the renderer | **refused.** The popup holds no form, so it has no business writing fields it never showed; `set_workspace_dir` sets that one field on the live config |
| keep a workspace field in Settings once the titlebar owns it | **removed — two pickers for one field, and the staged one could revert a switch.** The panel is a form for everything else; `handleSave` / `handleLanguageChange` / `applyMessageEditor` take `workspace_dir` from a fresh `getConfig()` |
| switch mid-turn and let the streaming reply land wherever it lands | **refused at both layers.** A running turn resolves tool paths and writes rows through the workspace it started in; the rows disable while `turnStartedAt` is set and the command returns an error for a hand-crafted call |
| draw a thin scrollbar in a 10-row menu | **refused.** The bar is reserved for the transcript and the panel lists; a menu caps its own height and hides it |

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
| `src/App.css` | **All tokens**, app shell, toolbar, **titlebar / window chrome**, transcript, **floating chat notices**, input row, dialogs, markdown editor, scrollbars, shared primitives (**incl. the dropdown primitive**) |
| `src/components/ChatMessage.css` | Transcript turns, reasoning, code blocks, tables (`--ts-*` are aliases of `--c-*`) |
| `src/utils/figures.ts` | Figure extraction (`svg` / `carve` / `html-fig` fences, raw tags) — slot tokens handed to the transcript |
| `src/utils/figureCard.ts` | The `.chat-figure` card: sanitised SVG, three.js canvas, sandboxed frame, save action, per-source cache |
| `src/utils/htmlFigure.ts` | `<html-fig>`: the frame document (CSP, token `:root`, height reporter), its message protocol, and the standalone document a save writes |
| `src/utils/carve3d.ts` | `<carve>` scene parsing and the shared off-screen `WebGLRenderer` |
| `src/utils/figurePng.ts` | SVG / canvas → base64 PNG (computed styles inlined, 2× backing store) |
| `src/utils/svgSanitize.ts` | Element allowlist and attribute blocklist for model-authored SVG |
| `src/components/ToolCallGroup.css` | Tool-call cards and streaming states |
| `src/components/TodoList.css` | Todo rows |
| `src/components/FileMentionPopup.css` | Composer mention popup — one surface shared by the `@file`, `/skill` and `+agent` pickers; `SlashMentionPopup.css` / `AgentMentionPopup.css` only add the glyph colour and the left-to-right description |
| `src/components/{Skills,Mcp,Tools,Agents,History}Panel.css` | Panel-specific layout only — chrome comes from the shared primitives |
| `src/components/SettingsPanel.css` | Settings page (`--t-*` are aliases of `--c-*`) |
| `src/components/Dropdown.tsx` | The one select / listbox (trigger, portal, keyboard contract) — the only dropdown in the app |
| `src/components/ModelSelect.css` | Model picker: the trigger's width cap and the capability badges, on the shared dropdown primitive |
| `src/components/TitleBar.tsx` | Custom window chrome: drag region, window controls, `⋯` app menu (styles in `App.css`) |
| `src/components/{Update,Monitor,AgentMission}Panel.css` | Feature surfaces, self-contained — `MonitorPanel.css` also owns the two log panes and the session-report strip |

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
