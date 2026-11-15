本页分两部分：面向使用者的安装与升级，以及面向开发者的源码运行、校验与打包。只想用应用的话，读「终端用户安装」即可。

## 终端用户安装

### Windows

两种方式：

1. **Microsoft Store（推荐）**：打开 <https://apps.microsoft.com/detail/9MV82J9G95GS> 点「获取」。由商店负责安装与更新，不需要管理员权限。
2. **NSIS 安装包**：打开 [Latest release](https://github.com/xlbbb-cn/ai-chat/releases/latest)，下载 `Rabbit-Chat-Studio-Windows-x64-setup.exe`，按向导安装。

> 商店版是 MSIX 包：应用配置被重定向到 `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\...`，与 NSIS 版不共享；工作区里的 `.data` 目录两者共享。Release 只提供 NSIS 的 `-setup.exe`，不提供 `.msi`（本地 `npm run tauri build` 仍会产出，供自己用）。商店版也不做应用内的 GitHub 更新检查：更新走商店。

包管理器（暂未提供）：winget、Homebrew、Scoop 清单都还没发布，`winget install` / `brew install --cask` / `scoop install` 找不到这个包，请用上面两种方式。

### macOS

- Apple Silicon：`Rabbit-Chat-Studio-macOS-Apple-Silicon.dmg`
- Intel：`Rabbit-Chat-Studio-macOS-Intel.dmg`

打开 `.dmg` 把应用拖进「应用程序」。构建只是 ad-hoc 签名，首次启动需要右键 → **打开**，或到「系统设置 → 隐私与安全性 → 仍要打开」。

### Linux

- 通用：`Rabbit-Chat-Studio-Linux-x86_64.AppImage`

  ```bash
  chmod +x Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ./Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ```

- Debian/Ubuntu：`.deb`，`sudo apt install ./Rabbit-Chat-Studio-*.deb`
- Fedora/RHEL：`.rpm`，`sudo rpm -i Rabbit-Chat-Studio-*.rpm`

### 升级

安装版启动时会查询最新的**已发布** GitHub Release，有新版本就在界面里显示提示条；也可以在「设置 → 关于与更新」手动检查。见 [更新](更新)。

## 从源码运行

### 依赖

| 依赖 | 版本 / 说明 |
| --- | --- |
| Node.js | 20.19+ 或 22.12+ |
| Rust | stable，建议用 rustup 安装 |
| Tauri CLI | 2.x |
| Python 3 | 仅 `docs/gen.py`（架构图）需要 |
| Windows | MSVC 工具链（Visual Studio Build Tools，含「使用 C++ 的桌面开发」） |
| macOS | Xcode Command Line Tools |
| Linux | Tauri 2 的 WebKitGTK 依赖，见下 |

Debian/Ubuntu：

```bash
sudo apt update
sudo apt install -y build-essential curl wget file libssl-dev \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
```

### 安装依赖

```bash
npm install
cargo install tauri-cli --version "^2"
```

### 开发模式

```bash
npm run tauri dev
```

它会在固定端口 `1420` 启动 Vite（`strictPort: true`），编译 Rust 侧并拉起 Tauri。关掉应用同时会停掉 Vite。

只跑前端：

```bash
npm run dev
```

然后打开 `http://localhost:1420`。纯浏览器里没有 Tauri IPC，文件对话框、窗口控制、MCP 等功能不可用。

### 校验与测试

```bash
# 前端类型检查
npx tsc --noEmit

# 前端生产构建（不需要 git tag）
npx vite build

# Rust 检查
cargo check --manifest-path src-tauri/Cargo.toml

# Rust 单元测试（工具模块：dispatcher、todos、timer ……）
cargo test --manifest-path src-tauri/Cargo.toml
```

改界面时要同时检查浅色/深色主题和 `prefers-reduced-motion`。视觉规范在 `DESIGN.md`：颜色、圆角、阴影与动效必须用 `src/App.css` 里的 token。

### 文档与站点

`docs/site/` 是纯静态站点；其中两个页面由仓库里的源生成，产物**不入库**（见 `.gitignore`）：

```bash
# 架构图 → docs/site/architecture.html
python3 docs/gen.py

# Wiki 页面 → docs/site/wiki/**，源是 docs/wiki/{zh,en}/*.md
node scripts/gen-wiki.mjs
```

`gen-wiki.mjs` 依赖 `markdown-it`（普通依赖，`npm install` 即可），站内断链会让它直接失败。

### 中国网络：Cargo 镜像

Cargo 拉不到依赖时，覆盖源：

```bash
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
```

仓库里签入的 `.cargo/config.toml` 可能指向中科大镜像；CI 会删掉它，因为 GitHub runner 访问不到。

## 生产构建

```bash
npm run tauri build
```

产物：

```text
src-tauri/target/release/bundle/
```

| 平台 | 打包格式 |
| --- | --- |
| Windows | `.msi`、NSIS `-setup.exe` |
| macOS | `.app`、`.dmg`（当前架构） |
| Linux | `.AppImage`、`.deb`、`.rpm` |

### 重要：必须有 git tag

`npm run build`（以及 `tauri build` 的 `beforeBuildCommand`）会跑 `sync-version:git-tag`，读取**最新的 git tag** 并写进：

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

没有 tag 构建会失败。先打一个：

```bash
git tag v2.1.1
git push origin v2.1.1
```

或手动指定版本：

```bash
node scripts/sync-version.cjs --version 2.1.1
```

其它版本同步命令：

```bash
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch
```

> 只验证前端样式时用 `npx tsc --noEmit && npx vite build`，不要用 `npm run build`。

### Microsoft Store（MSIX）包

商店包只能在 Windows 上构建，因为它要用 Windows SDK 的 `makeappx`：

```powershell
npm run msix
```

- `scripts/build-msix.ps1` 把 NSIS 产物装进 `src-tauri/msix/` 的完全信任 Desktop Bridge 清单，Partner Center 身份写在 `store.config.json`（`XLBBB.RabbitChatStudio`），并写出四段式 MSIX 版本号。
- 加 `-DevCert` 用本地开发证书签名；正式提交由商店签名。
- 商店安装通过 `WindowsApps` 路径识别，这也是应用内更新器在那里不插手的原因。
- CI 在 Windows job 里构建 `.msix`，只作为 Actions artifact 保留，不挂到 GitHub Release。
- 政策映射、认证说明与截图见 `docs/store-publishing.md` 和 `docs/store/`。

## GitHub Actions 发布

`.github/workflows/release.yml` 由以下方式触发：

- 推送 `v*` tag；
- 或手动 `workflow_dispatch`（只构建、不发布，适合当冒烟测试）。

```bash
git tag v2.1.1
git push origin v2.1.1
```

工作流在 4 个 runner 上构建：

| Runner | 目标 |
| --- | --- |
| `macos-latest` | `aarch64-apple-darwin` |
| `macos-latest` | `x86_64-apple-darwin` |
| `ubuntu-22.04` | Linux x86_64 |
| `windows-latest` | Windows x64 |

行为：

- **tag 决定版本**：推送的 tag 不是最新 tag 时工作流立刻失败。
- **Release 默认为草稿**：`releaseDraft: true`；四个平台的产物上传完后仍是 draft，由维护者在 Releases 页面手动发布，失败的平台不会留下半成品的公开版本。
- **macOS 为 ad-hoc 签名**：`APPLE_SIGNING_IDENTITY='-'`；正式分发请换成真实的签名/公证密钥。
- **MSIX 只是 artifact**：见上文。
- **winget 提交（未启用）**：`.github/workflows/winget.yml` 还留着，但包尚未被 `microsoft/winget-pkgs` 接受；要等 `WINGET_TOKEN` 和 fork 就绪后才提交。
- **CI 不用国内镜像**：工作流里执行 `rm -f .cargo/config.toml`。
- **草稿对更新检查不可见**：`GET /releases` 从不返回草稿。

Pages 工作流（`.github/workflows/pages.yml`）是另一条：它重新生成 `architecture.html` 和 Wiki 页面并发布 `docs/site/`，见 [开发与架构](开发与架构#发布流程)。

## 构建排错

| 现象 | 原因 / 处理 |
| --- | --- |
| `npm run build` 报没有 git tag | 先打 tag，或只用 `npx vite build` 构建前端 |
| `tauri dev` 报端口被占用 | Vite 固定用 `1420` 且 `strictPort`；先释放端口 |
| Windows 链接器报错 | 装 MSVC Build Tools，并使用 `x86_64-pc-windows-msvc`，不要用 GNU |
| 缺少 WebView2 | 安装 Edge WebView2 运行时 |
| Linux 找不到 `webkit2gtk-4.1` | 安装 Tauri 2 的 WebKitGTK 依赖 |
| macOS 拦下首次启动 | 右键 → 打开，或在「隐私与安全性」里放行 |
| Cargo 拉取超时 | 用 rsproxy 覆盖或配置代理 |
| 版本号不一致 | 跑 `node scripts/sync-version.cjs --from-git-tag` |
| `npm run msix` 找不到 `makeappx` | 安装 Windows SDK 签名工具；商店包只能在 Windows 上构建 |
| `gen-wiki.mjs` 报断链 | `docs/wiki/` 里目标页面或锚点不存在，改好 markdown 再跑 |

## 许可证

Rabbit Chat Studio 使用 MIT 许可证（v3.2.0 起由 GPL-2.0-only 改为 MIT），见 `LICENSE`。
