# 发布到 Microsoft Store（MSIX）

面向本仓库的实操手册：怎么把 Rabbit Chat Studio 打成 Store 能收的包、提交时要填什么、MSIX 下哪些行为会变。

---

## 1. 两条可走的路（先选）

| | A. MSIX 包（推荐，已落地） | B. 非打包 MSI/EXE（政策 10.2.9） |
|---|---|---|
| 签名 | **Store 用微软证书重签**，自己不需要买证书 | **必须自备**链到 Microsoft Trusted Root 的代码签名证书（OV/EV + 硬件令牌） |
| 安装方式 | Store 直接下发 MSIX | 提交一个版本化 HTTPS 直链，Store 静默安装（NSIS 用 `/S`） |
| 更新 | Store 托管 | 自己维护 URL 和更新器 |
| 包身份 | 有（package identity） | 无 |
| 隔离 | AppData / 注册表写操作被虚拟化（见第 6 节） | 无隔离，行为与现在的安装包一致 |

没有 CA 代码签名证书就走 A。本仓库已经在 A 上铺好了流水线。

Tauri 官方文档（`v2.tauri.app/distribute/microsoft-store`）写的是 B 路线，因为 Tauri 自己不出 MSIX；
MSIX 需要用 `makeappx`（Windows SDK）自己打，本仓库的 `scripts/build-msix.ps1` 就是这一步。

---

## 2. 一次性准备：Partner Center

1. 注册开发者账号（一次性 $19），完成身份验证。
2. <https://partner.microsoft.com/dashboard/apps-and-games/overview> → **Create new product** → 选 **MSIX or PWA app**
   （不要选 “EXE or MSI app”，那是 B 路线）。
3. **Reserve name**。产品已创建（Store ID `9MV82J9G95GS`，类型 MSIX/PWA），名称 `Rabbit Chat Studio`
   已预留；若名称审核要求调整，只改显示名，包结构不用动。
   - Publisher 名不能和产品名相同（Tauri 官方文档也强调这点），`store.config.json` 里的
     `publisherDisplayName` 用 `XLBBB`（与 Partner Center 完全一致）。
4. 产品页 → **产品身份（Product identity）**。三个值已填进
   [`src-tauri/msix/store.config.json`](../src-tauri/msix/store.config.json)，与 Partner Center 逐字符一致：

   ```jsonc
   {
     "packageName": "XLBBB.RabbitChatStudio",                 // Package/Identity/Name
     "publisher": "CN=2514CB06-3D94-4243-9E5E-0B1D1B3A8380",  // Package/Identity/Publisher
     "publisherDisplayName": "XLBBB"                          // Package/Properties/PublisherDisplayName
   }
   ```

   以后改产品名或换发布者，必须同步这里，否则 Store 会以身份不匹配拒收。

---

## 3. 打包

```powershell
npm run msix                        # 构建 exe（tauri build --no-bundle）+ 打成 .msix
npm run msix -- -SkipBuild          # 复用已有的 src-tauri\target\release\rabbit-chat-studio.exe，只打包
npm run msix -- -SkipBuild -DevCert # 再生成/复用自签开发证书签名，用于本地侧载
npm run msix -- -SkipBuild -Sign -CertPath .\code.pfx -CertPassword ***   # 用自己的正式证书签名
npm run msix -- -Arch arm64         # ARM64（需要先 rustup target add aarch64-pc-windows-msvc）
```

产物：`src-tauri/target/msix/msix/<PackageName>_<Version>_<Arch>.msix`。

- 版本号来自 `src-tauri/tauri.conf.json`，自动补成 MSIX 要求的四段数字（`3.1.1` → `3.1.1.0`）；
  每次提交的版本必须比上一版大。
- manifest 由 [`src-tauri/msix/Package.appxmanifest.template`](../src-tauri/msix/Package.appxmanifest.template)
  生成，桌面应用固定 `EntryPoint="Windows.FullTrustApplication"` + `rescap:runFullTrust`，
  否则读写工作区、启动 CLI 代理和 MCP 子进程都会被挡。
- **不声明 WebView2 框架包依赖**（`Microsoft.Win32WebView2` 依赖在 Store 上会导致安装失败），
  运行时装系统自带的 Evergreen Runtime；`MinVersion` 定在 Windows 10 2004（10.0.19041.0），
  Windows 11 全系预装。
- 提交给 Partner Center 的是**未签名**的 `.msix`，Store 收到后用微软证书重签。

---

## 4. 本地侧载验证（需要管理员）

```powershell
# 管理员 PowerShell：开发证书必须进 LocalMachine 的受信任存储，包部署服务才认
$pw = ConvertTo-SecureString 'rabbit-chat-dev' -AsPlainText -Force
Import-PfxCertificate -FilePath src-tauri\msix\devcert.pfx -CertStoreLocation Cert:\LocalMachine\TrustedPeople -Password $pw
Import-PfxCertificate -FilePath src-tauri\msix\devcert.pfx -CertStoreLocation Cert:\LocalMachine\Root -Password $pw

Add-AppxPackage -Path src-tauri\target\msix\msix\XLBBB.RabbitChatStudio_3.1.1.0_x64.msix
Get-AppxPackage -Name XLBBB.RabbitChatStudio | Format-List Name,Version,PackageFullName,InstallLocation
explorer.exe "shell:AppsFolder\$( (Get-AppxPackage -Name XLBBB.RabbitChatStudio).PackageFamilyName )!RabbitChatStudio"

# 验完卸载
Remove-AppxPackage -Package (Get-AppxPackage -Name XLBBB.RabbitChatStudio).PackageFullName
```

侧载后重点看三件事：窗口能不能起来（自绘标题栏 + 透明窗口在 MSIX 下不变）、
`%LOCALAPPDATA%\Packages\<PFN>\LocalCache\Roaming\com.leonard.aichat` 里有没有生成配置、
工具面板里跑一条命令能不能通。

WACK 静态校验（可选，也要管理员）：
`"C:\Program Files (x86)\Windows Kits\10\App Certification Kit\appcert.exe" test -packagepath <msix> -reportoutputpath report.xml -basic`

---

## 5. 提交时要填的东西

| Partner Center 页面 | 填什么 |
|---|---|
| **Packages** | 上传 `src-tauri/target/msix/msix/*.msix`（未签名）。设备家族只勾 PC；架构 x64（+ arm64 如果打了） |
| **Properties → 类别 / 系统要求** | Productivity 或 Developer tools；Windows 10 2004+，x64/x86 需 WebView2 Runtime |
| **Properties → 隐私政策 URL** | **必填**。Desktop Bridge / Win32 产品一律要有（政策 10.5.1），文本见 [`docs/store/privacy-policy.md`](./store/privacy-policy.md)，托管步骤见 §11.1 |
| **Properties → 年龄分级** | IARC 问卷。生成式 AI + 允许执行命令 + 无成人内容 → 一般落 12+/Teen |
| **Store listing** | 中英文描述（≤10000 字）、至少 1 张截图（1366×768 起，PNG/JPG，最多 10 张）、7 个以内的搜索词、StoreLogo 由包内资源提供 |
| **Submission options** | **必须勾「此产品使用生成式 AI（live generative AI）」**（政策 11.16）；`runFullTrust` 需要一句理由：“桌面应用需要读写用户工作区、运行用户确认后的命令” |
| **Notes for certification** | 见 §11.2（`docs/store/certification-notes.md`），尤其测试用 API key |

### 认证备注（英文，直接粘）

全文见 [`docs/store/certification-notes.md`](./store/certification-notes.md)：已按 `deepseek-flash` +
专用测试 key 写好（两步测试流程、`runFullTrust` 理由、生成式 AI 披露、网络访问清单）。
粘贴前把 `<TEST_API_KEY>` 换成私密渠道里的 key，认证结束后吊销。

---

## 6. MSIX 下的行为差异（务必知道）

1. **AppData 被虚拟化。** full-trust 打包应用写 `%APPDATA%` / `%LOCALAPPDATA%` 会被重定向到
   `%LOCALAPPDATA%\Packages\<PFN>\LocalCache\…`。这是兼容性虚拟化，不是安全沙箱，没有开关。
   后果：Store 版的应用配置（`%APPDATA%\com.leonard.aichat`）与 NSIS 版不共享；
   用户从 NSIS 换成 Store 版会看到“首次启动”。工作区里的 `.data\chat.db` 不受影响（在用户目录之下，
   不是被虚拟化的已知文件夹）。
2. **子进程继承同样的视图。** 从打包进程里启动的 `claude` / `codex` / `node` 等 CLI 也看不到用户
   真实 profile 里的 AppData → CLI 代理的登录态、`~/.claude` 之类配置与用户在普通终端里用的不是同一份。
   这是 MSIX 的已知坑（Codex 桌面版有同类报告）。
   缓解：启动子进程时带 `PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY` +
   `PROCESS_CREATION_DESKTOP_APP_BREAKAWAY_OVERRIDE` 让子进程脱离 desktop-app 运行时环境；
   在没做这件事之前，Store 版应在描述/首次运行提示里说明“CLI 代理需在应用内重新登录”。
3. **安装目录只读。** 应用不能写 `…\WindowsApps\<PackageFullName>\`，本应用不写安装目录，无影响。
4. **自动更新交给 Store。** `src-tauri/src/update.rs` 的 `is_store_packaged()` 在 exe 路径含
   `\WindowsApps\` 时让 `check_update` 直接返回“已是最新”，避免把用户引到 GitHub 去装第二份。
5. **完整信任仍在。** 用户选的任意工作区目录、注册表读、外部 CLI 都能用；受保护位置（Program Files、
   系统目录）的写入仍被重定向。

---

## 7. 每次发版的节奏

1. 改版本、打 `v*` tag → CI 出 NSIS 安装包（`release.yml`）。
2. `npm run msix -- -SkipBuild`（CI 的 Windows job 也会产出 `.msix` 作为 Actions artifact）。
3. Partner Center → 该产品 → **Update** → 上传新 `.msix`（版本号必须递增，例如 `3.1.2.0`）→ 提交。
4. 首次认证一般 24–48 小时；后续更新通常更快。

---

## 8. 政策对照（本应用）

| 政策 | 要求 | 本应用怎么满足 |
|---|---|---|
| 10.1.1 | 名称唯一、描述与实际功能一致 | 名称换掉通用词；描述里写明“BYO API key / 会执行用户确认后的命令” |
| 10.2.2 | 不得通过动态代码扩展功能 | 执行命令属于已描述的核心功能（类似 IDE/终端工具），描述中明示；不下载远程脚本变更自身功能 |
| 10.2.3 | 不得含恶意软件、不得安装无关第三方软件 | 不在安装流程里装别的软件；MCP/子代理都由用户显式配置 |
| 10.2.9 | 非打包 MSI/EXE 的签名要求 | 走 MSIX 路线，本条不适用（Store 重签） |
| 10.3 | 必须可测试 | 认证备注里给测试 endpoint + key（见模板） |
| 10.5.1 | 隐私政策 | 必填 URL，文本见 `docs/store/privacy-policy.md` |
| 10.6 | 能力声明与实际功能相符 | manifest 只声明 `runFullTrust` |
| 11.16 | 生成式 AI：披露 + 应用内举报 | Store listing 勾选 GAI；标题栏 `⋯ → Report an issue` 直达 issue 页 |
| 11.11 | 年龄分级 | 提交时完成 IARC 问卷并保持一致 |
| — | MIT | 不再有强制源码分发义务；描述里仍建议给源码地址（审核友好） |

---

## 9. 商店文案（可直接粘）

**产品名**：建议 `Rabbit Chat Studio Studio`（`Rabbit Chat Studio` 过于通用，名称审核容易被打回；Publisher 名不要与产品名相同）

**简短说明（一句话）**

> 自带密钥的 OpenAI 兼容 AI 桌面客户端：流式对话、工具调用、技能、MCP 与子代理，全部在本机运行。

**中文完整描述**

```
Rabbit Chat Studio 是一个本地优先的 AI 桌面客户端。它不自带模型、不需要注册账号：填入你自己的
OpenAI 兼容 API 地址与 API Key，就能开始对话。

主要功能
• 流式对话与推理展示，支持任何 OpenAI 兼容端点（OpenAI、OpenRouter、Azure、vLLM、Ollama…）
• 工具调用：读写文件、运行命令、待办、定时器、记忆。每一次写入文件和执行命令都会先弹出确认，
  风险按等级区分，作用范围限于你指定的工作区目录
• 技能（Skills）：把可复用的工作流存成技能，随时调用
• MCP 服务器：接入你自己的 MCP 工具，开关和权限由你控制
• 子代理：把任务分派给子代理并行推进
• 图表与 3D：对话里可以直接生成流程图、矢量图和可旋转的 3D 场景并保存
• @文件 提及：把工作区里的文件、PDF 指定页内联进提问
• 界面：亮/暗主题、四档毛玻璃、自绘标题栏

隐私
对话与配置只存在你的电脑上（工作区里的 SQLite + 本地配置），没有遥测、没有广告。
请求直接发给你填写的模型服务商，开发者不经手任何数据。

许可与源码
MIT，源码与问题反馈：https://github.com/xlbbb-cn/ai-chat
```

**英文完整描述**

```
Rabbit Chat Studio is a local-first AI desktop client. It ships with no models and no account:
enter your own OpenAI-compatible base URL and API key and start chatting.

Highlights
• Streaming chat with reasoning display, against any OpenAI-compatible endpoint
  (OpenAI, OpenRouter, Azure, vLLM, Ollama…)
• Tool calls: read/write files, run commands, todos, timers, memory. Every file write and shell
  command asks for confirmation first; risk levels are graded and the scope is the workspace
  folder you choose
• Skills: save reusable workflows and invoke them from the composer
• MCP servers: connect your own MCP tools, with per-server toggles
• Sub-agents: hand tasks to parallel agents and collect their results
• Figures: flowcharts, vector drawings and rotatable 3D scenes generated in-chat and saved to disk
• @file mentions: inline workspace files and specific PDF pages into a prompt
• UI: light/dark themes, four frosted-glass levels, custom title bar

Privacy
Conversations and settings stay on your machine (SQLite in your workspace + a local config file).
No telemetry, no ads. Requests go straight to the provider you configure; the developer never
sees your data. Privacy policy: <FILL IN the hosted URL>

License and source
MIT — source and issue tracker: https://github.com/xlbbb-cn/ai-chat
```

**搜索词（7 个以内）**：`AI chat`、`OpenAI client`、`LLM`、`MCP`、`AI assistant`、`chat client`、`skills`

**截图（至少 1 张，1366×768 起，PNG/JPG，最多 10 张）**：建议 4 张 ——
① 主对话界面（带工具调用卡片）② 设置里的 endpoint/模型页 ③ 技能面板 ④ 图表面板（flow / 3D）。
截图里不要出现真实 API Key，也不要出现带版权内容的第三方界面。

**"本次更新"（What's new）**：直接复用 `CHANGELOG.md` 里该版本条目。

---

## 10. 改名（AI Chat → Rabbit Chat Studio）的影响

已改：显示名（窗口标题、关于对话框、菜单、中英繁三语）、`productName`、Cargo 包名与产物
`rabbit-chat-studio.exe`、MSIX 身份（`XLBBB.RabbitChatStudio`，`Application Id=RabbitChatStudio`）、
CI 稳定产物名（`Rabbit-Chat-Studio-*`）、README 下载链接、winget id。

故意没改（改了会伤用户数据或外部链接）：

| 保留 | 原因 |
|---|---|
| `identifier = com.leonard.aichat` | 用户配置目录就是它（`%APPDATA%\com.leonard.aichat`）；改标识符等于让所有老用户配置"消失"，要动就得先写迁移 |
| GitHub 仓库 slug `xlbbb-cn/ai-chat` | 更新检查、Issue 模板、README 链接全指向它；真要改仓库名 GitHub 会 302，但不必要 |
| localStorage 键 `ai-chat.locale` | 界面语言是用户偏好，换键会重置 |

老用户升级注意：NSIS 安装目录随 `productName` 变成 `Rabbit Chat Studio`，升级会装到新目录（配置不受影响，
配置在 identifier 目录里）。对外说明里建议让老用户先卸载旧版再装新版。

还有一处**仍是旧字样**，需要人工处理：

1. winget：旧 id `Leonard.AIChat` 的 PR（winget-pkgs#458061）已关闭；新 id 定为 `XLBBB.RabbitChatStudio`，
   `.github/workflows/winget.yml` 与两份 README 的安装命令已同步，后续发版按新 id 提交。

`docs/banner.png` 已按新名称重绘（`.data/tmp/banner/build.sh` 是可重复运行的生成脚本）；商店截图
仍需在装好新名称窗口的机器上重拍。

---

## 11. 提交前最后三件事

### 11.1 隐私政策托管（必填，政策 10.5.1）

[`docs/store/privacy-policy.md`](store/privacy-policy.md) 已定稿，缺一个公网 URL：

1. 仓库 → **Settings → Pages** → Source 选 *Deploy from a branch*，分支 `main`，目录 **`/docs`**，保存。
2. 一两分钟后打开 <https://xlbbb-cn.github.io/ai-chat/store/privacy-policy.html>
   （Jekyll 会把 `.md` 渲染成 `.html`；若 404，先看 Pages 的构建状态）。
3. 这一行 URL 填进 Partner Center → Properties → **Privacy policy URL**。

不想公开 `/docs` 下其它文件，就单独开一个 `gh-pages` 分支只放隐私政策。注意 URL 不能是
`raw.githubusercontent.com`（会被当成下载而不是网页）。

### 11.2 认证备注（政策 10.3 可测试性）

[`docs/store/certification-notes.md`](store/certification-notes.md) 是可整段粘贴的英文版：应用用途、
两步测试步骤（DeepSeek endpoint + 测试 key + `deepseek-flash`）、`runFullTrust` 理由、生成式 AI 披露、
网络访问清单。**真实测试 key 不写进仓库**，粘贴时换成私密渠道的那份。

提交页「受限的功能」里 runFullTrust 的申请理由见
[`docs/store/runfulltrust-justification.md`](store/runfulltrust-justification.md)（中英各一份，
中文界面用中文那段）。清单只声明了这一项受限能力，没有 `broadFileSystemAccess` 等其它项；
首次批准后该产品沿用，后续版本不必重填。

测试凭据本身（2026-11-12 验证可用）：

| 字段 | 值 |
|---|---|
| Base URL | `https://api.deepseek.com/v1` |
| Model | `deepseek-flash`（另有 `deepseek-v4-pro`） |
| API Key | 不落库，见私密渠道；认证通过后建议吊销 |

### 11.3 截图

```powershell
pwsh -File scripts/capture-store-screenshots.ps1                  # 截前台窗口
pwsh -File scripts/capture-store-screenshots.ps1 -DelaySeconds 5  # 留时间切到目标界面
```

输出 `docs/store/screenshots/screenshot-<时间戳>.png`（该目录不入库）。窗口小于 1366×768 会提示放大。
建议 4 张：主对话（带工具调用卡片）、API 与模型设置、技能面板、图表面板。截图里不要出现真实 key。

### 11.4 提交顺序

1. **Packages** → 上传 `src-tauri/target/msix/msix/XLBBB.RabbitChatStudio_3.1.1.0_x64.msix`，等验证通过。
2. **Properties** → 类别（生产力 / 开发人员工具）、隐私政策 URL、系统要求（Windows 10 2004+，x64）、年龄分级问卷。
3. **Store listing** → 描述（§9 有中英两份）、截图、搜索词（≤7）。
4. **Submission options** → 勾选「使用生成式 AI」；`runFullTrust` 理由粘 §11.2 那段。
5. **Notes for certification** → 粘 §11.2 英文全文。
6. **Submit** —— 审核通常 1–3 个工作日；被拒会给出具体政策条款，按条款改再重新提交即可。
