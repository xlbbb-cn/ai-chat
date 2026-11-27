# Interface design — the docs/DESIGN.md contract

> Part of the Rabbit Chat Studio developer docs. Read this when you change
> anything the user can see.
> The index for all of them is [`AGENTS.md`](../../AGENTS.md) — it also holds
> the rules that apply to every change, whatever file you ended up in.

## UI design — mandatory

**Any change that affects the interface MUST follow [`docs/DESIGN.md`](../../docs/DESIGN.md).** It is the single source of truth for colours, typography, spacing, components, motion and both themes. Do not invent visual language and do not copy a style from another project — if a value is not in the spec, add it to the spec first.

Non-negotiables, repeated here so they are impossible to miss:

- **Tokens only.** Colours, radii, shadows, durations, easings and glass blur radii come from `src/App.css` (`:root` + `[data-theme="dark"]` + the palette blocks + the `[data-glass]` blocks). A literal hex — or a literal `blur(20px)` / `saturate(180%)` — outside those blocks is a bug.
- **One accent colour.** Electric Blue (`--c-accent` = `#3E6AE1`) marks the primary action of a view — never decoration, never a second chromatic accent. Semantic `--c-danger` / `--c-success` / `--c-warning` are for status only. An accent used as a *label or line* is `--c-accent-text`; a label on an accent *fill* is `--c-on-accent`.
- **No elevation.** `--shadow-*` are `none`; layering is z-index, opacity and the frosted `--glass-backdrop` surface. Never write `box-shadow: <literal>, var(--shadow-sm)` — the mixed declaration is invalid and is dropped silently.
- **4px radius** on interactive elements. No pills, no large radii. Toggles/badges square off at 4px too.
- **One motion curve:** `--t-dur` (`0.33s`) + `--t-ease`. Colour/border transitions only — no `scale`/`translate` on hover.
- **Hairlines separate.** 1px `--c-border` for structure, 1px `--c-border-i` for interactive outlines. No gradient surfaces.
- **Reuse the shared primitives.** `.close-btn`, `.btn-primary`, `.btn-secondary`, `.inline-edit-btn`, `.field-title-row` are defined once in `src/App.css` — never re-declare them in a component stylesheet (that is exactly how the Skills/MCP/Tools panels drifted apart).
- **Compact zone = 34px controls** (chat input row, sidebar panels, Settings). Settings also uses 13px text. Do not introduce 40px inputs.
- **Sentence case, weights 400/500 only, `letter-spacing: normal`.** No `text-transform: uppercase`, no negative tracking.
- **Check light *and* dark** (`data-theme` is driven by `config.theme` via `applyTheme()` in `src/App.tsx`), plus `prefers-reduced-motion` for any animation. The colour scheme is a second, independent dimension: `data-palette` (`default` / `cream` / `macaron`) comes from `config.palette`, and every palette ships a light *and* a dark token set — check all six combinations of a change. An old config naming the removed `sakura` palette is normalized to `default` on load. Frosted glass is a third: `data-glass` (`off` / `subtle` / `standard` / `strong`) comes from `config.glass_effect` via `applyGlassEffect()`, and it changes the bar fill as well as the blur, so an `off`-level check is part of any surface change. On Windows the level also drives the *window* backdrop (`set_window_vibrancy` → DWM Acrylic, then `<html data-vibrancy>` + `--c-window`): the frame turns translucent over the desktop while the transcript keeps its opaque `--c-bg`, so a surface change has to be checked with the frame glassy too — and with `data-glass-fallback` set (`applyWindowVibrancy()` writes it whenever that native call reports nothing), which is what Linux and macOS look like: the same translucent frame, over a wash the app paints itself instead of a desktop (docs/DESIGN.md, *Window-less backdrop*).

When a needed value has no token, **add a token** to `src/App.css` — do not hardcode it at the call site.

Verify style work with `npx tsc --noEmit && npx vite build` (not `npm run build`, which requires a git tag).
