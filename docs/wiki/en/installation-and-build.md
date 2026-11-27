This page has two parts: installing/upgrading for end users, and running, checking and packaging from source for developers. If you only want to use the app, read "End-user installation".

## End-user installation

### Windows

Two options:

1. **Microsoft Store (recommended)**: open <https://apps.microsoft.com/detail/9MV82J9G95GS> and click **Get**. The Store installs it and keeps it updated, no administrator rights needed.
2. **NSIS installer**: open [Latest release](https://github.com/xlbbb-cn/ai-chat/releases/latest), download `Rabbit-Chat-Studio-Windows-x64-setup.exe` and follow the wizard.

> The Store build is an MSIX package: its app config is redirected to `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\...`, not shared with the NSIS build; the `.data` folder inside your workspace is shared. Releases ship the NSIS `-setup.exe` only — no `.msi` (a local `npm run tauri build` still produces one, for your own use). The Store build also skips the in-app GitHub update check: updates arrive through the Store.

Package managers (not available yet): the winget, Homebrew and Scoop manifests are not published, so `winget install` / `brew install --cask` / `scoop install` cannot find the package — use the two options above.

### macOS

- Apple Silicon: `Rabbit-Chat-Studio-macOS-Apple-Silicon.dmg`
- Intel: `Rabbit-Chat-Studio-macOS-Intel.dmg`

Open the `.dmg` and drag the app into **Applications**. Builds are ad-hoc signed, so the first launch requires right-click → **Open**, or **System Settings → Privacy & Security → Open Anyway**.

### Linux

- Universal: `Rabbit-Chat-Studio-Linux-x86_64.AppImage`

  ```bash
  chmod +x Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ./Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ```

- Debian/Ubuntu: `.deb`, `sudo apt install ./Rabbit-Chat-Studio-*.deb`
- Fedora/RHEL: `.rpm`, `sudo rpm -i Rabbit-Chat-Studio-*.rpm`

### Upgrading

Installed builds check the latest **published** GitHub Release on startup and show a banner when a newer version exists. You can also check manually under **Settings → About & Updates**. See [Updates](Updates).

## Run from source

### Requirements

| Dependency | Version / notes |
| --- | --- |
| Node.js | 20.19+ or 22.12+ |
| Rust | stable, preferably installed via rustup |
| Tauri CLI | 2.x |
| Python 3 | only for `docs/gen.py` (the architecture page) |
| Windows | MSVC toolchain (Visual Studio Build Tools with Desktop development with C++) |
| macOS | Xcode Command Line Tools |
| Linux | Tauri 2 WebKitGTK dependencies, see below |

On Debian/Ubuntu:

```bash
sudo apt update
sudo apt install -y build-essential curl wget file libssl-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
```

### Install dependencies

```bash
npm install
cargo install tauri-cli --version "^2"
```

### Development mode

```bash
npm run tauri dev
```

This starts Vite on fixed port `1420` (`strictPort: true`), compiles the Rust side and launches Tauri. Closing the app also stops Vite.

Frontend only:

```bash
npm run dev
```

Then open `http://localhost:1420`. In a plain browser there is no Tauri IPC, so file dialogs, window controls, MCP and similar features do not work.

### Checks and tests

```bash
# Frontend type check
npx tsc --noEmit

# Frontend production build (does not require a git tag)
npx vite build

# Rust check
cargo check --manifest-path src-tauri/Cargo.toml

# Rust unit tests (tools module: dispatcher, todos, timer, …)
cargo test --manifest-path src-tauri/Cargo.toml
```

When changing UI, check both light and dark themes and `prefers-reduced-motion`. The visual spec is `docs/DESIGN.md`: colors, radii, shadows and motion must use the tokens in `src/App.css`.

### Documentation and the website

The published site in `docs/site/` is plain static files; two generated pages are built from sources in the repository and are **not** committed (see `.gitignore`):

```bash
# Architecture page → docs/site/architecture.html
python3 docs/gen.py

# Wiki pages → docs/site/wiki/** from docs/wiki/{zh,en}/*.md
node scripts/gen-wiki.mjs
```

`gen-wiki.mjs` needs `markdown-it` (a regular dependency, so `npm install` is enough) and fails the build on a broken internal link.

### China / restricted networks: Cargo mirror

If Cargo cannot fetch dependencies, override the source:

```bash
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
```

The checked-in `.cargo/config.toml` may point at the USTC mirror; CI deletes it because GitHub runners cannot reach it.

## Production build

```bash
npm run tauri build
```

Artifacts:

```text
src-tauri/target/release/bundle/
```

| Platform | Bundles |
| --- | --- |
| Windows | `.msi`, NSIS `-setup.exe` |
| macOS | `.app`, `.dmg` (current architecture) |
| Linux | `.AppImage`, `.deb`, `.rpm` |

### Important: a git tag is required

`npm run build` (and the `beforeBuildCommand` of `tauri build`) runs `sync-version:git-tag`, which reads the **newest git tag** and writes it to:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Without a tag, the build fails. Create one first:

```bash
git tag v2.1.1
git push origin v2.1.1
```

Or set the version manually:

```bash
node scripts/sync-version.cjs --version 2.1.1
```

Other version-sync commands:

```bash
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch
```

> To verify frontend styles only, use `npx tsc --noEmit && npx vite build`, not `npm run build`.

### Microsoft Store (MSIX) bundle

The Store package is built on Windows only, because it uses the Windows SDK `makeappx`:

```powershell
npm run msix
```

- `scripts/build-msix.ps1` packages the NSIS payload into `src-tauri/msix/`, a full-trust Desktop Bridge manifest whose Partner Center identity lives in `store.config.json` (`XLBBB.RabbitChatStudio`), and writes the four-part MSIX version.
- Pass `-DevCert` to sign with a local development certificate; real submissions are signed by the Store.
- Store installs are detected by their `WindowsApps` path, which is why the in-app updater stays out of the way there.
- CI builds the `.msix` in the Windows job and keeps it as an Actions artifact only — it is not attached to the GitHub Release.
- Policy mapping, certification notes and screenshots live in `docs/store-publishing.md` and `docs/store/`.

## GitHub Actions release

`.github/workflows/release.yml` is triggered by:

- pushing a `v*` tag;
- or manual `workflow_dispatch` (build only, no release; useful as a smoke test).

```bash
git tag v2.1.1
git push origin v2.1.1
```

The workflow builds on 4 runners:

| Runner | Target |
| --- | --- |
| `macos-latest` | `aarch64-apple-darwin` |
| `macos-latest` | `x86_64-apple-darwin` |
| `ubuntu-22.04` | Linux x86_64 |
| `windows-latest` | Windows x64 |

Behavior:

- **The tag drives the version**: the workflow fails fast if the pushed tag is not the newest tag.
- **The release is a draft until published by hand**: `releaseDraft: true`; assets from all four platforms upload while the Release stays invisible, then a maintainer publishes it on the Releases page.
- **macOS is ad-hoc signed**: `APPLE_SIGNING_IDENTITY='-'`; replace with real signing/notarization secrets for distribution.
- **The MSIX is an artifact, not a release asset**: see above.
- **winget submission (inactive)**: `.github/workflows/winget.yml` is kept, but the package is not accepted into `microsoft/winget-pkgs` yet; it submits only after `WINGET_TOKEN` and a fork exist.
- **CI does not use the China mirror**: the workflow runs `rm -f .cargo/config.toml`.
- **Drafts are invisible to the update check**: `GET /releases` never returns drafts.

The Pages workflow (`.github/workflows/pages.yml`) is separate: it regenerates `architecture.html` and the wiki pages and publishes `docs/site/` — see [Development and Architecture](Development-and-Architecture#release-flow).

## Build troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| `npm run build` says no git tag | Create a tag first, or use `npx vite build` for the frontend only |
| `tauri dev` fails because the port is busy | Vite uses fixed `1420` with `strictPort`; free the port |
| Windows linker errors | Install MSVC Build Tools and use `x86_64-pc-windows-msvc`, not GNU |
| Missing WebView2 | Install the Edge WebView2 Runtime |
| Linux cannot find `webkit2gtk-4.1` | Install the Tauri 2 WebKitGTK dependencies |
| macOS blocks the first launch | Right-click → Open, or allow it in Privacy & Security |
| Cargo fetch timeouts | Use the rsproxy override or configure a proxy |
| Versions are inconsistent | Run `node scripts/sync-version.cjs --from-git-tag` |
| `npm run msix` cannot find `makeappx` | Install the Windows SDK signing tools; the Store bundle builds on Windows only |
| `gen-wiki.mjs` reports a broken link | The target page or anchor in `docs/wiki/` does not exist — fix the markdown, then re-run |

## License

Rabbit Chat Studio is MIT licensed (it moved from GPL-2.0-only in v3.2.0); see `LICENSE`.
