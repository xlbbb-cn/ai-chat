# runFullTrust 受限功能申请理由

Partner Center 提交时「为何需要使用 runFullTrust 功能，如何在产品中使用？」这一栏的正式答复。
中文版用于当前中文界面，英文版用于审核员以英文审阅的情况（内容等价，可只贴中文）。

- 清单里只声明了这一项受限功能：`src-tauri/msix/Package.appxmanifest.template` 中
  `EntryPoint="Windows.FullTrustApplication"` + `<rescap:Capability Name="runFullTrust" />`，
  没有 `broadFileSystemAccess`、`packageQuery` 等其它受限能力。
- 首次提交需要人工批准，之后该产品的这项能力保持已批准状态，后续版本不必重复填写。

---

## 中文版（粘这段）

**一、应用性质**

Rabbit Chat Studio 是一个 Windows 桌面 AI 助手客户端，使用 Tauri 2（Rust 后端）+ WebView2 开发，
本质是标准 Win32 桌面程序，以 MSIX 打包（Desktop Bridge，EntryPoint = Windows.FullTrustApplication）。
应用本身不包含任何 AI 模型：用户自行填写兼容 OpenAI 的接口地址与密钥，应用只负责把对话发送到该
接口并显示返回内容。无账号体系、无内购、无遥测、无广告。

**二、为什么必须使用 runFullTrust**

1. 需要在用户选择的任意目录读写文件。产品核心功能是"在用户指定的工作区里修改代码、生成文档"，
   工作区可以是任意盘的任意路径、项目仓库、网络映射盘。MSIX 应用容器只能访问包自己的数据目录和
   库文件夹，无法访问用户选定的工程目录，功能无法实现。
2. 需要启动并接管用户本机已有的命令行工具。应用可调用 git、node、python、ripgrep、PowerShell、
   cmd 等外部程序，为其设置工作目录和环境变量，读取 stdout/stderr 并把结果回填到对话。
   AppContainer 内的应用不能在容器外创建进程，也无法继承用户已配置的 PATH、环境变量、git/SSH
   凭据和包管理器状态，"应用里能跑、终端里跑不了"会让功能直接失效。
3. 需要与用户既有开发环境保持一致。应用执行命令时使用的是用户自己终端中的那份配置
   （%USERPROFILE% 下的 .gitconfig、.ssh、npm/pip 全局配置等），沙箱化会让读写落在被隔离的
   副本上，产生误导性的结果。
4. 需要系统代理设置、系统证书存储以及本机回环地址访问：企业网络与自建网关依赖系统代理与根证书，
   用户也可能把模型指向本机推理服务（如 http://localhost:11434）。

**三、如何在使用中限制这些能力**

- 应用不是后台服务：没有开机自启任务、没有系统服务、不安装驱动、不需要管理员权限，始终以当前
  登录用户身份运行。
- 文件与命令的作用范围由用户在应用内显式选择的"工作区"目录决定；应用不会主动扫描或修改工作区
  以外的文件。
- 每条命令、每次文件写入都要经过风险评分引擎：命令会先展示给用户（含风险级别），只有在弹窗中
  确认后才真正执行；只读/安全操作可直接运行。自动批准最高风险级别的开关默认关闭。
- 应用在本地日志目录记录执行过的命令与工具调用，保留天数可配置，用户可随时查看与清理。
- 不收集遥测，不上传用户文件；对话内容只发送到用户自己填写的模型接口。
- 清单文件仅声明 runFullTrust 一项受限功能。

**四、补充**

这与 Visual Studio Code、Windows Terminal、PowerToys 等桌面应用声明 runFullTrust 的原因同类：
本质是打包后的 Win32 桌面程序，需要以用户身份访问文件系统与外部进程，不需要也不使用容器外的
额外特权。审核所需的测试凭据与操作步骤见「Notes for certification」。

---

## English version (paste this instead if the reviewer works in English)

**What the product is**

Rabbit Chat Studio is a Windows desktop AI assistant client built with Tauri 2 (Rust) and WebView2 —
a standard Win32 desktop application shipped as MSIX (Desktop Bridge,
`EntryPoint="Windows.FullTrustApplication"`). It bundles no model: the user supplies an
OpenAI-compatible endpoint and key, and the app sends the conversation there and renders the reply.
No account, no in-app purchase, no telemetry, no ads.

**Why runFullTrust is required**

1. **Read/write files in any folder the user picks.** The product's purpose is to edit code and write
   documents inside a workspace the user selects — any drive, any repository, network shares included.
   An MSIX app container can only reach its own data folders and libraries, so the core feature cannot
   work sandboxed.
2. **Launch the command-line tools the user already has.** The app runs git, node, python, ripgrep,
   PowerShell and cmd, sets their working directory and environment, and captures stdout/stderr back
   into the conversation. Processes cannot be created outside an AppContainer, and a sandboxed app does
   not inherit the user's PATH, environment, git/SSH credentials or package-manager state.
3. **Match the user's own development environment.** Commands must use the same `%USERPROFILE%`
   configuration the user sees in their terminal (`.gitconfig`, `.ssh`, npm/pip globals). Sandboxed
   redirects produce misleading results.
4. **System proxy, machine certificate store and loopback access.** Corporate networks and self-hosted
   gateways rely on the system proxy and root certificates, and users may point the app at a local
   inference server such as `http://localhost:11434`.

**How the capability is constrained**

- No background service, no startup task, no driver, no elevation: the app runs as the signed-in user
  only, for as long as its window is open.
- File and command scope is the workspace folder the user explicitly selects in the app; it does not
  scan or modify anything outside it.
- Every command and file write passes a risk engine: the command and its risk level are shown to the
  user and nothing executes until the user approves it in a dialog. Read-only/safe operations may run
  directly. The setting that auto-approves the highest risk level is off by default.
- Executed commands and tool calls are recorded in a local log with a configurable retention window
  that the user can inspect and clear.
- No telemetry; user files are never uploaded; prompts go only to the endpoint the user configured.
- `runFullTrust` is the only restricted capability declared in the manifest.

**Context**

This is the same justification used by Visual Studio Code, Windows Terminal and PowerToys: a packaged
Win32 desktop app that needs user-level access to the file system and to external processes, with no
additional privilege outside the container. Test credentials and steps are in "Notes for certification".
