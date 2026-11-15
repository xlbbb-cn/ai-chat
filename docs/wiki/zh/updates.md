Rabbit Chat Studio 通过 GitHub Releases 检查本仓库的版本。它不会自动替换文件，只是把你指向对应的安装包，由你下载并运行。

> 从 Microsoft Store 安装的版本不走这条通道：商店负责更新，应用内检查永远显示"已是最新"。见下文 [Microsoft Store 版本](#microsoft-store-版本)。

## 应用内检查

### 启动时

**设置 → 关于与更新 → 启动时检查更新** 默认开启。每次启动请求：

```text
https://api.github.com/repos/xlbbb-cn/ai-chat/releases
```

行为：

- 只看**已发布**的版本；草稿对公开 API 不可见；
- 预发布版本会被跳过，除非开启 **包含预发布版本**；
- 取 semver 最高的那个；
- 安装包按当前操作系统/架构排序，最合适的一个标记为 **推荐**；
- 有新版本时顶部出现横幅：`Version x.y.z available — current a.b.c`。

### 手动检查

**设置 → 关于与更新 → 立即检查…** 打开更新面板：

| 元素 | 说明 |
| --- | --- |
| 当前版本 / 平台 | 如 `v3.3.3 · windows-x64` |
| 重新检查 | 再查一次 GitHub API |
| 最新版本 | 已是最新时显示版本与发布日期 |
| 新版本 | 版本、预发布徽标、推荐资产 |
| 下载 | 下载对应包并显示进度 |
| 运行安装包 | 打开已下载的安装程序 |
| 在文件夹中显示 | 定位下载的文件 |
| 发布说明 | Markdown 格式的更新说明 |
| 打开发布页 | 在浏览器中打开 GitHub Release |

### 下载行为

- 下载到系统"下载"文件夹；
- 进度通过 `update-download-progress` 推送；
- 完成后运行安装包或定位文件；
- 不静默安装，也不改动系统权限。

## 手动更新

### Windows

- 下载 `Rabbit-Chat-Studio-Windows-x64-setup.exe`，覆盖安装旧版本；
- 商店版不要这样做：交给商店更新。

### macOS

- 下载对应架构的 `.dmg`，拖进 **Applications**；
- 构建是 ad-hoc 签名，更新后首次启动可能仍需右键 → **打开**。

### Linux

- AppImage：替换旧文件并 `chmod +x`；
- `.deb` / `.rpm`：用包管理器升级。

## Microsoft Store 版本

商店版是 MSIX 包，更新由商店负责：

- 商店在后台更新——不用下载安装包，工作区数据也不受影响；
- **设置 → 关于与更新 → 检查更新** 永远显示"已是最新"：`update.rs` 里的 `is_store_packaged()` 一旦发现可执行文件路径含 `\WindowsApps\` 就短路返回，避免把用户引向第二份 GitHub 安装；
- 想立即拿新版本：打开 Microsoft Store → **库** → **获取更新**，或重新打开 <https://apps.microsoft.com/detail/9MV82J9G95GS>；
- 商店版与 GitHub 安装包版**不共享**应用配置（MSIX 把 `%APPDATA%` 重定向到 `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\`），所以不要在两版之间来回切还指望同一份配置；工作区里的 `.data/chat.db` 两边是同一个文件。

`.msix` 包由 CI 构建为工作流产物，**不**附在 GitHub Release 上，因此商店通道与安装包通道保持分离。

## 为什么没看到更新

| 原因 | 说明 |
| --- | --- |
| 已是最新 | 面板显示"You are on the latest published version" |
| 从商店安装 | 商店负责更新，应用内检查永远"已是最新"；走 Microsoft Store → 库 → 获取更新 |
| 发布是草稿 | `GET /releases` 从不返回草稿；先发布 |
| 只有预发布版 | 打开 **包含预发布版本** |
| 没有对应平台的资产 | 面板会说明；到 Release 页手动下载 |
| 网络/代理挡住 GitHub API | 检查连接；应用会显示错误 |

## 发布与版本号（维护者）

- 应用版本来自**最新的 git tag**；`npm run build` 里的 `sync-version:git-tag` 会更新 `package.json`、`Cargo.toml` 与 `tauri.conf.json`。
- 推送 `v*` tag 触发 `.github/workflows/release.yml`，并行构建 Windows/macOS/Linux。
- Release 以草稿创建（`releaseDraft: true`）：四个平台的产物上传完后由维护者在 Releases 页面手动发布。
- macOS 构建为 ad-hoc 签名，未做公证。
- `.github/workflows/winget.yml` 保留但未启用：只有当包被 `microsoft/winget-pkgs` 接受且存在 `WINGET_TOKEN` 时才会提交。

见 [安装与构建](Installation-and-Build)。

## 常见问题

| 现象 | 排查 |
| --- | --- |
| 提示有更新但下载失败 | 网络/代理；资产可能还在上传；手动下载 |
| 下载了却运行不了 | macOS 右键打开；Windows SmartScreen；Linux 执行权限 |
| 更新后配置丢了 | 配置在应用数据目录，本应保留；确认用户与安装方式 |
| 想用 winget 安装 | 暂无包管理器构建（winget / Homebrew / Scoop）；用商店版或 Release 安装包 |
| 不想自动检查 | 关掉启动检查；手动检查仍可用 |
| 看不到预发布 | 打开 **包含预发布版本**；草稿始终不可见 |
