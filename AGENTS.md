# Rabbit Chat Studio — Tauri 2.0 (Rust + React)

Desktop AI chat client (Tauri 2 + React 19 + TS + Vite, Rust backend) that talks to any OpenAI-compatible API and adds **Skills**, **Tools**, and **Sub-Agents**.

**This file is the index.** The reference material lives in five files under
`docs/agents/`; open the matching one before the work it governs, not after.
Everything in them is normative — the split is by topic, not by importance.

## Docs map

| Read this | Before you… | It holds |
|---|---|---|
| [`docs/agents/interface-design.md`](docs/agents/interface-design.md) | change anything the user draws | the mandatory `DESIGN.md` contract and its non-negotiables: tokens only, one accent, no elevation, 4px radius, one motion curve, hairlines, the shared primitives, the 34px compact zone, sentence case, and the theme × palette × glass check matrix |
| [`docs/agents/tech-stack.md`](docs/agents/tech-stack.md) | build, check or test anything | dev / build / verify / test commands, per-crate `cargo test -p …`, version sync, and the Cargo mirror override for restricted networks |
| [`docs/agents/architecture.md`](docs/agents/architecture.md) | need the shape of the system | the frontend ↔ Rust split and the message path, the plugin bus (LLM → tool → bus → instance × N), services and the manager window, every runtime file (`%APPDATA%`, `.data`, sqlite), the sub-agent prompt files (`llm/`), and the Tauri 2 specifics of this repo |
| [`docs/agents/modules.md`](docs/agents/modules.md) | touch a feature and need its files | `src/` and `src-tauri/` file by file, the skills path-isolation rule, the tool family and dangerous commands, turn guards / parallel calls / turn identity, composer attachments |
| [`docs/agents/workflow.md`](docs/agents/workflow.md) | ship, publish or debug a trap | the release pipeline and its git-tag rule, Microsoft Store (MSIX), the GitHub Pages site (including `docs/site/assets/site.css`, which mirrors the tokens), and the gotcha list |
| [`DESIGN.md`](DESIGN.md) | make any visual decision | colours, typography, spacing, components, motion, both themes — the single source of truth |

`docs/agents/` is developer documentation; only `docs/site/` is published to
Pages (`.github/workflows/pages.yml`).

## Always true

These apply to every change, so they stay here instead of hiding in a file you
might not open.

- **The interface follows [`DESIGN.md`](DESIGN.md).** Tokens only — a literal hex,
  or a literal `blur(20px)` / `saturate(180%)`, outside the token blocks in
  `src/App.css` is a bug; when a value has no token, add the token, do not
  hardcode it at the call site. One accent colour, no elevation, 4px radii, one
  motion curve. Check light **and** dark, `data-palette`
  (`default`/`cream`/`macaron`) and `data-glass` (`off`/`subtle`/`standard`/`strong`),
  plus the Windows vibrancy frame. Details:
  [`docs/agents/interface-design.md`](docs/agents/interface-design.md).
- **Verify with `npx tsc --noEmit && npx vite build`.** Never `npm run build`
  for verification: it runs `sync-version:git-tag` first and aborts without a
  git tag. When Rust moved, add
  `cargo check --manifest-path src-tauri/Cargo.toml`, and use
  `cargo test --manifest-path src-tauri/Cargo.toml -p <crate>` for the
  standalone crates ([`tech-stack.md`](docs/agents/tech-stack.md)).
- **A skill never leaves its own directory.** Every path a skill touches is
  relative to the folder holding its `SKILL.md`, unless the user named an
  absolute path. Do not add code that lets a skill escape that root.
- **`CHANGELOG.md` is generated, not written.** `scripts/gen-changelog.cjs`
  renders the commits between two tags; put the information in the commit
  message (English subject, Chinese body) and never hand-edit the file.

## Keeping the split honest

- New material goes into the file that owns the topic, then gets **one line in
  the table above** — the map is the only list of files, so it cannot drift out
  of sync.
- Keep this index an index. It answers "where do I read?" and "what applies
  everywhere"; it is not the place for a new section that belongs in a
  `docs/agents/` file.
- `docs/wiki/{en,zh}/development-and-architecture.md` shows the repo tree to
  users — add a line there when the top level of the repository changes.
