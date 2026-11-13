# Rabbit Chat Studio 隐私政策 / Privacy Policy

生效日期：2026-11-12　·　适用于 Rabbit Chat Studio 桌面应用（Windows / macOS / Linux，含 Microsoft Store 版本）

> 这份文本就是为了填 Partner Center 的「隐私政策 URL」而写的：托管到 GitHub Pages、仓库 raw 链接
> 或项目主页后，把 URL 填进去即可（政策 10.5.1 要求 Desktop Bridge / Win32 产品必须有隐私政策）。

---

## 中文

### 1. 我们不收集什么

Rabbit Chat Studio 没有账号系统、没有遥测、没有统计 SDK、没有广告。开发者不会收到你的对话内容、API Key、
文件或使用记录——这些数据不会上传到开发者的任何服务器。

### 2. 数据存在哪里

| 数据 | 位置 |
|---|---|
| 对话历史、会话摘要、待办 | 你在应用里选择的工作区目录下的 `.data\chat.db`（SQLite） |
| 设置（endpoint、模型、技能开关等） | 应用数据目录的 `config.json` |
| 日志 | 应用数据目录的 `logs\`（保留天数可在设置里改，`log_retention_days`） |
| 生成的图片/网页图（figures） | 工作区下的 `figures\` |

Windows 上应用数据目录一般是 `%APPDATA%\com.leonard.aichat`。如果你安装的是 Microsoft Store（MSIX）
版本，Windows 会把它重定向到 `%LOCALAPPDATA%\Packages\<包名>\LocalCache\Roaming\com.leonard.aichat`
（MSIX 的标准行为，卸载应用时随之清除）。

### 3. 数据会发给谁

- **你配置的模型服务商**：聊天请求会直接发到你填写的 API base URL，并带上你填的 API Key。
  Key 只保存在本机配置文件里，只在请求该服务商时使用。服务商如何处理这些内容取决于它自己的隐私政策。
- **第三方 MCP 服务器 / 子代理 / CLI 工具**：如果你启用它们，应用会把提示和相关上下文交给这些
  你自行配置的进程或服务；它们由你选择和授权，其数据处理以各自条款为准。
- **模型元数据目录**：在设置里刷新模型列表时，应用会查询三个公开目录（`models.dev`、`openrouter.ai`、`basellm.github.io`
  的 llm-metadata）以获取模型的上下文长度与能力，最多每周一次，
  合并后的结果保存在本机应用数据目录；这些查询只包含模型名称，不包含对话内容。
- **更新检查**：启动时（可在设置中关闭「启动时检查更新」）会请求 GitHub Releases API
  （`api.github.com`）获取版本号，请求不含任何个人数据。
- 除上述情形外，应用不会把数据发送给任何第三方。

### 4. 文件与命令

应用的工具（读写文件、运行命令、待办、定时器、记忆等）只在你于界面中确认后才会执行，
每次写入文件和执行 shell 命令都会弹出确认（风险分级 L0–L3，可在「工具」面板调整自动确认范围）。
应用的默认作用范围是你在应用里指定的工作区目录；执行范围由你授予。

### 5. 你的控制权

- 你可以随时在设置里清空或删除对话；删除即从本地数据库移除。
- 卸载应用后，MSIX 版本会删除包内数据目录；工作区目录里的 `.data`、`figures` 需要你自己删除。
- 你可以关闭更新检查、关闭工具调用、清除 API Key（清空即不再发起请求）。
- 应用不保存你的任何账号凭证（应用本身没有账号）。

### 6. 儿童

本应用面向具备相应判断能力的开发者/成人用户，不面向 13 岁以下儿童，也不会主动收集儿童的个人信息。

### 7. 生成式 AI 内容

对话回复由你配置的生成式 AI 模型产生，可能不准确。你可以通过应用标题栏 `⋯ → Report an issue`
（打开项目 issue 页面）举报不适当的输出。

### 8. 安全

API Key 与配置保存在本机文件系统中，不写入日志；应用与模型服务商之间的通信使用 HTTPS。
请自行妥善保管你的 API Key 与工作区权限。

### 9. 变更与联系方式

本政策如有更新，会随新版本发布并更新上面的生效日期。
问题、疑问或数据相关请求：<https://github.com/xlbbb-cn/ai-chat/issues>

---

## English

### 1. What we do not collect

Rabbit Chat Studio has no account system, no telemetry, no analytics SDK and no ads. The developer never
receives your conversations, API keys, files or usage records — none of it is uploaded to any
server operated by the developer.

### 2. Where your data lives

| Data | Location |
|---|---|
| Conversations, session summaries, todos | `.data\chat.db` (SQLite) inside the workspace folder you pick |
| Settings (endpoint, model, enabled tools…) | `config.json` in the app data folder |
| Logs | `logs\` in the app data folder (retention configurable, `log_retention_days`) |
| Generated figures | `figures\` inside the workspace |

On Windows the app data folder is normally `%APPDATA%\com.leonard.aichat`. For the Microsoft Store
(MSIX) build, Windows redirects it to
`%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalCache\Roaming\com.leonard.aichat` — standard MSIX
behaviour, removed when the package is uninstalled.

### 3. Who your data is sent to

- **The model provider you configure**: chat requests go directly to the API base URL you enter, with
  the API key you enter. The key is stored locally and used only for requests to that provider. How the
  provider handles the content is governed by its own privacy policy.
- **Third-party MCP servers / sub-agents / CLI tools**: if you enable them, the app hands prompts and
  related context to those processes or services that you configured and authorised.
- **Model metadata catalogues**: refreshing the model list queries three public catalogues for context
  windows and capabilities (`models.dev`, `openrouter.ai`, `basellm.github.io` llm-metadata) at most
  once a week, and stores the merged result in the local app-data directory. Those requests contain
  model names only, never conversation content.
- **Update check**: on startup (can be turned off in settings) the app calls the GitHub Releases API
  (`api.github.com`) for version numbers. No personal data is included.
- Beyond the above, no data is sent to any third party.

### 4. Files and commands

Tools (file read/write, run command, todos, timers, memory…) only run after you confirm them in the UI;
every file write and shell command goes through a confirmation dialog (risk levels L0–L3, with the
auto-accept scope configurable in the Tools panel). The default scope is the workspace folder you choose.

### 5. Your controls

- Clear or delete conversations at any time; deletion removes them from the local database.
- Uninstalling an MSIX build removes the package data folder; `.data` and `figures` inside your workspace
  are yours to delete.
- Turn off the update check, turn off tools, or clear the API key (no key, no requests).
- The app stores no account credentials, because it has no accounts.

### 6. Children

The app targets developers and adult users and is not directed at children under 13.

### 7. Generative AI content

Replies come from the generative AI model you configure and may be inaccurate. Inappropriate output can
be reported via the title-bar `⋯ → Report an issue` entry, which opens the project issue tracker.

### 8. Security

API keys and settings are stored in local files and never written to logs. Traffic to the model provider
uses HTTPS. Keep your API key and workspace permissions safe.

### 9. Changes and contact

Updates to this policy ship with new versions and update the effective date above.
Questions or data-related requests: <https://github.com/xlbbb-cn/ai-chat/issues>
