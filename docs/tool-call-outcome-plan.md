# 工具调用可见性：参数、结果与错误补全计划

> 现象：`ToolCallGroup` 里的一行工具调用，只看到「动词 + 路径」这类标签，`run_shell` 失败只有红点没有错误文本，
> `file_actions read` 不显示读了多少行、读了哪一段。
>
> 结论先说：**不是渲染层偷懒，是数据通道只搬了"标签"。** 工具的真实输出从头到尾没有离开 Rust，
> 前端连失败原因都拿不到；`ToolCallEntry.error` 在主聊天路径上从未被赋值，是一段死字段。
> 所以必须先在 Rust 侧开一条结构化的 `outcome` 通道，前端才有东西可显示。

---

## 0. 判据

| # | 问题 | 结论 |
|---|---|---|
| Q1 | 只改 `ToolCallGroup.tsx` 够不够？ | **不够**。结果字符串只在 `engine.rs` 结算循环里存在，事件里没有它。 |
| Q2 | 前端能不能自己算行数？ | 不能。前端从不接收 `role:"tool"` 的消息内容。 |
| Q3 | 能不能把整段输出塞进事件？ | 不能。单次结果可达 8000 字符（`execute/mod.rs:210` 的 head+tail 预算），会让事件、DB 行、重渲染一起变胖。只送指标 + 裁剪预览。 |
| Q4 | 文案在哪一层？ | **前端**。三个 locale（en / zh-CN / zh-TW）都在 TS 里，Rust 拼出来的"40 行"没法翻译。后端只给数字和原文片段。 |

---

## 1. 现状链路（数据在哪一步丢的）

```
工具返回 String(result)
      │  engine.rs:1459-1530  结算循环（failure 判定、budget、turn 快照）
      ▼
tools::emit_tool_call_done        tools/mod.rs:121
      │  事件 tool-call = { session_id, call_id, phase:"done", ok, text:"*run_shell*" }   ← 只有名字
      ▼
App.tsx:2859 的 "tool-call" 监听
      │  phase/tool 行：从 text 首行 `*…*` 取 label，从 ```代码块``` 取 detail
      ▼
ToolCallEntry { description, summary, status, started_at }        types.ts:1-71
      │
      ▼
ToolCallGroup.tsx:81  detailFor → summary ?? error                 渲染
```

三处断点：

| 断点 | 位置 | 症状 |
|---|---|---|
| **G1 参数** | 只有自己写进度行的工具才带参数 | `file_actions read` 只发 `*Reading {path}*`（`file_actions.rs:58`），`start_line`/`end_line` 丢掉；`memory`、`todos`、`timer_list`、`calculator` 根本不发进度行，行由 `done` 事件创建，于是整行只有一个工具名，`summary` 为 `null`。`tool_args_block`（`background.rs:56`）只服务 MCP 和后台任务，普通工具没用上。 |
| **G2 错误** | `App.tsx:2897-2915` 的 done 分支 | 只写 `status: "error"`，**从不写 `error`**（全文 `error:` 只出现在 2774 的任务行和 agents 行）。`detailFor` 的 `summary ?? error` 因此永远取不到错误文本 → 失败行只剩一个红点。而且 `summary: tc.summary ?? detail` 保住的是**开始时**的命令，不是结果。 |
| **G3 度量** | `process.rs:212-217` 已经写 `Exit code: N`；行数、字节、耗时也都在手边 | 一个都没进 UI。`duration_ms` 甚至没有按调用计时。 |

---

## 2. 目标数据模型

一个可选字段，`ToolCallEntry` 上：

```ts
/** 一次调用的结果摘要。后端 measure 出来，前端负责文案。 */
export interface ToolCallOutcome {
  /** 原始工具名，用于前端取本地化名词（run_shell / file_actions …）。 */
  tool: string;
  /** 失败首行（已裁剪）。只有结果是错误时存在。 */
  error?: string;
  /** 进程退出码（run_cmd / run_shell）。 */
  exit_code?: number;
  /** file_actions read 实际返回的行区间（1-based，闭区间）。 */
  line_from?: number;
  line_to?: number;
  /** 结果正文行数 / 字节数 / 输出被 truncate 过。 */
  lines?: number;
  bytes?: number;
  truncated?: boolean;
  /** file_actions search 命中行数与文件数。 */
  matches?: number;
  files?: number;
  /** file_actions patch 的 +/− 行数。 */
  added?: number;
  removed?: number;
  /** 本次调用耗时。 */
  duration_ms: number;
  /** 结果头部预览（≤ 800 字符 / 12 行），不给全文。 */
  preview?: string;
}
```

`ToolCallEntry` 增加 `outcome?: ToolCallOutcome`。**旧数据没有它，渲染必须逐字不变。**

事件（`tools/mod.rs:121`）：

```json
{ "session_id": "…", "call_id": "call_1", "phase": "done", "ok": false,
  "text": "*run_shell*",
  "outcome": { "tool": "run_shell", "error": "Exit code: 1 …", "exit_code": 1,
               "lines": 42, "bytes": 1830, "duration_ms": 812, "preview": "…" } }
```

`text` 保留：它是老前端、后台驱动、日志的兼容面，不动语义。

---

## 3. Rust 改动（4 个接缝）

### S1 新模块 `src-tauri/src/tools/outcome.rs`

唯一一处"工具 → 指标"的映射，纯函数、可单测：

```rust
pub(crate) struct ToolOutcome { /* 与 TS 字段同名 snake_case */ }

/// 从一次调用的参数、结果和耗时量出 UI 需要的字段。纯函数，无 IO。
pub(crate) fn measure(tool: &str, args: &Value, result: &str, duration_ms: u64) -> ToolOutcome
```

规则（顺序：先通用、后逐工具覆盖）：

| 字段 | 通用规则 | 逐工具覆盖 |
|---|---|---|
| `error` | `tool_output_is_error(result)`（`turn/guard.rs:229`）时取首个 `Error…` 行，裁到 240 字符 | — |
| `lines` / `bytes` | `result.lines().count()` / `result.len()` | — |
| `truncated` | `result.contains("...[truncated ")` | — |
| `preview` | 头 12 行，再裁到 800 字符 | 错误时优先 `STDERR` 之后那段 |
| `exit_code` | 正则 `^Exit code: (-?\d+)$` | — |
| `line_from/to` | — | `file_actions` `read`：`args.start_line`/`end_line`，缺省 `1`/`lines` |
| `matches` / `files` | — | `file_actions` `search`：按 `path:line:text` 行计数、去重路径数 |
| `added` / `removed` | — | `file_actions` `patch`：复用 `tools::build_file_diff` 的统计；`write` 只给 `bytes` |

单测（`tools/tests.rs` 或模块内）：`read` 区间、`search` 计数、`run_shell` 非零退出、成功结果不带 `error`、
超长结果的 `truncated` 与 `preview` 上限。

### S2 `tools/mod.rs`：`emit_tool_call_done` 接上 outcome

签名加一个参数，事件里多一个键：

```rust
pub(crate) fn emit_tool_call_done(
    app: &AppHandle, session_id: Option<&str>, call_id: &str,
    name: &str, ok: bool, outcome: &crate::tools::outcome::ToolOutcome,
)
```

### S3 `engine.rs`：按调用计时 + 产出 outcome

- 建 future 的地方（`engine.rs:1395`、`1426`）把结果包一层：
  `async move { let t = Instant::now(); let r = run_tool_call(…).await; (r, t.elapsed().as_millis() as u64) }`
  —— 并发的调用各自计时，结算时读到的不是"轮次总耗时"。
- 结算循环（`1514`）：
  `let outcome = tools::outcome::measure(&call.name, &call.parsed, &result, elapsed);`
  然后 `emit_tool_call_done(…, !failed, &outcome)`。
- `turn.record_tool_row`（`1522-1533`）同一份 outcome 写进快照：webview 重挂载后行不退化。

> 这一处是**唯一**的生产点，40 多个工具自动全部受益，不需要逐个改工具。

### S4 计划阶段统一一行"开始"标签：补 G1

`engine.rs:1272-1330` 的 planned 循环里，为每个会真正执行的调用补发一条 progress：

```rust
let label = tools::call_label(name, &parsed);   // 新函数，单行、无代码块
tool_progress(…, format!("*{label}*"));
```

`call_label` 是一个 `match name` 的小函数（放 `tools/outcome.rs` 旁边），给没有自写进度的工具兜底：

| 工具 | 标签 |
|---|---|
| `run_cmd` | `run_cmd · build-frontend --release`（`args.command`，裁 80） |
| `run_shell` | `run_shell · pwsh: <首行>` |
| `file_actions` | `read src/a.ts (40–80)` / `search "foo" in src · max 20` / `write src/a.ts · 1.2 KB` / `patch src/a.ts` |
| `skill_read` | `skill_read · demo/data/x.md` |
| `memory` | `memory · get session/foo`（`action` + `key`） |
| `todos` | `todos · update_status` |
| `calculator` / `timer_*` / 其它 | `<name> · <参数键值单行摘要，裁 120>` |

顺序保证：这条先发，工具自己的进度行后发，前端 progress 分支用后到的 label 覆盖
（`App.tsx:2927` `description: label || tc.description`）——**工具特有文案优先，兜底只在工具不说话时出现**。

同时修掉两处丢参数的动词行（一行改动）：
`file_actions.rs:58` → `*Reading {path} ({from}–{to})*`；`file_actions.rs:272` 的 search 行补 `max_matches`。

---

## 4. 前端改动（3 个文件 + i18n）

### F1 `src/types.ts`

`ToolCallOutcome` + `ToolCallEntry.outcome?`（见 §2）。

### F2 `src/App.tsx` 的 `"tool-call"` 监听（2859 起）

done 分支：

```ts
const outcome = e.payload.outcome as ToolCallOutcome | undefined;
… { ...tc, status, outcome, error: outcome?.error ?? tc.error,
    summary: tc.summary ?? detail }
```

同时把该 `listen` 的 payload 类型补上 `outcome?: ToolCallOutcome`。
progress 分支不动（`outcome` 只在 done 上有）。

### F3 `src/backgroundTurns.ts:132-140`

快照映射里透传 `outcome`，否则重挂载/后台会话的行会退回"只有标签"。

### F4 `src/components/ToolCallGroup.tsx`

1. **指标文本**：新增 `metricsFor(entry, t): string`，用 i18n 模板拼 `1–40 行 · 1.8 KB · 812ms`；
   为 `null` 的字段直接不出（不渲染 `0`）。放进新的一行 `<span className="tool-call-entry-meta">`。
2. **错误**：`entry.status === "error"` 或 `outcome.error` 时，`.tool-call-entry-error` 显示错误首行
   （单行 ellipsis，`title` 给全文）。这是 G2 的**可见修复**——不展开也能看到为什么失败。
3. **输出预览**：每行自己的 `<details className="tool-call-entry-output">`（默认折叠，`summary` 文案
   `查看输出` / `查看错误`），内部 `<pre>` 渲染 `outcome.error ?? outcome.preview`，
   `max-height: 320px; overflow: auto`。没有 `preview` 的行不渲染这个 details。
4. **折叠态预览**（`.tool-call-preview-sub`）：`detailFor` 返回文本后面接一段结果后缀
   （`✓ 40 行` / `✗ exit 1`），只在 `status !== "running"` 时加；高度不变（该行本来就是 1 行 ellipsis）。
5. **CSS**（`ToolCallGroup.css`）：`.tool-call-entry` 的 `grid-template-rows` 从 `auto auto auto` 变四行，
   meta 占 row 3、mbus 顺延 row 4；`meta` 与 `summary` 同款 11px `--c-muted`。
   **遵守 `docs/DESIGN.md:772`**：元数据"never a pill, never a fill, never the accent" → 指标是分隔符文本，
   不是 chip；危险色只用 `--c-danger`（`--ts-danger` 已有回退）。
6. 无障碍：`details > summary` 是可点元素，给 `role`/`aria-expanded` 用原生语义即可；
   `StatusLight` 已 `aria-hidden`，错误文本作为真实文本存在，红点不再是唯一信号。

### F5 i18n（`en.ts` / `zh-CN.ts` / `zh-TW.ts` 的 `toolGroup`，行 379 / 339 / 339）

```ts
metrics: {
  lines: "{count} lines",
  linesRange: "lines {from}–{to}",
  bytes: "{size}",
  matches: "{count} matches · {files} files",
  exitCode: "exit {code}",
  duration: "{seconds}s",
  truncated: "truncated",
},
output: { show: "Show output", hide: "Hide output", error: "Show error" },
result: { ok: "✓ {detail}", failed: "✗ {detail}" },
tools: { run_shell: "run_shell", file_actions: "file_actions", … },   // 仅在需要本地化名词时用
```

三份必须同时加：`MessageKey` 由 `en.ts` 推导，缺键在编译期就红。

---

## 5. 验收矩阵（一个回合内逐条可看）

| 工具 | 期望的行内文本 |
|---|---|
| `run_shell` 成功 | `run_shell · pwsh: npm run …` + `0.8s · 42 行` + 可展开输出 |
| `run_shell` 失败 | 红字 `Exit code: 1`（或 stderr 首行），`0.8s`，可展开完整 stderr |
| `file_actions read` 带区间 | `Reading src/a.ts (40–80)` + `lines 40–80 · 41 行 · 1.2 KB` |
| `file_actions read` 整文件 | `Reading src/a.ts` + `120 行 · 3.4 KB` |
| `file_actions search` | `Searching src for "foo"` + `18 matches · 3 files` |
| `file_actions write` | `Writing src/a.ts` + `1.2 KB` |
| `file_actions patch` | `Patching src/a.ts` + `+42 −3` |
| `memory get`（原本无进度行） | `memory · get session/foo` + `12 行` |
| `calculator` | `calculator` + 结果首行预览 |
| MCP 工具 | 保持现在的参数 JSON summary，另加 `0.3s` |
| 被预算/禁用拦下 | `not executed (turn tool budget spent)`，`status=error`，**不**进失败计数（沿用 `counts_as_failure`） |
| 历史会话重载 | 指标、错误、预览全部还在（`historyRows.ts:64` 的 `...e` 展开 + F1 类型） |
| 老库（没有 outcome） | 行文本与今天逐字相同 |

---

## 6. 分批开工

| 批 | 内容 | 独立可验收 | 风险 |
|---|---|---|---|
| **B1**（先做，收益最大） | S1 + S2 + S3 + F1 + F2 + F4 的 1/2/5 + F5 的 metrics | `run_shell` 失败能看到 `Exit code`，`read` 能看到行数 | 低。新字段可选，老前端/老库不受影响 |
| **B2** | S4 的 `call_label` + 两处动词行补区间 + F4 的折叠态后缀 | 每个工具都有参数，`memory`/`todos` 不再只有名字 | 中。要防"兜底标签覆盖工具专有文案"——顺序已在 S4 定死，用一条集成测试按钮 |
| **B3** | F4 的输出预览 details + F3 快照透传 + `turn.record_tool_row` 同步 | 断线/重载后行不退化，需要时能看完整错误 | 低 |
| **B4**（可选） | 把标签搬进前端 i18n（事件里带 `tool`+`args`，前端拼词） | 三种语言的行文案统一 | 高。会动 40 多处 Rust 文案，单独一轮 |

---

## 7. 验证

- **前端**：按 `frontend-build-and-probe` 技能（tsc + vite build + `render-probe` 截图）。
  本机不要用 `npx`；截图用 `png-ink` 看墨迹，确认红字/指标行真的出现了。
- **Rust**：`cargo check --manifest-path src-tauri/Cargo.toml`（`tauri dev` 占着 `target/` 时先关，
  否则 LNK1104）；单测走 `rust-verification-gate` 里记的路径（`cargo test` 二进制在本机
  有 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 的坑）。
- **端到端手测**：一个回合里依次触发 `file_actions read`（带区间）、`search`、`run_shell` 失败各一次；
  看行内文本 → 展开输出 → 刷新窗口看快照恢复 → 重开会话看历史行。
- **回归**：折叠组标题（`toolGroup.completed*`）、并行调用的行不串号、`run_in_background` 行、
  子智能体行（`agent` 行没有 outcome，渲染必须原样）。

---

## 8. 明确不做

- 不把完整工具输出塞进事件或 DB；`preview` 上限 800 字符。
- 不改 `summary` 的语义（它是"动词短语 + 命令"，历史行依赖它）。
- 不动折叠组的标题与计数逻辑。
- 不给 `agent` / `planner` 行套 outcome（它们有自己的 `agent-task-*` 通道）。
- 不在 Rust 里拼任何人类文案（"40 行"），只出数字。

---

## 9. 实施状态（2026-11-28）

### 已落地（B1 + B2）

| 层 | 文件 | 改动 |
|---|---|---|
| Rust | `src-tauri/src/tools/outcome.rs`（新增） | `measure()` 单一测量点 + `call_label()` 参数标签 + 单测 |
| Rust | `tools/mod.rs`、`events.rs` | `emit_tool_call_done` 带 `outcome`；事件结构加可选字段 |
| Rust | `llm_complete/engine.rs` | 计划阶段统一发 `*tool · 参数*` 行；`timed()` 按调用计时；结算处测量并同时写入事件与 `record_tool_row` 快照 |
| Rust | `tools/file_actions.rs` | `Reading {path}` 补行区间；`Searching …` 补 `(max N)` |
| 前端 | `types.ts`、`api.ts`、`backgroundTurns.ts` | `ToolCallOutcome` 接口与三处透传 |
| 前端 | `App.tsx` | done 分支写入 `outcome`，并把 `outcome.error` 落到 `entry.error`（修死字段） |
| 前端 | `ToolCallGroup.tsx`、`ToolCallGroup.css` | `metricsFor()` 指标行、失败文本优先、行内可折叠输出 |
| i18n | `en.ts`、`zh-CN.ts`、`zh-TW.ts` | `toolGroup.metrics.*`、`toolGroup.output.*` |

### 验证记录

- `cargo check --manifest-path src-tauri/Cargo.toml` → 通过。
- `tsc --noEmit`、`vite build` → 通过。
- `render-probe` 实测（五种行：带区间 read / 失败 run_shell / search / memory / 无 outcome 的老行）：
  - `lines 40–80 · 12ms`、`18 matches · 3 files · 250ms`、`12 lines · 3ms`；
    指标色 `rgb(92,94,98)`，几何上位于描述行下方（`metaBelowDesc: true`），不重叠。
  - 失败行：`boom: missing dependency @types/node`，色 `rgb(192,57,43)`；`Show error` 展开后 `pre` 高 80px。
  - 无 outcome 的老行：无指标行、无展开块，渲染与改动前完全一致。
  - 折叠卡片副行：最新一行失败时直接显示失败原因，不必展开。
- Rust 单测**未执行**（本机 `cargo test` 二进制启动失败，见 `rust-verification-gate`）；
  `measure()` 的行为由 `cargo check` + 夹具覆盖，真机跑一轮仍需人工确认。

### 未做（有意留空）

- **B3**：历史会话老数据的 outcome 回填——老行渲染正确，只是没有指标。
- **B4**：工具名与动词短语搬进前端 i18n；标签仍是 Rust 拼的英文（与改动前一致）。
- 失败行不显示 `exit_code` 指标（错误文本优先），退出码在展开的 preview 里可见。
