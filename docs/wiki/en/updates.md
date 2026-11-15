Rabbit Chat Studio checks GitHub Releases for this repository. It does not replace files automatically; it points you at the matching installer and lets you download and run it.

> Builds installed from the Microsoft Store do not use this channel: the Store updates them, and the in-app check always reports "up to date". See [Microsoft Store build](#microsoft-store-build) below.

## In-app check

### On startup

**Settings → About & Updates → Check for updates on startup** is on by default. Each launch requests:

```text
https://api.github.com/repos/xlbbb-cn/ai-chat/releases
```

Behavior:

- only **published** releases; drafts are invisible to the public API;
- pre-releases are skipped unless **Include pre-release versions** is on;
- the highest semver wins;
- installers are ranked for the current OS/architecture, and the preferred one is marked **Recommended**;
- when a newer version exists, a banner appears at the top: `Version x.y.z available — current a.b.c`.

### Manual check

**Settings → About & Updates → Check Now…** opens the update panel:

| Element | Description |
| --- | --- |
| Current version / platform | e.g. `v3.3.3 · windows-x64` |
| Check again | Query the GitHub API again |
| Latest version | Shows the version and publish date when up to date |
| New version | Version, pre-release badge, recommended asset |
| Download | Download the matching bundle; shows progress |
| Run installer | Open the downloaded installer |
| Show in folder | Reveal the downloaded file |
| Release notes | Markdown release notes |
| Open release page | Open the GitHub Release in a browser |

### Download behavior

- downloads to the OS Downloads folder;
- progress is delivered through `update-download-progress`;
- when done, run the installer or reveal the file;
- no silent install and no system permission changes.

## Manual update

### Windows

- download `Rabbit-Chat-Studio-Windows-x64-setup.exe` and install over the old version;
- the Store build should not use this: let the Store update it.

### macOS

- download the `.dmg` for your architecture and drag it into **Applications**;
- builds are ad-hoc signed, so the first launch after an update may still require right-click → **Open**.

### Linux

- AppImage: replace the old file and `chmod +x`;
- `.deb` / `.rpm`: upgrade with the package manager.

## Microsoft Store build

The build installed from the Microsoft Store is an MSIX package, and the Store owns its updates:

- the Store updates it in the background — no installer to download, and your workspace data is untouched;
- **Settings → About & Updates → Check for updates** always reports "up to date": `is_store_packaged()` in `update.rs` short-circuits as soon as the executable path contains `\WindowsApps\`, so nobody is pointed at a second GitHub install;
- to pull a new version immediately: open Microsoft Store → **Library** → **Get updates**, or reopen <https://apps.microsoft.com/detail/9MV82J9G95GS>;
- the Store build and the GitHub installer build do **not** share their app config (MSIX redirects `%APPDATA%` into `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\`), so do not alternate between them expecting one profile; `.data/chat.db` inside your workspace is the same file for both.

The `.msix` bundle itself is built by CI as a workflow artifact — it is not attached to the GitHub Release, so the Store channel and the installer channel stay separate.

## Why an update may not appear

| Reason | Explanation |
| --- | --- |
| Already latest | The panel says "You are on the latest published version" |
| Installed from the Store | The Store updates it, and the in-app check always says "up to date"; use Microsoft Store → Library → Get updates |
| Release is a draft | `GET /releases` never returns drafts; publish it first |
| Only pre-releases | Turn on **Include pre-release versions** |
| No asset for your platform | The panel says so; download from the Release page manually |
| Network/proxy blocks the GitHub API | Check the connection; the app shows the error |

## Release and versioning (maintainers)

- The app version comes from the **newest git tag**; `sync-version:git-tag` inside `npm run build` updates `package.json`, `Cargo.toml` and `tauri.conf.json`.
- Pushing a `v*` tag triggers `.github/workflows/release.yml` and builds Windows/macOS/Linux in parallel.
- Releases are created as drafts (`releaseDraft: true`): all four platforms upload while the Release stays invisible, then a maintainer publishes it by hand.
- macOS builds are ad-hoc signed and not notarized.
- `.github/workflows/winget.yml` is kept but inactive: it only submits once the package is accepted into `microsoft/winget-pkgs` and `WINGET_TOKEN` exists.

See [Installation and Build](Installation-and-Build).

## FAQ

| Symptom | Check |
| --- | --- |
| Update offered but download fails | Network/proxy; assets may still be uploading; download manually |
| Downloaded but cannot run | macOS right-click Open; Windows SmartScreen; Linux execute permission |
| Config lost after update | Config lives in app data and should survive; check user/install method |
| Want to install with winget | No package-manager build yet (winget / Homebrew / Scoop); use the Store build or the Release installer |
| Do not want automatic checks | Turn off startup checking; manual checks still work |
| Pre-release not visible | Enable **Include pre-release versions**; drafts stay invisible |
