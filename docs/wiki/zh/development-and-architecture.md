写给想读代码、改代码或发布版本的人。普通用户不需要这一页。

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | React 19、TypeScript、Vite |
| 桌面外壳 | Tauri 2（`decorations: false`，自绘标题栏） |
| 后端 | Rust，Tauri 命令系统 |
| HTTP / 流式 | `reqwest` + `futures-util`（SSE） |
| 数据库 | `rusqlite`（SQLite） |
| 序列化 | `serde` / `serde_json` / `serde_yaml` |
| PDF → Markdown | `pdf2md-rs`（工作区成员 crate，进程内） |
| 知识图谱客户端（库，非工具） | `neo4rs`（Neo4j Bolt） |
| 打包 | Tauri bundler、MSIX（`makeappx`）、GitHub Actions |

crate/二进制名：`rabbit-chat-studio`（`src-tauri/Cargo.toml`），库目标 `rustgui_lib`；npm 包名与应用 id 同为 `rabbit-chat-studio`。

## 架构

```text
┌────────────────────────────── React 19 (src/) ──────────────────────────────┐
│  App.tsx  ChatMessage  HistoryPanel  SettingsPanel  SkillsPanel  ToolsPanel  │
│  McpPanel  AgentsPanel  MonitorPanel  ProfilePanel  UpdatePanel  TitleBar    │
└───────────────┬───────────────────────────────────────────────┬─────────────┘
                │ invoke(command, args)                         │ listen(event)
                ▼                                               ▲
┌──────────────────────────── Tauri 2 (src-tauri/src/) ─────────┴─────────────┐
│ lib.rs             命令注册 / AppState / 工作区 / 菜单 / 更新                 │
│ llm_complete.rs    Chat Completions（SSE）、工具循环、压缩、停止              │
│ llm_complete/      轮次循环、上下文压缩器、流式、结论蒸馏                      │
│ turn_guard.rs      轮次预算（请求数/工具调用数）+ 工具失败守卫                │
│ agents.rs          子智能体、规划、执行、任务状态                             │
│ agents/cli.rs      本地 CLI 子进程                                            │
│ agents/cloud.rs    远程 HTTP 端点 + 轮询                                      │
│ agents/files.rs    文件型 `llm` 定义（Markdown frontmatter）                  │
│ agents/exchange.rs `.data/deliver` 交付物交换区                                   │
│ agents/mbus.rs     任务总线：发布/订阅 + 实时状态登记                          │
│ agents/supervisor.rs 规划器监督器（事件驱动，仅总线工具）                      │
│ instance.rs        单实例 / 影子窗口（配置分叉）                              │
│ pdf_mentions.rs    `@paper.pdf#pages` → Markdown（经 pdf2md-rs）              │
│ mcp.rs             stdio / SSE / HTTP / Streamable HTTP                       │
│ skills.rs          SKILL.md 解析、技能状态                                     │
│ workspace.rs       <工作区>/.data 布局                                        │
│ backup.rs          导出/导入 zip                                              │
│ model_metadata.rs  合并模型目录（models.dev + OpenRouter + llm-metadata）      │
│ update.rs          更新检查/下载                                              │
│ logger.rs          文件/终端日志                                              │
│ process_guard.rs   子进程登记与进程树清理                                     │
│ tools/             文件、命令、patch、搜索、风险、记忆、待办、                 │
│                    定时器、后台任务、自建 MCP（mcp_self）                     │
│ db.rs              SQLite：history / api_requests / interaction_log / missions│
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       ▼
                         <工作区>/.data/chat.db + 文件状态
```

### 一次对话轮次的生命周期

1. 前端调用 `invoke("chat_completion", …)`。
2. 后端为**本轮**安装新的 `Arc<AtomicBool>` 取消标志；上一个标志永久保持 `true`。
3. 若编排开启且有启用的智能体：`agents::orchestrate` 规划 → 执行子智能体 → 把任务报告注入主上下文。
4. 组装消息：系统提示、已加载技能、历史、会话摘要、待办、任务报告、最后一条用户消息。
5. 可能先跑预测式压缩（见 [提示词与上下文](Prompts-and-Context)）。
6. `POST {base_url}/chat/completions`，`stream: true`。
7. 解析 SSE；后端发出 `chat-token` / `chat-reasoning-token` / `chat-usage`。
8. 模型返回 `tool_calls` 就跑工具，结果以 `tool` 消息追加，循环继续。
9. **目标循环**在待办仍有开放条目时可能继续本轮（最多 4 次续跑），除非有待触发的定时器或 `[WAIT_EVT: …]` 使其挂起。
10. 成功发 `chat-done`，失败发 `chat-error`。控制标记在落库前被剥离。
11. 消息、工具调用与 token 写入 SQLite。

### 停止

- `stop_chat_completion` 置位**本轮**标志。
- CLI 智能体每 250 ms 轮询并杀掉自己的进程树。
- `llm` 智能体在循环边界退出。
- 云端请求停止等待。
- 剩余任务标为跳过；任务状态变 `stopped`。
- 规划/执行阶段在步骤之间检查，不会取消已在飞行中的 LLM 请求。
- 新轮次安装新标志，所以空闲时按 Stop 不会影响下一轮。

### 主要事件

| 事件 | 方向 | 用途 |
| --- | --- | --- |
| `chat-token` | 后端 → 前端 | 答复流 |
| `chat-reasoning-token` | 后端 → 前端 | 推理流 |
| `chat-usage` | 后端 → 前端 | token（本轮费用芯片由此推算） |
| `chat-retry` | 后端 → 前端 | 断流后正在重试 |
| `chat-done` / `chat-error` | 后端 → 前端 | 结束 / 错误 |
| `chat-injected` | 后端 → 前端 | 中途更新或后台结果被附加到助手消息 |
| `chat-notice` | 后端 → 前端 | 浮动警告/错误（预算耗尽、工具被禁用等） |
| `tool-call` | 后端 → 前端 | 工具状态文本 |
| `todo-state` | 后端 → 前端 | 待办实时更新 |
| `skill-loaded` | 后端 → 前端 | 技能已加载 |
| `timer-state` / `timer-fired` | 后端 → 前端 | 定时器 |
| `sleep-lock` | 后端 → 前端 | 系统唤醒锁状态（输入框上方徽标） |
| `agent-plan-start` / `agent-task-start` / `agent-task-token` / `agent-task-reasoning` / `agent-task-tool` / `agent-task-done` / `agent-task-error` / `agent-planner-state` / `agent-planner-action` / `agent-aggregate-done` | 后端 → 前端 | 编排进度——任务行自带状态，规划决策以广播行流式呈现 |
| `confirm-required` | 后端 → 前端 | 风险确认 |
| `workspace-changed` | 后端 → 前端 | 工作区切换 |
| `update-download-progress` | 后端 → 前端 | 更新下载 |

## 设计原则

核心意图：从第一性原理出发推理，把自由还给用户（工具、LLM API、提示词），界面保持克制专业。

1. **最小依赖闭包**：所有模型调用都走 OpenAI 兼容 API。
2. **能力与权限解耦**：默认只是聊天；工具按需开启；技能进一步收窄。
3. **本地优先可观测**：配置、历史、请求日志都留在本地。
4. **显式边界**：技能目录隔离、工具白名单、工作区路径约束。
5. **可替换**：前端、命令层、模型服务与图后端都是模块化的。

落到代码：

- `get_all_tools(selected_tools)` 决定工具表；
- `execute_tool(..., allowed_commands, ...)` 是唯一执行入口；
- `resolve_safe_path` / `ensure_mutation_target_allowed` 是路径边界；
- `process_guard` 是进程边界；
- `workspace.rs` 是状态边界。

## 项目结构

```text
ai-chat/
├─ src/                         React 前端
│  ├─ App.tsx                   主界面外壳
│  ├─ api.ts                    invoke/listen 封装
│  ├─ types.ts                  共享 TS 类型
│  ├─ fileMention.ts            `@文件` 弹窗查询/打分（行区间）
│  ├─ pathRef.ts                `&路径` 引用语法（不附加内容）
│  ├─ slashMention.ts           `/技能` 弹窗查询/打分
│  ├─ attachments.ts            发送时把 `@路径` 提及落成附件
│  ├─ components/               Chat/History/Settings/Skills/Tools/MCP/Agents/
│  │                            Profile/Monitor/Update/TitleBar/FileMentionPopup/
│  │                            SlashMentionPopup/BackgroundTaskGroup…
│  ├─ i18n/                     en / zh-CN / zh-TW
│  └─ utils/                    模型目录匹配（modelMetadata.ts）等
├─ src-tauri/
│  ├─ src/lib.rs                入口、命令、AppState、菜单
│  ├─ src/llm_complete.rs       SSE 对话、工具循环、压缩、停止
│  ├─ src/llm_complete/         turn.rs / context.rs / stream.rs / conclusion.rs
│  ├─ src/agents.rs             子智能体、编排、任务
│  ├─ src/agents/cli.rs         CLI 后端
│  ├─ src/agents/cloud.rs       Cloud 后端
│  ├─ src/agents/exchange.rs    `.data/deliver` 交付物交换区
│  ├─ src/agents/files.rs       `llm` 定义（Markdown 解析/序列化/扫描）
│  ├─ src/agents/mbus.rs        任务总线（发布/订阅 + 状态登记）
│  ├─ src/agents/supervisor.rs  规划器监督器
│  ├─ src/instance.rs           单实例 / 影子窗口
│  ├─ src/turn_guard.rs         轮次预算 + 工具失败守卫
│  ├─ src/pdf_mentions.rs       `@paper.pdf#pages` 转换 + 缓存
│  ├─ src/mcp.rs                MCP 客户端
│  ├─ src/skills.rs             SKILL.md 解析、技能状态
│  ├─ src/workspace_files.rs    `@` 提及用的工作区文件列表
│  ├─ src/workspace.rs          `.data` 布局
│  ├─ src/backup.rs             导出/导入
│  ├─ src/model_metadata.rs     合并能力目录
│  ├─ src/update.rs             更新检查/下载
│  ├─ src/logger.rs             文件/终端日志
│  ├─ src/process_guard.rs      子进程清理
│  ├─ src/tools/                工具实现
│  │  ├─ mod.rs                 派发/schema/测试
│  │  ├─ file_actions.rs
│  │  ├─ patch.rs
│  │  ├─ paths.rs
│  │  ├─ risk.rs
│  │  ├─ scripts.rs             run_cmd 脚本库（解析、元数据、批准）
│  │  ├─ search.rs
│  │  ├─ serve.rs
│  │  ├─ background.rs
│  │  ├─ memory.rs
│  │  ├─ todos.rs
│  │  ├─ timer.rs
│  │  └─ mcp_self.rs            自建 MCP 服务器（自我进化模式）
│  ├─ utils/pdf2md-rs/          PDF → Markdown 库 crate（工作区成员）
│  ├─ utils/knowledge_graph/    知识图谱客户端库（未接入工具集）
│  ├─ capabilities/default.json Tauri 权限
│  └─ tauri.conf.json
├─ scripts/
│  ├─ sync-version.cjs          三方版本同步
│  ├─ gen-changelog.cjs         由 git tag/提交生成 CHANGELOG.md
│  ├─ gen-wiki.mjs              docs/wiki/*.md → docs/site/wiki/**.html
│  ├─ build-msix.ps1            MSIX 打包
│  └─ capture-store-screenshots.ps1
├─ docs/
│  ├─ wiki/                     Wiki 源：nav.json + en/ + zh/
│  ├─ site/                     GitHub Pages（纯静态，无构建步骤）
│  ├─ gen.py                    CI 里生成 site/architecture.html
│  ├─ store/                    商店上架素材
│  ├─ DESIGN.md                 界面设计规范
│  └─ agents/                   开发文档：架构 / 技术栈 / 模块 / 界面设计 / 工作流
├─ .github/workflows/           pages.yml / release.yml / store.yml / winget.yml
├─ AGENTS.md                    文档索引（规则在 docs/agents/）
└─ CHANGELOG.md                 自动生成，勿手改
```

## 数据库

`<工作区>/.data/chat.db` 主要表：

| 表 | 内容 |
| --- | --- |
| `history` | 会话消息、工具调用、附件、推理 |
| `session_meta` | 标题、收藏、归档 |
| `session_summaries` | 压缩后的会话摘要 |
| `api_requests` | 每次 LLM 请求的指标 |
| `interaction_log` | LLM/工具/MCP 交互明细 |
| `agent_missions` | 子智能体任务 |
| `agent_tasks` | 任务内的子任务 |
| `agent_bus_messages` | 任务总线发布历史（任务面板回放用） |

## 开发命令

```bash
# 安装依赖
npm install
cargo install tauri-cli --version "^2"

# 开发（Vite :1420 + Tauri）
npm run tauri dev

# 仅前端
npm run dev

# 类型检查
npx tsc --noEmit

# 前端构建（不需要 git tag）
npx vite build

# Rust 检查
cargo check --manifest-path src-tauri/Cargo.toml

# Rust 单元测试
cargo test --manifest-path src-tauri/Cargo.toml

# 生产构建（需要最新 git tag）
npm run tauri build
```

文档与打包：

```bash
python3 docs/gen.py              # 生成 docs/site/architecture.html
node scripts/gen-wiki.mjs        # docs/wiki → docs/site/wiki
node scripts/gen-changelog.cjs   # 由 git 重新生成 CHANGELOG.md
pwsh scripts/build-msix.ps1      # MSIX 包（Windows + makeappx）
```

版本同步：

```bash
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch
```

## 发布流程

1. 合并进 `main`。
2. 打 tag 并推送：

   ```bash
   git tag v3.3.3
   git push origin v3.3.3
   ```

3. `.github/workflows/release.yml` 在 4 个 runner 上构建。
4. 第一个平台完成即发布 Release，其余随后上传。
5. `store.yml` 构建 `.msix` 工作流产物，不附在 Release 上。
6. `winget.yml` 暂未启用：只有包被 `winget-pkgs` 接受且存在 `WINGET_TOKEN` 时才提交。

见 [安装与构建](Installation-and-Build) 与 [更新](Updates)。

## 路线图

来自 README：

- 增加模型供应商预设（OpenAI / Azure / 本地网关）；
- 工具调用审计与回放；
- 技能签名与发布；
- 端到端测试与安全基线扫描。

## 贡献

- 改界面前先读 `docs/DESIGN.md`：只用 `src/App.css` 里的 token；4px 圆角、单一电光蓝强调色、无阴影、一条动效曲线。
- 改 Rust 工具时同步更新 schema 与测试。
- 新增 Tauri 插件时：加 crate、注册 `.plugin(...)`、把权限写进 `capabilities/default.json`（Tauri 2 默认拒绝）。
- Wiki 页在 `docs/wiki/{en,zh}/*.md`；`docs/site/wiki/` 下的 HTML 是生成产物，不要提交。改完跑 `node scripts/gen-wiki.mjs`。
- 提交前跑 `npx tsc --noEmit` 与 `cargo check`；工具逻辑跑 `cargo test`。
- 绝不提交真实 API 密钥或用户数据。

## 许可

**MIT**。见仓库中的 `LICENSE`。

## 相关文档

- `README.md` / `README.zh.md`：项目介绍与快速开始
- `AGENTS.md`：开发文档索引（架构、技术栈、模块、界面设计、工作流）
- `docs/agents/`：开发文档本体（架构、技术栈、模块、界面设计、工作流）
- `docs/DESIGN.md`：界面设计规范
- `config.example.md`：配置字段示例
- `CHANGELOG.md`：自动生成的更新记录
