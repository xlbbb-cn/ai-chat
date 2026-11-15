记忆、待办、定时器是让智能体跨越多轮工作的三件套：记住结论、维护计划、等待慢任务。三者在 **工具** 里分别开关。

## 记忆（`memory`）

### 作用域

| 作用域 | 存储 | 生命周期 | 适合 |
| --- | --- | --- | --- |
| `session` | 进程内存 | 本次会话，重启即失效 | 任务上下文、进行中的笔记 |
| `user` | `<工作区>/.data/memory/user/<key>.md` | 跨会话、按工作区 | 用户偏好、通用经验 |
| `repo` | `<工作区>/.data/memory/repo/<key>.md` | 跨会话、按工作区 | 代码约定、构建命令、项目事实 |
| `evolution` | `<工作区>/.data/memory/evolution/<key>.md` | 跨会话、按工作区 | 已验证的可复用事实（见下） |

> `user` 虽然叫"用户级"，实际仍存在当前工作区的 `.data` 里，**不跨工作区**。换工作区就换记忆。

### `evolution` 作用域（自我进化模式）

第四个作用域**只在自我进化模式开启时存在**，是助手自己的长期笔记本，专门记它*验证过*、必须跨会话保留的事实：

- **运行时环境** —— 默认解释器是 `python` 还是 `python3`、包管理器、shell、项目解释器路径；
- **工具的正确用法** —— 怎么驱动浏览器、怎么跑项目测试、哪个命令真的有效；
- **常用文件位置** —— 重要路径、输出目录、配置文件。

两点让它不只是个文件夹：

1. **提示词只指向、不预加载。** 系统提示只告诉助手"需要以往经验时搜索这个作用域"，模型自己用 `memory list` / `get` / `search` 去查。
2. **自动结论记录**在模式开启时写到这里，而不是 `repo`。

模式关闭时该作用域会被拒绝：

```text
Error: the 'evolution' memory scope is only available while self-evolution mode is on.
```

#### 排序与预算

条目不是按字母排的。每条都带 YAML front matter（`key`、`title`、`kind`、`tags`、`importance`、`confidence`、`evidence`、`uses`、`pinned`、`source`、时间戳），按下面打分：

```text
2 × importance + confidence + 1.5 × ln(1 + evidence) + 1.2 × ln(1 + uses) + 3 × e^(−age/30)
```

`pinned`（或 `importance ≥ 5`）会浮到最前面。`list` / `search` 返回的就是这个顺序——按经验加权，而不是按 key 字母；记忆面板也用同一套标出头部条目。

重复添加同一个 key 是 **upsert**（`evidence += 1`），标题相近会合并，内容完全相同的在读取时折叠。老格式文件（`# key` + 自由文本）按需升级；提示词构建器从不写文件。

结果可在 **设置 → 运行时与调试 → 记忆** 里查看（该行只在模式开启时出现）。

#### 把记忆归档成技能

笔记本变大后，`memory action="consolidate"` 能把一个热点簇折成一个技能：

| 阶段 | 作用 |
| --- | --- |
| `plan` | 按 tags、标题词与 kind 聚类；报告占预算 ≥20% 且条目 ≥4 的块，给出 key 列表与建议技能名 |
| `apply` | 接收 `keys` + 完整 `skill_md`：校验名称、解析技能、确认每个 key 都被覆盖、写入技能（带 `.bak.n`）、在 `skills.status` 里启用，并把条目移到 `<工作区>/.data/memory/evolution/archive/<技能>/` |
| `restore` | 把某技能归档的条目移回活动笔记本 |
| `list` | 列出归档及条目数 |

`apply` 按上述顺序作为一次事务执行。归档条目不再出现在 `list` / `search`，但 `memory get` 仍能按 key 解析。归档后的技能会自动启用，让它的一行描述进入提示词；正文由 `use_skill` 按需加载。

### 动作

| 动作 | 参数 | 说明 |
| --- | --- | --- |
| `add` | `key`、`content`，可选 `tags`、`importance`、`title` | 新增或覆盖同 key 条目（upsert 会累加 `evidence`） |
| `get` | `key` | 按 key 读取（也能读到归档条目） |
| `list` | — | 按排序列出作用域内条目 |
| `search` | `query` | 搜 key 与内容 |
| `delete` | `key` | 删除条目 |
| `consolidate` | `phase`（`plan`/`apply`/`restore`/`list`）、`keys`、`skill_md`、`skill` | 把进化条目归档成技能——仅自我进化模式 |

`scope: "all"` 会跨作用域执行 `get` / `list` / `search` 并返回合并结果；`add` 与 `delete` **必须**指定具体作用域。

### 自动结论记录

记忆工具启用后，后端每轮结束还会检查最终答复。若其中出现明显结论标记（如"根因是""修复方法是""我们决定"等）且模型自己没调用 `memory`，应用会把答复蒸馏（首句 + 最多三条要点，≤320 字符）写进文件型作用域：

```text
<工作区>/.data/memory/repo/conclusion-<会话>-<时间戳>.md      ← 普通模式
<工作区>/.data/memory/evolution/conclusion-<会话>-<时间戳>.md ← 自我进化模式
```

它记的是摘要而不是整篇答复，并限制 `source: auto` 条目数量，笔记本不会每轮膨胀。用 `memory list scope="repo"` 复查并删掉过期条目。

### 记忆文件与 file_actions

记忆文件在 `<工作区>/.data/memory/`，位于工作区内，所以 `file_actions` 能像普通文件一样读写。但仍建议走 `memory` 工具：它维护 key → 文件的映射、标签和合并列表的一致性。唯一绝不能碰的是 `.data/chat.db`。

### 示例

```text
memory add scope="user" key="response-style" content="用户偏好简洁中文：先结论，再理由。"
memory add scope="repo" key="build" content="前端检查：npx tsc --noEmit && npx vite build。Rust：cargo check。"
memory add scope="evolution" key="python-default" content="这台机器默认解释器是 `python`（3.12），不是 `python3`。"
memory search scope="all" query="build"
```

## 待办（`todos`）

### 作用域

待办是**按会话**的：

```text
<工作区>/.data/todos/<会话 id>/
  .active                 当前列表 id
  lists/<列表 id>.json    列表内容
```

同一会话里的主智能体与并行子智能体共用一份列表。新对话拿到新的 `会话 id`，不会继承旧列表。

### 动作

| 动作 | 参数 | 说明 |
| --- | --- | --- |
| `list` | — | 返回当前列表、条目 id 与状态 |
| `add` | `title`，可选 `description` | 追加条目，初始为 `pending` |
| `update_status` | `todo_id`、`status` | `pending` → `in_progress` → `completed` / `cancelled` |
| `clear_completed` | — | 移除已完成条目 |
| `archive` | 可选 `new_title` | 归档当前列表并新建（默认标题 `Working plan`） |

### 界面

会话的"工作计划"镜像在**最新**一条助手回复下方，通过 `todo-state` 事件实时刷新。卡片挂载时读取该会话的活动列表，所以恢复或重载的会话仍显示当前计划。收起时只有两行：列表标题 + 完成/总数，以及当前进行项（或第一个待办）。没有待办或进行项时整卡隐藏，且只有最新回复承载列表，历史回复保持干净。卡片只做展示，状态由模型通过 `todos` 工具修改。

### 目标循环（goal loop）

待办列表不是装饰：只要列表里还有 `pending` / `in_progress` 条目，一轮答复的结束**不会**结束这一轮。后端会把最新列表（按当前状态重新渲染，而不是轮次开始时的快照）重新注入，并要求继续工作，模型接着干——每轮最多 **4 次续跑**，之后停止，避免模型打转。

有两种情况会直接结束本轮而不强推目标：

- **有到期待触发的延迟定时器**：剩余条目是在等通知，故意挂起。
- **最终答复里显式写了 `[WAIT_EVT: 在等什么]`**：同样是等待，但等的是应用无法自排的信号（CI 跑完、同事回复、发布窗口）。

两种情况都记为 *Todo goal suspended*；下一轮（定时器触发或用户回复时）计划自动恢复。`[goal:DONE]`、`[WAIT_EVT: …]` 是控制信号而非正文，落库前会从气泡里剥掉；只有这类标记的答复也不会在记录里留下空气泡。

### 技巧

- 让模型先 `list` 再规划，避免重复计划。
- 条目要可执行、可验证，别写"处理一下"。
- 长任务收尾时用 `clear_completed` 或 `archive`；只有确实想继续跑时才留开放条目。
- 待办是面向模型的计划，不是项目管理工具；跨会话的工作交给记忆或外部系统。

## 定时器（`timer`）

### 为什么需要

有些任务一轮之内完不成：30 分钟的日志收集、长时间构建、下载、部署、限流窗口。在 shell 里 sleep 或轮询既浪费轮次也浪费 token。定时器让模型结束本轮、稍后再来——配合上面的目标循环，它才是"等待"的正当方式。

### 工具

| 工具 | 参数 | 说明 |
| --- | --- | --- |
| `timer_set` | `delay_seconds`、`message`，可选 `label` | 安排一次性定时器 |
| `timer_list` | — | 列出待触发定时器（id、标签、触发时间、剩余秒数、消息） |
| `timer_cancel` | `timer_id` | 取消 |

`delay_seconds` 范围：1 秒 – 30 天（2,592,000 秒）。

### 触发行为

1. 到期时后端发出 `timer-fired`。
2. 前端把 `message` 作为新的用户消息注入创建它的会话：

   ```text
   ⏱️ [Timer fired: <标签>]
   <消息>
   ```

3. 若有回复正在流式输出，消息排队，空闲时再发。
4. 若你已切到别的会话，它不会劫持当前聊天，而是给出提示，可一键插入当前输入框。
5. 定时器能跨重启：待触发项镜像到 `<工作区>/.data/timers.json`，启动时重新装载，已过期的立即触发。镜像**按工作区**，切换工作区会把当前集合写回旧文件、装载新工作区的集合。
6. 只要还有待触发定时器——或仍有对话轮次在跑——应用会持有共享 `sleep-lock`，请求系统不要空闲休眠（Windows `SetThreadExecutionState`，macOS IOPM 断言，Linux 取会话提供的所有抑制项并按获取方式释放）。最后一个定时器消失且本轮结束后释放；失败只记一次日志，且绝不阻塞定时器。

### 定时器条

输入框上方：

- 标签；
- 倒计时（`mm:ss` 或 `h:mm:ss`）；
- 取消按钮；
- 持有休眠锁期间显示 **已阻止休眠** 徽标。

### 写好那条消息

`message` 是那一轮**唯一的新指令**，必须自包含。不要写"继续"。写清楚要检查什么、输出在哪、下一步做什么。

好：

```text
15:07 启动的日志收集此时应已完成。检查 ./logs/collect-2026-09-24.log，按根因归类错误并继续给出修复建议。文件缺失就查看 serve 进程输出并说明原因。
```

差：

```text
继续上一个任务。
```

### 限制

- 子智能体拿不到定时器工具，只有主智能体可以安排。
- 定时器是一次性的；要反复等待就再排一个。
- 取消定时器不会删除聊天记录。
- 应用必须在运行；关闭期间到期的定时器会在下次启动时触发。

## 组合用法

典型的长任务：

1. `run_shell serve` 启动日志收集脚本。
2. `todos add` 把后续动作记成开放条目。
3. `timer_set delay_seconds=1800`，消息写清要检查什么。
4. 告诉用户 30 分钟后继续并结束本轮——期间目标循环保持挂起。
5. 定时器触发，模型带着完整历史恢复，继续执行计划。
6. 用户说"别等了"，模型调用 `timer_cancel` 取消。

## 常见问题

| 现象 | 排查 |
| --- | --- |
| 找不到记忆 | 作用域对不对；是否换了工作区；`session` 作用域重启即失效 |
| `scope: "all"` 不能新增 | 设计如此；add/delete 需要具体作用域 |
| `evolution` 作用域被拒 | 自我进化模式没开，该作用域只在开启时存在 |
| 笔记本不断被自动结论塞满 | 正常：每条都经过蒸馏且限制了 `source: auto` 数量；删掉过期条目或跑 `memory action="consolidate"` |
| `consolidate` 说无事可做 | 需要 ≥4 条、占预算 ≥20% 的簇 |
| `apply` 之后某条从提示里消失 | 它被归档到 `.data/memory/evolution/archive/<技能>/`；用 `phase: "restore"` 取回 |
| 待办卡片不刷新 | 待办工具开了吗？模型调用 `todos` 了吗？ |
| 已经"最终答复"了却还在跑 | 列表还有开放条目，这就是目标循环；做完或取消，或用定时器 / `[WAIT_EVT]` 等待 |
| 还有开放条目却结束了本轮 | 有定时器待触发、答复里声明了 `[WAIT_EVT: …]`，或已用满 4 次续跑 |
| 定时器没触发 | 应用在运行吗？`<工作区>/.data/timers.json` 存在吗？会话被删了吗？设置后换过工作区吗？ |
| 定时器在别的会话触发 | 正常，用提示里的 **插入到输入框** |
| 重复定时器 | 先 `timer_list` 再 `timer_set` |
| 模型一直 sleep/轮询 | 打开定时器工具，并明确要求"用定时器而不是 sleep" |
