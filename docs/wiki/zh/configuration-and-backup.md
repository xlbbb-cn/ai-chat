Rabbit Chat Studio 的配置分两层：

- **应用级**：操作系统应用数据目录里的 `config.json`、`mcp_servers.json`、`sub_agents.json`、`profiles.json`。
- **工作区级**：`<工作区>/.data/`，含聊天数据库、技能、记忆、待办、定时器、技能状态、`run_cmd` 脚本库与工作区 MCP 清单。

## 应用数据目录

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\`（Roaming） |
| macOS | `~/Library/Application Support/com.leonard.aichat/` |
| Linux | `~/.local/share/com.leonard.aichat/` |

> 商店（MSIX）安装会被 Windows 重定向到
> `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat\`。

| 文件 | 内容 |
| --- | --- |
| `config.json` | 模型、API、工具、工作区、外观、日志保留、更新开关 |
| `mcp_servers.json` | MCP 服务器定义 |
| `sub_agents.json` | 子智能体 + 编排 |
| `profiles.json` | 应用内 **配置档** |
| `agents/` | `llm` 智能体定义（Markdown）与 `index.yaml` 发现清单 |
| `model_metadata.json` | 合并后的模型目录（约 2900 条、约 1.2 MB），最多每周刷新一次 |
| `shadow/` | 单实例锁 + 每个额外窗口一份 `config-<时间戳>-<id>/`（见[多窗口](#多窗口)） |
| `app.log` | 调试版日志（发布版默认不写） |
| `workspace/` | 默认工作区 |

## `config.json` 字段

### 模型与 API

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `api_base_url` | string | `https://api.openai.com/v1` | OpenAI 兼容根地址 |
| `api_key` | string | 空 | Bearer 密钥，明文存储 |
| `model` | string | `gpt-4o-mini` | 默认模型 |
| `model_catalog` | string[] | `["gpt-4o-mini"]` | 模型选择器条目 |
| `model_context_lengths` | object | `{}` | 按模型的上下文窗口，如 `{"gpt-4o-mini": 128000}` |
| `model_metadata` | object | `{}` | 从合并目录匹配到的能力；仅本地 |
| `model_reasoning_effort` | object | `{}` | 按模型的思考深度：`minimal` / `low` / `medium` / `high` |
| `model_settings` | object | `{}` | 温度、Top P、补全上限等 |
| `ds_format` | boolean | `false` | 为 DeepSeek 思考模型补 `reasoning_content`（模型 id 命中 DeepSeek 时自动开启） |
| `system_message` | string | 空 | 你的基础系统提示 |
| `conversation_id_header` | string? | 空 | 携带会话 id 的额外请求头，如 `X-Conversation-Id` |
| `custom_headers` | `{name, value}[]` | `[]` | 加到每个聊天请求上的额外头 |

### 模型目录

"获取模型"不只拉一个源。应用下载**三个**源，按*规范化*后的名字合并成一条：

| 顺序 | 来源 | 为什么 |
| --- | --- | --- |
| 1 | `models.dev` | 覆盖最广，且是唯一提供精确 token 上限的源 |
| 2 | OpenRouter | 补充 id 与上下文提示 |
| 3 | `llm-metadata` | 唯一提供 `tags`（`Audio` / `Files` / `Open Weights` / `Deprecated`）的源 |

值得知道的规则：

- 低优先级源只填空字段，并补充其他源缺失的模型；
- 某个源失败或不再能解析就跳过，其余照常合并；只有**全部**无响应且本地无文件时才报错；
- 名字统一小写并扁平化，所以拉到的 id 走直接比对即可匹配目录；相似度打分只用于目录不认识的 id（设置行会显示 `Matched <名称> · N% 相似`）；
- 合并结果缓存在 `<应用数据>/model_metadata.json`，最多**每周**刷新一次，因此重启、断网重启、镜像慢都还能用；
- 拉取期间选择器显示转圈。

`model_settings` 字段：

| 字段 | 说明 |
| --- | --- |
| `temperature` | 0–2 |
| `top_p` | 0–1 |
| `reasoning_effort` | 全局兜底；`model_reasoning_effort[模型]` 优先 |
| `max_complete_tokens` | 单条回复补全上限（推荐） |
| `max_tokens` | 旧字段；同时作为上下文窗口的兜底 |

### 工具与工作区

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `selected_tools` | string[] | `[]` | 启用的工具 id |
| `self_evolution_mode` | boolean | `false` | 允许助手演进自己的技能；强制开启 `file_actions` + `memory` |
| `background_tools` | boolean | `false` | 为 `run_cmd` / `run_shell` / `file_actions` 搜索 / MCP 工具 / `call_subagent` 下发 `run_in_background`（设置 → 运行时与调试 → 后台执行） |
| `parallel_tools` | boolean | `true` | 同一轮回复里的独立工具调用同时执行（设置 → 运行时与调试 → 并行工具调用）；`false` = 严格逐个执行 |
| `max_parallel_tools` | number | `4` | 单轮最多同时跑几个调用，限制在 1–8；`1` 等同关闭 `parallel_tools` |
| `workspace_dir` | string? | 空 | 工作区绝对路径；空 = 默认 |
| `workspace_history` | string[] | `[]` | 最近用过的工作区（新的在前，最多 10 个），在 设置 → 工作区 里作为选择器 |
| `main_agent_max_rounds` | number | `40` | 主智能体每轮可用请求数（设置 → 高级）；`0` 不限，上限 1000 |
| `main_agent_max_tool_calls` | number | `120` | 每轮工具调用数；`0` 不限，上限 5000 |
| `tool_failure_limit` | number | `3` | 连续失败多少次后该工具在本次会话余下时间被禁用；`0` 关闭 |
| `auto_accept_confirm_kinds` | string[] | `[]` | 自动接受的确认类型；`"*"` 全部 |

`selected_tools` 取值：

```text
file_actions, run_cmd, run_shell, memory, todo_list, timer
```

别名 `todo`。`todo_list` 以 `todos` 名义暴露给模型。已废弃 id（例如 `knowledge_graph`）在加载与保存时会被剔除。

> `self_evolution_mode: true` 会**强制**把 `file_actions` 与 `memory` 写进 `selected_tools`。每次保存与启动都会重写选择，因此工具面板、配置档、备份恢复或手改 `config.json` 都无法在模式开启时关掉它们。它同时解锁 `evolution` 记忆作用域，并允许助手在 `<工作区>/.data/skills` 下安装、编辑、删除技能。

`auto_accept_confirm_kinds` 取值：

```text
dangerous, system_config, user_software, user_data, external_agent, *
```

内置命令按风险等级 L0–L3 在运行前询问；自动接受对应类型（或 `*`）可在本次会话余下时间跳过询问。`external_agent` 不在工具面板的快捷项里，需要在这里设置。

### 外观、日志与更新

| 字段 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `theme` | string? | `auto` | `auto` / `light` / `dark` |
| `palette` | string? | `default` | 配色：`default` / `sakura` / `cream`；各有浅色与深色两套 token |
| `single_instance` | boolean | `false` | 仅发布版。开：第二次启动只聚焦已有窗口。关：每次启动都是一个窗口（见[多窗口](#多窗口)）。下次启动生效 |
| `language` | string? | 自动 | `en` / `zh-CN` / `zh-TW` |
| `logger_output` | string | `file` | `file` / `println`；仅调试版 |
| `log_retention_days` | number | `90` | 请求/交互日志保留天数；`0` 永久 |
| `check_updates_on_startup` | boolean | `true` | 启动时检查更新 |
| `include_prerelease_updates` | boolean | `false` | 包含预发布 |

### 完整示例

```json
{
  "api_base_url": "https://api.openai.com/v1",
  "api_key": "sk-...",
  "model": "gpt-4o-mini",
  "model_catalog": ["gpt-4o-mini", "gpt-4.1-mini"],
  "model_context_lengths": {
    "gpt-4o-mini": 128000
  },
  "model_settings": {
    "temperature": 0.7,
    "top_p": 0.95,
    "max_complete_tokens": 4096
  },
  "system_message": "你是一个乐于助人的助手。",
  "selected_tools": ["file_actions", "memory", "todo_list", "timer"],
  "background_tools": true,
  "parallel_tools": true,
  "max_parallel_tools": 4,
  "workspace_dir": "D:/projects/my-repo",
  "main_agent_max_rounds": 40,
  "main_agent_max_tool_calls": 120,
  "tool_failure_limit": 3,
  "theme": "auto",
  "language": "zh-CN",
  "log_retention_days": 90,
  "check_updates_on_startup": true,
  "include_prerelease_updates": false
}
```

> 改 `config.json` 前请完全退出应用，否则设置面板可能用内存里的值把它覆盖掉。

## 配置档（Profiles）

**设置 → 工作区与配置档 → 配置档** 保存的是一套**能力集**：

| 包含 | 不包含 |
| --- | --- |
| `selected_tools` | API 地址/密钥 |
| 子智能体列表 | 默认模型 |
| 编排设置 | 系统提示 |
| MCP 服务器 | 工作区目录 |
| — | 技能启用状态 |

适合：

- 在"只读分析""全量编码""启用 MCP 的工作流"之间切换；
- 为不同项目准备不同的智能体/MCP 组合；
- 备份 MCP 与子智能体配置。

操作：

- 输入名字 → **保存当前**；
- **应用** → 覆盖当前的工具/智能体/MCP/编排；
- 删除 → 从 `profiles.json` 移除。

技能启用状态属于工作区（`.data/skills.status`），配置档不碰它。这是有意为之。

## 完整导出与恢复

入口：

- 标题栏 `⋯` → **导出配置…** / **恢复配置…**；
- macOS 原生文件菜单；
- 命令 `app_export_profile` / `app_import_profile`。

导出期间会锁住发送按钮并显示进度，保证快照期间数据库不被写入。

### 压缩包内容

```text
config.json             AppConfig
profiles.json           命名配置档
mcp_servers.json        MCP 服务器
sub_agents.json         子智能体 + 编排
agents/...              `llm` 智能体定义（应用级，Markdown）
workspace/memory/...    当前工作区记忆
workspace/todos/...     当前工作区待办
workspace/skills/...    当前工作区技能
workspace/scripts/...   run_cmd 脚本库
workspace/mcp/...       自建 MCP 服务器脚本（自我进化模式）
workspace/mcp.conf      这些服务器的清单
workspace/deliver/...   子智能体交付物，每个会话一个目录
workspace/skills.status 技能启用状态
chat.db                 SQLite 快照（VACUUM INTO）
```

子智能体的交付物交换区（`<工作区>/.data/deliver/<会话 id>/`）**会**导出；删除会话时一并清理。

`chat.db` 是一致性的 `VACUUM INTO` 快照；生成期间应用会短暂禁止发送。

### 导入行为

导入会：

1. 读取并应用 `config.json`（可能切换工作区）；
2. 写入 `sub_agents.json`；
3. 写入 `profiles.json`（仅当它确实是配置档文件）；
4. **合并 `agents/**/*.md`** 到应用级 `agents/` 库（完全相同的文件不动）；
5. 把 `workspace/memory`、`workspace/todos`、`workspace/skills`、`workspace/scripts`、`workspace/mcp`、`workspace/mcp.conf` 恢复到**当前**工作区的 `.data`；
6. 恢复 `workspace/skills.status`。

目前**不会**恢复：

- `chat.db`

压缩包里虽然有它，但导入路径不写回。需要历史记录时：退出应用，把 `chat.db` 复制到 `<工作区>/.data/chat.db`。

导入也会写入应用数据目录的 `mcp_servers.json`。

> 这是当前行为，后续版本可能变化。升级后请核对导入结果。

### 手动完整备份

最可靠的做法是退出应用后把两处一起复制：

```text
<应用数据>/                    → config.json、mcp_servers.json、sub_agents.json、
                                 profiles.json、agents/、model_metadata.json、shadow/
<工作区>/.data/                → chat.db、memory/、todos/、skills/、skills.status、
                                 timers.json、scripts/、mcp/、mcp.conf
```

先退出，避免 SQLite 还有未落盘的 WAL。

## 多窗口

`single_instance` 决定第二次启动能否存活：

- **开**：第二次启动把参数交给运行中的实例，已有窗口被显示并聚焦。
- **关**：每次启动都是一个窗口。第一个进程是**主窗口**，继续共用 `config.json`；之后每个窗口都是**影子窗口**：

  - 启动时把当前配置复制到 `<应用数据>/shadow/config-<时间戳>-<id>/config.json`；
  - 副本里的 `workspace_dir` 被改写到它自己的 `workspace/` 分叉——**两个窗口绝不共享工作区**，因此不会争用同一个 SQLite 数据库；
  - 影子窗口只写自己的分叉；退出时副本被删除，只有 `workspace_history` 合并回共享配置（影子的临时工作区被过滤掉）；
  - 在影子里清空 `workspace_dir` 会回到它自己的分叉，而不是共享默认工作区；
  - 分叉窗口会在 设置 → 工作区 里说明；崩溃遗留的副本在下次启动时清理。

设置 → 工作区 列出最近用过的目录（`workspace_history`，新的在前，最多 10 个）。选中只是*暂存*，**保存更改**才真正切换。标题栏的工作区名带一条细红下划线，分叉窗口一眼可辨。

## 换到新机器

1. 旧机器：完全退出应用。
2. 旧机器：复制应用数据目录与当前工作区的 `.data`。
3. 新机器：安装同版本或更新版本。
4. 新机器：启动一次以创建默认目录。
5. 新机器：退出应用。
6. 新机器：覆盖应用数据文件；把 `.data` 放到新工作区。
7. 启动并确认 **设置 → 工作区** 指向正确目录。
8. 检查模型/API 密钥、MCP 服务器、子智能体、技能开关。
9. 打开历史记录，确认 `chat.db` 已读到。

工作区路径变了就在设置里重新选，或改 `config.workspace_dir`。

## 安全

- `config.json` 里的 API 密钥是**明文**。
- `mcp_servers.json` 里的 `auth_token` 与 `env` 也是明文。
- 不要把应用数据目录提交到 Git。
- 共享机器上启用全盘加密；妥善保管导出的压缩包。
- 定期轮换密钥；泄露立即吊销。
- 不要把生产密钥放进工作区文件——模型能读到。

## 常见问题

| 现象 | 排查 |
| --- | --- |
| 改了配置不生效 | 完全退出再启动；设置面板是否覆盖了？ |
| 导入后 MCP 服务器没了 | 手动复制 `mcp_servers.json`，或用当前版本重新导出 |
| 导入后历史没了 | `chat.db` 不会自动恢复；手动复制 |
| 导入后技能开关不对 | 压缩包里有 `skills.status` 吗？恢复到了正确的工作区吗？ |
| 应用配置档后模型没变 | 配置档不含模型/API，只切换工具、智能体、MCP 与编排 |
| 导出的包很大 | `chat.db` 含请求体与日志；先压缩或调低保留天数 |
| 配置档里没有技能 | 设计如此；技能是工作区状态 |
| 模型能力信息缺失 | `model_metadata.json` 可能早于该模型；缓存每周刷新——再"获取模型"一次强制合并 |
