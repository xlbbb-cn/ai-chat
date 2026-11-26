# Workflow — releases, website and common gotchas

> Part of the Rabbit Chat Studio developer docs. Read this when you are shipping, publishing or stepping into a known trap.
> The index for all of them is [`AGENTS.md`](../../AGENTS.md) — it also holds
> the rules that apply to every change, whatever file you ended up in.

## Releases (CI)

`.github/workflows/release.yml` (trigger: `v*` tag push, or manual `workflow_dispatch`) builds via
`tauri-apps/tauri-action@v1` on 4 runners — `macos-latest` ×2 (`aarch64-apple-darwin`, `x86_64-apple-darwin`),
`ubuntu-22.04`, `windows-latest` — and uploads `.dmg` / `.AppImage` + `.deb` + `.rpm` / NSIS `.exe`
(Windows ships the NSIS bundle only, no `.msi`)
to a **draft** GitHub Release for the tag (also kept as workflow artifacts), published by hand
once every platform finished.

- **The tag drives the version.** `tauri build` runs `beforeBuildCommand` → `npm run build` → `sync-version:git-tag`, which reads the newest tag; a workflow step fails fast when the pushed tag is not the newest one.
- **Version numbering: the patch number is the default.** Unless the request explicitly names a larger version, every release tag bumps only the patch digit (`3.5.2` → `3.5.3`) via `node scripts/sync-version.cjs --bump patch` — a feature or a fix does not by itself justify a minor bump. `--bump minor` (`x.Y.0`) or `--bump major` (`X.0.0`) happen only when the user asks for that version level; do not raise the version on your own judgement.
- CI runs `rm -f .cargo/config.toml` so the committed rsproxy.cn mirror is not used on GitHub runners.
- **CI toolchains mirror the local machine**: every workflow pins `node-version: '24.20.0'` and `dtolnay/rust-toolchain@1.99.0` — exactly what `node -v` / `rustc -V` report here — instead of `lts/*` and `@stable`, so a CI-only failure is reproducible locally; bump both together with the local toolchain. Keep every `uses:` on its newest major (`checkout@v7`, `setup-node@v7`, `upload-artifact@v7`, `configure-pages@v6`, `upload-pages-artifact@v5`, `deploy-pages@v5`, `microsoft-store-apppublisher@v1.4`, `winget-releaser@v2`) so no action still runs on a deprecated Node runtime — that deprecation warning in the log is what "Node too old" actually means.
- macOS uses `APPLE_SIGNING_IDENTITY='-'` (ad-hoc) — replace with real signing/notarization secrets when available.
- `.github/workflows/winget.yml` is **inactive for now**: the package is not in `microsoft/winget-pkgs` (the earlier PR was closed), so nothing can be submitted and winget / Homebrew / Scoop must not be documented as install options. Keep the file — once a package entry is accepted it needs the `WINGET_TOKEN` secret (classic PAT, `public_repo` + `workflow`) plus a fork under `xlbbb-cn`, and it waits for the NSIS `-setup.exe` upload first.
- `releaseDraft: true` keeps the Release a **draft** through the whole run: the assets of all four platforms accumulate while it stays invisible (also to `releases/latest/download/…` and the update check), then a maintainer publishes it on the Releases page — a failed platform can no longer leave a partial release public, and no publish job runs automatically.

### Microsoft Store (MSIX)

`scripts/build-msix.ps1` (`npm run msix`) packs the release exe into an unsigned `.msix` with the Windows
SDK (`makeappx`); the identity lives in `src-tauri/msix/store.config.json` (Partner Center product
identity) and the manifest in `Package.appxmanifest.template`, so a name/publisher change is one JSON
edit. Tauri has no MSIX target, hence our own step; the Store re-signs what you upload, so submission
uses the unsigned file. The `windows-x86_64` CI job runs the same script with `-SkipBuild` after
`tauri-action` and keeps the `.msix` as an Actions artifact (never a Release asset — end users cannot
install an unsigned package). Runbook, submission fields and policy mapping: `docs/store/store-publishing.md`.

- **Live listing**: <https://apps.microsoft.com/detail/9MV82J9G95GS> (Store ID `9MV82J9G95GS`,
  protocol link `ms-windows-store://pdp/?productid=9MV82J9G95GS`). Because the store build is an MSIX
  package, its AppData lives under `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\…` and does
  **not** share config with the NSIS installer build (the workspace `.data` stays shared). New versions
  are submitted by hand (`docs/store/store-publishing.md` §7) or with `msstore publish` from
  `.github/workflows/store.yml` (§12) — that needs four Partner Center secrets, a Manager-role Entra app
  and one earlier manual submission; the workflow skips itself when they are missing.

- MSIX runs the app as a full-trust Desktop Bridge package (`runFullTrust`,
  `EntryPoint="Windows.FullTrustApplication"`): filesystem and process access are unchanged, but writes
  under AppData land in `%LOCALAPPDATA%\Packages\<PFN>\LocalCache\…`, and child processes started by the
  app see the same redirected view — CLI agents therefore do not share the login state of the user's own
  terminal. Do not declare a WebView2 framework package dependency (Store installs with it fail); rely on
  the machine's Evergreen runtime and keep `MinVersion` at Windows 10 2004.
- `update::is_store_packaged()` keeps the GitHub updater out of Store builds: when the exe runs from
  `…\WindowsApps\…`, `check_update` answers "already latest" instead of pointing users at the NSIS
  installer for a second copy.

## Website (GitHub Pages)

`docs/site/` is a dependency-free static landing page, deployed by `.github/workflows/pages.yml`
(`push` to `main` touching `docs/site/**`, `docs/wiki/**`, `docs/gen.py` or
`scripts/gen-wiki.mjs`, a weekly `schedule:` cron, or `workflow_dispatch` — the
cron rebuilds `model_metadata.json`, see the Model metadata bullet).
Live URL: `https://xlbbb-cn.github.io/ai-chat/` — the repository needs Pages set to
**Source: GitHub Actions** once (Settings → Pages). Only this one folder is published; the rest of
`docs/` (`gen.py`, `store/…`, `banner.png`) stays repository-only.

- **No build step, relative paths only.** The page is served from a project sub-path, so every asset
  reference is `./assets/…`. The only absolute links are in `404.html`, because GitHub serves it for
  unknown paths at any depth and relative assets would break there.
- **`docs/DESIGN.md` still applies.** Tokens are copied into `docs/site/assets/site.css` (light + dark). The
  accent is the primary CTA only, radius is 4px, there are no shadows, and hairlines do the
  separating. Change a token in `src/App.css`? Mirror it in the site stylesheet.
- **Copy comes from the READMEs.** Feature and boundary claims are `README.zh.md` / `README.md`
  content, not new marketing. Download buttons use the stable Release asset names
  (`releases/latest/download/…`), so a new tag needs no site edit — keep them in sync with
  `stableAsset` in `release.yml` if a filename ever changes.
- **i18n:** Chinese is the markup default (the page renders with JS off); English lives in the `EN`
  dictionary in `docs/site/assets/site.js` and is applied through `data-i18n` / `data-i18n-html`.
  A new string needs both halves: the zh text in `docs/site/index.html` and the `EN` entry.
  Use `data-i18n-html` whenever the string contains markup (inline `<code>`), otherwise the text
  swap drops the element.
- **The architecture page is generated, never committed into the site.** `python3 docs/gen.py`
  writes `docs/site/architecture.html` directly (build output, gitignored), and the Pages workflow
  runs it as a step before `upload-pages-artifact`, so the generator is the single source and no
  stale copy can be deployed.

- **Two pages, one policy text.** `docs/site/privacy-policy.html` (bilingual, no JS, tokens only) is the
  public URL for the Privacy policy required by the Store; its source of truth is
  `docs/store/privacy-policy.md`. Edit both together.

- **The docs live in the repo; the site renders them.** `docs/wiki/nav.json` is the page manifest
  (19 pages, groups, en/zh titles, original Wiki page names as link aliases) and the prose sits in
  `docs/wiki/en/<slug>.md` + `docs/wiki/zh/<slug>.md`. `node scripts/gen-wiki.mjs` renders them to
  `docs/site/wiki/**.html` — zh at the root, en under `/en/` — rewriting Wiki-style `(Page-Name)`
  links to site slugs and generating the sidebar, page TOC, pager and language switch. It fails
  loudly on a missing source file, a dead link or a dead anchor, and `pages.yml` runs it before
  `upload-pages-artifact`; the output is build product and gitignored (`docs/site/wiki/`), so never
  edit it by hand.
- **The GitHub Wiki is a forwarding shell, not a source.** Its `Home.md` / `简体中文.md` are landing
  pages pointing at the site and every other page is a stub carrying the matching site URL, so old
  links keep working and nobody reads a stale copy. Pushing to
  `git@github.com:xlbbb-cn/ai-chat.wiki.git` still works (HTTPS to github.com is reset in some
  environments, SSH works); do it only to refresh those pointers. Before the site existed the Wiki
  carried the prose and had 18 Chinese pages — those were superseded by `docs/wiki/zh/`, which is the
  maintained translation.

Preview locally (any static server; the folder has no build step):

```powershell
python docs/gen.py                 # writes docs/site/architecture.html
node scripts/gen-wiki.mjs          # writes docs/site/wiki/**.html from docs/wiki/
node .data/tmp/serve.cjs 8765 .   # then open http://127.0.0.1:8765/docs/site/
```

Check the page in both themes, both languages and at ≈390px width before pushing.

## Common gotchas

- **A `file` content part costs the whole turn — never send one.** `400 invalid_request_error … .messages[2]: file content parts are not supported here; send the image as an image_url part` fails the entire request from one `{"type":"file"}` part, and because the part is persisted with its message, every later turn of that session fails the same way. The app has exactly three sendable forms — text (inlined), `image_url`, `input_audio` — and `file` is not one of them, whatever the catalogue says: the `deepseek-v4.1-flash` entry advertised `supports_files` while the gateway in front of it refused every one, so a capability flag is not a licence. Four places enforce it: (1) `classifyName` (`src/attachments.ts`) treats an extensionless name (`scripts/user_admin`), a dotfile and an unknown extension as text, `looksLikeBinaryText` refuses the payload when that guess was wrong, and `supportsAttachmentKind` answers `false` for `file` so the `@` popup never offers a binary; (2) `partsForContent` (`src/messageParts.ts`) decides the part type from the **payload** — a `data:image/…` URL is always `image_url`, whatever `Attachment.kind` says — and names an unsendable binary in a text stub; (3) `llm_complete::stream::normalize_content_parts` runs at the streaming choke point (main agent *and* sub-agents) and rewrites any `file` part that still arrives: image payload → `image_url`, base64 that decodes to UTF-8 → the text itself (this is what repaired `@scripts/user_admin` rows saved as `application/octet-stream`), anything else → a stub naming the file; (4) the failed-request log line carries `parts=m1=image_url,m2=file`, so the provider `.messages[N]` can be matched to a message without guessing. History is replayed verbatim on every turn, so this rewrite happens on the request body and never on a stored message — that keeps the prompt prefix cacheable.
- **Production build fails on missing git tag.** `npm run build` → `sync-version:git-tag` calls `git tag --sort=-creatordate`; no tag = hard error. Tag first or use `--version x.y.z`.
- **Tauri 2 permissions are runtime, not compile-time.** A new `fs`/`shell`/`http` call will fail at runtime if the permission is missing from `capabilities/default.json`.
- **`async` commands returning `Result<T, E>`** — `E` must be `Serialize`. Use `String` for ad-hoc errors.
- **Vite `strictPort: 1420`** — don't change the port; Tauri expects it.
- **An `import.meta.glob` over `node_modules` breaks the build when the root is spelled with a different drive-letter case.** `utils/printDocument.ts` used to collect the KaTeX faces with `import.meta.glob("/node_modules/katex/dist/fonts/*.woff2", …)`. An *absolute* pattern makes the build compute a relative specifier from the importer to every matched file, and the two sides disagree: the importer id keeps `E:/ai-chat/…`, the glob returns `e:/ai-chat/…` when Vite's root came in lowercase, no common prefix is found, and the emitted import becomes `../../../../e:/ai-chat/node_modules/…` — rollup then fails with `Could not resolve` (`vite build e:/ai-chat` reproduces it; the uppercase spelling passes, which is why it can hide for months). The fonts are now imported one by one (`katex/dist/fonts/X.woff2?url`), which goes through node resolution and does no path arithmetic; keep it that way for anything the PDF export has to carry, and never reintroduce a glob over `node_modules`.
- **Workspace dir** is mutable at runtime via `get_workspace_dir` / config; skills and the in-app browser operate on it. Hot-swap carefully — open handles may be invalidated.
- **A vanished workspace falls back, it is never rebuilt.** `workspace::resolve_startup_workspace` (startup, `lib.rs` setup) opens `config.workspace_dir` only when it still exists as a writable directory; otherwise the window opens the default workspace (`<app_data>/workspace`, or the shadow's fork) and logs the reason to `app.log` as `Workspace fallback: …`. The configuration keeps the configured path, so a drive that is plugged back in is used again on the next start — do not "fix" this by writing the fallback path into `config.workspace_dir`, and remember a fallback start does not `remember_workspace` the default path. The strict check (`check_startup_workspace`, no creation) is deliberate: a missing folder means the history is missing too, and silently recreating an empty workspace hides that. An *explicit* choice is different — `set_workspace_dir` validates with `prepare_workspace` (creates if needed) and returns the reason as an error, because `apply_config` writes the config before it opens the database and would otherwise persist a workspace it never opened.
- **Tauri codegen**: `src-tauri/gen/` is auto-generated (gitignored). If IPC types look stale, run `npm run tauri dev` once to regenerate.
- **Two windows should not share a workspace.** Every database is `<workspace>/.data/chat.db`; `db::open_db` waits up to 5 s for another connection's write lock (`busy_timeout`) and startup still `.expect`s the open, so two live windows on one directory survive the lock but show interleaved sessions, each with its own in-memory state. That is why a shadow window forks its own `workspace/`; only the user can still point two windows at the same directory on purpose.
- **A shadow window's settings are temporary.** They live in `shadow/config-<id>/config.json` and are deleted on exit; only `workspace_history` is merged back (and its own fork path is filtered out). Do not "fix" that by letting a shadow write the shared config — moving another window's workspace is exactly what the roles prevent.
- **`timers.json` is workspace-scoped, not app-data.** A timer resumes a session id that only exists in its workspace's database, so the mirror lives in `<workspace>/.data` and moves with `apply_config`; a legacy `<app_data>/timers.json` is migrated once at startup (`timers.json.migrated` when the target already exists).
- **Draft releases are invisible to the update check.** `GET /releases` never returns drafts, so an in-app “update available” only shows up after the draft created by CI is published.
- **A figure never reaches the DOM through markdown.** `html: false` is the injection boundary, so `svg` / `carve` / `html-fig` blocks are cut out of the reply *before* `md.render` (`utils/figures.ts`) and replaced by an invisible U+2063 slot token; `mountFigures` swaps the token for a sanitised card in the `useLayoutEffect` right after `innerHTML`. Removing the sanitizer or enabling raw HTML undoes the only defence a reply has.
- **Figure cards are cached by source hash, not rebuilt.** The transcript subtree is replaced on every streamed token; a card that were rebuilt would drop three.js pixels and the orbit angle the user just dragged. The cache *moves* the same element back into the new tree — extend `mountFigures` rather than mounting in React.
- **A dynamic figure runs in a frame, never in the transcript.** `utils/htmlFigure.ts` assembles an `html-fig` fragment into a complete document and hands it to `<iframe sandbox="allow-scripts">` — `allow-same-origin` must never be added, because the pair together is same-origin access to the app. The wrapper carries `default-src 'none'` (inline style/script open, `img-src data: blob:`), which refuses every request the fragment makes — fourteen channels measured on WebKitGTK, zero hits. Navigation is *not* covered by CSP (`navigate-to` never shipped), so `figureCard.ts` counts the frame's loads and closes a figure on the second one; `parent.document` and `localStorage` throw in the frame, and the two channels across the boundary are `postMessage` height reports and token updates, both checked against `event.source` on the parent side.
- **Execution is tiered by script, and consent is per session.** A sandboxed frame shares the web process with the transcript in both engines (measured: a 2 s `while` left a ~2 s hole in the parent's 50 ms timer on WebKitGTK *and* Chromium), so `htmlFigure.hasInlineScript` decides: no script — CSS/SVG animation — runs on the settle commit without a click; script waits for a click on the poster, or for the `sessionStorage` consent its `Play automatically this session` button records. Do not auto-run script without that record, do not move consent to `localStorage`, and do not add a watchdog that "stops" a runaway fragment — it would run on the frozen thread. Mid-stream even a safe frame waits: a re-inserted frame reloads, so it mounts on the settle commit, and a cached card's load counter resets in `beforeMount` because that move-reload is not a navigation.
- **A frame is mounted on the settle commit, not while streaming.** Chromium reloads a frame whenever the element is re-inserted, and every commit replaces the transcript subtree: a frame mounted mid-stream restarts on each token. `mountFigures` takes `streaming` and `buildHtmlCard` shows `Plays when the reply finishes` until it is false; `ChatMessage` forces one extra rebuild when the flag flips, because the slots a card needs only exist in a freshly written tree. Do not "optimise" that rebuild away.
- **A dynamic figure is saved as `.html`, not PNG.** The pixels live in a cross-origin frame that cannot be captured, and an animation has no single frame; `figures::save_figure_html` validates (`<!doctype`, 2 MiB cap, whitelisted slug) and writes the standalone document to `<workspace>/figures/<slug>.html` with the tokens baked in. The PDF export keeps the live frame and pins its measured height onto the clone.
- **The PDF engine is a launcher that exits at once.** `msedge.exe --headless=new --print-to-pdf` returns in ~50 ms with status 0 while the real browser writes the file a second later, so waiting for the child yields an empty export. `html_pdf::run` polls the target instead, and only accepts it once it ends with `%%EOF` (45 s cap) — that is also why the temp document and profile are deleted after the poll, never before. The `--user-data-dir` must be a fresh directory per run: pointed at the default profile, a running browser accepts the command line, prints nothing and exits 0.
- **Three.js is a lazy chunk with exactly one renderer.** `utils/carve3d.ts` dynamic-imports `three` and owns the single off-screen `WebGLRenderer` that blits into each figure's 2D canvas; a renderer per card hits WebView2's context limit after a few scenes. A conversation without `<carve>` never loads the chunk (it builds as its own ~750 kB asset). Keep at most eight recently used scene runtimes, keyed by parsed-scene identity and palette: orbiting schedules one rAF and changes only the camera, while a theme change rebuilds that scene. Geometry/material/texture creation must not move back into the frame path. A lost context recreates the shared renderer on the next draw and reuses the CPU-side scenes.
- **A connected figure card is never cache-evicted.** The 48-card bound in `figureCard.ts` applies only to detached cards; disposing a connected handle breaks a live iframe's listeners and loses its 3D resources. `ChatMessage` releases its message's handles on unmount through a microtask-token protocol (`retainMessageFigures` / `deferMessageFigureDisposal`) so StrictMode's effect replay cancels the release but a real unmount still disposes listeners, theme callbacks and Three.js resources. Keep `disposeCarve` paired with every carved card's final disposal, including an async draw that finishes after disposal.
- **Rust never encodes a figure PNG.** The pixels only exist in the webview, and there is no `image`/`resvg` dependency: the frontend rasterises and sends base64, `figures::save_figure` validates (PNG magic, 12 MiB cap, whitelisted slug) and writes `<workspace>/figures/<slug>.png` — never `<workspace>/.data`.
- **The drawing toggle changes the prompt, not the renderer.** `enhanced_figures` (Settings → Advanced, on by default) picks between `figures::prompt_block()` (`svg` / `carve` / `html-fig`) and `figures::mermaid_prompt_block()`; the transcript renders all four kinds either way, so flipping it never orphans an earlier reply.
- **A Mermaid diagram is a figure card with a second renderer.** `utils/mermaidDiagrams.ts` cuts `mermaid` fences out with the same U+2063 slot token and renders them asynchronously with `securityLevel: "strict"` + `htmlLabels: false`, pruning the SVG again (`script` / `foreignObject` / `on*` / external `url()`); Mermaid's id-scoped `<style>` survives, which is why the SVG is not run through `svgSanitize.ts`. Colours are baked into the SVG, so a theme or palette change re-renders instead of repainting, and a graph wider than ~2.5× the card keeps its natural width and scrolls.
- **Both extractors read fences through `utils/fenceLines.ts`; never re-declare a fence regex locally.** Two shapes break a flat `^ {0,3}` scan and turn a diagram into a code block: a reply with CRLF line endings (`\r` is not part of a closing fence, so every block looked unclosed) and a fence indented by the list item that holds it (markdown-it reads a fence there, a flat scan read an indented code block). `splitLines` normalises, `fenceIsOpen` honours a list container up to 8 spaces, the body is dedented before the renderer sees it, and the slot token keeps the fence's column capped at three spaces so it never lands inside `<pre><code>`. A fence that really is code — a `mermaid` example inside a ```` ```markdown ```` block, or a 4-space indented block outside any list — still stays code.
- **A card that cannot be drawn states it; it does not dump its source.** `figureCard.showUnrenderable` (shared with the Mermaid card) shows one line, a copy button and the markup behind a fold, and every failure path logs `console.warn("[figures] <kind> not drawn (<reason>)")` first — a diagram silently turning into a code block is the bug this replaces. The fold keeps the text because it is the only way to see what went wrong.
- **An empty completion is a failed attempt, never an answer.** Some relays close an aborted generation politely: a usage chunk, then `data: [DONE]`, with no `finish_reason`, no content and no tool call (observed at 55k–100k prompts right after a mission report was injected, so it looked like "the turn exits when the main agent summarises"). `[DONE]` satisfies the completeness check in `stream_llm_request`, so it used to return `Ok` with an empty body; the turn then took `final_answer = Some("")` and closed the transcript on the previous partial text. `stream::is_empty_completion` now rejects it (retry while attempts remain, then `chat-error`), and `run_chat_turn` refuses an empty final answer. Neither check may be relaxed — the empty reply must stay a retryable failure.
- **A tool-call round carries its narration back into the history.** `stream::assistant_tool_message` writes the round's `content` next to the calls, plus its `reasoning_content` (thinking-mode APIs require it); a silent round keeps `content: null` — an empty string would be a fake message. `turn.rs` used to hard-code `null`, so the model could not see what it had just told the user: it re-read the same files and repeated itself while the streamed text diverged from the history it was reasoning over. The same round reaches the UI, the request history and the compaction summary from this one value. `summarize_message_for_context` still skips tool-call rounds on purpose — a compact summary keeps decisions, not narration.
- **A marker-only answer is an empty answer.** `[goal:DONE]` and `[WAIT_EVT: …]` are signals, not text, and the frontend strips them from the stream. A reply that carried nothing else passed the `sr.content.trim().is_empty()` check (it ran *before* stripping), closed the turn and left a blank bubble — the model declaring itself finished looked exactly like "the LLM did not answer", and it happened right after a Stop left the plan unfinished and the goal loop nudged it. `run_chat_turn` now strips first and treats an empty result as the same retryable failure; the `wrap_up` fallback answer is stripped too.

- **External links open in the system browser, never in-app.** Use `openUrl()` from `@tauri-apps/plugin-opener`: `capabilities/default.json` already carries `opener:default`, whose permission set covers `allow-open-url` plus `allow-default-urls` (`mailto:`/`tel:`/`http:`/`https:`), so wiring a new link needs no Rust change. Never point a bare `<a href>` at an external page from the UI - the WebView navigates the app window itself instead of handing off to the browser. Settings > About is the reference pattern: `EXTERNAL_LINKS` plus `openExternalUrl` in `SettingsPanel.tsx`.
