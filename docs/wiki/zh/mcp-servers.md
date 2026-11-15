MCP（Model Context Protocol）服务器把外部工具与数据源接进对话。启用后，它的工具会以普通函数的形式出现在模型的工具表里，可以像内置工具一样调用。

配置有两份清单，读取时合并：

- 你在面板里创建的服务器存在应用数据目录的 `mcp_servers.json`，**不随工作区切换**；
- 助手自己搭的服务器（仅自我进化模式）存在 `<工作区>/.data/mcp.conf`，脚本放在旁边，**随工作区切换**。

id 冲突时面板条目优先。无论哪种，stdio 服务器都以**当前工作区**为工作目录运行。

## 面板与生命周期

顶部工具栏打开 **MCP**：

| 元素 | 用途 |
| --- | --- |
| 开关 | 启用/禁用；启用 stdio 立即启动，禁用即停止 |
| 名称 | 显示名与日志标识 |
| 传输摘要 | `stdio: 命令 参数` 或 `transport: url` |
| `self-built` 徽标 | 标记助手在自我进化模式下自建的服务器；编辑/删除只写工作区的 `mcp.conf`，绝不写应用数据 |
| ⚡ 测试连接 | 发一次真实的 `initialize` 握手；自动打开日志 |
| 日志 | 该服务器的诊断日志（每 1.5 秒刷新），可清空 |
| 编辑 / 删除 | 修改或移除 |
| + 添加服务器 | 新建 |

其他行为：

- 启动时会自动拉起所有**已启用的 stdio** 服务器（预热）。
- 切换工作区会停掉运行中的 stdio 子进程；下一次工具调用在新工作区重新拉起。
- 退出应用、关窗、崩溃都会清理 stdio 子进程（Windows 用 job object，Unix 用进程组）。
- HTTP/SSE/Streamable HTTP 服务器没有常驻子进程，每次调用现连。

## 传输方式

| 传输 | 适用 | 配置 |
| --- | --- | --- |
| `stdio` | 本地可执行文件 / `npx` / `uvx` | command、args、env |
| `sse` | 旧式 HTTP + Server-Sent Events | url、auth_token |
| `http` | 纯 JSON-RPC POST | url、auth_token |
| `stream-http` | Streamable HTTP（推荐） | url、auth_token |

### stdio

| 字段 | 说明 |
| --- | --- |
| 命令 | 可执行名或路径，如 `npx`、`uvx`、`python`、`./server` |
| 参数 | **空格分隔**，如 `-y @modelcontextprotocol/server-filesystem .` |
| 环境变量 | 每行一个 `KEY=value`，如 `API_KEY=abc123` |

参数输入框按空白切分，因此单个参数不能含空格。确实需要时用包装脚本，或直接改 JSON 配置。

工作目录：

- 子进程的 `current_dir` 就是当前工作区。
- 同时设置 `PWD`，让依据 `PWD` 推断项目根的 CLI 不会看到应用的启动目录。
- 因此 `./server`、`./data.db` 和 `.` 都相对工作区解析。

Windows 上 `npx` / `npm` / `pnpm` / `yarn` / `uvx` / `conda` 以及 `.cmd` / `.bat` 会经 PowerShell 包装，以便解析 PATH；Unix 上经登录 shell，以便带上 `nvm`、Homebrew、pyenv 等 PATH 设置。

### SSE / HTTP / Streamable HTTP

| 字段 | 说明 |
| --- | --- |
| URL | SSE 通常是 `http://host/sse`；HTTP 是 JSON-RPC 端点；Streamable HTTP 通常是 `http://host/mcp` |
| 认证令牌 | 可选，以 `Authorization: Bearer <token>` 发送 |

- SSE：先开事件流，等 `endpoint` 事件，再向该端点 POST JSON-RPC。
- HTTP：直接 POST JSON-RPC 并读 JSON 响应。
- Streamable HTTP：`initialize` 后保留会话 id，再做 `tools/list` / `tools/call`；响应可能是 JSON 或 SSE 流。

## 添加一个 stdio 服务器

以官方 filesystem 服务器为例：

1. **MCP** → **+ 添加服务器**。
2. 名称：`filesystem`。
3. 传输：`stdio (subprocess)`。
4. 命令：`npx`。
5. 参数：`-y @modelcontextprotocol/server-filesystem .`
6. 环境变量：留空。
7. 保存。
8. 打开开关；首次运行可能要下载包，会慢一点。
9. 点 ⚡ **测试连接**，日志里应看到初始化成功。
10. 在对话里说："列出工作区根目录的文件。"

> `.` 指当前工作区根目录。filesystem 服务器只在你给的目录内活动；除非确实需要，不要指向整个磁盘。

### 其他示例

| 服务器 | 命令 | 参数 |
| --- | --- | --- |
| Fetch（抓网页） | `uvx` | `mcp-server-fetch` |
| SQLite | `uvx` | `mcp-server-sqlite --db-path ./data.db` |
| Memory（图记忆） | `npx` | `-y @modelcontextprotocol/server-memory` |
| GitHub | `npx` | `-y @modelcontextprotocol/server-github`；环境里放 `GITHUB_PERSONAL_ACCESS_TOKEN=...` |

上游的包名与参数会变，请对照服务器 README。先测连接。

## 自建服务器（自我进化模式）

开启 **设置 → 自我进化模式** 后，助手会得到 `mcp_self` 工具，可以自己搭一个小型 stdio 服务器：

- 脚本写在 `<工作区>/.data/mcp/<slug>/`（更新时保留 `.bak.<n>` 备份），并注册到工作区清单 `<工作区>/.data/mcp.conf`；
- 后端从白名单（`python` / `node` / `shell`）挑解释器——模型只能把工作区里的代码变成服务器，无法注册任意命令；
- 动作：`list` / `write` / `enable` / `disable` / `test` / `restart` / `logs` / `remove`。写入、启用、移除各确认一次（`user_software` 类，可在 设置 → 自动接受 里放行）；
- `tools/list` 握手失败的条目保持禁用，坏服务器不会拖慢后续请求——`logs` 返回日志尾部，让模型自己改脚本；
- 清单变化后工具表在下一个请求边界重建，因此*写入 → 启用 → 调用*可以在同一轮完成；
- 子智能体与主智能体辅助器拿不到 `mcp_self`——它们会争抢清单，确认弹窗也会卡住任务。

面板里这些行带 `self-built` 徽标；编辑或删除只写工作区文件。你也可以手改 `<工作区>/.data/mcp.conf`，再用服务器的 **restart**（或面板编辑）让它生效。

## 添加远程服务器

1. **+ 添加服务器**。
2. 名称：`my-remote`。
3. 传输：**Streamable HTTP（推荐）**。
4. URL：如 `https://mcp.example.com/mcp`。
5. 认证令牌：服务器要求 Bearer 认证时填。
6. 保存并启用。
7. 测试连接；失败时看日志里的 HTTP 状态与响应。

## 工具如何到达模型

启用服务器后，应用调用 `tools/list`，把每个工具转成 OpenAI 函数：

```json
{
  "type": "function",
  "function": {
    "name": "<MCP 原始工具名>",
    "description": "<描述> [MCP: <服务器名>]",
    "parameters": { "...": "inputSchema" }
  }
}
```

注意：

- 工具名**不**加服务器名前缀，沿用 MCP 服务器自己声明的名字。
- 两个服务器暴露同名工具时调用会有歧义；只启用需要的，或换一个不冲突的实现。
- 应用负责把调用路由到对应服务器并执行 `tools/call`。
- 每次调用都会写一条 MCP 诊断日志和一条交互日志。

### 进程归属

| 调用方 | 进程池 |
| --- | --- |
| 主对话 | 一个预热池（`mcp_children`），启动时拉起并复用；切换 `workspace_dir` 会停掉 stdio 子进程，下次调用在新工作区重拉 |
| 带 `enable_mcp` 的 `llm` 子智能体 | 每个任务自己的 `AgentMcpSession`——任务开始拉起，成功、报错、达到最大迭代或被 Stop 时关闭 |
| 后台 MCP 调用（`run_in_background`） | 每次调用一个短命独立进程，因为共享池串行在一把互斥锁上且单次往返上限 10 秒 |

两个池不共享子进程，因此主对话与并行任务不会互相卡住。

## 诊断日志

点服务器上的 **日志** 按钮：

- 记录保存、启动、`initialize`、`tools/list`、`tools/call` 与错误；
- 测试连接会自动打开日志；
- 每 1.5 秒刷新；
- 可手动刷新或清空；
- 内存环形缓冲，重启即清空。

典型内容：

```text
Stdio process started and initialized
LLM requested tools/list
tools/list returned 12 tools
LLM tools/call: read_file
tools/call failed: ...
```

## 安全边界

- stdio 服务器以你的用户权限运行，并不限制在工作区内。把 filesystem 指向工作区是约定，不是沙箱。
- 远程服务器会收到你发出的参数；不要在参数里塞不必要的密钥。
- `auth_token` 与 `env` 以明文保存在 `mcp_servers.json`。
- 只启用你信任的服务器；第三方 MCP 服务器可能执行任意代码。
- 后台 MCP 调用跑在独立短命进程里，但仍绑定当前轮次，任务取消或轮次结束即被杀掉。
- 删除服务器不会删除它在外部写入的数据。
- 应用只负责登记与清理服务器进程，远程侧数据不受影响。

## 常见问题

| 现象 | 排查 |
| --- | --- |
| 找不到命令（`npx` / `uvx`） | Node/Python 装了吗？Windows 的 PATH 对吗？看 MCP 日志 |
| 测试卡住 | 首次 `npx` 下载慢；看日志与网络/代理 |
| 日志报 `initialize` 失败 | 该命令真的是 MCP 服务器吗？是否打印了非 JSON 的横幅输出？ |
| 远程 401 | `auth_token`；以及服务器要求的其他头 |
| 远程 404 | URL 路径；SSE 与 Streamable HTTP 端点通常不同 |
| 工具列表为空 | 服务器实现 `tools/list` 了吗？启用了吗？看日志 |
| 工具重名 | 只启用一个同名服务器，或换实现 |
| 换工作区后第一次调用失败 | stdio 被停了，下一次调用会重启 |
| 自建服务器一直启用不了 | 启动时 `tools/list` 失败会保持禁用；打开 **日志** 修脚本后再启动 |
| 残留进程 | 正常退出会清理；崩溃由 Windows job object 兜底。可在任务管理器确认 |
| 参数里有空格 | 界面按空白切分；用包装脚本或改 `mcp_servers.json` |
