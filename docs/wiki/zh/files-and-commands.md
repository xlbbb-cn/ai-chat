这三个工具工作在不同层级：

| 工具 | 适合 | 不适合 |
| --- | --- | --- |
| `file_actions` | 读/写/搜索/patch/改名/删除工作区文件 | 访问工作区外；运行命令 |
| `run_cmd` | 运行一个经 PATH 解析的程序或工作区脚本，如 `git status`、`npm test` | 管道、重定向、`&&`、变量展开、绝对路径 |
| `run_shell` | PowerShell/Bash 脚本、管道、循环、构建、后台 serve | 绕过风险评估 |

三者都从**工作区根目录**启动。`run_shell` 脚本自己可以 `cd` 到别处，但路径操作仍受风险评估约束；`file_actions` 没有「在工作区外」这种出口。

## 三个作用域一览

| 工具 | 能到哪 | 审计 |
| --- | --- | --- |
| `file_actions` | 工作区根目录，**包括**隐藏的 `.data`（技能、记忆、待办、技能状态、脚本、MCP 名单）。唯一例外是工作区数据库 `.data/chat.db`，不可修改。 | 路径沙箱——无越界出口 |
| `run_cmd` | 一个由操作系统经 `PATH` 解析的程序——裸名如 `python`、`python3`、`git`，或工作区脚本库里的一条脚本。绝对路径与相对路径都会被**拒绝**。 | 风险 L0–L6 + 确认 + 技能命令白名单 |
| `run_shell` | 任意位置，含工作区外 | 风险 L0–L6 + 确认 + 语法闸门 |

## `file_actions`

### 动作

| 动作 | 必填 | 说明 |
| --- | --- | --- |
| `read` | `path` | 读文件；可选 `start_line` / `end_line`（1 起，闭区间） |
| `write` | `path`、`content` | 覆盖写；自动创建父目录 |
| `list` | `path` | 列目录（含 `.data`） |
| `search` | `path`、`query` | 内置搜索引擎，不需要外部 `rg` |
| `mkdir` | `path` | 递归创建目录 |
| `patch` | `path`、`patch` | 应用 unified diff；按内容定位，不靠行号 |
| `diff` | `path`、`new_path` | 比较两个文件；返回增删计数与 diff |
| `rename` | `path`、`new_path` | 重命名文件/目录 |
| `move` | `path`、`new_path` | 移动文件/目录 |
| `delete` | `path` | 删除文件或整个目录（递归） |

路径相对工作区根目录。推荐写法：

```text
./src/App.tsx
./logs/build.log
.
```

只有当绝对路径确实位于工作区根目录内时才被接受；`../`、`C:\Windows`、`/etc` 此类会被拒绝。

临时与草稿输出放在 `.data/tmp/`（直接放在该目录下的普通文件一天后过期，长期产物应放进子目录）；子智能体交付物落在 `.data/deliver/<session-id>/`。

### 路径与保护规则

- 工作区根目录本身不可写、不可改名、不可删除。
- `.data/` **在工作区内**，所以它和普通目录一样可列、可搜、可读、可写——助手可以查自己的技能、记忆、脚本与待办，并直接管理。
- 唯一例外是工作区数据库（`.data/chat.db` 及其 `-wal` / `-shm` / `-journal` 边车文件）：`file_actions` 对它只能截断或删除、无法重写，因此被拒绝。丢失它会毁掉聊天历史、请求监控、交互日志与智能体任务。
- 唯一的技能根 `./.data/skills` 不能整体删除——请针对某个技能目录操作。
- 技能根下的每个文件或目录在被覆盖或删除前都会生成 `.bak.N` 备份：文件得到 `skill.md.bak.1`，整个目录得到 `demo.bak.1`。备份是安全网而不是权限——任何模式下都会创建。
- 路径会做规范化与软链检查；指向工作区外的软链会被拒绝。解析对路径**形状**宽松（`./` 前缀、重复分隔符、`<root>/ai-chat/src` 这类重复命名都能容忍），对**越出根目录**绝不宽松。

### 搜索参数

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `recursive` | `true` | 递归子目录 |
| `case_sensitive` | `false` | 区分大小写 |
| `smart_case` | `false` | 查询里出现大写即切换为区分大小写 |
| `use_regex` | `false` | 把 `query` 当正则 |
| `glob` | 无 | 文件过滤，如 `**/*.rs`、`src/**/*.ts` |
| `include_hidden` | `true` | 包含隐藏文件 |
| `respect_gitignore` | `false` | 遵循 `.gitignore` / `.ignore` / git exclude |
| `max_results` | `1000` | 最大命中行数，达到即提前结束 |

大仓库推荐：

```json
{
  "action": "search",
  "path": ".",
  "query": "TODO",
  "glob": "src/**/*.ts",
  "respect_gitignore": true,
  "max_results": 200
}
```

### Patch

`patch` 使用 unified diff：

```diff
@@ -10,7 +10,7 @@
 function greet(name: string) {
-  return `Hello ${name}`;
+  return `Hello, ${name}!`;
 }
```

规则：

- **按内容定位**：`@@` 行号只是提示，上下文行决定 hunk 落在哪。
- 每处改动带约 3 行上下文。
- 上下文/删除/新增行必须与文件逐字符一致，包括缩进与空格。
- 一处连续改动一个 hunk；相距较远的改动拆成多个 hunk。
- `---` / `+++` / `diff --git` 头部可选。
- LF 与 CRLF 都能用。
- 匹配只容忍行首尾空白差异。
- 失败时报错会显示最接近的真实内容，重读文件修正上下文后重试。
- 大段重写用 `write`，不要手搓 patch。

### Diff

```json
{
  "action": "diff",
  "path": "./src/old.ts",
  "new_path": "./src/new.ts",
  "context_lines": 3,
  "ignore_whitespace": false
}
```

返回 `+N / -M lines across K hunk(s)` 与完整 diff。

## `run_cmd`

直接运行一个程序，不经过 shell。

```json
{
  "action": "run",
  "command": "git status --short",
  "timeout_seconds": 30
}
```

行为：

- **第一个词分两步解析**：先查**工作区脚本库** `<workspace>/.data/scripts`——裸名，扩展名可选（`build-frontend`、`deploy.py`）；脚本库会被前置到子进程 `PATH`，所以同名时脚本优先。否则必须是操作系统经 `PATH` 解析的裸程序名——`python`、`python3`、`git`、`npm`、`node`、`cargo`、`curl`。绝对路径（`C:\Python\python.exe`、`/usr/bin/python3`）与相对路径（`.\build\tool.exe`、`sub/tool`）在风险评估之前就被**拒绝**。确实需要路径时改用 `run_shell`。
- 第一个词是程序，其余是参数。
- 支持引号与反斜杠转义。
- **不支持**管道、重定向、`&&`、`||`、`$VAR`、通配展开——这些走 `run_shell`。
- 工作目录：工作区根目录。
- 默认超时 30 秒，最大 3600。
- 有技能激活时，可执行文件必须在 `allowed-commands` 并集里。
- 执行前跑 L0–L6 风险评估；L0–L3 需要确认。

适合：`git`、`npm`、`node`、`cargo`、`python`、`curl`。

### 工作区脚本库

`run_cmd` 维护 `<workspace>/.data/scripts` 下的脚本库（启动时创建，整个工作区共享）：

```json
{ "action": "list" }
```

- `list` 列出每条脚本的解释器、`@description` / `@usage` 头部注释与使用次数。元数据只从头部注释里读（`#`、`//`、`REM`、`::`）。
- 脚本库会写进系统提示词，所以模型会复用已有脚本而不是重敲一长串命令。约定：一条多步命令成功两次以上、或你要求保留时，就存成脚本；Windows 上用 `.ps1`，Linux/macOS 上用 `.sh`；保持参数驱动、自包含；绝不在里面放凭据。
- 脚本正文若含破坏性或敏感模式，会一次性请求确认；该批准对这份确切内容记到文件变化为止。
- 脚本就是普通文件：用 `file_actions` 管理，纳入版本控制；配置方案备份包含 `workspace/scripts/`。

## `run_shell`

在构建时确定的 shell 里运行脚本。工具 schema 会写明宿主系统与固定语言，因此**没有 shell 类型参数**：Windows 上写 PowerShell，Linux/macOS 上写 Bash。

```json
{
  "run_type": "direct",
  "code": "Get-ChildItem -Recurse -Filter *.ts | Select-String 'TODO'",
  "timeout_seconds": 60
}
```

参数：

| 参数 | 说明 |
| --- | --- |
| `run_type` | `direct`（默认，等退出）或 `serve`（后台进程） |
| `action` | 仅 serve：`start` / `stop` / `list` |
| `serve_id` | serve 进程 id；`stop` 必填；`"all"` 停止全部 |
| `code` | 脚本正文；`direct` 与 `serve start` 必填 |
| `sudo` | 仅 Bash：整段脚本经 `sudo` 执行（会要求凭据） |
| `elevated` | 仅 PowerShell：请求管理员提权（UAC） |
| `timeout_seconds` | direct 超时，默认 30，最大 3600 |

行为：

- 脚本以 `Set-Location` / `cd` 到工作区根目录开始。
- **脚本有完全自由**：可以 `cd` 到任何地方、运行任何东西。唯一闸门是风险引擎——每段脚本都打分，L0–L3 执行前确认。
- 输出原样返回（`run_shell` **不**剥离 ANSI 颜色码；cli 子智能体会剥离）。
- 超时、停止、应用退出都会杀掉进程树。
- Windows 上隐藏窗口；Unix 上独立进程组。
- 执行前有语法闸门：括号或引号不匹配会被拦下并报出行/列，而不是真的执行。
- 需要更高权限时用 `sudo` / `elevated`：脚本在调用里声明，而不是假定应用本身是高权限运行。

### 后台 serve

```json
{
  "run_type": "serve",
  "code": "npm run dev -- --host 127.0.0.1"
}
```

返回 `serve_id`。随后：

```json
{ "run_type": "serve", "action": "list" }
```

```json
{ "run_type": "serve", "action": "stop", "serve_id": "serve-..." }
```

重要：**serve 进程只活在当前这一轮对话里。** 这一轮结束（正常完成、停止、出错）时，应用杀掉整个进程树并校验退出。应用退出也会清理。

适合：

- 为本轮启动一个服务，然后用 `run_cmd` 请求它、跑测试或收日志；
- 在本轮结束前显式停掉。

不适合：

- 需要跨用户轮次存活的开发服务器。那种请在应用外启动。

运行中的 serve 进程会显示在记录的**后台**卡片里（pid、运行时长、停止按钮），与同一条回复的后台任务并列；模型也可以用上面的 `serve` 动作列出与停止。

## 后台任务（`run_in_background`）

`run_shell` 的 serve 是*为这一轮*保住一个进程；后台任务正相反——它让**模型继续干活**，同时一个调用在后面跑完。

打开 **设置 → 运行时与调试 → 后台执行**后，下列调用接受 `run_in_background: true`：

```json
{ "action": "run", "command": "cargo test", "run_in_background": true }
```

```json
{ "run_type": "direct", "code": "npm run build", "run_in_background": true }
```

```json
{ "action": "search", "path": ".", "query": "TODO", "glob": "**/*.ts", "run_in_background": true }
```

```json
{ "agent_id": "reviewer", "task_description": "审计 src-tauri/src/mcp.rs 的阻塞调用", "run_in_background": true }
```

| 工具 | 支持的后台形式 |
| --- | --- |
| `run_cmd` | 任意调用 |
| `run_shell` | 仅 `run_type: "direct"`（`serve` 有自己的 list/stop） |
| `file_actions` | 仅 `action: "search"` |
| MCP 工具 | 任意调用；stdio 服务器拿到独立短命进程 |
| `call_subagent` | 任意调用 |

### 模型看到什么

1. 工具结果是带 `bg-…` id 的占位，模型被告知先去做别的。
2. 真实输出在它**下一次请求**前以 `user` 消息注入，前缀 `[BACKGROUND TASK RESULT …]`，并保存在助手消息行上，重载或切换会话后仍在。
3. 若模型在任务还在跑时想收尾，这一轮会被挂住（最多 3 轮 × 120 秒），之后剩余任务被取消。
4. 结果**不会**作为消息追加进聊天记录。进度与结果在回复末尾的**后台**卡片里可见——模型也能看到。

### 管理任务

模型会拿到 `background_task` 工具：

| 动作 | 参数 | 效果 |
| --- | --- | --- |
| `list` | — | 本会话运行中与已完成的任务（id、工具、状态、耗时） |
| `wait` | `task_ids`？、`timeout_seconds`？ | 阻塞到指定（或全部）任务结束，返回已可用的结果 |
| `get` | `task_id` | 取某个已完成任务的完整结果 |
| `cancel` | `task_id` 或 `"all"` | 杀掉进程树并丢弃结果 |

你也可以在回复末尾的**后台**卡片里取消——每个任务一个「取消」按钮，每个 serve 进程一个「停止」；`list_background_tasks` / `cancel_background_task` 是那个界面的后端。

### 限制与清理

- 后台 `run_cmd` / `run_shell` 走带超时的 runner：可取消、杀进程树、默认 **600 秒**。
- 后台 `file_actions` 搜索被取消时会在并行遍历器内部中止。
- 后台 stdio MCP 调用使用独立的短命进程，因为共享池在一把互斥锁上串行、单次往返上限 10 秒。
- 确认对话框串行处理；被取消的任务不再等待确认。
- 任务是**按轮**的：`end_turn_cleanup`、停止、应用退出都会置位所有取消标记并杀掉各自进程树。任务绝不会在之后的轮次里继续。

### serve 与后台任务

| | `run_shell` serve | `run_in_background` |
| --- | --- | --- |
| 目的 | 保住长驻进程（开发服务器） | 做慢活而不阻塞模型 |
| 返回 | `serve_id`，进程继续跑 | `bg-…` id，输出稍后到达 |
| 生命周期 | 到这一轮结束 | 到它完成、被取消或这一轮结束 |
| 交互 | `serve` list/stop | `background_task` list/wait/get/cancel |

## 示例

### 摸清一个项目

```text
file_actions list path="."
file_actions search path="." query="fn main" glob="**/*.rs" respect_gitignore=true
```

### 改一个函数

1. `file_actions read ./src/utils.ts` 读原文。
2. 用真实上下文行 `file_actions patch`。
3. `run_cmd` 跑 `npm test`。
4. 失败就 `read` 失败区域再 patch。

### 启服务并验证

1. `run_shell serve` 启动 `npm run dev -- --host 127.0.0.1`。
2. `run_cmd` 跑 `curl -s http://127.0.0.1:5173`。
3. `run_shell serve action="list"` 看输出。
4. `run_shell serve action="stop"` 停止。

### 抓网页

```json
{ "action": "run", "command": "curl -s https://example.com", "timeout_seconds": 30 }
```

多页抓取或正文提取建议用 MCP fetch 服务器（见 [MCP 服务器](MCP服务器)）。

## 常见错误

| 错误 | 原因 / 处理 |
| --- | --- |
| `Path ... is outside the workspace root` | `file_actions` 出不了工作区；改用 `run_shell` 并接受风险提示 |
| `The workspace database (.data/chat.db) is managed by the application` | 聊天数据库是 `file_actions` 唯一不能改的文件；请用应用自带的历史/监控面板 |
| `The skills root itself cannot be modified or deleted` | 删某个技能目录（`.data/skills/<name>`），不是技能根 |
| `run_cmd only runs programs that PATH resolves` | 用裸程序名（`python`），或需要路径时改用 `run_shell` |
| `Command 'xxx' is not in this skill's allowed-commands list` | 技能命令白名单里没有这个可执行文件 |
| `Syntax gate blocked` | 脚本语法错误；按报出的行/列修正 |
| `Execution rejected by user` | 你拒绝了确认；模型能看到原因 |
| 命令超时 | 调大 `timeout_seconds`（最大 3600），或改用 serve |
| `run_cmd` 里管道不生效 | 用 `run_shell` |
| serve 进程下一轮就没了 | 设计如此：serve 进程树在轮末被杀 |
| 工具 schema 里没有 `run_in_background` | 打开 **设置 → 运行时与调试 → 后台执行** |
| 后台任务一直没送结果 | 它被取消了，或这一轮结束了；在当前轮重新启动 |
| 这一轮一直等待 | 还有任务在跑：等它，或从后台卡片 / `background_task action="cancel"` 取消 |
| 输出被截断 | 工具结果保留前 6,000 + 后 2,000 字符；缩小查询范围或分段读 |

## 相关页面

- [工具总览](工具总览)
- [安全与边界](安全与边界)
- [工作区与数据](工作区与数据)