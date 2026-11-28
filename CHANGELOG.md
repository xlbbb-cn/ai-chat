# Changelog

本文件由 `scripts/gen-changelog.cjs` 从两个 tag 之间的提交信息生成，请勿手工编辑：

```bash
npm run changelog -- --from <上一个 tag> --to <本次 tag> --details --write
```

条目按 Conventional Commits 的类型分组，作用域加粗；工作区里提交标题是英文、正文是中文，
所以带 `--details` 时是中英对照。

## [v3.9.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.9.2) — 2026-11-29

### 新增

- **tools,chat**: 提示词下沉收尾与会话级模型设置 ([`508d163`](https://github.com/xlbbb-cn/ai-chat/commit/508d163c2fc69d1a63b78bfef4854a44ae106549))
  - schema：压缩复述代码已强制事实的 7 段描述
  - memory：consolidate 的 apply 校验 keys 属于本次会话的 plan，且要求用户 明确要求整理记忆；.data/memory 由 file_actions 写保护
  - file_actions：.data/todos 同样写保护；patch 前校验本会话已读（未读返回 文件头）；写入 .data/tmp 时提示交付物应放项目目录
  - timer：同会话同 label/message 的 pending 定时器直接复用
  - run_cmd/scripts：脚本库策略段压缩；同一命令第二次成功时提示存成脚本
  - engine：{skill:<name>} 自动加载；显式调用语法仅在用户消息命中时注入； TODOS 段瘦身；回合尾“只写 tmp 未交付”提醒一次
  - 后台轮次运行期间切走再切回，工具栏恢复该轮实际使用的模型与思考深度
  - 后端在 turn-state / list_active_turns / get_turn_snapshot 上补 reasoning_effort
  - 运行中的轮次锁定模型与深度控件

### 杂项

- bump version to 3.9.2 ([`60b9091`](https://github.com/xlbbb-cn/ai-chat/commit/60b9091a5719b95ffb64502ee831a766638a293f))

**完整对比**：[v3.9.1...v3.9.2](https://github.com/xlbbb-cn/ai-chat/compare/v3.9.1...v3.9.2)

## [v3.9.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.9.1) — 2026-11-28

### 新增

- **tools**: 工具调用行显示参数、结果与错误 ([`c48db41`](https://github.com/xlbbb-cn/ai-chat/commit/c48db4181d9d6e15a2fd64eae8508d9f7687343d))
  - `tools/outcome.rs`（新）是唯一的测量点：`measure()` 把工具结果折成行数 / 行区间 / 退出码 / 匹配数 / 错误首行 / 耗时 / 输出首段，`call_label()` 给出单行参数标签。四十多个工具共用这一处，加新工具不需要动 UI。
  - `engine.rs` 结算处把 outcome 同时写进 `tool-call` 事件（`events.rs` 新增可选字段）和 `record_tool_row` 快照，重挂载不退化；新增 `timed()` 按调用计时，并发段里各算各的，不互相冒领耗时。
  - 计划阶段为每个会执行的调用先发一条带参数的标签行：没写进度行的工具（`memory`、`todos`、`calculator`）不再只剩一个工具名。后台任务与 `call_subagent` 跳过——它们有自己的行、结算不走 `tool-call`，提前建行会留下永久 running。`file_actions` 的读取行补 `(lines 40–80)`、搜索行补 `(max N)`。
  - 前端修掉 `ToolCallEntry.error` 从不赋值：`App.tsx` 的 done 分支只改 `status`，所以失败行永远只有一个红点，`detailFor` 的 `summary ?? error` 也就永远取不到错误。现在 outcome 的失败首行落到 `error`，行内直接显示，折叠卡片也显示最新一步的失败原因。
  - 指标行是 11px `--c-muted` 的分隔文本（`DESIGN.md` 的元数据规则：不做 pill、不填色、不用强调色），失败行不显示指标只显示错误；每行的输出预览按需展开（`<details>` + `pre`，上限 320px）。没有 outcome 的历史行渲染完全不变。
  - 三语补 `toolGroup.metrics.*` / `toolGroup.output.*`；计划与实施状态记在 `docs/tool-call-outcome-plan.md`。
- **chat**: 输入框加 &path 引用，只带路径不带文件内容 ([`455fc5d`](https://github.com/xlbbb-cn/ai-chat/commit/455fc5d25d721c43ba46a9f9575ff5f1ddca4a80))
  - `src/pathRef.ts`（新）：`&` 必须在行首或空白之后，终止集是空白 / `&` / `@` / `#` / `<` / `>`，所以 `R&D`、`A & B`、`cmd &&x` 都不触发；`isPathRefExpressible` 把含空格或终止字符的路径挡在列表外（插进去会指向另一个文件）。`activeMentionQuery` 的终止集补 `&`——否则 `@a&b` 会让 `@` 弹窗留在打开态，破坏「最多一个弹窗」的契约。
  - `FileMentionPopup` 加 `mode="path"`：同一个外壳、同一套键盘契约，只换 aria/header 文案，并把行内图标改成中性色（path 模式不过滤能力，图标不该再暗示"这个模型能不能吃这个格式"）。没有新增组件，也没有新增 CSS。
  - `App.tsx` 复用 `ensureWorkspaceFiles` 的 15s 缓存，两个弹窗共享一次工作区遍历；`handleKeyDown` 里三段 ternary 改写成一张四弹窗的表（`open/count/move/accept`），加第五个触发器时不会再漏掉 ↑↓ / Enter / Tab / Esc。`&` 是唯一在回复进行中仍保留的弹窗：注入的文本不携带附件，`&path` 只是文本。
  - 发送链路零改动就达成了"不附加内容"：`MENTION_RE` 只匹配 `@`，`resolveFileMentions` 看不到 `&` token，`partsForContent` 收到的附件列表为空，请求里没有 file/image part，也没有 handoffNote。`ChatMessage` 只补一层 `highlightPathRefs`，沿用 `.file-mention-token` 强调色——两者都是"指向一个工作区文件"，差别由弹窗解释，不该由正文颜色承担。
  - 三语补 `app.pathRef.{title,hint,noMatches}`，placeholder 与注入态提示加 `&`；文档：`modules.md` 记下这个 token 永不进附件管线及由此推出的三条结论，`DESIGN.md` 从三个 trigger 改为四个并写明 `&` 的注入例外，wiki 中英两页与模块树同步。
- **agents**: 编排设置收进子智能体行，展开入口改成无框文字 ([`2a5a8ef`](https://github.com/xlbbb-cn/ai-chat/commit/2a5a8efadaac7ce64588717e80c15b163cc0cbca))
  - 触发器去掉框：新增共享原语 `.text-btn`（透明、`border: none`、`--c-accent-text` 12px/500 标签、`--r-sm` 圆角），不再借用 `.inline-edit-btn` 的框体；组件样式表只留 `flex-shrink: 0`，不覆盖原语的框。
  - hover 走 `--c-accent-soft` 填充而不换字色：玫瑰粉 palette 的 `--c-accent-text` 在纸面上 7.1:1，换成 `--c-accent` 只剩 4.6:1，12px 的文字不能这样掉对比。
  - 触发器高度从 28px 回到 20px，与其余设置行齐平；chevron 取 `currentColor` 跟随 accent 标签，不再是 `--c-muted`。`aria-expanded` / `aria-controls` 保留。
  - 三语补 `agents.moreSettings`；原 `agents.orchestration` 文案转作按钮的 `title`，key 不删。DESIGN.md 新增「Text action」小节，并改写 §Disclosure row 的行内变体。
- **chat**: 空态背景改为逐代生长的年轮 ([`ad332d6`](https://github.com/xlbbb-cn/ai-chat/commit/ad332d683d70f524ad7b46298d229425724e8bca))
- **sidebar**: 侧栏外缘跟随窗口圆角 ([`3fbea59`](https://github.com/xlbbb-cn/ai-chat/commit/3fbea596fbc3dc6343b23ea7cd288bc8d8fbd16f))
- **tools**: 工具项不再折叠，自动确认模式挂到运行脚本行上 ([`77f1d91`](https://github.com/xlbbb-cn/ai-chat/commit/77f1d91222ae23af254f504eed20bcc03591324f))
  - 工具描述已是单行，去掉每一项的展开箭头与折叠描述框（`expandedTool` 状态、 `.tool-actions` / `.tool-action-btn*` / `.tool-desc-expanded` 一并删除）。
  - 「自动确认模式」从面板底部的独立分组挪进「运行脚本」这一项：标题行尾部是折叠 箭头，展开后 4 类确认与「全部自动获批」在名称列下方展开，左边线与工具名对齐； 默认收起（`useState(false)`，不再有加载完成后再收起的闪动）。
  - 工具项改成两列网格（`.tools-list .tool-item`）：开关固定在第 1 列第 1 行并垂直 居中，`.tool-info` 在第 2 列第 1 行，展开块在第 2 列第 2 行，所以展开与收起时 开关坐标完全相同；对应删除 `.tool-row` 包裹层、`:has` 两条对齐补丁和 `.tool-info` 的 `flex: 1`。
  - 关闭态变灰只作用于标题、说明和开关（从 `App.css` 的共享规则里移除 `.tool-item.disabled`），展开的确认选项仍可读可点。
  - 验证：`tsc --noEmit` 与 `vite build` 通过。本机没有可用的无头浏览器 （node_modules 无 playwright/puppeteer，`msedge --dump-dom` 返回空），布局按 盒模型与网格行核对，未截图验证。
- **tools**: 精简模型侧工具描述，去掉示例只留约束 ([`0eb06f2`](https://github.com/xlbbb-cn/ai-chat/commit/0eb06f2e7b3e1753d2477266fad95c897aef2faa))
  - `src-tauri/src/tools/{schema,experience,mcp_self,skill_admin}.rs` 的工具描述 合计约 9.5k → 5.8k 字符：`run_cmd` 1349→744、`run_shell` 1361→768、 `file_actions` 1286→409、`memory` 1509→979、`todos` 701→406、 `experience` 810→604、`skill_merge` 555→388、`mcp_self` 约 1.8k→0.8k。
  - 保留会让调用直接出错的那几条：`run_cmd` 的脚本库 / 裸程序解析与拒绝路径和 shell 解释器，`run_shell` 的本机 shell 与无 UAC，`file_actions` 的 `./` 路径与 `.data/chat.db` 只读，`memory` 的作用域和 TRUST CALIBRATION（含 `tests.rs` 锁定的 "is prepended to that message" 与 `conflict: true` 措辞），计算器的 "NEVER guess a number" 及 `run_cmd` / `run_shell` 兜底分支。示例、教程式的 分步说明移交给 `.data/manuals`。
  - 参数描述保持原样：模型靠它们决定参数形状。
  - 验证：`cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` 通过； 测试二进制在本机仍以 0xc0000139 起不来（改动前即如此，与本次无关）。
- **i18n**: 说明性文案压成一行，工具说明压到 10–30 字符 ([`8109788`](https://github.com/xlbbb-cn/ai-chat/commit/8109788adaf49f5cd7d5f5754a69693ccd90bdc0))
  - 三语 `src/i18n/messages/{en,zh-CN,zh-TW}.ts` 共 96 条说明性文案（hint、副标题、 空状态、tip）改写成一句话：英文平均 144 → 64 字符，中文 59 → 27 字符。占位符 （`{prompt}` `{cwd}` `{model}` `{rounds}` 等）与各语言既有术语保留：zh-CN 用 智能体 / 自我进化 / 插件，zh-TW 用代理程式 / 自我演化 / 外掛，`+智能体` 与 `+代理程式` 这类本地化触发词照旧。
  - 模型侧文案不在此列：`app.fileMention.handoffNote`、`agents.toolHints.*` 原样 保留，只动界面上的说明文字。
  - 工具面板的 7 条 `tools.items.*.description` 由多段（最长 600 字符）压成单行 10–30 字符，只留"做什么"；细节仍由工具 schema 提供给模型。
  - 验证：`node node_modules/typescript/bin/tsc --noEmit` 与 `node node_modules/vite/bin/vite.js build` 通过。
- **prompt**: inject no drawing block when enhanced figures is off ([`f8e8dee`](https://github.com/xlbbb-cn/ai-chat/commit/f8e8dee6ee9e269a208fddf48f2faa366c67ff44))
  - 关闭「增强画图」后提示词里不再有任何画图说明：`mermaid` 从关闭路径上撤掉， 画图提示只存在于增强画图手册体（`figures::prompt_block()`）里，`mermaid` 仍排在 `flow` / `svg` / `carve` / `html-fig` 前面当最便宜的选项。
  - 删除 `figures::mermaid_prompt_block()` 与两个只服务于它的单测 (`mermaid_prompt_block_asks_for_mermaid` / `prompt_blocks_differ`)； `engine.rs` 的关闭分支改成空串。`prompt_block_names_every_markup` 继续覆盖 mermaid / caption。
  - 渲染不变：`mermaid` 围栏（含用户自己粘贴的）仍由 `mermaidDiagrams.ts` 画成卡片， 与开关无关。
  - 文档与界面说明同步：`lib.rs` / `types.ts` 字段注释、三语 `enhancedFiguresHint`、 DESIGN.md、workflow.md、wiki 中英文绘图表（新增 Mermaid 行）。
  - 验证：`node node_modules/typescript/bin/tsc --noEmit` 与 `cargo check --manifest-path src-tauri/Cargo.toml --lib --tests` 通过；运行中的应用 占着 target 锁，`cargo test` 二进制仍以 0xc0000139 起不来。
- **prompt**: list Mermaid in the enhanced figures manual, and gate background_task on a usable flag ([`8cb82f9`](https://github.com/xlbbb-cn/ai-chat/commit/8cb82f9cfd31e2a4d5608da7b63c78c5e600f3eb))
  - 增强画图提示词：`figures::prompt_block()` 增加 `mermaid` 一条（支持的图表类型、caption 约定、含 `()[]/` 的标签要加引号），并把 `flow` 的取舍改成“需要配色或边动画时才优先”；手册指针描述同步写成 `mermaid` / `flow` / `svg` / `carve` / `html-fig`。手册体在 `.data/manuals/enhanced-figures.md`，指纹变化后下次注入自动刷新并留备份。
  - 后台执行：`run_in_background` 由三处加上（本地工具、MCP 工具、`call_subagent`），所以新增 `tools::advertises_background` 与 `tools::prune_unreachable_background_control`，由 `build_turn_tools` 在整份工具表组装完成后调用——表里没有任何一个工具带这个参数时，`background_task` 不再插进提示词，避免模型面对一个什么都管不了的控制工具。
  - 文档同步：`docs/agents/workflow.md`、`docs/DESIGN.md`、wiki 中英文工具表。
  - 测试：新增 `background_control_tool_needs_a_tool_that_accepts_the_flag`，`prompt_block_names_every_markup` 增加 mermaid / caption 断言。本机运行中的应用会让测试二进制以 0xc0000139 起不来，改以 `cargo check --lib --tests` 验证编译。
- **env**: mount the app-data environment in run_shell too, and read whatever ready.json describes ([`5fef9cf`](https://github.com/xlbbb-cn/ai-chat/commit/5fef9cfd9c33c119540f88b9541132de9b403c63))
  - run_shell (and its `serve` processes) now mount the host environment like run_cmd: `<env root>/bin`, the uv directory the flag names and the venv's `bin/` (`Scripts/`) join the child's PATH, VIRTUAL_ENV names the venv. The workspace script library stays run_cmd's alone.
  - read_ready() takes the fields by name instead of a fixed struct, so a flag that spells `python` as an object (`{exe, venv, version}`), `created_at` and unknown keys (`schema_version`, `tools`, `notes`) is understood rather than read as unreadable. flagged_venv() derives the venv from the flagged interpreter and wins over the `<env root>/py313` + pyvenv.cfg convention.
  - publish() also exports AI_CHAT_PYTHON (the venv interpreter, when the flag names one) and refreshes on every prompt build, so an artifact re-execs under 3.13 without parsing the flag itself.
  - The prompts and tool descriptions stop explaining what the tool now does on its own: the ENVIRONMENT note, the setup-env manual and PYTHON_SECTION keep only what the model must decide (install there, record the inventory, stay stdlib-only), and use_venv() shrinks to a read of AI_CHAT_PYTHON.
  - process::build_command had only serve's tests left as callers; serve builds through build_command_with_path directly.
- **env**: install assistant software under app data/env and inventory it in ready.json ([`ffd192a`](https://github.com/xlbbb-cn/ai-chat/commit/ffd192a8cfa1fc295e1252eacabb42d772bda8a6))
  - `ready.json` 新增 `description`：每个安装项一行（路径 + 版本），提示词按原文引用； 自检重写 flag 时保留该字段，不会把清单清空。
  - 环境块只在 `run_cmd` / `run_shell` 开启时注入——没有 shell 工具就没有可运行的东西， 清单只会变成提示词噪声（关闭时返回空串，有测试）。
  - `setup-env` 与三本 create-* 手册改写：app data 是宿主安装根，装完必须更新 `description`；`AI_CHAT_ENV_ROOT` 仍是运行时入口。
  - 审查前 5 个提交时发现的同类问题一并修掉：挂载 venv 需清除 `PYTHONHOME`； 手册里「3.14」的表述改成与宿主无关的说法。
- **run_cmd**: mount the app-data Python 3.13 venv in its children ([`b18daa3`](https://github.com/xlbbb-cn/ai-chat/commit/b18daa35934a1c91659c4c483c751a85d547e371))
- **env**: self-check an existing venv and write the completion flag ([`d0c0f10`](https://github.com/xlbbb-cn/ai-chat/commit/d0c0f103072b85a022a9cf83eed8da95bec817f7))
- **env**: keep the host Python environment in app data behind a completion flag ([`fa4dfc1`](https://github.com/xlbbb-cn/ai-chat/commit/fa4dfc1883c25d9c70dee8e33259f0b19d923267))
- **prompt**: recommend Python plus a uv-managed 3.13 environment in the authoring manuals ([`6a0f529`](https://github.com/xlbbb-cn/ai-chat/commit/6a0f5298419f0974d278f305566b6f35c4573727))
- **agents**: identify sub-agent and cloud-agent traffic with a User-Agent ([`35923b0`](https://github.com/xlbbb-cn/ai-chat/commit/35923b08997079861243f5654732344b47932939))
  - `http.rs` 新增 `AGENT_USER_AGENT` 与 `build_agent_client()`；两个流式客户 端共用的超时策略抽成 `stream_policy()`，`build_stream_client` 行为不变。
  - `agents/planning.rs` 的 `build_agent_client()` 是全部子 Agent 请求的唯一 客户端（选择器、规划、任务执行、supervisor 都是 clone 它），一处改动覆盖 整条链路；顺带清掉 `agents.rs` 里随之失效的 `build_stream_client` 导入。
  - `agents/cloud.rs` 的面板探活 `probe` 与 `run` 两个客户端都带同一 UA，成果 媒体下载复用 `run` 的客户端，因此也带。
  - 单测锁定格式：小写、ASCII、版本等于 `CARGO_PKG_VERSION`。

### 修复

- **turn**: 一轮若从未发出终止事件，前后端各自收尾 ([`ad622c5`](https://github.com/xlbbb-cn/ai-chat/commit/ad622c5d62d202b2c92ceceb02919ff53b873680))
  - `LlmTurn` 新增 `announced`：`emit_done` / `emit_error` 置位；`chat_completion` 在 invoke 直接返回 `Err` 时 `mark_announced()`，因为前台已按 rejection 收尾，backstop 不该再为同一次失败报第二遍。
  - `TurnEndGuard` 改为持有 `Arc<LlmTurn>`，不再复制那一堆散字段。清理时若这一轮从未 announce，guard 自己发一条终止失败（`TURN_INTERRUPTED`），而不只是把自己从 registry 摘掉。
  - `chatCompletion` 返回 `TurnHandle`：调用方式与原来的 unlisten 完全一致，多一个 `abandon` 供对账使用。registry 里已经没有这一轮（按 turn id、宽限期后复查一次，因为 base 会先于事件被 unpublish 几毫秒）时，前台自己收尾；新增三语文案 `app.turnLostEvent`。
  - streaming 期间每 10s 主动对账一次：丢了终止事件的一轮和"很慢的一轮"看起来一模一样，只在 focus / 挂载 / 切会话时才对账，会把 Stop 一直卡在界面上，而用户盯着的那一轮正是唯一能靠这一眼救回来的情况。
- **dev**: 让 Vite 监听双栈地址，WebKitGTK 才能连上 devUrl ([`0e7b9b1`](https://github.com/xlbbb-cn/ai-chat/commit/0e7b9b1166a956e13d5e3470d49fa219ddff4b93))

### 重构

- **agents**: 把编排判断下沉成确定性代码 ([`7d54154`](https://github.com/xlbbb-cn/ai-chat/commit/7d541542ee0cd27e5ac80f7b93a563a5183e5a0c))
  - `OrchestrationMode`（新增 `agents/mode.rs`）：开关 × roster × bus 的组合只在一处判定，`engine` 用它替掉原来的 `planning_enabled` 与内层 `has_enabled` 两层嵌套，并在日志里直接给出本轮模式。
  - `execution_mode` 由依赖图派生：按最长路径分层，最宽的一层只有一个任务才判 sequential。模型填的值只在冲突时记一条 issue，不再采信——执行器本来按 wave 调度，让模型决定并发只会把菱形图的并行波一起杀掉。
  - `delegation_hint`：短、单行、无链接/路径/代码块的请求判 `No`，直接直答（省掉选择器与规划两次调用）；两个以上链接、成对代码块或多工标记判 `Yes`，跳过选择器用全量 roster；其余仍问模型。判 `No` 代价有限：主智能体自己握着全部工具，丢的只是并行度。
  - 选择器只回答"谁参与"，不再回答"要不要派"（老模型仍返回 `delegate` 时记 INFO 后忽略）；roster 内所有 agent 的能力签名完全相同时不调模型——那种选择没有信息量。
  - 规划两次解析失败时回落到单任务（能力最全的 agent），不再把整轮静默降级成直答；`MAX_PLAN_TASKS` 渲染进提示词，原来模型不知道上限、多出来的任务被静默截断。
  - `cwd` 在解析期做词法校验，越界路径清空并记 issue（不校验存在性：既有语义允许任务 scaffold 新目录）。
  - 监督器：所有任务已是终态时直接收工，不调模型；重复的 `mbus send` 由工具拒绝，那条"不要重复之前的指导"从请求变成强制。
  - 文档：`docs/prompt-sinking-review.md`（提示词层评审）、`docs/model-judgment-plan.md`（21 项全景清单、逐项改法与落地状态）。

### 文档

- **manuals**: describe the environment root as the install home, not only a venv ([`6afc3e3`](https://github.com/xlbbb-cn/ai-chat/commit/6afc3e3e3d75aa50cb32694fb8ba68635d37d130))

### 杂项

- bump version to 3.9.1 ([`4de9ce6`](https://github.com/xlbbb-cn/ai-chat/commit/4de9ce6aa14c9f9e57750236b1595aceb886b1cc))

### 其它

- Plugin creation manual: document the settings schema, and stop punishing a miss ([`cebbc3c`](https://github.com/xlbbb-cn/ai-chat/commit/cebbc3c887ade914efecd0d69379ab7e3e2e828e))
  - `create-plugin` 手册的「设置」从一段括号清单改成一节完整参考：`sections[] → fields[]` 两级、**section 与 field 都必须写 `key`**、可直接复制的 `config.schema.yaml` 示例（text / enum / integer / secret / advanced / reveal_when）、字段表，以及「解析失败怎么办」——把 `sections[0]: missing field 'key'`、`unknown variant`、`options: invalid type`、缩进类报错逐条对上修法。创建插件时反复踩的就是这一类：手册原先只列类型名，既没有示例，也没说 `key` 是必填。
  - 同样的示例进了 `docs/wiki/{en,zh}/plugins.md` 的「设置」小节（各 +36 行），那是面向用户的一份。
  - 手册里的 YAML 示例现在有测试兜底：`plugins::tests::the_creation_manual_settings_example_parses` 从 `prompt::plugin_manual()` 取出 ```yaml 块按 `ConfigSchema` 解析，并断言每个 section / field 都有 `key`，文档不会再漂成一份加载不了的形状。
  - 解析器对「差一点」的 schema 不再直接翻脸：`sections[].key` 可以省略（用标题 slug 派生，重名追加 `_2`，无 ASCII 的标题退化为 `section_<n>`），`label` / `help` 读作 `title` / `description` 的别名 —— 手写 schema 最容易漏这两处，而漏 `label` 从前只会静默变成空标签。字段缺 `key` 仍然报错：它是存储名，猜一个会让已存的值变孤儿。
  - 导出不再被坏 schema 卡死：`config.yaml` 留在工作区，`PluginExportReport.settings_note` 带解析器原文，前端提示改成「设置未随包导出：<原因>」。读不懂 schema 就无法证明哪个值是 secret，那就一个值都不打包 —— 不泄漏，也不阻断。
  - `docs/agents/architecture.md`：包与设置两条 bullet 补上 `settings_note` 以及 key / 别名的规则。
- Plugin manager: export and import plugin packages ([`256c0ba`](https://github.com/xlbbb-cn/ai-chat/commit/256c0ba4113501ce8394a700f5957e14beb29707))
  - 新增 `plugins/package.rs`：`export_plugin_package` 把单个插件文件夹打成 `.zip`（根目录 `plugin.json`），跳过隐藏文件、`*.bak.*`、`node_modules/` 与各类缓存目录；仅当插件声明 `config.schema.yaml` 时才带上 `config.yaml`，且先按 schema 删掉所有 `secret` 字段的值。`import_plugin_package` 接受这一格式或手工打的单层文件夹 zip，从清单推导 id，先解到 `.data/plugin-backups/.staging-<uuid>/`，校验清单与声明过的 `entry` / `config_schema`，把已安装副本移到 `.data/plugin-backups/<id>-<ms>/` 后再就位；加载器拒绝的清单会回滚整次导入。`safe_join` 防 zip-slip，`MAX_PACKAGE_FILES` / `MAX_PACKAGE_BYTES` 防 zip 炸弹；清单只从已读文本写一次，包里不会出现两个 `plugin.json`。
  - 重装运行中的插件：导入前先用 `package_id()`（只读中央目录与清单，不落盘）取出包要安装的 id 并停掉其实例，因为 Windows 不允许重命名仍被进程占用的目录；导入失败时把原先运行的插件重新拉起。为此 `package.rs` 抽出 `open_archive` / `survey` / `read_archive_manifest` / `derive_id`，由 `import_plugin` 与 `package_id` 共用。
  - 插件管理窗：工具栏新增“导入插件包”，Start/Stop 旁新增“导出”，保存对话框预填 `<id>-<version>.zip`。提示区分成功与失败，并报告文件数、旧副本位置、移除的密钥数、设置是否随包走；插件列表新增过滤框与 `shown of total` 计数，列表本身只留清单，不再放按钮。新增 `PluginExportReport` / `PluginImportReport` 类型与 `exportPluginPackage` / `importPluginPackage` 封装，三语文案补齐。
  - `PluginManagerPanel.css` 回归设计契约：删掉自建的 `.pm-button*` / `.pm-icon-button`，改用共享的 `.btn-primary` / `.btn-secondary` / `.close-btn` / `.panel-search-input`（只留 34px 的 `.pm-action` 尺寸修饰），字面色值换成 `--c-success` / `--c-danger` / `--c-warning`，6–14px 圆角与胶囊换成 `var(--r-sm)`，投影换成 `var(--shadow-lg)`，对话框面回到 `--c-surface` + `--c-border-i`，遮罩换成 `--c-bg` 混合加 `--glass-blur-scrim`，去掉全大写与字距，控件统一 34px。
  - `capabilities/plugin-manager.json` 增加 `dialog:allow-open` / `dialog:allow-save`：该窗口此前只有 `core:default`，包的打开/保存对话框在运行时被拒（`dialog.open not allowed on window "plugin-manager"`）。只多这两项，仍未授予任何 fs/shell 能力（`notify.json` 保持原样）。
  - 文档：`docs/agents/architecture.md`、`docs/wiki/{en,zh}/plugins.md` 记录包格式、密钥策略、备份目录、重装时先停实例与新的 IPC。
  - 验证：`cargo test --lib plugins::` 22 项通过，`cargo check --lib` 通过，`npx tsc --noEmit` 与 `npx vite build` 通过。
- Identify outbound requests as Rabbit Chat Studio with version ([`41cdac1`](https://github.com/xlbbb-cn/ai-chat/commit/41cdac15858563dd4e29052e544703f8e94f90e5))
  - 常量从 agent 专用提升为应用级出站身份 `rabbit-chat-studio/<版本>`， 版本仍由 `Cargo.toml` 编译期注入，测试锁定形态（小写 ASCII + 含版本）。
  - `update::build_client` 改用共享常量，GitHub 拒绝无 UA 的约束保留在注释里。
  - 云 Agent / 子 Agent 客户端沿用同一常量；聊天主链路依旧不发 UA， 网关才能区分后台 agent 流量与对话流量。
  - 模型目录（独立 crate）的 `rabbit-chat-studio-catalog/<版本>` 保持不动， 该后缀标识的是另一个子系统。

**完整对比**：[v3.9.0...v3.9.1](https://github.com/xlbbb-cn/ai-chat/compare/v3.9.0...v3.9.1)

## [v3.9.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.9.0) — 2026-11-27

### 新增

- **chat-notice**: slide the bar in and out of the toolbar band ([`f25a95e`](https://github.com/xlbbb-cn/ai-chat/commit/f25a95eb4b4e60355b1a483c0ba0c92574a14d2c))
- **chat-notice**: span the reading column and tone the bar ([`ca85b00`](https://github.com/xlbbb-cn/ai-chat/commit/ca85b00fcb171e1f99c3b0c865bcdc819500544b))
- **appearance**: drop the toolbar and composer hairlines ([`d9dad78`](https://github.com/xlbbb-cn/ai-chat/commit/d9dad78eee26d3b8102e65be3b7d15fb7ebfb921))
- **titlebar**: draw macOS traffic lights on macOS only ([`fe7b91b`](https://github.com/xlbbb-cn/ai-chat/commit/fe7b91b7e833494a991f90c4c7ba18e02af683e2))
  - macOS 的关闭 / 最小化 / 最大化换成系统交通灯：12px 圆点、8px 间距（原生 的 20px 节拍），颜色走 `--c-traffic-close` / `--c-traffic-minimize` / `--c-traffic-maximize`，另加 0.5px 的 `--c-traffic-edge` 内描边；圆点 不加深色填充、不加描边环、不放大，`:active` 只是 `brightness(.88)`。
  - 圆点里的符号（`×`、`−`、绿点的两个实心三角，`--c-traffic-glyph`）默认 `opacity: 0`：颜色本身就是提示，符号只在鼠标移入 `.titlebar-lights` 或 键盘 `:focus-visible` 时确认「哪个点做什么」，与 macOS 一致。
  - Windows / Linux 不动：仍是右侧 34×28 的 `.titlebar-btn` 图标按钮， `is-close` 的破坏性 hover 保留，`⋯` 菜单继续共用这套中性配方。 `TitleBar` 因此把控件拆成 `macControls` / `winControls` 两支。
  - 三个交通灯颜色写在 `:root` 的 token 块里，主题、调色板、毛玻璃档位都不 重指（macOS 明暗两种外观下就是这三个色）；圆点的 `50%` 是形状而不是 半径 token，这也是 chrome 里唯一的圆形控件。
  - `docs/DESIGN.md` 的 Window chrome 表格按平台拆行并补一段说明；中英文 wiki 的标题栏条目同步，不再说「窗口按钮在 macOS 在左、别处在右」。

### 杂项

- bump version to 3.9.0 ([`7e139e8`](https://github.com/xlbbb-cn/ai-chat/commit/7e139e8fae1af9710e43559cff0635069ff05392))

**完整对比**：[v3.8.3...v3.9.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.8.3...v3.9.0)

## [v3.8.3](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.8.3) — 2026-11-27

### 新增

- **appearance**: make the side panel and the page sheets opaque ([`dce343c`](https://github.com/xlbbb-cn/ai-chat/commit/dce343c1fb0704dcd65fba96074f94e43213db76))
  - 侧栏面板与设置页不再是半透明：`--c-panel` = `--c-bg`、`--c-sheet` = `--c-surface`，在任何毛玻璃档位、任何平台上都是实底，13px 正文下面再也 透不出对话的字影（6% 透出时 `--c-muted` 只剩 4.6–6.0:1，三分之二只有 1.9–3.1:1，低于配色承诺的 4.5:1）。
  - `.sidebar` / `.panel-header` 改画画布对（`--c-panel` + `--c-bg-image`， 按 100vw 铺），Cream 的渐变仍从标题栏连续延伸，不会在 400px 盒子里重开。
  - 实底不需要滤镜：面板与其头部去掉 `backdrop-filter`，`--glass-blur-panel` / `--glass-saturate-panel` 一并删除；设置页与提示词编辑器也不再吃 scrim blur（`body:has()` 的淡出只为指针保留）。
  - `DESIGN.md` 补 PATCH v1.9 与所有受影响的 token/表格段落；wiki 的外观说明 同步（`off` 只让标题栏和工具栏变实，面板一直是实底）。

### 修复

- **llm**: release the upstream connection deterministically on Stop ([`1c450fd`](https://github.com/xlbbb-cn/ai-chat/commit/1c450fd8ce743d06d3297073e08dd3a2e5b1d474))
- **turns**: keep the Running badge tied to the turn that owns it ([`5ba6fa0`](https://github.com/xlbbb-cn/ai-chat/commit/5ba6fa0f1a117e8968d04a0e96cf296bfe880010))
  - the mount path seeded from list_active_turns() and subscribed to turn-state as two parallel IPC calls, so a turn that ended in between kept its badge forever;
  - a driver dropped a chat-done/chat-error that arrived between subscribe() and arm(), leaving it unsettled for the rest of the run;
  - the registry entry and its terminal event were cleaned up only after run_chat_turn() returned, so a panic leaked them (and release builds write no log, so the leak left no trace).
  - App: reconcileTurns() compares the local map with list_active_turns on mount (after the listener is registered, never before), on focus/visibilitychange and when a driver settles; ensureDriverFor() re-attaches a running turn that has no driver, and drops the badge plus loads the rows when the backend no longer has that turn.
  - App: handoffForegroundTurn() re-asks the backend right before arming, so a turn that ended during the subscribe round trip is never armed.
  - App: stopStreaming() clears the entry when the stop found no turn.
  - backgroundTurns: terminal events seen before arm() are replayed by arm(); idleFor()/settleExternally() let the sweep end a driver whose turn is gone instead of waiting for an event that will never arrive; stop() settles locally when the backend holds no turn.
  - turn: stop_chat_completion() returns whether it hit a turn, and TurnEndGuard publishes the terminal turn-state from Drop as well.

### 重构

- **prompts**: tighten P1/P2 prompt wording and manual seeding ([`ceced8d`](https://github.com/xlbbb-cn/ai-chat/commit/ceced8dcb7af2527a74cd3d585e79f79e72cecb4))
  - engine: add plan-confirmation rule and one shared WAIT_EVT wording; - planning: share the skills= clause between both prompts; - exchange: tell sub-agents to name the file the main agent should promote; - schema: soften the calculator mandate and clarify mission tasks; - skills: shorten the duplicated TRUST CALIBRATION block; - manuals: refresh seeded manuals on body drift, honour the pinned marker

### 文档

- move DESIGN.md into docs/ and repoint every reference ([`fe853e9`](https://github.com/xlbbb-cn/ai-chat/commit/fe853e92e8a7952c19df57a41b1a6441fe593dca))
  - `DESIGN.md` → `docs/DESIGN.md`（git mv，正文未改）：视觉规范与 docs/agents 五份开发文档同处 docs/，仓库根只剩 AGENTS.md（索引）与两个 README。
  - 引用同步 23 处代码注释（src/App.css、App.tsx、components、notify、src-tauri/src）、AGENTS.md 的文档地图与硬规则行、docs/agents 四份正文与 `../DESIGN.md` 相对链接、docs/site/assets/site.css 头注释。
  - wiki en/zh：仓库树把 DESIGN.md 挪进 docs/ 分支，贡献指南与相关文档改用 docs/DESIGN.md；gen-wiki 40 页无断链。
  - 校验：npx tsc --noEmit ✅ / npx vite build ✅。
- **agents**: split AGENTS.md into docs/agents/ and keep it as the index ([`080d55f`](https://github.com/xlbbb-cn/ai-chat/commit/080d55fe2622aa2c585201dedc011da5faba1d8f))
  - AGENTS.md 从 162KB 收成索引：文档地图表（什么活读哪份）+ 永远适用的硬规则（DESIGN.md 契约、tsc/vite 校验、技能路径隔离、CHANGELOG 由脚本生成），外加「怎么加新规则」的维护条款。
  - 正文按主题搬到 docs/agents/：architecture.md（进程分工、插件总线、服务与管理器窗口、运行时文件、子 Agent 提示词、Tauri 2 特性）、tech-stack.md（命令与工具链、Cargo 镜像）、modules.md（src 与 src-tauri 文件地图、技能路径隔离、工具族与危险命令、回合守卫/并行/身份、附件）、interface-design.md（界面硬规则与 theme×palette×glass 检查矩阵）、workflow.md（发布、MSIX、站点、坑）。
  - 搬运逐行原样：463 行非空内容全部落地、无重复，只把 DESIGN.md 的相对链接补一级，每份文件带标题和指回索引的说明。
  - 引用同步：src/attachments.ts 的 file-part 注释指向 docs/agents/workflow.md，wiki en/zh 的仓库树与文档清单列出 docs/agents/，coding-plan 技能改为按任务查五份文档。

### 杂项

- bump version to 3.8.3 ([`66b8b6d`](https://github.com/xlbbb-cn/ai-chat/commit/66b8b6db1eb020a693a640bfe2d7cc8c391cd3e2))

**完整对比**：[v3.8.2...v3.8.3](https://github.com/xlbbb-cn/ai-chat/compare/v3.8.2...v3.8.3)

## [v3.8.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.8.2) — 2026-11-26

### CI

- **workflows**: pin node and rust to the local toolchain ([`4b425fe`](https://github.com/xlbbb-cn/ai-chat/commit/4b425fe8bf06c5fba9bcbae7c48392d9b9547efa))
  - Node 24.20.0 and Rust 1.99.0 in all three build workflows, matching the development machine instead of lts/*, stable and the stale 22.
  - Checkout, setup-node, upload-artifact, configure-pages, upload-pages-artifact, deploy-pages and microsoft-store-apppublisher on their newest major, so no step still runs on the deprecated Node 20 runtime.
  - winget-releaser pinned to v2 instead of the floating main branch.

**完整对比**：[v3.8.1...v3.8.2](https://github.com/xlbbb-cn/ai-chat/compare/v3.8.1...v3.8.2)

## [v3.8.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.8.1) — 2026-11-26

### 新增

- hand unreadable formats to the assistant, keep deliverables out of .data/tmp ([`84ab0d1`](https://github.com/xlbbb-cn/ai-chat/commit/84ab0d11a1b63731cdc49322576e04d7c0d01a18))
- **plugins**: label the start button "Restart" while a plugin runs ([`1b38c24`](https://github.com/xlbbb-cn/ai-chat/commit/1b38c246debe0b8b454fd4d90e9b3e8354acec4f))

### 修复

- **docs**: fence the plugin bus diagram as text ([`1dad3d0`](https://github.com/xlbbb-cn/ai-chat/commit/1dad3d03738ce284e9969f7650d4441d869ca4d4))

### 文档

- **wiki**: sync the pages with v3.3.4..v3.8.0 ([`84dc9f2`](https://github.com/xlbbb-cn/ai-chat/commit/84dc9f2c080dff2a91af07e200db02f463721d2e))
  - add Plugins, Appearance/chat background, notification card, memory hint, trust calibration and cross-workspace experience packages
  - document background turns in sessions/history and the turn guards
  - document plugin instances/tools, .data/plugins and backup exclusions
  - drop the removed Profiles feature and its profiles.json entries
  - rename export/restore to the current 'export backup'/'restore backup'
  - switch workspace selection to the title-bar switcher (Settings → Run for the self-evolution mode), bump the documented version to v3.8.0

### 杂项

- bump version to 3.8.1 ([`7c0f963`](https://github.com/xlbbb-cn/ai-chat/commit/7c0f963c9c1161009c2da6f62be60140ad0b6754))

### 其它

- update： icon ([`90e2dae`](https://github.com/xlbbb-cn/ai-chat/commit/90e2dae4179a2ab4d85046dedb0fc1761b7f7222))

**完整对比**：[v3.8.0...v3.8.1](https://github.com/xlbbb-cn/ai-chat/compare/v3.8.0...v3.8.1)

## [v3.8.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.8.0) — 2026-11-24

### 新增

- **settings**: fold the plugin switch into Run, hide its manager when off ([`de8073d`](https://github.com/xlbbb-cn/ai-chat/commit/de8073da4926139b3ef6bcc5a245b82af946a197))
  - the "enable the plugin system" toggle moves to the end of the Run section, and the plugin-manager row only renders while it is on
  - drop the Plugins section entirely: nav entry, section markup, `settings.sections.plugins`, the now unused `plugins.sectionHint` and its `.settings-plugins-hint` rule — the sheet is seven sections again
  - `settings.plugins.manager` label added in en / zh-CN / zh-TW; the enable hint says the manager is hidden instead of "plugins stay listed here", which no longer held once the section went away
  - comment and embedded-manual paths follow the move to `Settings → Run` (`prompt.rs` manual, `create-plugin.md`, `plugins_config`, `manager.rs` and the plugin-manager window header)
- **plugins**: plugin system on a message bus, manager window and creation manual ([`cf23b3c`](https://github.com/xlbbb-cn/ai-chat/commit/cf23b3c33abef8fef2a871c30e5e370d8a59ef26))
  - manifest + registry under `.data/plugins/<id>/`, the folder name is the id
  - plugin message bus: request/reply correlation, MQTT-style topics, bounded audit ring, expiry so a late reply is dropped instead of delivered
  - instances: spawned children (python/node/shell/exec, never a shell string) plus services that attach over the loopback link endpoint with a workspace token; one connection becomes one instance
  - per-plugin settings: `config.schema.yaml` drives a generic form, values live in `config.yaml`, validation returns field errors, secrets are masked on read and hot-pushed to the running instances on save
  - `plugin` control tool plus one `plugin_<id>_<tool>` per declared tool, gated on `plugins_enabled`; the payload is rebuilt with `mcp_revision` in the turn loop so a plugin created mid-turn is callable in the same turn
  - separate `plugin-manager` webview opened from Settings → Plugins, with its own capability; created on a blocking thread so WebView2 does not deadlock the main loop
  - plugin list, instance table, settings form, logs and a bus audit strip; the book icon next to the title opens the "creating a plugin" guide
  - `create-plugin` manual (prompt body + `Manual` registration) announced when the plugin system is enabled, seeded to `.data/manuals/create-plugin.md`

### 杂项

- bump version to 3.8.0 ([`89406db`](https://github.com/xlbbb-cn/ai-chat/commit/89406dbeea42bff64615a3dec2d5a4f653e0cdda))

**完整对比**：[v3.7.0...v3.8.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.7.0...v3.8.0)

## [v3.7.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.7.0) — 2026-11-24

### 新增

- **history**: running badge shows this turn's elapsed time ([`12533be`](https://github.com/xlbbb-cn/ai-chat/commit/12533be386c03e0816388210f8ad6456a6ecd9d7))
  - App.tsx hands the whole `runningSessions` registry to HistoryPanel instead of just the ids, so the badge can read the turn's real `started_at_ms` (a reload mid-turn keeps counting instead of restarting at zero).
  - HistoryPanel ticks `nowMs` once a second, and only while a turn runs; the duration renders through `formatUptime`, tabular-nums so the badge does not twitch as the counter advances.
  - The badge is retoned to the success family (soft wash + 6px dot + 11px label), matching the state chips of the other panels. New token `--c-success-text` (#17702E light) is the text twin of `--c-success`, which only reaches 3.9:1 on its own wash an 11px label needs 5:1.
  - DESIGN.md documents the fill/label split for success; `--c-success-text` is defined in every palette block, light and dark, in App.css.
  - Drop the "· current" marker from the row footer — the active row already carries the highlight and the aria-current — plus `history.current` in en / zh-CN / zh-TW.
- **settings**: configurable max concurrent sessions ([`4e51a13`](https://github.com/xlbbb-cn/ai-chat/commit/4e51a13f2c6a8018682d6d89c6ccdd85b9531787))
  - types.ts: AppConfig.max_concurrent_turns (0 = unlimited, backend clamps to 16; same-session supersede never counts as a new slot).
  - SettingsPanel → Advanced: 'Max concurrent sessions' number input next to the turn-limit fields, same parseLimitInput semantics ('0' = unlimited, blank = backend default); handleSave's {...config} spread carries it.
  - i18n en / zh-CN / zh-TW keys + hint explaining the background-continuation behaviour the limit bounds.
- **multi-session**: reload re-attach for background turns, per-session drafts ([`ea3f2aa`](https://github.com/xlbbb-cn/ai-chat/commit/ea3f2aaa2c166bb241e8c8aa240e55d2240fe993))
  - Reload re-attach: on mount, the running-turn registry effect now rebuilds a BackgroundTurnDriver per still-running session from get_turn_snapshot (content, reasoning, settled tool rows, delivered injections, usage totals) plus the persisted DB rows, so a turn that kept running through a webview reload keeps streaming and its assistant row lands in the database — previously every listener died with the reload and nobody persisted the reply. A turn that settles in the gap between the two calls remains the known narrow window.
  - historyRows.ts: recordsToMessages / parseStoredContent / parseStoredAttachments moved out of the lazy HistoryPanel into a leaf module shared with the re-attach path (the boot graph stays main+App).
  - Shared driverHost (useMemo): one hook object feeds both the foreground handoff and the reload re-attach; notifications, undelivered-injection requeue and confirm records live in one place.
  - Per-session composer drafts: composerDraftRef is a Map keyed by session; switching conversations restores that conversation's draft (history load and driver-visible sessions), paste/mention splicing stays inside its own session, and resetConversation starts the fresh chat empty.
- **files**: cross-conversation race protection for file_actions ([`b3b4f31`](https://github.com/xlbbb-cn/ai-chat/commit/b3b4f31aaff22b6bf743e3fef1d75db410a89980))
  - tools/file_locks.rs: FileLockRegistry — per-path async write locks plus stale-write detection. A session writing a file that ANOTHER conversation modified after this session's last read is refused with an explicit 'STALE WRITE — re-read and retry' error instead of silently clobbering the other conversation's change. A session's own writes never count as stale; a fresh read clears the verdict; deleting/moving forgets the history so a recreated file starts clean; anonymous (panel) callers are never blocked.
  - Locks are taken in sorted path order (opposite-direction moves cannot deadlock) and the backup + write + record sequence runs entirely inside the guards, so two writers of the same path are serialised and the second one always sees the first one's change. Failed actions (Error-prefixed results, the tool_output_is_error convention) publish no 'last write'.
  - file_actions integration: read records the session's read stamp; write / patch / delete / move run backup+apply inside with_write_guard; a move locks src+dst together and forgets the source on success, a delete forgets the path.
  - Known boundary (documented): run_shell/run_cmd bypass the registry by design; patch's own context matching remains the second line of defence.
  - 5 unit tests: stale refusal + fresh-read recovery, own-writes never stale, delete/anonymous semantics, concurrent writes serialise (exactly one wins, guards never overlap), opposite moves lock in sorted order.
- **multi-session**: background turn driver, free session switching, per-chat confirm routing ([`b018bcf`](https://github.com/xlbbb-cn/ai-chat/commit/b018bcf28a0d123d63f9939e8adc8a58008fee17))
  - src/backgroundTurns.ts: BackgroundTurnDriver, the frontend mirror of the LlmTurn base. Handing off a streaming turn keeps it listening to its own chat-* / tool-call / agent-* / confirm-required events, accumulates the transcript (control markers stripped, retry tails rewound), tracks usage like settleTurnStats, settles running tool rows and persists the assistant row (done/error/stopped with the same saveHistory semantics; stopped notifies nothing, empty failures persist nothing).
  - App.tsx: handoffForegroundTurn publishes the minted turn id (chatCompletion exposes it via turnIdHolder) plus content/reasoning/tool rows/injections/ turn stats into a driver in one synchronous arm step, then resets the foreground; driver-owned visible sessions adopt the live transcript and route Enter/Stop through the driver; clearChat and History switching no longer stop a running reply — they hand it off.
  - Global listeners skip any session owned by a driver (chat-usage, tool-call, agent-*, chat-injected, confirm-required...), so foreground and driver never double-process.
  - HistoryPanel: disableSessionSwitch removed — switching is always allowed; running conversations show an accent 'Running' badge (turn-state event + list_active_turns seed, i18n en/zh-CN/zh-TW, tokens-only CSS, reduced-motion safe); the old switch-hint block is gone.
  - Per-chat confirmations: ConfirmRequired payloads carry session_id (request_tool_confirmation signature + run_shell/run_cmd callers); background conversations surface a waiting badge and their dialog opens only when that chat is visible; answered dialogs clear the pending record.
  - pendingAutoPrompts wait for a driver-owned session to settle before auto-sending, so a resume cannot open a second turn in the same session.
  - turnMarkers.ts: goal-loop / WAIT_EVT marker stripping shared by the foreground transcript and the driver.
- **agents**: pointer-style manuals and explicit sub-agent creation ([`0a0e47b`](https://github.com/xlbbb-cn/ai-chat/commit/0a0e47b5c495c3681db74e6b87d9a70acced2601))
  - centralize all concrete prompt/manual texts in new prompt.rs
  - manuals.rs seeds reference manuals into <workspace>/.data/manuals (never overwrites user edits) and builds the pointer block
  - enhanced figures: full syntax moves into the enhanced-figures manual; the prompt now only announces the manual until a figure is drawn
  - self-evolution: skill/mcp/sub-agent creation guidance becomes a pointer; full text is read on demand via file_actions before the action
  - new agent_admin tool (list/save/delete) lets the main agent explicitly create cli/llm/cloud sub-agents, reusing save/delete core functions extracted from agents.rs; only injected when multi-agent + self-evolution
  - fix mcp_self parent_dir legacy separator normalization (cfg(windows) -> always, restores failing test on Linux)

### 重构

- **turn**: funnel progress writes through LlmTurn, close inbox on every path ([`265aa29`](https://github.com/xlbbb-cn/ai-chat/commit/265aa290fbdbbe502e555d6bc990565890581052))
  - StreamOptions now holds &dyn turn::TurnProgressSink, implemented only by LlmTurn (append_content / append_reasoning / rewind_attempt). Engine writes go through record_tool_row / record_injection / record_usage; no public progress_handle remains.
  - emit_done / emit_error close the inbox before announcing the end, and an InboxGuard in the engine covers error and unwind paths that announce nothing.
  - Move end_turn_cleanup into turn/lifecycle.rs, next to the class it tears down; chat_completion calls it from the crate-level turn module.
  - Rename LlmTurn.turn_id: u64 to window_id and ui_turn_id to turn_id, so the registry/supersede id and the UI event id no longer share one name.
  - Snapshot tool rows use the frontend's ToolCallEntry shape; move the re-attach mapping into BackgroundTurnDriver.attach() and drop TurnSeed.userMsgId.
  - Rename llm_complete/turn.rs to engine.rs to avoid colliding with crate::turn.
- **turn**: LlmTurn base + class-organized turn tree, per-session turns ([`245601a`](https://github.com/xlbbb-cn/ai-chat/commit/245601a638b37f20589c7e08154dc7e79a51c44e))
  - New turn/ class family: LlmTurn base (identity, frozen config snapshot, cancel flag, live progress, injection inbox, budget, turn-scoped emission, private MCP pool), TurnRegistry (session -> base, concurrency admission, same-session supersede), progress DTOs, guard (ex turn_guard), commands.
  - AppState: single chat_cancelled/active_turn/turn_inbox slots retired; several sessions can now run a turn at once (max_concurrent_turns config, default 4, 0 = unlimited, clamped to 16).
  - chat_completion publishes the base before any side effect; stop/inject/ list/snapshot are session-addressed; turn-state event + list_active_turns/ get_turn_snapshot commands back the History running badge and re-attach.
  - Engine (llm_complete) runs on the base: config/cancel/injections/budget/ emission come from it; progress snapshot accumulates tokens (retry rewinds the failed attempt's tail), settled tool rows, deliveries and usage totals.
  - MCP independence: AgentMcpSession -> McpSession; chat turns list/call through their OWN stdio pool (get_server_tools_in / invoke_mcp_tool_in), killed at turn-end cleanup; panel warmup pool untouched.
  - Serve processes are session-scoped (shutdown_session); turn-end cleanup no longer kills another conversation's servers; delete_history and workspace switch guarded against running turns.
  - Frontend: stopChatCompletion(sessionId); ActiveTurnInfo/TurnSnapshot API.

### 文档

- **agents**: sync architecture notes with the multi-session turn base ([`57249c3`](https://github.com/xlbbb-cn/ai-chat/commit/57249c37d56cf8f8f8765d40e8a8fc50423b61f9))
  - Turn family: LlmTurn base (turn/ class tree: mod/progress/registry/ guard/commands) owns identity, frozen config, cancel flag, live TurnProgress, injection inbox, budget, turn-scoped emission and its private MCP pool; TurnRegistry is session-keyed with same-session supersede and concurrency admission.
  - Multi-session: max_concurrent_turns, begin_turn-before-side-effects, turn-state event + list_active_turns/get_turn_snapshot, delete_history and workspace-switch guards while ANY turn runs.
  - Injection inbox moved onto the base (close_inbox); confirm-required payloads carry session_id; Stop is session-addressed.
  - Serve processes are session-scoped (shutdown_session); turn-end cleanup is end_turn_cleanup(app, turn) keyed on the registry's superseded check; the private MCP pool dies with the turn.
  - MCP: AgentMcpSession -> McpSession (alias kept); chat turns and sub-agent tasks own per-owner pools via get_server_tools_in / invoke_mcp_tool_in; the panel warmup pool stays shared only for the panel.
  - file_actions race protection: tools/file_locks.rs path locks + stale-write rejection, documented with its run_shell/run_cmd boundary.
  - Frontend: backgroundTurns driver (handoff, re-attach after reload, per-session drafts, driver-owned listeners skip, per-chat confirms).
  - Project-layout tree gains src/turn/.

### 杂项

- bump version to 3.7.0 ([`9231a3d`](https://github.com/xlbbb-cn/ai-chat/commit/9231a3d4505fddbdf595451e0aae3b035e60660d))
- **files**: drop an unnecessary mut in file_locks ([`528b83d`](https://github.com/xlbbb-cn/ai-chat/commit/528b83dd44ce23b18958e95e8433e74d370e1d18))

**完整对比**：[v3.6.2...v3.7.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.6.2...v3.7.0)

## [v3.6.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.6.2) — 2026-11-23

### 修复

- **skills**: drop skill identities the panel cannot toggle from the badge ([`f2f5395`](https://github.com/xlbbb-cn/ai-chat/commit/f2f539584f693a1555596b701f5486a029b38d32))
- **chat**: render user messages as plain text instead of markdown ([`5a753dc`](https://github.com/xlbbb-cn/ai-chat/commit/5a753dc308518873f4d9b60d3cc0aaf575b01004))

### 杂项

- bump version to 3.6.2 ([`a8bc6d5`](https://github.com/xlbbb-cn/ai-chat/commit/a8bc6d5c235762f03d40073216fa2dab1e2c29a8))

**完整对比**：[v3.6.1...v3.6.2](https://github.com/xlbbb-cn/ai-chat/compare/v3.6.1...v3.6.2)

## [v3.3.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.3.2) — 2026-11-14

### 新增

- **chat**: show turn cost and suspend open todos while waiting ([`58c910e`](https://github.com/xlbbb-cn/ai-chat/commit/58c910e4a0ddafa855294268fe5180694dea32de))
- **deliverables**: keep session deliverables and reveal them in the mission panel ([`d167e8c`](https://github.com/xlbbb-cn/ai-chat/commit/d167e8c95c392d3de6d6409b6063a1a83e039604))

### 修复

- **chat**: route turn events to the session that emitted them ([`8fd5ed6`](https://github.com/xlbbb-cn/ai-chat/commit/8fd5ed6b6a687cdf7050de5b782e2dc6550f8b18))
  - Stop now ends the main tool loop before the request-boundary drains, so a cancelled turn never announces chat-injected / background results the model will not see; the UI re-sends undelivered items as the next turn instead.
  - Every turn-scoped event carries the publishing turn's session_id and the frontend drops a mismatched payload (a missing/null id passes — planner supervisor and evolution maintenance have no transcript to leak into): chat-injected, chat-usage (StreamOptions.session_id), chat-notice, tool-call, skill-loaded, agent-task-start/done/error/skipped, agent-task-token/reasoning/tool, agent-help-*, agent-selection, agent-plan-start, agent-aggregate-done. tool-call and skill-loaded payloads are now {session_id, text} / {session_id, skill_name} objects; emit_tool_progress takes the id as impl Into<Option<&str>> so handlers that already unwrapped it can pass a bare &str.
- **workspace**: land the switch on a fresh conversation ([`8342775`](https://github.com/xlbbb-cn/ai-chat/commit/83427756adb13aa903ff8204ae89c25efc3fe10c))

**完整对比**：[v3.3.1...v3.3.2](https://github.com/xlbbb-cn/ai-chat/compare/v3.3.1...v3.3.2)

## [v3.3.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.3.1) — 2026-11-13

### 修复

- **monitor**: read the interaction log after the turn, not on a timer ([`fa800b6`](https://github.com/xlbbb-cn/ai-chat/commit/fa800b6463a3836ab2c7fae294a1d31e34794890))
- **workspace**: fall back to the app-data workspace when the path is gone ([`d15ce1b`](https://github.com/xlbbb-cn/ai-chat/commit/d15ce1b3db1fb9c6691c764324d4433a990904e4))

### 其它

- **subagent**: break llm ([`c2de7f8`](https://github.com/xlbbb-cn/ai-chat/commit/c2de7f8d7213c96253614f20fc796b1823a5fa62))

**完整对比**：[v3.3.0...v3.3.1](https://github.com/xlbbb-cn/ai-chat/compare/v3.3.0...v3.3.1)

## [v3.3.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.3.0) — 2026-11-13

### 新增

- **site**: hero web create ([`ba26871`](https://github.com/xlbbb-cn/ai-chat/commit/ba2687183591658aa2017f9dc20b60c988afcdc2))
- **ui**: unify sidebar list rows and fold orchestration settings ([`d3ce53c`](https://github.com/xlbbb-cn/ai-chat/commit/d3ce53c0aadf9a655245072404f46084227bfba4))
  - Give History, Skills, Tools, MCP and Agents one flat row language: transparent rows, inset hairlines and a --c-hover hover/focus fill.
  - Share the 36x20 toggle switch, neutralise agent type/tool badges and flatten the Tools auto-accept footer.
  - Add a leading History search glyph and clear action, one segmented tab control, and relative timestamps with a full-date tooltip.
  - Fold the orchestration rows from smart selection through max concurrency behind a shared .section-toggle; Enable mode stays visible.
  - Document the side-panel list rows and disclosure primitive in DESIGN.md.
- **models**: store the catalogue flat and match normalised names exactly ([`89789ed`](https://github.com/xlbbb-cn/ai-chat/commit/89789ed835e9aa31ba0ef34fa2fb8f5229c010f8))
  - model_metadata.rs: the merge writes the normalisation key back into `model_name`, so every stored entry is lowercase and flattened (`gpt-4o`, `glm-5-3`) and the file holds one entry per normalised name; the three parsers stay shape-specific (models.dev's nested providers, OpenRouter's `data[]` with `architecture` / `top_provider`, llm-metadata's `{data:[...]}` with comma-separated `tags`)
  - the catalogue format is bumped to 2: an older file still parses and serves as the offline fallback, but `catalog_is_fresh` (and the memory cache) treat a version mismatch as expired, so a new build re-downloads instead of matching against pre-normalisation names for the rest of the TTL
  - modelMetadata.ts: `matchModelCatalog` indexes entries by normalised name and returns the direct hit with `match_score: 1`; only unknown ids reach the existing similarity scorer (`fallbackMatch`, MIN_MATCH_SCORE unchanged), so `deepseek-chat-v3.1` still finds `deepseek-chat`
  - SettingsPanel: the Fetch Models button shows a Font Awesome spinner plus `aria-busy` while the model list and the catalogue build load (the first build downloads ~7 MB), and `.settings-btn` gains a 6px gap for the icon
  - tests: the merged catalogue must equal its own normalisation with unique keys, llm-metadata is read wrapped or bare, OpenRouter falls back to `top_provider.context_length`, and a version-1 file never counts as fresh; the parser tests now assert the stored flattened names
  - Verified: `cargo test model_metadata::` 19 passed, the ignored live merge passes against the public feeds, `tsc --noEmit` clean, `vite build` succeeds.
- **models**: merge three capability catalogues into one weekly file ([`964cf20`](https://github.com/xlbbb-cn/ai-chat/commit/964cf20d57ec1434ba7d30766a7cbf3593bebde7))
  - `src-tauri/src/model_metadata.rs`: three parsers behind `build_catalog`, a `<app_data>/model_metadata.json` snapshot (`version`, `updated_at`, `entries`) with a 7-day TTL, an atomic `.json.tmp` + rename write and a `tokio::sync::Mutex` around the refresh so two Settings panels download once. Resolution is memory -> file -> network; a single failing feed logs a WARN and the merge continues, and when every feed fails the stale file is still served, so the model list never depends on this feature. `force` bypasses both caches.
  - Real data: 2930 entries (was ~370), 2806 with a context window and 2682 with a max-output limit, 1.2 MB on disk; llm-metadata keeps 62 entries the others miss.
  - OpenRouter writes `"hugging_face_id": ""` for closed models, so the parser must require a non-empty id -- `gpt-4o` was being flagged as open weights.
  - `src/types.ts`, `SettingsPanel.tsx`: `max_output_tokens` and `sources` are surfaced as an output-limit badge plus the source list in the match hint; en / zh-CN / zh-TW catalogues and the doc comments of `api.ts`, `modelMetadata.ts`, `ModelSelect.tsx` and `App.tsx` follow the merged feed.
  - AGENTS.md, config.example.md, the store privacy policy (three hosts instead of one), `docs/gen.py` + the generated architecture page and the wiki pages no longer describe the feature as a single llm-metadata download.
  - Verified: `cargo test` 406 passed / 2 ignored (live-network), the ignored `live_merge_produces_a_usable_catalogue` passes against the public feeds, `tsc --noEmit` clean, `vite build` succeeds.
- **agents**: let the planner run on its own model ([`6be2a48`](https://github.com/xlbbb-cn/ai-chat/commit/6be2a48f9ce84a95f01cebb6b54d23cea6ce6c23))
  - agents/config.rs: `AgentOrchestration.planner_model` (missing or blank = inherit) is normalized in `normalize_config`, so load and save agree, and `planner_model_or` is the single resolver for every planner call
  - agents/planning.rs: orchestrate() resolves the model once and hands the same one to select_agents, plan_tasks and the supervisor
  - AgentsPanel: a picker after "Smart agent selection" offers "Follow the main model" plus the composer's catalogue, and keeps a configured id that has since left the catalogue
  - AGENTS.md: document the field next to Agent orchestration
- **i18n**: adopt the OS UI language on first launch ([`47b3d53`](https://github.com/xlbbb-cn/ai-chat/commit/47b3d5341ed9674b32fee71ea8ce119f4a098f97))
  - src-tauri/src/system_locale.rs: detect() reads the OS user-interface language (Windows GetUserDefaultUILanguage -> LCIDToLocaleName, macOS `defaults read -g AppleLocale`, Linux LC_ALL / LC_MESSAGES / LANG) and maps it with the same rule as the frontend's detectSystemLocale() (Hant/TW/HK/MO -> zh-TW, any other Chinese tag -> zh-CN, else -> en)
  - lib.rs setup: when config.language is missing or not one of the three, claim the detected locale and persist it in the single startup write that already seeds workspace_history; a pick in Settings or an imported backup is never overwritten, and a shadow window writes only its own copy
  - detectSystemLocale() stays the fallback for the very first paint
  - AGENTS.md: document the rule next to Theming
- **site**: add web site ([`c3e42df`](https://github.com/xlbbb-cn/ai-chat/commit/c3e42df315bbd02b9e0ad5995cfa8dc377f26085))
- **settings**: fetch models before saving; workspace switcher polish ([`03d9bc4`](https://github.com/xlbbb-cn/ai-chat/commit/03d9bc4d7ce3bcf0a8f254541105cd6bf0a3be20))
  - `fetch_models` takes the endpoint the settings form currently holds (base url, api key, custom headers) instead of reading the saved config, so "Fetch Models From API" works before "Save Changes". Nothing is written back, and an empty base url fails with a clear message.
  - Titlebar workspace list: the folder name is never truncated (it is the identity of the row), the parent path is the only part allowed to ellipsize, and the popup ceiling grew to 480px so a whole name fits.
  - Switching workspace closes the sidebar first (`onBeforeWorkspaceSwitch`), so no panel repaints against the workspace being replaced.
- **ui**: switch workspaces from the titlebar, one dropdown, no named profiles ([`d7fd4fe`](https://github.com/xlbbb-cn/ai-chat/commit/d7fd4fe3d600973cf6aa34c101cfcbb531238038))
  - The workspace indicator is the picker now: clicking it lists the recent workspaces (`workspace_history_entries`, current/missing marked) and applies one immediately through `set_workspace_dir`, which writes that single field onto the live config and is refused while a reply is streaming. The menu also offers "choose another folder" and notes that a shadow window keeps its own settings copy while sharing the history.
  - `src/components/Dropdown.tsx` is the one listbox primitive (portal, viewport clamp, keyboard contract); ModelSelect, the agents panel and the MCP inspector use it instead of their bespoke selects, and the duplicated CSS is gone.
  - The workspace row is removed: the titlebar owns `workspace_dir`, and every save path (`handleSave`, `handleLanguageChange`, `applyMessageEditor`) takes it from a fresh `getConfig()`, so a switch made while the panel is open can no longer be rolled back by "Save Changes". Self-evolution mode moved to Runtime.
  - Named profiles deleted end to end: `list_profiles` / `save_profile_config` / `delete_profile_config` / `apply_profile_config`, `ProfilesFile`, `AppState::profiles_path`, `AppState::profiles.json`, the unused `materialize_agent`, the Settings section, `ProfilePanel`, the `Profile` type and the i18n keys.
  - The backup archive no longer packs or restores `profiles.json`, and the export/import wording, menu ids and default file name say "backup".

### 修复

- **site**: generate the architecture page in the Pages deploy ([`9287bac`](https://github.com/xlbbb-cn/ai-chat/commit/9287bac2e855066151b4669cb337dfd61892a1e4))
- **i18n**: the app menu entry opens the tmp folder, not deliverables ([`720a778`](https://github.com/xlbbb-cn/ai-chat/commit/720a7783839bb22c9c52dd7f67d04a47bffd0c43))
- **ui**: keep the orchestration picker off the row label ([`e1adb75`](https://github.com/xlbbb-cn/ai-chat/commit/e1adb7563a9bf9933b9225f529095113877316e1))

### 性能

- **chat**: stop re-rendering the whole transcript on every streamed token ([`b799225`](https://github.com/xlbbb-cn/ai-chat/commit/b79922541394589bc9f5589c1138c3af7c747aab))
  - 根因：流式 token 通过 `prev.map` 只替换增长的那一行（未变行对象身份不变），但 `ChatMessage` 没做 memo，且 `handleDeleteMessage` / `handleForkMessage` 的依赖数组里有 `messages`——每来一个 token 就换一次身份，于是整段历史每个 token 重建一次
  - 新增 `messagesDataRef`（`useLayoutEffect` 同步，handler 永远读不到比已提交渲染更旧的 transcript），两个 handler 改读它并从依赖数组里去掉 `messages`：删除/分叉的语义与原先的闭包取值一致，只是不再让回调身份跟着 token 变
  - `onRetry` 只传给真正会显示重试按钮的那一行（其余为 `undefined`），它的依赖里仍有 `messages`
  - `ChatMessage` 改成 `memo(ChatMessageRow)`，另两个 memo 化 props（`backgroundTasks`/`backgroundWaiting`）只在最新一行传入，`todoMessageId` 是字符串、`skillNames`/`agentTokens` 的依赖与流式无关
  - 逐项核对：delete [t]、fork [sessionId, resetEmbeddedAgents, t]、confirm [updateActiveAssistantToolCalls([]), t]、cancelBackground [input, streaming, sessionId, backupExporting]——流式期间都不变
  - 验证：`tsc --noEmit` 无错、`vite build` 通过；本机没有 jsdom / testing-library，故未做渲染次数量化，Memo 的安全前提（props 身份稳定）由上面的依赖数组审计保证
- **db**: index the two log tables ([`e05c0e3`](https://github.com/xlbbb-cn/ai-chat/commit/e05c0e3ee0f2f5eb223a7f012ba91d61d854a42f))
  - `api_requests` 与 `interaction_log` 只有表没有索引，而它们被三类查询反复扫：Monitor 列表按 session_id 取并按 id 倒序、会话报告按 session_id 取、保留策略按 timestamp 删。加 4 个幂等索引（每表 `(session_id, id)` 与 `timestamp`），`CREATE INDEX IF NOT EXISTS` 对老库同样安全
  - 20 万行 interaction_log 实测：Monitor 列表 32 ms → 1 ms（SCAN → SEARCH USING COVERING INDEX）；保留策略 DELETE 89 ms → 0.3 ms，而这条在每次启动都会跑（lib.rs prune_old_logs）；api_requests 按会话查询 5 ms → <0.5 ms
  - cargo test 389 passed / 1 ignored

### 重构

- **site**: move the landing page into docs/site/ ([`def58d7`](https://github.com/xlbbb-cn/ai-chat/commit/def58d7e190db33212d22a2d7341fac1fdd6669e))
  - site/* -> docs/site/* (everything inside the page uses relative paths, so no markup changed; git records the moves as renames)
  - pages.yml publishes docs/site and still copies docs/architecture.html in as docs/site/architecture.html; triggers now watch docs/site/** and docs/architecture.html
  - .gitignore: drop the local preview copy of architecture.html
  - AGENTS.md, READMEs: new paths, and the note that only docs/site/ is published
  - docs/store-publishing.md -> docs/store/store-publishing.md, with the Pages section corrected: the site is deployed by Actions from docs/site/, so the privacy policy needs its own public URL
- split the three god files into topic modules ([`6079d0a`](https://github.com/xlbbb-cn/ai-chat/commit/6079d0acfde547ee9dae3d4f96cec496edf8a7b0))
  - `agents.rs` 6613 → 656 行 + `agents/{config,mission,planning,execution,helper,tests}.rs`。`#[tauri::command]` 全部留在模块根：属性会同时生成 `__cmd__*` / `__tauri_command_name_*`，`generate_handler!` 按 `agents::<name>` 取名，搬走就连不上；公开类型保持 `pub`，只有原本私有的项放宽到 `pub(crate)`，crate 外可见性没变
  - `llm_complete.rs` 5677 → 342 行 + `llm_complete/{stream,background,context,conclusion,turn,tests}.rs`。`chat_completion` 同理留在根模块
  - `tools/mod.rs` 3499 → 74 行 + `execute.rs`（run_cmd 与分发）/ `schema.rs`（工具 JSON 目录）/ `tests.rs`；`split_command_line`、`machine_label` 等被兄弟模块引用的辅助函数从 `mod.rs` 重新导出，`super::name` 路径不变
  - `instance.rs` 的测试同样移出到 `instance/tests.rs`
  - 纯搬运，没有改一行逻辑：函数名集合逐模块比对前后完全一致（agents 364、llm_complete 123、tools 544、instance 38）；`cargo check --all-targets` 零 warning，`cargo test` 389 passed / 1 ignored
- finish the profile → backup rename and dedupe drifted helpers ([`d67b058`](https://github.com/xlbbb-cn/ai-chat/commit/d67b0585561c9934290e9a84f675a4906bdb179b))
  - `profile` → `backup` 收尾：命令 app_export_profile / app_import_profile → app_export_backup / app_import_backup（Rust 命令 + TitleBar/api 包装），事件 profile-export-* / profile-restored → backup-export-* / backup-restored，前端 exportProfileTo / importProfileFrom、profileExporting|Phase|Status、CSS .profile-export-progress* 一并改名，AGENTS.md 的命令清单同步；界面早就叫 Backup，代码里的旧名到此为止
  - 新增 src-tauri/src/util.rs：now_ms（原来 5 份拷贝，外加 mcp.rs 的 u64 变体 → now_ms_u64）、truncate_chars / truncate_with_ellipsis / truncate_with_trailing_ellipsis（三种语义此前共用 truncate_chars 一个名字，差异只写在各自的函数体里）、slugify(text, max)；agents/files.rs 与 tools/mcp_self.rs 只保留各自的空值兜底（agent-<uuid> / server）
  - 新增 src/utils/format.ts：formatBytes（MonitorPanel 固定 0/1 位小数、UpdatePanel 自适应，同一尺寸在两处读数不同）、formatDate、escapeHtml（markdownMath 与 printDocument 各一份）
  - 硬编码色进 token：App.css 六个 token 块补 --c-knob / --c-favorite，三个暗色块另加 --c-syn-* 八色；ChatMessage.css 的 hljs 覆盖、三处 toggle 滑块、收藏星、三处 mask 渐变全部改走 token

### 文档

- drop winget/brew/scoop install commands until the packages exist ([`403a747`](https://github.com/xlbbb-cn/ai-chat/commit/403a747c908f84213b0a05db9a658d8799311d18))
- **store**: link the Microsoft Store listing from README, site and wiki-facing docs ([`41fcebf`](https://github.com/xlbbb-cn/ai-chat/commit/41fcebf162efd5957f901f1cec748521db7d4bc9))
- redraw the README banner with the current product name ([`bdad7f7`](https://github.com/xlbbb-cn/ai-chat/commit/bdad7f7590b123c746c5dbda911728dab121b95d))
  - docs/banner.png 重绘为 1672×941、8-bit、193 KB：暗色渐变 + 强调色光晕 + 应用图标 + 新名称 + 中英副标题 + 版本页脚，配色取自界面 token（#141E38/#3E6AE1/#9DB7FF）
  - 生成脚本可重复运行：`.data/tmp/banner/build.sh`（先量文本宽度再排版，中文用 Noto Sans CJK）
  - store-publishing.md 的待办同步：banner 已处理，商店截图仍需在装好新名称窗口的机器上重拍

### 杂项

- **icons**: refresh the app, store and favicon artwork ([`a5bb7f7`](https://github.com/xlbbb-cn/ai-chat/commit/a5bb7f7dddeb196a4a9f49b6a41d4f9dff26dcb6))
- drop the dead CSS, IPC commands and exports left by the panel rework ([`0afdbaa`](https://github.com/xlbbb-cn/ai-chat/commit/0afdbaa18bedf3c53513ded9857ba188199e22e4))
  - 死 CSS：.message-export-*（ChatMessage，已被 .message-action-buttons 取代）、.history-delete-btn 五条（HistoryPanel，已由 .history-action-btn 取代）、AgentsPanel 里退役的 “CLI agents” 列表（.cli-agents-block / -header / -intro / -empty、.cli-agent-item / -info / -command / -hint / -probe-hint），保留仍在用的 .cli-agent-probe-row 与 -probe-result*
  - 不可达的 api_requests 监控通道：db.rs 的 ApiRequestRecord / ApiRequestDetail 与 list/get/delete/clear_api_requests 四个命令、lib.rs 注册、api.ts 的两个类型与四个包装。Monitor 面板改读 interaction_log 之后没有任何调用方；api_requests 表本身仍在（llm_complete 写入、会话统计与日志保留策略在读），因此只退通道不退表
  - db::delete_session_meta 与 api/db::clear_interactions 一并删除：命令注册齐全但前端没有入口，面板的“清除”走 clear_logs
  - 未引用导出清空（审计脚本现为 total: 0）：FILE_EXTENSIONS、getActiveLocale、LegacyCliAgent、AgentMbusDeliveredEvent
  - docs/gen.py 的实测计数同步为 92 命令 · 48 事件（删掉 6 个命令后的真实值），architecture.html 重新生成
  - 验证：tsc 无错、vite build 通过（主 chunk 1 908.71 kB / gzip 601.10 kB 不变）、cargo check --all-targets 零 warning、cargo test 389 passed / 1 ignored
- **build**: trim the main chunk and refresh stale docs after the merge ([`38851b1`](https://github.com/xlbbb-cn/ai-chat/commit/38851b1bf2989baf1c2b87e86296a67cb672ace4))
  - highlight.js 改走 lib/common，再补 12 个常用语法（dockerfile、powershell、nginx、cmake、dart、elixir 等）；主 chunk 2692 kB → 1909 kB，gzip 851 kB → 601 kB
  - 删掉无引用的字体资源：.eot/.svg/.ttf、重复的 RazerF5.ttf、Web Fonts Demo.html、react.svg，共 1.39 MB；@font-face 只留 woff2/woff
  - .gitignore 里的 docs\store\screenshots/ 用了反斜杠，任何平台都匹配不到，改成正斜杠
  - CHANGELOG 用 gen-changelog.cjs 补齐 v2.9.6 → v3.2.0 六个版本（共 150 行）
  - docs/gen.py 仍写着旧名 AI Chat 与 GPL-2.0-only，重跑会把 architecture.html 改回旧值；同步为 Rabbit Chat Studio / MIT，并按源码实测刷新计数

### 其它

- gitignore ([`b81e932`](https://github.com/xlbbb-cn/ai-chat/commit/b81e9320cac9efea3e32c6dc08fab2438c4b46db))

**完整对比**：[v3.2.0...v3.3.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.2.0...v3.3.0)

## [v3.2.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.2.0) — 2026-11-12

### 新增

- add Microsoft Store (MSIX) packaging pipeline ([`88bedb5`](https://github.com/xlbbb-cn/ai-chat/commit/88bedb581601a21725941507bbb7753990fa4b9e))
  - scripts/build-msix.ps1 + 'npm run msix': makeappx packaging, 4-part version, optional dev cert
  - src-tauri/msix/: full-trust Desktop Bridge manifest, Partner Center identity in store.config.json
  - update: skip the GitHub updater on Store installs (WindowsApps path detection)
  - CI: build the .msix in the windows job, keep it as an Actions artifact only
  - docs/store-publishing.md + docs/store/: policy mapping, certification notes, privacy policy
  - scripts/capture-store-screenshots.ps1 for store screenshots

### 重构

- rename AI Chat to Rabbit Chat Studio, relicense to MIT ([`18e27e1`](https://github.com/xlbbb-cn/ai-chat/commit/18e27e100cd1296e0c471b791d8bac75d71225ce))
  - display name, productName, crate and binary become Rabbit Chat Studio / rabbit-chat-studio
  - MSIX identity target: XLBBB.RabbitChatStudio, CI stable assets renamed to Rabbit-Chat-Studio-*
  - identifier com.leonard.aichat, repo slug and the ai-chat.locale key stay put (user config continuity)
  - GPL-2.0-only -> MIT: LICENSE, Cargo/package manifests, README badges, architecture doc, store copy
  - winget id follows to XLBBB.RabbitChatStudio (winget-pkgs#458061 for Leonard.AIChat was closed)

**完整对比**：[v3.1.1...v3.2.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.1.1...v3.2.0)

## [v3.1.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.1.1) — 2026-11-12

### 修复

- **figures**: keep a diagram out of the code path in the transcript ([`a513b0e`](https://github.com/xlbbb-cn/ai-chat/commit/a513b0e8abf1b187866c93b8796767976293fc6e))

### 杂项

- **icons**: regenerate the app icons ([`4ead3ca`](https://github.com/xlbbb-cn/ai-chat/commit/4ead3ca2884d8013b1f980d3b17fbcacbbb11129))

**完整对比**：[v3.1.0...v3.1.1](https://github.com/xlbbb-cn/ai-chat/compare/v3.1.0...v3.1.1)

## [v3.1.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.1.0) — 2026-11-12

### 新增

- **chat**: write a reply's PDF export to a file, not through the print dialog ([`9028336`](https://github.com/xlbbb-cn/ai-chat/commit/9028336c152640a299b44c885a02b4c2fce2a9b3))
- **markdown**: export the editor draft to a real PDF file ([`154b5f9`](https://github.com/xlbbb-cn/ai-chat/commit/154b5f9bf4f7deb2fcaa3e14db8720196cae0bd3))
- **tools**: fix run_shell's shell at build time and carry the host OS in its schema ([`2c56c61`](https://github.com/xlbbb-cn/ai-chat/commit/2c56c61a1b24acaf479dde3cca9b065ecc870be7))
- **figures**: declarative flow charts and scene-diagram vocabulary for carve ([`dbea81e`](https://github.com/xlbbb-cn/ai-chat/commit/dbea81e4fcbf477df9ced1ec712327caffe121c6))
  - `flow` fence (or raw <flow> tag): JSON spec of nodes (6 shapes, 5 tones), labelled edges, LR/TB direction. Node sizes are measured text, positions are dagre's, colours are baked interface tokens — light/dark and every palette follow on re-render. Wide graphs scroll past 2.5x the card; tall TB charts never letterbox. Lazy dagre chunk.
  - Motion without script: anim "flow" (dashes + an SMIL packet per edge) and "step" (rank-order reveal) play on the settle commit without the script-consent gate, honour prefers-reduced-motion twice (build time and inside the emitted style), and still save as PNG.
  - carve: `edge` objects (line + cone arrowhead, dashed option), `label` sprites with a surface halo, a soft contact shadow by default, and MeshStandardMaterial shading — a scene diagram, not floating solids.
  - prompt: flow is the flowchart path, html-fig narrows to touch/ interaction, carve documents edge/label/shadow; the prompt's own flow example is test-pinned as valid JSON. #name also parses at the end of a fence line.
- **figures**: render dynamic HTML5 figures as sandboxed html-fig cards ([`398e91a`](https://github.com/xlbbb-cn/ai-chat/commit/398e91adbce01b3fcde41b2d688bf1402a9a72fd))
  - utils/htmlFigure.ts (new): token projection into the frame document, the CSP, the height reporter, buildFrameDocument/buildStandaloneDocument
  - figures.rs: save_figure_html + check_html_document, dynamic-figure bullet in the drawing prompt
  - figureCard.ts: buildHtmlCard, the execution gate and the navigation guard, FigureHandle.beforeMount/dispose
  - ChatMessage.tsx: settle-commit rebuild so a deferred frame can mount
- **glass**: keep the frost on platforms with no window backdrop ([`2a01081`](https://github.com/xlbbb-cn/ai-chat/commit/2a01081d277b12a50a2cc2cefa5c7be04b015c2b))

### 修复

- **chat**: truncate the failed turn before retrying ([`b7d3d6b`](https://github.com/xlbbb-cn/ai-chat/commit/b7d3d6bce3d5fffa0d74297acc3f77d7b7b3e0b7))
- **ui**: keep the [goal:DONE] control marker out of the chat bubble ([`89f3c51`](https://github.com/xlbbb-cn/ai-chat/commit/89f3c51148f8a7c7681a77c25898d08bf24b8fc2))
- **llm**: stop the chat turn from dying silently mid-task ([`887519a`](https://github.com/xlbbb-cn/ai-chat/commit/887519a22305c365c2944441ebcdf559c81b9258))
  - stream requests without a total timeout (read_timeout 120s instead of the 300s wall that killed every long thinking stream)
  - execute streamed tool calls also when finish_reason is "stop"/absent (relay dialects); "length" truncations are regenerated, never executed
  - continue once or twice after finish_reason "length" instead of silently ending the turn with a half answer
  - treat a stream that closes without finish_reason/[DONE] as an error instead of truncating silently
  - retry transient HTTP statuses (429/5xx) honoring Retry-After
  - retry mid-stream disconnects even when tokens already streamed: a new chat-retry event lets the frontend drop the dead attempt tail first; after 3 attempts the turn fails with the error visible
  - goal loop: with the todos tool on, a final answer does not end the turn while the list has open items; the model ends explicitly with [goal:DONE] (bounded at 4 continuations)

### 杂项

- **figuers**: op figuers system prompt ([`f2a72a5`](https://github.com/xlbbb-cn/ai-chat/commit/f2a72a57a33719a276853d14c17d77473b69ec97))

**完整对比**：[v3.0.0...v3.1.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.0.0...v3.1.0)

## [v3.0.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.0.0) — 2026-11-11

### 新增

- **composer**: embed a sub-agent role with +agent in a new conversation ([`f0f30d9`](https://github.com/xlbbb-cn/ai-chat/commit/f0f30d925d63ba343fe901d5d4f3f1530bd54e29))
- **figures**: draw with Mermaid when the enhanced toggle is off ([`260dfe9`](https://github.com/xlbbb-cn/ai-chat/commit/260dfe9fc9c4be5a922ee0e1dcb0a7200d529d04))
- **ui**: report an issue from the titlebar, above About ([`2c1c790`](https://github.com/xlbbb-cn/ai-chat/commit/2c1c790234a003cb9e2614036dd16f2b5ddb9b2a))
- **composer**: address tmp files as @@name and name attachments instead of listing them ([`00fc6a9`](https://github.com/xlbbb-cn/ai-chat/commit/00fc6a9dce17d257b270c345cae134b8c8234ef7))
- **composer**: a pasted or dropped file becomes an @ mention under .data/tmp ([`406edd5`](https://github.com/xlbbb-cn/ai-chat/commit/406edd5dda4a0e95623b8efca2ad42180bab4dbd))
- **ui**: the reply can draw, in two dimensions and in three ([`b895af2`](https://github.com/xlbbb-cn/ai-chat/commit/b895af20af3b5a25d515e7bb024753b5b178d372))
- **ui**: the transcript as glass under the desktop ([`bf865d7`](https://github.com/xlbbb-cn/ai-chat/commit/bf865d7a555f0ec0dd8292824290c9141ecccf3e))
- **ui**: spring-driven sidebar motion with framer-motion ([`1156002`](https://github.com/xlbbb-cn/ai-chat/commit/1156002e1b4adabe7ec6c04c135492cf67aa5817))
- **ui**: frosted glass as a window-level DWM backdrop ([`04553a5`](https://github.com/xlbbb-cn/ai-chat/commit/04553a5f1e480643d93f6192df91ea247a9ae95a))
  - the window is transparent and the desktop behind it is blurred by DWM, not by CSS: window-vibrancy applies the Windows 11 system backdrop DWMSBT_TRANSIENTWINDOW (Acrylic) rather than Mica, because Mica only samples the wallpaper and reads as a flat pane; Mica stays as the fallback, and apply_mica still runs first for DWMWA_USE_IMMERSIVE_DARK_MODE so the Acrylic tint follows the app theme instead of the system one
  - new set_window_vibrancy(level, dark) command, applied in setup and from Settings, returns the kind that is now in effect; <html data-vibrancy> is written only when a backdrop really is applied, so a host without one keeps the opaque canvas instead of showing a raw or black window
  - --c-window (24 / 40 / 58% of --c-surface for subtle / standard / strong) takes over --glass-backdrop under data-vibrancy: titlebar, sidebar, toolbar, input area and panel headers turn translucent over the blurred desktop while .messages keeps its opaque --c-bg, so reading and scrolling are untouched
  - Settings -> Appearance: the glass level drives the native backdrop live, and a theme change re-applies it so the Mica/Acrylic light-dark variant follows
  - docs: DESIGN.md (Window vibrancy section + the --c-window token row), config.example.md, AGENTS.md and the three i18n glass hints

### 修复

- **figures**: clean up a failed Mermaid draw and an export rejection ([`f076b32`](https://github.com/xlbbb-cn/ai-chat/commit/f076b3293a662aa1990bd2168a456639f0bde442))
- **figures**: keep a drawing inside its card and code out of the scan ([`23bbfba`](https://github.com/xlbbb-cn/ai-chat/commit/23bbfbafe61dc1fc91dd11535fdda0e4a50bd9ef))
- **chat**: broadcast the end of background tasks their turn tore down ([`aeba54c`](https://github.com/xlbbb-cn/ai-chat/commit/aeba54c877568ea51bfac7d54bf4ea53b88e8f28))
- **ui**: readable select rows and chevrons in the dark palettes ([`9c782bc`](https://github.com/xlbbb-cn/ai-chat/commit/9c782bcdfb28d097fa3a724507b74c82673fd4fb))

### 文档

- add a self-contained architecture map ([`8b0ea99`](https://github.com/xlbbb-cn/ai-chat/commit/8b0ea994391aee90084bb3430ae96e9917ab8a62))
  - docs/architecture.html: layers, one chat turn and the sub-agent pipeline as inline SVG, coloured by the design tokens in src/App.css
  - docs/gen.py: regenerates the page with no dependencies (python3 docs/gen.py)
  - README + README.zh.md: link the map from Documentation, Tech Stack and Architecture, and the project tree

### 其它

- add： update README ([`1c0fd8b`](https://github.com/xlbbb-cn/ai-chat/commit/1c0fd8bf3c32e62acaa51dee3741b8bcf11f9946))

**完整对比**：[v2.9.7...v3.0.0](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.7...v3.0.0)

## [v2.9.7](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.7) — 2026-11-10

### 新增

- show maintenance progress and results on the empty state ([`49f7618`](https://github.com/xlbbb-cn/ai-chat/commit/49f7618e7eacffcd0f3f912ed969aa006ed1c67e))
  - running events carry phase/round/toolCalls and drive the status line (Dreaming Scene N/M) that replaces the empty-state greeting
  - a completed pass leaves a clickable result bubble with the model's report; clicking it restores the greeting
  - partial passes persist a resume note in the evolution memory scope and the next pass continues from it; budget wrap-up ends with a report instead of an error

### 杂项

- bump version to 2.9.7 ([`cff16d0`](https://github.com/xlbbb-cn/ai-chat/commit/cff16d0d0b96565ab0690019ff0622b8975955f0))

**完整对比**：[v2.9.6...v2.9.7](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.6...v2.9.7)

## [v2.9.6](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.6) — 2026-11-10

### 新增

- add nap-face evolution maintenance with resumable progress ([`70a36a4`](https://github.com/xlbbb-cn/ai-chat/commit/70a36a499467f7ee41e86e71c9d1b7b9602f1902))
- **chat**: LLM-structured context compression with cross-turn replacement ([`0f52872`](https://github.com/xlbbb-cn/ai-chat/commit/0f52872502d953fd4dfc7dc0a56ddd72b471605c))
  - Summarize older context with a 9-section compact prompt (intent, concepts, files, errors/fixes, problem solving, all user messages, pending, current work, next step) via the main model; mechanical line summarizer stays as a fallback so compression never fails a turn.
  - Preserve tool traces: tool rounds render as a one-line ledger (name(args) -> result preview) in both the summarizer input and the fallback; the recent window only starts on a clean round boundary.
  - Inject the summary as a user message with continuation framing, placed before the remaining history (stable prompt-cache prefix).
  - Replace the covered history across turns: session_summaries now stores covered_messages + boundary_text; the next turn verifies the boundary and drops the summarized prefix (stale summaries are discarded safely).
  - Add a predictive pre-check (~4 chars/token, tools included, saturation guard) and reactive recovery on provider context-overflow errors (one hard compression + single retry).
  - db migration for the new coverage columns; 12 new unit tests.

### 文档

- update READMEs and CHANGELOG for v2.9.5 ([`e19dff4`](https://github.com/xlbbb-cn/ai-chat/commit/e19dff43b965809b8df27f3d234a9acb8fef88de))

### 杂项

- bump version to 2.9.6 ([`ec21d20`](https://github.com/xlbbb-cn/ai-chat/commit/ec21d209a5c178feb67d1126cbe9fd5ddb89dcd9))

**完整对比**：[v2.9.5...v2.9.6](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.5...v2.9.6)

## [v2.9.5](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.5) — 2026-11-10

### 新增

- add hopping smiley face animation to the empty chat state ([`5b7b4a5`](https://github.com/xlbbb-cn/ai-chat/commit/5b7b4a5b265bf17dde7a25652a7b4246aac3650b))
- **monitor**: give the panel a light theme and a session report ([`5222b34`](https://github.com/xlbbb-cn/ai-chat/commit/5222b34549a0a04a270dae3d75c8aa3ae122a28a))
- **instance**: run extra windows on forked configs and scope timers per workspace ([`6db0fbc`](https://github.com/xlbbb-cn/ai-chat/commit/6db0fbcb83680f561fbb99d04929e15f61a8bd70))
  - shadow windows copy config.json into shadow/config-<ts>-<id>/ with a private workspace and merge only workspace_history back on exit
  - record workspace_history in apply_config and expose it as a Settings dropdown
  - move timers.json into <workspace>/.data and rebase the registry on workspace switch
  - fix the pre-Tauri app-data lookup to dirs::data_dir() (Roaming on Windows) so startup reads the real config.json again; setup re-claims if it ever diverges
  - keep a shadow's cleared workspace_dir inside its own fork

### 重构

- **palette**: rename the fig palette to sakura ([`26f083d`](https://github.com/xlbbb-cn/ai-chat/commit/26f083dcf4b146ef7d9b3d3607e30f24b8053e41))

### 杂项

- bump version to 2.9.5 ([`307624a`](https://github.com/xlbbb-cn/ai-chat/commit/307624a14221c4154e65dcc84fdfbe5f8122f2c6))

### 其它

- **chat**: reveal streamed replies character by character ([`65940df`](https://github.com/xlbbb-cn/ai-chat/commit/65940df786f433dd6c42f2445e575668111ae057))
- **panel**: unify the side panel header chrome ([`ddb3891`](https://github.com/xlbbb-cn/ai-chat/commit/ddb389152fe1947ff0b23177aa625ed7fb45aea9))
- **sidebar**: crossfade panels instead of sliding the frosted surface ([`a284336`](https://github.com/xlbbb-cn/ai-chat/commit/a284336501fb099ad5bc5c57f2b01ee5251ae174))
- **chat**: slide the panel selection and deepen the typewriter fade ([`2a7eb51`](https://github.com/xlbbb-cn/ai-chat/commit/2a7eb51787cc84b035b1494dee06b0e1d00e0fd5))
- **titlebar**: underline the active workspace label in red ([`04cd90a`](https://github.com/xlbbb-cn/ai-chat/commit/04cd90a676078b4a43537f925cdd59ebf83792e2))

**完整对比**：[v2.9.4...v2.9.5](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.4...v2.9.5)

## [v2.9.4](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.4) — 2026-11-09

### 新增

- **release**: generate release notes and drop the Windows MSI bundle ([`4701f4b`](https://github.com/xlbbb-cn/ai-chat/commit/4701f4bcdbd50612affe9df1af377024bcc8ab41))
- **agent**: cap the turn budget and disable a failing tool for the session ([`fdf28bd`](https://github.com/xlbbb-cn/ai-chat/commit/fdf28bd26cbde4fd5eee20c3e19e1ecc2e1a0b0e))
  - turn_guard.rs: TurnBudget caps requests and tool calls per turn, ToolFailureGuard disables a tool after N consecutive failures for the rest of the session. Both are pure logic and unit tested.
  - The main loop, the sub-agent loop and run_main_agent_helper share the guard, so a tool the chat gave up on stays disabled for its sub-agents too.
  - When the budget is spent the turn sends one wrap-up request that keeps the identical tool list and only sets tool_choice=none, so the prompt prefix and its KV cache survive; a rejected wrap-up ends the turn with a fallback answer instead of an error.
  - Tools disabled mid-turn stay advertised until the next turn and are refused by the tool loop, for the same prefix-cache reason.
  - Settings -> Advanced gains main_agent_max_rounds (40), main_agent_max_tool_calls (120) and tool_failure_limit (3); 0 means unlimited and every value is clamped on save.

**完整对比**：[v2.9.3...v2.9.4](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.3...v2.9.4)

## [v2.9.3](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.3) — 2026-11-08

### 新增

- **mcp**: let the assistant build and run its own MCP servers ([`9656a3c`](https://github.com/xlbbb-cn/ai-chat/commit/9656a3c0fa8a97d22d0a4053010152252964700d))
  - `mcp_self` tool (`tools/mcp_self.rs`): `list` / `write` / `enable` / `disable` / `test` / `restart` / `logs` / `remove`. The script lives in `<workspace>/.data/mcp/<slug>/` (`.bak.<n>` on update); the interpreter is picked by the backend from a whitelist (`python` / `node` / `shell`), never named by the model, so the tool can only turn workspace code into a server, not register an arbitrary command. Writes, enabling and removal ask the user once (`user_software`, pre-approvable via `auto_accept_confirm_kinds`).
  - Two rosters merged on read: manually created servers stay in the app-data `mcp_servers.json`, self-built ones live next to their script in `<workspace>/.data/mcp.conf` (YAML, JSON tolerated). `load_effective_servers` merges them with the panel entry winning on an id clash; panel save/delete route by provenance, so a self-built row is edited through the workspace file and is never duplicated into app data. Startup migrates entries an earlier build wrote into app data.
  - `AppState::mcp_revision` + `build_turn_tools`: the tool payload is rebuilt at each request boundary once the registry changed, so write -> enable -> call works inside one turn instead of the next conversation turn. Sub-agents and the main-agent helper drop `mcp_self` (`agents::drop_mcp_self`): they would race on the roster, an `AgentMcpSession` only reads servers at task start, and the confirmation dialog would block a mission.
  - A start that fails `tools/list` leaves the server disabled so it cannot stall later requests, and the error carries the log tail; `logs` returns the tail on demand so the model can fix its own script.
  - Profile export/import carries `workspace/mcp.conf` and now really restores the app-data `mcp_servers.json` (it was exported but ignored on import), then retires live stdio children so the restored roster takes effect.
  - Docs: AGENTS.md (tool family, `.data` layout, self-evolution bullets, MCP warmup), DESIGN.md (provenance badge spec), the self-evolution system prompt, and the MCP panel provenance badge plus a refresh on `mcp-servers-changed`.

### 性能

- **prompt**: shrink the first request and stop preloading evolution facts ([`2c29981`](https://github.com/xlbbb-cn/ai-chat/commit/2c29981521279a9c35a35acb9eff73dcf0737db7))
  - Tool schemas (`get_all_tools`, `mcp_self`, `skill_read`): rendered JSON for the 13 main-agent tools drops 28 731 -> ~23 600 characters (-18%) with the parameter shapes unchanged. `run_shell` reuses `run_cmd`'s risk-level wording, the MCP stdio contract is documented once in the `mcp_self` schema, and the timer/todo/background descriptions keep the mechanics while the system prompt keeps the policy.
  - System prompt: `self_evolution_prompt` 4 475 -> 3 175 characters (-29%), plus compacted skills/agents lists (one line per entry), memory/timer/todo guidance, the explicit-invocation line and the active-skill context.
  - Orchestration: selector, planner, supervisor and the sub-agent static/mission prompts lose duplicated sentences (-8%..-17%); the JSON contracts and the capability-fit rules stay verbatim.
  - Evolution memory is no longer preloaded into any request (it could take the whole ~4,000-character budget on every turn): the prompt only tells the model to search the scope when a problem needs earlier experience. `render_evolution_context` stays as the `#[cfg(test)]` reference renderer; `EVOLUTION_CONTEXT_BUDGET_CHARS` still drives the panel's top tier and the `consolidate` plan.
  - Memory panel and i18n (en / zh-CN / zh-TW): "in prompt" becomes "top tier"; AGENTS.md documents the on-demand policy instead of the VERIFIED FACTS block.

### 重构

- **kg**: extract the knowledge graph client into its own crate ([`fa6516f`](https://github.com/xlbbb-cn/ai-chat/commit/fa6516faa1ba7faa0058b7b399a4dec006a78b22))
  - The app crate drops `neo4rs` and `async-trait` (only that client used them), so the binary no longer links a Neo4j driver for code it never calls.
  - `lib.rs` stops declaring the module through `#[path = "utils/knowledge_graph/mod.rs"]`: `src/utils/` is a plain folder holding standalone crates, not a module.
  - The crate's tests are targeted by package name (`cargo test -p knowledge-graph`), because the app crate's default build only covers the root package.

### 文档

- **changelog**: add the v2.9.2 section ([`b77a190`](https://github.com/xlbbb-cn/ai-chat/commit/b77a190c4ef92830ebeaf3995c35bbb0c1531422))

**完整对比**：[v2.9.2...v2.9.3](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.2...v2.9.3)

## [v2.9.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.2) — 2026-11-08

### 新增

- **release**: generate the changelog and release notes from commits ([`874a572`](https://github.com/xlbbb-cn/ai-chat/commit/874a572b3c4c83c9f42c4f61678ced6a43768bfd))
  - `--write` upserts the section into CHANGELOG.md and replaces a section for the same version, so re-running a release is idempotent.
  - `--format body --output <file>` produces the short list the workflow needs.
  - `release.yml` gains a "Generate release notes" step: it resolves the previous tag with `git describe --tags --abbrev=0 "${GITHUB_REF_NAME}^"`, renders the body, and hands it over through `$GITHUB_OUTPUT` with a heredoc delimiter (a plain `echo` would flatten the newlines). `releaseBody` embeds it above the install notes.

### 重构

- **tools**: retire the knowledge graph tool, keep its client in utils ([`fd38515`](https://github.com/xlbbb-cn/ai-chat/commit/fd38515e09b8fe3088000fb137b75630bf362cd8))
  - `src-tauri/src/utils/knowledge_graph/` is a self-contained module library: `mod.rs` (the `KnowledgeGraph` trait), `settings.rs` (`Engine` + `Settings`) and `neo4j.rs` (`Neo4jRepo`). It is registered from the crate root with `#[path]`, so `src/utils/` stays a plain folder next to the `pdf2md-rs` crate instead of growing a `mod.rs` of its own.
  - `Settings` owns the connection defaults (`bolt://localhost:7687`, user `neo4j`, empty password) that used to live in `AppConfig::default()`, so re-enabling the tool is a tool definition plus a dispatch arm calling `Neo4jRepo::connect(&Settings)`.
  - `AppConfig` loses `kg_engine` / `neo4j_uri` / `neo4j_user` / `neo4j_password`. A `config.json` written by an older version still parses — serde ignores the unknown keys — and the next save drops them.
  - `RETIRED_TOOL_IDS` with `AppConfig::strip_retired_tools()` prunes the stale id on startup, on every save, in `get_config` and in `effective_tools()`, so a selection carried over from an old config cannot reach the prompt.
  - The tool definition, the dispatch arm, the Tools-panel row, the engine selector, its CSS and the i18n keys are gone. `neo4rs` stays a dependency of the library, and `build_config` is split out so the settings → driver mapping is covered by a test that never touches the network.

### 杂项

- keep the local test book PDF out of the repository ([`5d108f2`](https://github.com/xlbbb-cn/ai-chat/commit/5d108f219b342e4792f55ccfc21294e64ece8447))

**完整对比**：[v2.9.1...v2.9.2](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.1...v2.9.2)

## [v2.9.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.1) — 2026-11-08

### 新增

- **composer**: show PDF mention conversion progress, store it by path ([`850a6bd`](https://github.com/xlbbb-cn/ai-chat/commit/850a6bd784ff04913d0cbb3ac9fe81f617ac5d8f))
  - `convert_pdf_mention` takes a `tauri::ipc::Channel<PdfMentionProgress>` and forwards the pipeline's own ticks (stage, page/total, percent), deduped on stage + page + total so a fast conversion stays a few frames;
  - `convertPdfMention` / `resolveFileMentions` take an optional callback and tag every tick with the file it belongs to; `sendMessage` renders the strip and clears it in a `finally`, so a failed or cancelled mention leaves nothing behind;
  - the strip is one muted 12px row with a page counter and the same 2px accent track as the context bar, in flow next to the injection bar (DESIGN.md).
  - `compactAttachmentsForDb` drops `text_content` from any attachment that has a `path` — the Markdown already reaches every later request through the message `content` — so a stored PDF mention keeps one string plus the cache path;
  - live attachments keep their text, so sending never depends on a cache file that may since have been pruned, and a text slice without `path` is untouched.
- **composer**: convert `@paper.pdf#2` mentions to Markdown ([`c703f4e`](https://github.com/xlbbb-cn/ai-chat/commit/c703f4eaa28ca7421efbbe5f64adf2ab81ad2400))
  - `pdf_mentions.rs`: page planning (a bare mention converts a 60-page preview, `#2-` runs to the last page), 48 MiB and 60k-character guards, cache keyed by path + range + file size + converter version, and a provenance comment that names the cache file so a later tool call can read the rest. Conversion runs in `spawn_blocking`, so a hundred-page mention cannot freeze the UI.
  - The cache keeps the *full* conversion (`prompt_markdown` only budgets the copy that goes into the prompt, note included).
  - `pipeline::page_count` lets the range be clamped before converting, which is what makes "page 9 of 30" a better error than a failed range.
  - `supportsMentionName` keeps PDFs in the popup for models without `supports_files`; `fenceLanguage` fences the inlined Markdown as `markdown` instead of letting the `kk.pdf#2` label become the fence tag.
  - AGENTS.md documents the flow next to the other attachment rules.
- **pdf2md**: ship pdf2md-rs as an embeddable document library ([`1ce2257`](https://github.com/xlbbb-cn/ai-chat/commit/1ce2257e5978dc1a9f858f3ed93471e85eff464c))
  - read `/FirstChar`, `/Widths` and `/MissingWidth` and advance the pen by the font's own metrics. Runs that resume from an explicit `Tm` no longer overlap the text before them (`the GNU` used to interleave into `tGhNeU` on real papers).
  - decode literal strings that are valid multi-byte UTF-8 as UTF-8 before falling back to the font encoding, so bullets stop turning into `¢`.
  - group lines into paragraphs by the page's own leading (smallest gap in the column, capped) instead of a fixed fraction of the font size, and drop the "short line ends a paragraph" rule that split ragged-right text at every line break.
  - grow the measured text box to the mirrored margin so a centred title or equation no longer defines the right edge of the page — which is what the formula centring test compares against.
  - ruled tables assign words, not lines, to cells; all-empty trailing columns are trimmed and a grid with a single column is not a table.
  - aligned-text tables must repeat the same number of cells at the same x on most rows, so a schematic or a poster is no longer one giant table.
  - the "by A. Author" line becomes metadata only, and list markers are stripped before the IR keeps the item text.
  - inline math requires a short variable and one short subscript, so identifiers such as `Conn_02x10_Odd_Even` stay plain text; `*` and `/` are operators, which keeps `y[n] = h[n] * x[n]` in one math run.
- **pdf2md**: add the pdf2md-rs extractor crate as a workspace member ([`328a14a`](https://github.com/xlbbb-cn/ai-chat/commit/328a14acd59ebd57aeab3a527129d1f5e507e64e))
  - `src-tauri/Cargo.toml` gains `[workspace] members = ["src/utils/pdf2md-rs"]`, so the extractor is built and tested by the same toolchain as the app; the root `Cargo.lock` picks up its dependencies (lopdf, image/png, thiserror, serde, and the optional GROBID stack: ureq + quick-xml).
  - The crate holds the PDFOxide text-extraction layer (font tables and ToUnicode CMaps, image re-encoding, content-stream decoding), a line/word layout pass with reading-order sorting, the unified IR, a Markdown router with heading and table parsers, formula handling and optional GROBID layout backend.
  - Landing it as-is keeps the scaffolding reviewable: `src/lib.rs` is still the placeholder and declares none of the modules, and `src/parse/` has no `mod.rs`, so `cargo check -p pdf2md-rs` passes without compiling any of the new code. Wiring the module tree is the follow-up commit.
- **chat**: float warnings and errors above the transcript ([`2cf5104`](https://github.com/xlbbb-cn/ai-chat/commit/2cf51045575e13009096a231c5be8061e24383cb))
  - Warnings and errors were a bar inside the message flow: it pushed the conversation around and stayed on screen until the next request. `ChatNotice` renders them at the top-left of the chat window instead, in a zero-height anchor strip below the toolbar, so nothing is reserved and nothing shifts. A notice leaves on a click and on its own after five seconds (opacity on `--t-dur` / `--t-ease`); one that carries an action waits instead, so its button cannot vanish under the pointer reaching for it.
  - The failed turn's own failure row is gone — the notice is the only report, and it carries "continue" when the turn left something to resume. `resumeAction()` withholds that action from turns that produced nothing at all (an immediate rate limit has nothing to push forward). A failed turn also stops offering PDF export: there is no finished reply to hand out, and with the failure row gone that icon was all that remained at the bottom.
  - Injections longer than three lines fold to three with a measured `Show more` / `Show less` (the `.inline-edit-btn` primitive at message density), so one turn stays scannable. The fold is measured after paint rather than counted from newlines: a single long line wraps past three rows and a short note may sit on two, so a note that fits never gets a dead toggle.
  - Size overrides on `.inline-edit-btn` now use compound selectors (`.inline-edit-btn.xxx`): the bundled sheet emits App.css after the component CSS, so single-class overrides lost the specificity tie and rendered at the primitive's 28px / 13px.
  - New i18n keys `chat.foldMore`, `chat.foldLess`, `app.noticeDismiss`; `app.errorPrefix` dropped along with the row that used it. DESIGN.md documents the notice surface and the three-line fold.

### 修复

- **pdf2md**: read the font's own CMap, and fall forward when there is none ([`c9ac2ed`](https://github.com/xlbbb-cn/ai-chat/commit/c9ac2edbad30e8de2e34bc4b348228b1ef0b6a95))
  - `cmap.rs` reads `ToUnicode` itself, tolerantly: code space ranges, `bfchar`, all three `bfrange` forms, comments, odd-length hex, several sections per CMap, and headers that do not parse at all. Codes are matched longest-first; codes the CMap does not cover are counted, not fatal, so a partial map still decodes with holes instead of collapsing into mojibake.
  - `fallback.rs` covers fonts that genuinely carry no CMap. A predefined CMap name states the code space outright (`Uni*-UCS2/UTF16` → UTF-16BE, `GBK-EUC`/`GBK` → GBK, `BigFive`/`CNS`/`ETen` → Big5, `*RKSJ` → Shift_JIS, `EUC` → EUC-JP, `KSC*` → EUC-KR), and so does a `UCS` CID ordering, where the code *is* the code point. With neither, the CJK byte encodings are tried in the order the font name suggests and kept only when the text could not plausibly be anything else: ≥40% CJK under a CJK name, 100% without one, and never a replacement or control character — a Latin run is never "rescued".
  - `fonts.rs` folds that into one ladder: own CMap → declared code space → UTF-8 sniff for simple fonts → gated guess → `lopdf`'s table → Latin-1, so no byte is dropped silently.
  - `pdf2md-rs` 0.2.0: the PDF mention cache digests the converter version, and this changes what the converter returns, so conversions cached from the mojibake build are invalidated instead of replayed.
  - `encoding_rs` supplies the GBK/Big5/Shift_JIS/EUC-KR tables; it was already in the lock as a transitive dependency.
- **pdf-mentions**: expire cached conversions after a week ([`6ab8b27`](https://github.com/xlbbb-cn/ai-chat/commit/6ab8b27396f912087dbc29fe7f8598d7618b3797))
- **test**: refuse Windows paths on every host ([`1c8eea1`](https://github.com/xlbbb-cn/ai-chat/commit/1c8eea1d433bd0eaa01e4e756b08c5344a2b7c1f))

### 文档

- **pdf2md**: describe the crate as a library in its front page ([`7ba75a1`](https://github.com/xlbbb-cn/ai-chat/commit/7ba75a10605534e51430cc8301f2ed2c914e8e7e))

**完整对比**：[v2.9.0...v2.9.1](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.0...v2.9.1)

## [v2.9.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.0) — 2026-11-07

### 新增

- **composer**: add a /skill picker beside @file mentions ([`111e228`](https://github.com/xlbbb-cn/ai-chat/commit/111e228a1aa93125ac8e23e9fd3107c882abbd92))
  - `src/slashMention.ts` mirrors the `@` grammar: the `/` must open the line or follow whitespace and may not cross whitespace, `@` or `#`, so at most one popup can be open.
  - The skill list is read from the workspace at most every 15s and ranked by relevance with enabled skills first. Nothing is filtered out: `use_skill` resolves any workspace skill, enabling only preloads its description.
  - Accepting inserts a plain `/skill-name` token and leaves resolution to the model. Known skill tokens in a user message wear the `@file` accent, so `/usr/bin` and `1/2` stay plain text.
  - Also threads the session id and the only-newest-assistant flag into the working-plan panel, which shares these two files.
- **chat**: mirror the session working plan in the transcript ([`9f3405c`](https://github.com/xlbbb-cn/ai-chat/commit/9f3405c1390b7a497e42973295dd2be01a1939f0))
  - Read the session's active list on mount (`get_active_todo_list`) and keep the event stream on top of it. A sequence guard stops an in-flight fetch from overwriting a fresher event snapshot, and events are filtered by session so parallel sessions cannot mirror each other's list.
  - Snapshot the list once per turn in `run_chat_turn` and feed both channels from that one snapshot: the `ACTIVE TODO LIST` prompt block and a single `todo-state` event. Finished lists are still pushed — that event is what makes a 100% panel disappear.
  - Collapsed panel is two lines: title with the completed/total counter, then the running task (or the first pending one when nothing is running). The description line is gone.
  - Nothing pending and nothing running hides the panel entirely, the same rule the prompt block already followed.
  - Only the newest assistant message mirrors the list; every other message renders nothing, which also stops historic replies from each drawing the same plan.
- **chat**: group background tasks and serve processes in the transcript ([`095b29d`](https://github.com/xlbbb-cn/ai-chat/commit/095b29dc9df998a7bdcbb5a1a8fa85420ba3caff))
- **chat**: keep background task results out of the transcript ([`7594a0f`](https://github.com/xlbbb-cn/ai-chat/commit/7594a0f684204d9f8dfe06ec0ef4de8520dc2a14))
- **chat**: accent the @file mention instead of listing attachments ([`3bbc0d1`](https://github.com/xlbbb-cn/ai-chat/commit/3bbc0d1d85cd903291222a87a325311bb6fffb9e))
- **composer**: @file mentions with line ranges in the input box ([`e1007d7`](https://github.com/xlbbb-cn/ai-chat/commit/e1007d7585cf21df90c957118708afa2450ffdd7))

### 修复

- **sleep-lock**: hold logind and the desktop screensaver ([`32b9a86`](https://github.com/xlbbb-cn/ai-chat/commit/32b9a8613882519dabe56e6647c36a5c6b199820))
- **window**: keep the native menu bar to macOS only ([`7cc145f`](https://github.com/xlbbb-cn/ai-chat/commit/7cc145ff0f675ab013ff4e85f3a0d7665542de24))

### 重构

- **composer**: drop the paperclip picker in favour of @file mentions ([`a6fa5eb`](https://github.com/xlbbb-cn/ai-chat/commit/a6fa5ebdf5d416f8ab426de742585f5d1d7d5763))

### 杂项

- **deps**: re-align package-lock with package.json ([`24aa10c`](https://github.com/xlbbb-cn/ai-chat/commit/24aa10c84fccccaf1907845789d3cf65b96e4572))

### 其它

- add .data git ignore ([`b9fb537`](https://github.com/xlbbb-cn/ai-chat/commit/b9fb537b10fb6f891b51e664c37a29bef13e6ef8))

**完整对比**：[v2.8.9...v2.9.0](https://github.com/xlbbb-cn/ai-chat/compare/v2.8.9...v2.9.0)
