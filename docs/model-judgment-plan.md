# 模型判断下沉：全景审查与编码计划

> 上一轮 `docs/prompt-sinking-review.md` 只看"提示词里的规则"。这份是**全方位**：把
> 编排层（选择 / 规划 / 监督）、子智能体终态、工具校验一起算进来，给出可以直接开工的批次。
>
> 结论先说：**最大的浪费不是提示词太长，是同一个判断问了两次模型、以及把图上的计算交给模型填。**

---

## 0. 方法与判据

一条"模型判断"值得下沉，要同时过四问：

| # | 问题 | 不通过的表现 |
|---|---|---|
| Q1 | **选项是有限的吗？** 模型在闭集里挑（开关组合、枚举值、固定动作） | 选项由自然语言定义，如"是否值得派子智能体" |
| Q2 | **答案已经在代码里吗？** DB / 内存 / 文件 / 本会话工具历史算得出 | 需要读用户意图的细微差别 |
| Q3 | **是不是被问了不止一次？** 同一决定落在两处，规则还不一致 | 两处都能答"是/否"，且兜底策略不同 |
| Q4 | **答错的代价可逆吗？** 错了能重来 → 放心让模型答；错了静默丢能力 → 必须代码兜底 | 解析失败 = 任务消失 |

下沉的四种手法（按侵入度排序）：**删**（代码已强制）→ **算**（代码派生，覆盖模型值）→ **拒**（前置条件不满足报错）→ **点处注入**（静态常驻文本挪到触发时的结果里）。

判断类别 `K1..K6`：

- `K1` 重复决策：同一个 yes/no 问两遍 → 合并
- `K2` 可计算：依赖图、能力签名、终态、cap → 代码算
- `K3` 枚举/协议：execution_mode、marker → 归一化或换状态
- `K4` 兜底策略：模型输出坏掉时怎么办 → 确定性默认，不丢能力
- `K5` 约束单源：代码强制的上限没出现在提示词里 → 单一 const 渲染进提示词
- `K6` 规则 → 工具校验（上一轮的 A/B 类）

---

## 1. 全景清单

### 1.1 编排层（收益最大）

| ID | 决定 | 位置 | 现状 | 判定 |
|---|---|---|---|---|
| H1 | 要不要派子智能体 | `planning.rs:928-939`（selection）＋`planning.rs:1106-1113`（plan） | **同一个问题问两次**，两套规则；selection 答 false 短路，plan 还能再答 false | `K1` 合并 |
| H2 | 同上 | 同上 | 明显不需要/明显需要的请求也付两次 LLM 往返；最坏 3 次（selection 1 + plan 重试 2，`planning.rs:1157`） | `K2` 预判闸 |
| H3 | plan 解析失败 | `planning.rs:1204` → `engine.rs:1668-1672` | 整个 mission 降级为直答，日志一行 WARN，用户无感 | `K4` 确定性兜底计划 |
| H4 | `execution_mode` | `planning.rs:457` 直通、`planning.rs:684` | 纯可计算：执行器本来就按 `dependencies` 波浪调度（`execution.rs:163-196`） | `K2` 由图派生 |
| H5 | 选哪些 agent | `planning.rs:864-870`、`923-925` | 已跳过 ≤1 agent 与空查询；但能力签名全同的 roster 仍在调模型 | `K2` 等价短路 |
| H6 | "live 数据需要 mcp" | `planning.rs:934`、`1140-1143` | 规则教模型判断；选择后无确定性校验 | `K6` 候选过滤 |
| H7 | 监督器下一步动作 | `supervisor.rs:146-158`、`859-867` | 每轮都调模型，哪怕合法动作只有一个（全部终态 → finish） | `K2` 状态机 |
| H8 | 卡住时怎么办 | `supervisor.rs:782-786` | 只在触发条件里用 `stalled()`，动作仍问模型 | `K2` 阶梯升级 |
| H9 | `cwd` 是否合法 | `planning.rs:394` `normalize_task_cwd` | 只做字符串归一，不校验存在性/越界 | `K6` 校验后回落 |
| H10 | plan 任务数上限 | `planning.rs:403-409` `MAX_PLAN_TASKS` | 提示词里**没有这个数**，超了静默截断 | `K5` 单源渲染 |
| H11 | 三个开关的组合语义 | `engine.rs:1630-1654`、`planning.rs:526`、`658` | `use_agents / auto_plan / auto_configure / mbus_enabled` 的判定散在三处 | `K3` 收敛成 `OrchestrationMode` |

### 1.2 子智能体终态

| ID | 决定 | 位置 | 现状 | 判定 |
|---|---|---|---|---|
| H12 | 任务何时算完成 | `mission.rs:289-321` | `mark_mission_accomplished` 只关 root task（`task_id = ?1`），子任务靠模型自觉 | `K2` 完成时统一收口 |
| H13 | "完成前清空任务" | `mission.rs:473` 提示词 | 规则写在提示词里，代码不做 | `K6` 删除 + 代码收口 |
| H14 | 完成标志未设 | `mission.rs:489-493` | 每轮把"还没完成"塞进快照，模型据此决定继续 | 保留（这是状态回显，不是判断） |

### 1.3 主循环 / 工具（沿用上一轮结论）

| ID | 决定 | 位置 | 判定 |
|---|---|---|---|
| P1 | memory `plan → apply` | `skills.rs` archiving 段、`memory.rs:2393-2416` | `K6` 校验 plan 存在 |
| P2 | `.data/memory` 只读 | `schema.rs:258` 声称 app-managed，`paths.rs:248-301` 只保护 `chat.db` | `K6` **提示词在替代码兜底** |
| P3 | patch 先读 | `file_locks.rs:45/66` 已有读记录，`patch.rs` 不消费 | `K6` |
| P4 | 手册先读 | `prompt.rs:30-42`、`manuals.rs` | `K6` |
| P5 | timer 去重 | `timer.rs` 有 registry | `K6` |
| P6 | 交付物别进 `.data/tmp` | `engine.rs:509-517`、`workspace_files.rs:452` | `K6` 写入时提示 |
| P7 | `{tool:}`/`{skill:}` 语法 | `engine.rs:74-79` | 全仓无解析器 → 要么删，要么检测到就注入 + 自动加载 |
| P8 | 静态常驻段 | `engine.rs` 多处 | `K6` 点处注入 |

### 1.4 明确不下沉（保留）

| 决定 | 理由 |
|---|---|
| 何时算"定论"、写哪个 memory scope | 语义判断 |
| 交付物该放进哪个项目目录 | 语义判断 |
| skill 是否"clearly needed" | 语义判断 |
| 子智能体类型偏好（llm/cli/cloud） | 产品口味，下沉会变禁令 |
| 要不要问用户、问什么 | 交互判断 |
| 复述"绝不猜数字" | 无确定性替代，只能靠工具兜底 |

---

## 2. 编码计划

四个批次，按"零风险 → 高收益 → 结构改动"排。每批独立可发布。

### 批次 0 · 收敛（无行为变化，1 天）

**0.1 `OrchestrationMode` 单点判定**

- 新增 `src-tauri/src/agents/mode.rs`：
  ```rust
  pub enum OrchestrationMode { Off, Manual, Planned, PlannedWithBus }
  pub fn resolve(use_agents: bool, auto_plan: bool, has_enabled: bool, mbus: bool) -> OrchestrationMode
  ```
- 替换 `engine.rs:1634` 的 `planning_enabled` 与 `engine.rs:1652` 的 `has_enabled` 组合、`planning.rs:526` 的 `auto_configure` 分支、`planning.rs:658` 的 `mbus_enabled` 分支。
- 验收：8 种开关组合的矩阵测试；`Off` 时 `call_subagent` 也不上工具表（现有行为不变）。

**0.2 上限单源（`K5`）**

- `MAX_PLAN_TASKS` / `MAX_SELECTED_AGENTS` / `MAX_ACTIVATIONS` 从 `const` 渲染进各自的提示词（plan 提示词目前只提了 agent 数，没提任务数）。
- 验收：`grep` 提示词里出现常量；截断路径（`planning.rs:403`）加 `WARN` 日志。

### 批次 1 · 用计算替代判断（低风险，2 天）

**1.1 `execution_mode` 由图派生（H4）**

```rust
fn derive_execution_mode(tasks: &[Task]) -> &'static str  // 任一 task 有依赖 → "sequential"
```
- `parse_task_plan` 用派生值覆盖模型值；模型填的与派生值冲突时走已有的 `issues` 通道记一条。
- 验收：全空依赖 → parallel；任一非空 → sequential；与 `orchestration.mode` 的 AND 语义不变（`planning.rs:684`）。
- 风险：低。执行器已按依赖调度，此字段只是限流提示。

**1.2 `cwd` 校验（H9）**

- `normalize_task_cwd` 之后：`workspace.join(cwd)` 必须存在且 canonicalize 后仍在 workspace 内，否则清空为根 + issue。
- 验收：越界（`../`）、不存在、`./apps/web` 存在 三例。

**1.3 能力等价短路（H5）**

- `fn capability_signature(agent) -> (type, sorted(tools), sorted(skills), mcp)`；全 roster 同一签名时跳过 selection，`full_roster_selection(.., "identical capability signatures")`。
- 收益：选择器调用直接省掉（常见单 agent 类型场景）。

### 批次 2 · 合并重复决策（结构改动，收益最大，3–4 天）

**2.1 确定性预判闸（H2）**

```rust
enum DelegationHint { No, Yes, Unsure }
fn delegation_hint(messages: &[ChatMessage]) -> DelegationHint
```
信号全部确定性：消息字符数、workspace 内已存在路径数、代码块数、`http(s)` URL 数、显式并列结构（"分别 / 各自 / 同时 / 并行 / 两个"）。

- `No` → 跳过 selection + plan，直接直答（0 次 LLM 调用，UI 仍发 `agent_selection` 事件的 `delegate:false`）。
- `Yes` → 跳过 selection，直接 plan（省 1 次往返）。
- `Unsure` → 现行两步路径。

**2.2 只问一次 delegate（H1）**

- selection 响应去掉 `delegate` 字段，只回答"哪些 agent 参与"；`delegate` 的唯一来源是 plan。
- 兼容：老模型仍返回 `delegate` 时忽略掉，但**不**短路（避免两处不一致）。
- 验收：`selection.delegate=false` 的旧测试改写成"roster 为空时不进 plan"。

**2.3 确定性兜底计划（H3）**

- plan 解析连续失败 → 不再返回 `Err` 让整轮降级，而是 `fallback_plan(agents, messages)`：单任务、`agent_id` = 能力签名最全的 agent、`cwd` 为空、描述取用户消息前 200 字。
- 验收：喂坏 JSON / 空 tasks / 未知 agent 三种坏输出，都要产出 1 个可执行任务并记一条 WARN。

### 批次 3 · 监督器状态机（3 天）

**3.1 合法动作集（H7）**

```rust
fn legal_actions(bus, tasks) -> Vec<Action>   // Finish | Nudge(task) | Ask(task) | Mediate(pair) | None
```
- 空集或 `[Finish]` → 直接执行，不调模型（当前每轮都写一次 prompt + 调一次模型）。
- `[Nudge(t)]` 且连续第 2 次 stalled → 固定模板消息（`STALL_NUDGE`），不调模型。

**3.2 `mbus send` 去重（H8 的规则部分）**

- 归一化（trim + 折叠空白 + 小写）后与最近 K 条 note 相同时拒绝："the same note was already sent at activation N"。

**3.3 pair_limit 中介（H10 相关）**

- pair 到上限时由代码直接生成 escalation 消息（`supervisor.rs:863` 的规则），不再指望模型读到这行规则后照做。

验收：supervisor 单测覆盖 终态/单动作/多动作/连续 stalled 四条路径；`MAX_ACTIVATIONS` 预算不变。

### 批次 4 · 提示词 → 工具校验（上一轮 P1–P8，2 天）

按 `docs/prompt-sinking-review.md` §4 的表执行，优先级：P2（`.data/memory` 写保护）→ P1（plan→apply）→ P3（patch 先读）→ P5（timer 去重）→ P4（手册先读）→ P6（交付物提示）→ P7/P8（条件注入）。

**共同验收模板**：每条下沉后，删掉对应提示词句子，跑一个回归会话；行为不变才算下沉成功（否则说明那句子是活的，恢复它）。

---

## 3. 顺序与依赖

```
批次0 ──► 批次1 ──► 批次2 ──► 批次3
                └──► 批次4（可与 1/2/3 并行）
```

- 批次 0 必须先做：批 2/3 都要读 `OrchestrationMode`。
- 批次 4 与其它批次无耦合，可以随时插入发布。
- 批次 2.2 是唯一会改协议（selection 响应形状）的项，放在 2.1/2.3 之后，便于回滚。

## 4. 度量（怎么证明有效）

| 指标 | 采集点 | 目标 |
|---|---|---|
| 每轮 mission 的前置 LLM 调用数 | `log_agent_event` 已有 INFO | 简单请求 2 → 0；可委派请求 2 → 1 |
| plan 解析失败导致的直答率 | `engine.rs:1671` WARN 计数 | → 0（改为兜底计划） |
| 监督器 activation 的模型调用占比 | `supervisor.rs:136` | 单动作场景不再调用 |
| 每个 prompt 块的字符数 | 上一轮测得的脚本 | 固定策略文本 −30% |

## 5. 不做什么

- 不把"要不要问用户"变成规则。
- 不引入新的 LLM 分类器去替代旧的（那是把判断搬个地方，成本更高）。
- 不改 `MAX_ACTIVATIONS` 等预算常量（只让它们可见）。
- 不动 `mark_mission_accomplished` 的面板语义，除非产品先确认"被放弃的子任务"该显示成什么（见 H12 备注）。

---

## 6. 落地状态

### 6.1 已实现

| ID | 落地位置 | 做了什么 |
|---|---|---|
| H11 / 0.1 | `agents/mode.rs`（新）、`engine.rs` | `OrchestrationMode` 收敛 `use_agents / auto_plan / 有启用 agent / mbus` 四个开关；11 种组合的真值表测试；引擎日志输出 `orchestration mode: …` |
| H10 / 0.2 | `planning.rs` 规划提示词 | `MAX_PLAN_TASKS` 渲染进提示词（原来只告诉模型任务上限 4 个 agent，没告诉上限 16 个任务） |
| H4 / 1.1 | `planning.rs::derive_execution_mode` | `execution_mode` 由依赖图分层算出，覆盖模型值；冲突走 `issues` 记一条 WARN；规划提示词里删掉这个字段 |
| H9 / 1.2 | `planning.rs::is_safe_relative_dir` | `cwd` 在解析期做词法校验，越界/绝对路径清空为根并记 issue（执行期的沙箱校验不变） |
| H5 / 1.3 | `planning.rs::capability_signature` | 全 roster 能力签名相同时不调选择器 |
| H2 / 2.1 | `planning.rs::delegation_hint` | 确定性预判闸：`No` 直接直答（0 次 LLM 调用），`Yes` 跳过选择器（省 1 次），`Unsure` 走原路径 |
| H1 / 2.2 | `planning.rs::select_agents` | 选择器不再回答"要不要派"，只回答"谁参与"；旧模型仍返回 `delegate` 时记 INFO 后忽略 |
| H3 / 2.3 | `planning.rs::most_capable_agent` | 规划连续两次解析失败 → 单任务兜底计划（能力最全的 agent），不再让整轮静默降级为直答 |
| H7 / 3.1 | `supervisor.rs::settled` | 全部任务终态时直接结束监督，不再调模型 |
| H8 / 3.2 | `supervisor.rs::repeated_message` | 重复的 `mbus send` 由工具拒绝（对比最近若干条已发消息的前 120 字符） |

### 6.2 与计划的偏差（附理由）

| 计划 | 实际 | 理由 |
|---|---|---|
| 1.1 "任一 task 有依赖 → sequential" | 按依赖图层宽判定：最宽层 ≤ 1 才 sequential | "有依赖"就串行会杀掉菱形图里的并行波；执行器本来就按波浪调度，层宽才是真实并发度 |
| 1.2 "cwd 必须存在" | 只做词法校验，不要求存在 | `resolve_task_cwd` 的既有语义是"任务可以 scaffold 一个还不存在的目录"，加存在性校验会破坏这个能力 |
| 3.1 "连续 2 次 stalled → 固定模板" | 未实现 | 固定模板要先决定"催哪个任务、催什么"，等于把判断换个地方写死；风险是骚扰子智能体，收益只有一次 activation。留作后续，需真实任务分布数据 |
| 3.2 "pair_limit 由代码直接中介" | 未实现 | 该分支与 `mbus` 的投递语义耦合较深，单独改容易造成重复投递；本轮先做零风险的重复消息拒绝 |
| H12 / H13（子智能体终态收口） | 未改 | 需产品先确认"被放弃的子任务"在面板上显示成什么（`completed` 会撒谎） |

### 6.3 验证

- `cargo check --tests` 通过（含新增测试的类型检查）。
- **`cargo test --lib` 在本机跑不起来**：测试二进制一启动就 `STATUS_ENTRYPOINT_NOT_FOUND`（0xC0000139，缺 `WebView2Loader.dll`）；把 DLL 拷到 `deps/` 后又挂在启动阶段无输出。这是环境问题（与本次改动无关：加载期崩溃，源码已编译通过），因此新增的 4 组纯函数测试目前只有编译保证，需要在能跑测试的环境（CI 或桌面会话）里执行一次。
- 新增测试：`agents/mode.rs` 的开关真值表；`agents/tests.rs` 的 `execution_mode_follows_the_dependency_graph`、`an_escaping_cwd_is_cleared_at_parse_time`、`delegation_hint_settles_only_the_clear_cases`。

### 6.4 上线后要看的三条曲线

1. 日志里 `delegation hint:` 三种取值的占比 —— `Unsure` 长期占绝大多数说明阈值没抓住真实分布。
2. `planner: planner said execution_mode=…` 的出现率 —— 高说明模型仍在纠结这个已废弃字段。
