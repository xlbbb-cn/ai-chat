# 提示词下沉评审：哪些提示词应该搬进工具代码

评审对象：本仓库 `src-tauri/` 里所有面向模型的提示词（系统提示的固定策略块、工具 schema 描述、手册 pointer）。
问题：哪些内容本来不需要模型"记住并遵守"，而应该由工具代码直接判定/拒绝/容错。

---

## 0. 结论

1. **每轮固定注入的策略文本 ≈ 5.0k 字符**（常规模式；自演进模式再加 3.6k，配置了子智能体再加 0.9k）。工具 schema 里的 97 段 `description` 另计 **≈ 12.0k 字符**。后者是最大的单块提示面，但"把话搬进 schema"不省预算——只是换了个位置。
2. 按可下沉程度分三类：
   - **A. 提示词在复述代码已经强制的事实** → 直接删，零风险。约 1k 字符。
   - **B. 代码里已有状态（读记录、计划、注册表、开关），但只当"说明"用** → 改成硬校验/容错/点处注入。约 1.5k 字符，且能真正消除一类失败（误写记忆、跳过 plan、盲改未读文件）。
   - **C. 语义判断与产品口味**（何时算"定论"、用哪个子智能体、要不要问用户）→ 不要下沉，下沉只会得到更多工具，而不是更少提示词。
3. **最值得先做的三件**：`memory` 的 plan→apply 与 `.data/memory` 写保护（现在只有提示词在兜底，走偏的代价是数据）；`patch` 复用已有的读记录做"先读再改"；`[goal:DONE]` / `[WAIT_EVT:]` 从静态协议瘦成 nudge 里的一句。

---

## 1. 判据：一条提示词能不能下沉

三个问题**全是"是"**才值得下沉：

| 判据 | 含义 | 反例 |
|---|---|---|
| 可判定 | 判定所需信息全在代码里：文件状态、本会话的调用历史、配置开关、磁盘路径 | "if the request clearly needs the skill" —— 需要语义 |
| 可闭合 | 工具能给出一个确定的后果：拒绝、改写、自动补全、换一条等价做法 | "tell the user when it will fire" —— 没有判定点 |
| 不丢信息 | 用户看到的结果仍然可解释，工具能在返回值里说明原因 | 静默改写输入且不写原因 |

规则里出现 `clearly needs` / `prefer` / `definitive conclusion` / `trade-offs` / `only when the user really asks`，基本就是不可下沉的信号。

把"规则形态"和可下沉性对齐（这是本仓库最有用的经验）：

| 规则形态 | 承载它的状态在哪 | 可下沉性 | 手法 |
|---|---|---|---|
| 单次调用内的形状约束（路径、参数、格式、编码） | 调用参数本身 | 强 | 拒绝 + 容错 |
| 单会话内跨调用（先读再改、先 plan 再 apply、别重复 arm 定时器） | `FileLockRegistry`、`timer` 注册表、会话内存 | 强 | 前置条件校验 |
| 跨会话持久状态（记忆、技能、脚本库） | 工作区文件 | 中 | 工具侧校验，但"值不值得记"仍是判断 |
| 语义偏好与取舍（用哪种语言、要不要委派、要不要问用户） | 无 | 不可 | 保留在提示词 |

---

## 2. 标尺：仓库里已经做对的六处

这些是"下沉"的现成范式，评审其他条款时用它们当标准：

| 位置 | 做法 | 为什么对 |
|---|---|---|
| `tools/schema.rs:104-107` | 机器的 OS / 脚本语言写进 `run_shell` 的 schema | 只在工具启用时才占预算，且和"谁来执行"绑在一起 |
| `tools/paths.rs:270` | `.data/chat.db` 直接拒绝，提示词里不再解释为什么 | 边界由拒绝动作表达，不需要模型理解 |
| `tools/skill_admin.rs:103` `clean_skill_md` | 模型套了 ```` ``` ```` 围栏就剥掉，而不是再叮嘱一次 | 容错优于教育，这是最该复制的一条 |
| `tools/todos.rs:427-436` `settle_interrupted` | "别留假的 in_progress"变成状态机（Stop 时回落 pending） | 规则的执行者是代码，模型不必记得 |
| `turn/guard.rs:146,195` | 同一个工具连续失败 → 禁用并给出原因 | 把"别死循环"从劝告变成机制 |
| `llm_complete/engine.rs:2622` | 目标循环的 nudge 里就地复述 `[goal:DONE]` 的用法 | 点处注入：指令出现在真正需要的那一刻，而不是每轮常驻 |
| `prompt.rs:30` + `manuals.rs` | 手册只注入 pointer，正文按需 `file_actions read` | 已经证明"全文常驻"是可以省掉的 |

---

## 3. 表 A：可删——提示词在复述代码已经强制的事实

零行为风险，纯预算收益（主要省 schema 的 12k）。

| # | 位置 | 原文摘要 | 代码在哪强制 | 动作 |
|---|---|---|---|---|
| A1 | `schema.rs:78` `run_cmd.command` | "first word is a library script name or a bare PATH program (never a path and never a shell interpreter such as bash/sh/cmd/powershell)" | `execute/run_cmd.rs:49`（拒绝路径）、`:81` `reject_shell_interpreter` | 缩成一句"脚本名或 PATH 程序"，拒绝理由由错误信息给 |
| A2 | `schema.rs:147` `file_actions` | "anything resolving outside is refused… `.data` 可写（除 chat.db）" | `paths.rs:248-301` | 删，留 `./...` 路径写法示例 |
| A3 | `schema.rs:258` `memory` | "add/delete need one concrete scope" | `memory.rs:3041-3046` 返回 Error 并列出可用的 scope | 删 |
| A4 | `schema.rs:390` `todo_id` | "copy it from a 'list' result, never invent one" | `todos.rs:375` `Todo not found` | 删 |
| A5 | `schema.rs:434` `questions` | "1–8 per call" | `todos.rs:478` `ASK_MAX_QUESTIONS`，越界有明确 Error | 只保留一次（schema 是唯一权威） |
| A6 | `schema.rs:218` `patch` | 四步操作法（先读、3 行上下文、抄行号、一 hunk 一改） | `patch.rs:547,611,640` 失败时已给出"下一步怎么做"+ 最接近的真实内容 | 缩成 1 行"失败信息会给出最接近的原文与下一步" |
| A7 | `tools/skill_admin.rs` 的 `skill_merge` 区块 | "only self-evolution skills may be merged" | `skill_admin.rs:183,209` | 删 |

---

## 4. 表 B：建议下沉成硬校验（要写少量代码，但消除真实失败）

| # | 现状 | 已有数据 | 下沉后的机制 | 收益 / 风险 |
|---|---|---|---|---|
| **B1** | "plan 先、apply 后""只在用户明确要求整理记忆时才归档"只写在 `skills.rs:337-357`（`archiving` 段）和 `engine.rs:189-195` | `memory.rs:2393-2416` `consolidate_report`；`memory.rs:2579` `wants_memory_consolidation` 已存在但只用于拼提示词 | 会话级记录上次 `plan` 的 cluster/key 集与时间戳；`apply` 时校验 `keys ⊆ 上次 plan` 且 plan 在本次会话内，否则返回 Error + 当前 plan 摘要。另外 `apply` 前校验 `wants_memory_consolidation(最后一条用户消息)`，并在子智能体上下文里直接禁用 | 消除"没看 cluster 就写 skill 并归档"；提示词那两段可整段删除 |
| **B2** | `schema.rs:258` 说 `user`/`repo` 是 app-managed、"never edit them with file tools"，但 `paths.rs` 只保护了 `chat.db` | 目标路径本身 | `ensure_mutation_target_allowed` 增加一条：`.data/memory/**`、`.data/todos/**` 的 mutation 一律拒绝，错误里点名应该用哪个工具的哪个 action | 现在这是**唯一防线是提示词**的地方，一次误写就破坏 frontmatter |
| **B3** | `patch` 的"先读、再抄"只写在 schema | `file_locks.rs:45,66` 已有 `(session, path) → 读时间`，`file_actions.rs:74` 已在记，但 `patch` 不消费 | patch 前查读记录：本会话没读过 → 返回 Error + 文件头 40 行，让模型"顺手"读到 | 顺带把 A6 的 4 步压到 1 行；误 patch 的概率显著下降 |
| **B4** | `engine.rs:445-451` 要求"先 `timer_list` 避免重复""告诉用户什么时候触发" | `timer.rs` 注册表已有 `count_for_session` | `timer_set` 遇到同 session 同 label/同 message 的 pending timer 直接返回既有 `timer_id` + 触发时间；结果文本里固定写"将在 … 触发，请在回复中告知用户" | 定时器策略从 550 字符降到 1 句 |
| **B5** | `prompt.rs:33-42` 要求"首次动作前先读手册，不要凭猜" | `file_actions` 的读记录（同 B3） | `mcp_self write` / `skill_merge` / `agent_admin save` 前校验对应 manual 是否本会话读过；没读就把手册正文塞进 Error 里返回 | pointer 里那句 "do not perform the action from guesswork" 可以删 |
| **B6** | `engine.rs:509-517` DELIVERABLES（614 字符）+ `schema.rs:147` | `workspace.rs:82` `tmp_root`、`workspace_files.rs:452` `sweep_composer_tmp` | `file_actions write` 目标在 tmp_root 时，在结果里附一行"scratch 区，一天后清理；交付物放 `./<project>/`"；回合结束时若本轮写过 tmp 而未复制出去，在回合尾提示一次 | 从"每轮 614 字符"降到"只在发生时出现的 1 行" |
| **B7** | `tools/scripts.rs:784-797` 的 SCRIPT LIBRARY 保存策略（911 字符） | `run_cmd` 的脚本库有 use count；`list` 已有 | 同一条命令行第二次成功时，在结果里附"存成 `.data/scripts/<name>.ps1`"；`run_cmd list` 校验 `@description` / `@usage` 头注释，缺失时在列表里标出来 | 策略段可压到 3 行；不写头注释的脚本会被自动标红 |
| **B8** | `engine.rs:74-79` explicit invocation：仓库里**没有任何解析器**（Rust 和 TS 都只命中这句提示词） | 用户消息文本 | 二选一：删掉；或在用户消息里检测到 `{tool:}` / `{skill:}` / `{mcp:}` 时才注入这段，并把 `{skill:<name>}` 直接翻译成 `skill_ids` 自动加载 | 315 字符常驻 → 条件注入；`{skill:}` 本来就该是确定性的 |

---

## 5. 表 C：不建议下沉（附理由）

| 条款 | 理由 |
|---|---|
| `MEMORY`：何时算"定论"、选哪个 scope | 纯语义判断。`memory.rs:1160` 那条自动结论记录已经是"代码兜底"的正确形态：判断仍归模型，代码只做漏记的补救 |
| `DELIVERABLES`：交付物该进哪个项目目录 | 需要理解任务语义（"这个任务属于哪个项目"） |
| 技能是否"clearly needed"、要不要投机加载 | 语义判断。工具能做的是加载后回显内容，而不是替模型决定 |
| 子智能体类型偏好（`llm` 推理 / `cli`、`cloud` 端到端编码） | 产品口味 + 任务判断 |
| `ASKING`：该不该问、问什么 | 语义判断。工具侧已经把可确定的部分收下了（1–8 题、选项校验、表单不阻塞） |
| `calculator` 的"绝不猜数字" | 无法判定"是否猜的"。真要收，只能靠对最终答案做数字核验，成本远大于收益 |
| `PYTHON_SECTION`（`prompt.rs:201`）"Python first" | 这是**建议**不是约束。下沉成"非 Python 就拒绝"会把建议变成禁令，属于产品决策，不属于代码重构 |
| 目标循环 / WAIT_EVT 的完整语义 | "目标是否真的完成"是判断。但**协议本身**可以瘦身，见下 |

关于目标循环的一段具体建议：`engine.rs:467-496` 的 `TODOS` 段共 1551 字符，其中 `[goal:DONE]`、`[WAIT_EVT: …]` 的玩法在 nudge（`engine.rs:2622`）和暂停分支里已经重复过一遍。可以只留一行"结束时写 `[goal:DONE]`；等外部事件写 `[WAIT_EVT: …]`，细节见后续 nudge"，把语义留到 nudge 出现的那一刻。代价是：模型第一次过早收尾时会多花一轮 nudge（有上限 `GOAL_LOOP_MAX_NUDGES`），换来每轮约 500 字符。

---

## 6. 顺带发现：两处"提示词在替代码兜底"

这两处不是预算问题，是数据安全问题：

1. **`.data/memory` 未受保护**（B2）。`schema.rs:258` 明确说"never edit them with file tools"，但 `paths.rs` 只拦了 `chat.db`。提示词一旦被忽略，记忆 frontmatter 会被写坏，且没有任何报错。
2. **`plan → apply` 未校验**（B1）。`consolidate_report` 完全信任 `args`；`wants_memory_consolidation`（`memory.rs:2579`）存在却只用来拼一句提示。模型跳过 plan 直接 apply 时，归档的 key 集和 cluster 可能对不上，而且**条目已经从检索池移走**。

---

## 7. 落地顺序与验收

| 批次 | 内容 | 验收方式 |
|---|---|---|
| 1 | 表 A（纯删除） | `cargo test`；刻意让工具报错一次，确认错误信息本身说清了原因（A1、A4 依赖这点） |
| 2 | B1、B2、B3（拒绝类） | 每条 1 个新单测：未 plan 的 apply 被拒、`.data/memory` 写入被拒、未读的 patch 被拒。 |
| 3 | B4、B6、B7、B8 + `TODOS` 段瘦身（点处注入类） | 一次真实会话：确认"结果里的一行提示"能替代原来的常驻段落；模型没有因为信息变少而走错 |

一条贯穿的原则：**下沉之后要敢删掉对应那句提示词**。删掉后行为不变，说明那句话本来就是死的；行为变了，说明下沉不够闭合——回到第 1 节的"可闭合"判据重做。

---

## 8. 预算账（实测，字符数）

| 块 | 位置 | 字符 | 何时注入 |
|---|---|---|---|
| explicit invocation | `engine.rs:74-79` | 315 | 每轮 |
| batching | `engine.rs:85-91` | 343 | `parallel_tools` 且并发 >1 |
| skills 列表 | `engine.rs:153-157` | 动态 | 有技能时 |
| agents | `engine.rs:350-356` | 871 | 有可用子智能体时 |
| memory | `engine.rs:424-434` | 424 | `memory` 工具启用 |
| timer | `engine.rs:445-451` | 550 | `timer` 启用 |
| todos（含 WAIT_EVT） | `engine.rs:467-496` + `:10-17` | 1551 | `todos` 启用 |
| deliverables | `engine.rs:509-517` | 614 | `file_actions` 启用 |
| manuals pointer | `prompt.rs:33-42` | 302 | 自演进模式 |
| scripts 策略 | `tools/scripts.rs:784-797` | 911 | 自演进 + `run_cmd` |
| self-evolution 主体 | `skills.rs:359-415` | 3572 | 自演进模式 |
| 目标循环 nudge | `engine.rs:2618-2624` | 383 | 仅触发时 |
| **工具 schema 的 97 段描述** | `tools/schema.rs` | **11956** | 工具启用时 |

常规模式固定策略 ≈ 5.0k 字符；自演进模式 ≈ 8.6k；schema 描述 ≈ 12.0k。表 A 主要从 schema 里省≈1k，表 B 主要从 5.0k 里省≈1.5k，且两者都不改变模型被要求做的事。
