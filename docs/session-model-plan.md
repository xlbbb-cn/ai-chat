# 后台轮次的模型与思考深度：保留与恢复

> 现象：一个会话的回复还在流式输出时切到别的会话（它被交给后台驱动、History 里留着 Running 徽标），
> 在别的会话里换了模型，再点回那个正在跑的会话——工具栏显示的是**刚换的那个模型和它的深度**，
> 而那一轮实际仍在用**原来**的模型跑。
>
> 结论先说：**这不是"没恢复"，是从来没有按会话记过。** 模型是 `App.tsx` 里的一个全局
> `selectedModel`，思考深度是 config 里按模型存的 `model_reasoning_effort`，两者都与会话无关；
> 而"这一轮到底用什么在跑"其实**后端早就知道并已经发过来了**（`turn-state` / `list_active_turns` /
> `get_turn_snapshot` 都带 `model`），前端在三处都把它丢掉了。

---

## 0. 判据

| # | 问题 | 结论 |
|---|---|---|
| Q1 | 后端知道这一轮的模型吗？ | **知道，而且已经在发。** `events.rs:140` 的 `TurnState.model`、`registry.rs:33` 的 `ActiveTurnInfo.model`、`progress.rs:64` 的 `TurnSnapshot.model` 都是现成字段。 |
| Q2 | 前端拿到了吗？ | 拿到了又扔了：`App.tsx:1846` 的 `turn-state` 监听只解构 `{session_id, turn_id, running, started_at_ms}`；`App.tsx:1782` 的 `reconcileTurns` 只写 `turn_id`/`started_at_ms`；`backgroundTurns.ts` 的 `attach` 从不读 `snapshot.model`。 |
| Q3 | 深度能不能也由后端给？ | 能，但**目前没发**。深度是 `llm_client.rs:628` 的 `resolve_reasoning_effort(&config, model)`，用的是 `LlmTurn.config`（构造时冻结的副本，`turn/mod.rs:71`）。加一个可选字段即可，语义天然正确。 |
| Q4 | 恢复"深度"是不是等于恢复"模型"？ | **基本是。** 深度按模型 id 存（`model_reasoning_effort: HashMap<String,String>`），模型一恢复，`reasoningEfforts[model]` 就把深度控件带回原值。唯一例外：运行中的轮次用的是**冻结**的配置副本，之后同模型的深度被改过时，控件会显示新值而轮次仍按旧值跑 —— 所以要发 Q3 那个字段。 |
| Q5 | 需要落盘吗？ | **不需要。** 工具栏选的模型本来就不落盘（`onChange={setSelectedModel}`，只有 Settings 的 `saveConfig` 才写 config 的 `model`）；所以恢复是**进程内**行为，不新增文件、不改 DB。 |
| Q6 | 改动面在哪一层？ | 全在前端 `App.tsx` + `HistoryPanel`；Rust 侧只有"多一个只读字段"的批次 2（可独立上线）。 |

---

## 1. 现状链路

```
sendMessage (App.tsx:3636)
      │  chatCompletion(history, skills, sessionId, selectedModel, …)   ← 只有模型 id
      ▼
chat_completion (Rust)  model_override ─► TurnRegistry::begin(model)    ← turn 冻结 config
      │
      ├─ emit TURN_STATE { session_id, turn_id, running:true, started_at_ms, model }   events.rs:140
      │                                   ▲ 前端丢掉 model（App.tsx:1846）
      ▼
handoffForegroundTurn (App.tsx:3416)   ← 用户切走
      │  BackgroundTurnDriver.arm(seed)  seed 里没有模型信息
      ▼
History 徽标 (HistoryPanel.tsx:585)   ← 只显示 "Running 1:23"，模型的唯一去处是 title 里的一句常量
      │
      ▼
点回该会话 (App.tsx:5784 onLoad)  →  driversRef 命中 → 驱动 transcript 接管
      │                              ✗ 没有任何一处碰 selectedModel / reasoningEfforts
      ▼
工具栏 (App.tsx:6390/6404)  value={selectedModel} = 上一个会话最后一次选的值
                            disabled={streaming}  ← 后台轮次下 streaming=false，**控件居然是能改的**
```

四个断点：

| 断点 | 位置 | 症状 |
|---|---|---|
| **G1 无会话级模型** | `App.tsx:897` `selectedModel`、`App.tsx:907` `reasoningEfforts` | 全局唯一，切会话不换，也不记"这个会话上次用什么"。 |
| **G2 收到后扔掉** | `App.tsx:1846`、`:1782`、`backgroundTurns.ts:attach` | 后端三次都给了 `model`，前端一次都没存。 |
| **G3 运行时控件可用** | `App.tsx:6394` `disabled={streaming \|\| backupExporting}` | 后台轮次可见时 `streaming===false`，模型/深度都能点，改动对那一轮**完全无效** —— UI 在撒谎。 |
| **G4 深度没有冻结值** | `events.rs:140` / `progress.rs:64` 无 `reasoning_effort` | 运行中轮次的真实深度只存在于 Rust 的冻结 config 里，前端只能拿"现在的配置"猜。 |

---

## 2. 目标模型

一个进程内的表，**每个会话一条**，外加一个纯函数决定"当前该显示谁"：

```ts
/** 一个会话自己的模型设置。 */
export interface TurnSettings {
  /** 模型 id（就是 `selectedModel` 的取值域）。 */
  model: string;
  /** 思考深度：`""` = provider 默认（与 `reasoningEfforts` 同一个空值语义）。 */
  effort: string;
}
```

```ts
// src/turnSettings.ts（新叶子模块，无 import，和 historyRows.ts 同级）
/**
 * 当前该显示的设置，按权威度排序：
 * 1. 正在跑的那一轮（后端冻结值，切回后台会话时就是它）；
 * 2. 这个会话自己的记录（轮次已结束，仍要恢复）；
 * 3. 兜底：当前全局值（新会话、从未跑过轮次的老会话）。
 */
export function pickTurnSettings(
  live: TurnSettings | undefined,
  sticky: TurnSettings | undefined,
  fallback: TurnSettings,
): TurnSettings;
```

三条不变式（写进模块注释，实现和 review 都按它判断）：

1. **运行时唯一的真相是 `selectedModel` / `reasoningEfforts`**，不引入第二份"当前值"。
   sticky 只在切换会话的那一刻把值**写回**这两个状态。
2. **运行中的轮次不可改参**：它的 `{model, effort}` 来自后端冻结值，控件禁用。
3. **深度不因切换而写盘**：`reasoningEfforts` 已经按模型存且由用户显式保存（`handleReasoningEffortChange`），
   恢复模型就恢复了控件值；切换会话不调用 `saveConfig`，不做 config 的 ping-pong。

---

## 3. 批次 1 · 保留与恢复（纯前端，先修 bug）

### F1 `App.tsx`：会话设置表

放在 `composerDraftRef`（`App.tsx:1014`，同样是"按会话记忆"的既有先例）旁边：

```ts
/** 每个会话自己的模型设置：切走再切回时恢复。进程内，不落盘（模型本来也不落盘）。 */
const sessionSettingsRef = useRef(new Map<string, TurnSettings>());
```

### F2 `runningSessions` 带上后端已经发来的模型

`App.tsx:1113`：

```ts
const [runningSessions, setRunningSessions] = useState<
  Record<string, { turn_id: string; started_at_ms: number; model?: string; effort?: string }>
>({});
```

三个写入点补齐：

- **`turn-state` 监听**（`App.tsx:1846`）：payload 类型加 `model: string`，`running:true` 分支写
  `{ turn_id, started_at_ms, model }`，并同一步 `sessionSettingsRef.current.set(sid, …)`
  （`effort` 批次 2 才有；**没有时不要覆盖**已记录的值）。
- **`reconcileTurns`**（`App.tsx:1782-1800`）：合并时把 `turn.model` 一起写进去；`live` 里没有的会话保持原样。
- **`dropRunningSession`**（`App.tsx:1643`）：只删 `runningSessions`，**不动** sticky 表。

### F3 四个"记下来"的写点

| # | 位置 | 写什么 |
|---|---|---|
| W1 | `sendMessage` 在 `chatCompletion` 之前（`App.tsx:3636` 上方）、`retry` 同理（`:4089` 上方） | `set(sessionId, { model: selectedModel, effort: reasoningEffort })` —— 这一轮真正要发出去的值。 |
| W2 | `turn-state` `running:true`（`App.tsx:1846`） | 后端冻结值覆盖 W1（含 legacy 全局深度回退，见批次 2）。 |
| W3 | `ensureDriverFor` 拿到 `snapshot` 后（`App.tsx:1695-1710`） | `snapshot.model`（+ 批次 2 的 `reasoning_effort`），覆盖"重载后重新挂载"这一路的空白。 |
| W4 | 用户显式换模型 | 见 F4。 |

`handoffForegroundTurn`（`App.tsx:3416`）**不需要**额外写：它的模型恒等于 W1 刚记下的那个；
少一个写点就少一处将来漂移的地方。

### F4 用户显式选择也要记

`onChange={setSelectedModel}`（`App.tsx:6395`）换成 handler：

```ts
const handleModelChange = useCallback((model: string) => {
  setSelectedModel(model);
  sessionSettingsRef.current.set(sessionId, { model, effort: reasoningEfforts[model] ?? "" });
}, [sessionId, reasoningEfforts]);
```

`handleReasoningEffortChange`（`App.tsx:4471`）在 `saveConfig` 之后同步同一行的 `effort`
（它的 `selectedModel` 全部改读新值，见 F5）。

为什么必须有 W4：没有它，"切回来 → 改模型 → 再切走切回"会被 sticky 表里的旧值覆盖，
用户会觉得"我改的模型又变回去了"。

### F5 切会话时把值写回状态

紧挨"driver 可见"的那个 effect（`App.tsx:2261-2305`），新增一个：

```ts
// 切到另一个会话：恢复它自己的模型设置（正在跑的轮次优先，其次是它上次的记录）。
useEffect(() => {
  const target = pickTurnSettings(
    runningSessions[sessionId] && { model: runningSessions[sessionId].model ?? "", effort: … },
    sessionSettingsRef.current.get(sessionId),
    { model: selectedModel, effort: reasoningEfforts[selectedModel] ?? "" },
  );
  // 用函数式更新，避免把 selectedModel 塞进依赖（否则每选一次模型都重跑一遍 effect）。
  if (target.model) setSelectedModel((prev) => (prev === target.model ? prev : target.model));
}, [sessionId, runningSessions, driversVersion]);
```

- `driversVersion` 已在同类 effect 的依赖里（`App.tsx:2304`），保证"重挂载拿到首个 `turn-state`"也会重跑。
- 函数式更新是这儿的要点：不写 `selectedModel` 到依赖数组，就不会因为用户选模型而反向触发恢复。
- **不写 depth**：`reasoningEfforts[model]` 已经跟着模型走了（判据 Q4）；运行中的冻结深度由 F6 单独显示。

### F6 工具栏：显示运行中的真实值，并把它锁住

`App.tsx:4870-4873`：

```ts
/** 后台/前台正在跑的那一轮的冻结深度；没有轮次时为 undefined。 */
const liveEffort = composerBusy ? runningSessions[sessionId]?.effort : undefined;
const reasoningEffort = liveEffort ?? reasoningEfforts[selectedModel] ?? "";
const reasoningSupported =
  selectedMetadata?.supports_reasoning === true || reasoningEffort !== "";
```

`App.tsx:6390-6410`：

```tsx
<ModelSelect
  models={availableModels.includes(selectedModel) ? availableModels : [selectedModel, ...availableModels]}
  value={selectedModel}
  disabled={composerBusy || backupExporting}          // ← 原为 streaming
  onChange={handleModelChange}
/>
{reasoningSupported && (
  <Dropdown
    value={reasoningEffort}
    disabled={composerBusy || backupExporting}        // ← 原为 streaming
    … />
)}
```

两处细节：

- `disabled` 从 `streaming` 换成 `composerBusy`（`App.tsx:4538`，= 前台流式 **或** 可见的后台驱动）：
  与"前台跑一轮"完全同构，堵掉 G3 的谎话。`composerBusy` 已经是"回车变成注入"的判据，
  语义上正好一致。
- `models` 里补上 `selectedModel`：恢复出来的模型可能已经被移出 `model_catalog`（Settings 改过），
  `Dropdown` 对未知 value 只显示 placeholder（`Dropdown.tsx:41` 的注释），会变成空白触发器。
  补一行就不会。

### F7 三语文案（锁住的说明）

`en.ts:223` / `zh-CN.ts` / `zh-TW.ts`，`app` 命名空间：

| key | EN | zh-CN |
|---|---|---|
| `app.modelLockedTitle` | `This conversation's turn is still running — its model and thinking depth stay as they are until it ends` | `这个会话的这一轮还在跑，模型与思考深度在结束前不会变` |
| `app.reasoningLockedTitle` | 同上（可复用同一个 key） | 同上 |

`title` 跟随 `disabled` 状态切换（`title={composerBusy ? t("app.modelLockedTitle") : t("app.modelSelectTitle")}`），
`Dropdown` 的 `title` 同理。

---

## 4. 批次 2 · 深度的真实值（Rust，4 处填充）

只为判据 Q4 的那个例外存在：**运行中的轮次用的是冻结配置，UI 必须显示它，而不是"现在的配置"。**
零行为变化（只多一个只读字段），但让"运行中控件显示的深度"可证明地等于"实际发出去的 `reasoning_effort`"。

### R1 `LlmTurn` 上加一个访问器（`src-tauri/src/turn/mod.rs:60`）

```rust
impl LlmTurn {
    /// 这一轮实际使用的思考深度（config 在构造时冻结，之后不再读盘）。
    pub fn reasoning_effort(&self) -> Option<String> {
        crate::llm_client::resolve_reasoning_effort(&self.config, &self.model)
            .map(str::to_string)
    }
}
```

`resolve_reasoning_effort` 已是 `pub(crate)`（`llm_client.rs:628`），优先级（per-model → legacy 全局 → None）原样保留。

### R2 三个载荷各加一个可选字段

| 结构 | 文件:行 | 填充点 |
|---|---|---|
| `ActiveTurnInfo` | `turn/registry.rs:33` | `turn/mod.rs:323` `info()` |
| `TurnSnapshot` | `turn/progress.rs:64` | `turn/mod.rs:363` `snapshot()` |
| `TurnState` | `events.rs:140` | `llm_complete.rs:344`（开始）、`turn/lifecycle.rs:158`（结束） |

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub reasoning_effort: Option<String>,
```

`serde(skip_serializing_if)` 保证老前端、日志、快照 diff 的观感不变。

### R3 前端类型与读取

- `src/api.ts:551` `ActiveTurnInfo`、`:560` `TurnSnapshot`：加 `reasoning_effort?: string | null`。
- `App.tsx:1846` 的 `turn-state` payload 类型加同一字段；`effort` 取 `payload.reasoning_effort ?? ""`。
- `reconcileTurns` / `ensureDriverFor` 同源写入。

### R4 单测

`turn/mod.rs` 内联测试（纯函数，无 IO）：

- `config.model_reasoning_effort["m"] = "high"` → `Some("high")`；
- per-model 为空、`model_settings.reasoning_effort = "low"` → `Some("low")`（legacy 回退）；
- 两者都空 → `None`（字段被 `skip_serializing_if` 省掉）；
- 非法值（`"thinking"`）→ `None`（与 `REASONING_EFFORTS` 白名单一致）。

> 本机 `cargo test` 二进制起不来（`rust-verification-gate`），这组测试只有 `cargo check --tests` 的编译保证，
> 真跑要等 CI 或桌面会话。

---

## 5. 批次 3 · 可见性（History 徽标）

判据 Q2 的正面收益：`turn-state` / `list_active_turns` **已经**给了模型，徽标只是没花那 20 行。

### H1 数据类型

`HistoryPanel.tsx:39`：

```ts
runningSessions?: Record<string, { started_at_ms: number; model?: string; effort?: string }>;
```

`App.tsx:5775` 传的就是同一份 state，无需额外接线。

### H2 徽标

`HistoryPanel.tsx:585-589`：

```tsx
<span
  className="history-running-badge"
  title={t("history.runningTitleWithModel", {
    model: model ?? "—",
    depth: effort ? t(`app.reasoning${cap(effort)}`) : t("app.reasoningDefault"),
  })}
>
  {model && <span className="history-running-model">{model}</span>}
  {t("history.running")}
  <span className="history-running-time">{formatUptime(runningSeconds)}</span>
</span>
```

- 宽度：`.history-running-model { max-width: 96px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 400 }`
  —— `.history-footer` 是 400px 侧栏里的 flex 行，模型名必须能被截断，不能把 `history-created` 挤掉。
- 颜色：沿用 `--c-success-text` / `--c-success-soft`，不引入新颜色（`HistoryPanel.css` 现有徽标规则不动）。
  若 `interface-design.md` 的 compact 区判定为"密度超标"，退化方案是**只做 tooltip**（`title` 里带 model + depth），
  可见文本保持 `Running 1:23`。
- 深度只进 tooltip，不占徽标版面：三语各一条 `history.runningTitleWithModel`，
  原 `history.runningTitle` 保留（无模型时兜底）。

### H3 复用的深度文案

`app.reasoningDefault/Low/Medium/High`（`en.ts:226-229`）已存在，徽标里按 `effort` 映射过去，
不新增一套"low/medium/high"字符串。

---

## 6. 行为矩阵（改完要逐条对上）

| # | 场景 | 今天 | 改后 |
|---|---|---|---|
| 1 | A 会话用 `A模型/high` 起一轮 → 立刻切到新会话 | 工具栏仍是 A（没动过） | 同 |
| 2 | 新会话里改选 `B模型/low` | 工具栏 B | 同 |
| 3 | 点回 A 会话（轮次还在跑） | 工具栏 **B**，且控件**可点**（点了对那一轮无效） | 工具栏 **A / high**，控件**禁用**，tooltip 说明原因 |
| 4 | 此时在输入框回车 | 注入到后台轮次（`App.tsx:4451`） | 同（不改注入语义） |
| 5 | 轮次在后台结束，再点回该会话 | 工具栏 B | 工具栏 **A**（sticky 表） |
| 6 | 在该会话继续发一条 | 用 **B** 发 ← 真 bug | 用 **A** 发 |
| 7 | webview 重载（轮次仍在跑） | 工具栏 = config 的 `model` | 工具栏 = 该轮次的模型（`turn-state` / `list_active_turns` / 快照） |
| 8 | 切到上周的老会话（本次进程没跑过） | 保持当前 | 保持当前（sticky 表里没有） |
| 9 | 新会话按钮 / `forkSession` | 继承当前模型 | 继承当前模型（新 sid 无 sticky → 兜底值） |
| 10 | Settings 点保存 | `onConfigSaved` 把 `selectedModel` 拉回 `cfg.model`（`App.tsx:5691`） | 不变（写在表之前的既有行为，不本次处理） |
| 11 | History 徽标 | `Running 1:23` | `A模型 Running 1:23`，tooltip 带 `high` |

---

## 7. 验证

前端的验证路径按 `frontend-build-and-probe`（本机 `npx` 不可用）：

```bash
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vite/bin/vite.js build
# 批次 2 才有：
cargo check --lib --tests --manifest-path src-tauri/Cargo.toml
```

**组件级 render-probe**（不需要 Tauri 后端）：`.data/tmp/model-probe.html` + `.ts`，四种状态各渲一次：

1. `ModelSelect` + `Dropdown` 正常态（`A模型` + `High`）；
2. `composerBusy` 态：两个控件 `disabled`，`title` 是锁定文案，读 `getComputedStyle` 确认 disabled 的视觉
   （不新造颜色，只允许既有的 disabled token）；
3. 超长模型 id：触发器与徽标都截断，`history-created` 仍在原位（读 `getBoundingClientRect` 的 `right` 是否越界）；
4. 未知模型（不在 `availableModels` 里）：触发器显示该 id，不是 placeholder 空白。

```bash
render-probe -Url http://127.0.0.1:1420/.data/tmp/model-probe.html -Expr .data/tmp/model-probe.js -Shot .data/tmp/model-probe.png
node .data/scripts/png-ink.cjs stats .data/tmp/model-probe.png <x> <y> <w> <h> <r,g,b>
```

用完删掉 probe 文件（`.data/tmp` 是草稿区），截图结论写回本文档的"实施状态"。

**真机手测**（需要 API key，用户或 CI 桌面会话）：
矩阵第 2/3/5/6/7 条，每步看三处 —— 工具栏两控件、History 徽标的模型名、那一轮的日志行
（后端日志里有每次请求用的 model；深度差异用 provider 端可见的 usage/reasoning 差异确认）。

---

## 8. 风险与边界

| 风险 | 处理 |
|---|---|
| sticky 表按会话累积（进程内每个跑过轮次的会话一条，几十字节） | 不设 LRU：条数天然被"本次进程访问过的会话"限制；不做清理逻辑，避免引入淘汰 bug。 |
| 批次 2 之前 `effort` 只能取前端状态（`reasoningEfforts[model]`），"同模型深度后台被改过"的边角会显示新值 | 已知偏差，批次 2 换成后端冻结值即闭合；批次 1 不承诺这一点。 |
| 恢复出来的模型不在 `model_catalog` | F6 的 `models` 兜底一行（否则触发器空白）。 |
| 切会话时 `setSelectedModel` 与用户手选互相打架 | W4：用户显式选择同时写 sticky；effect 用函数式更新且不把 `selectedModel` 放进依赖。 |
| 多窗口 | `turn-state` 是全局事件，别的窗口起的轮次会写进本窗口的 sticky —— 同一份后端真相，不冲突（`turn_id` 不变）。 |
| 工作区里 `src-tauri/src/tools/*.rs` 有 8 个未提交改动 | 本计划**不碰** `tools/`；批次 2 只动 `turn/`、`events.rs`、`llm_complete.rs`、`llm_client.rs` 的读侧。 |
| 徽标版面 | 400px 侧栏 + hairlines + compact 区（`docs/DESIGN.md`）：模型名必须可截断，深度只进 tooltip；判不过就退化成纯 tooltip。 |

---

## 9. 明确不做

- **不**把模型/深度按会话落盘（DB 不加列，config 不加键）：工具栏选模型本来就不落盘（判据 Q5），
  恢复是进程内的；重启后回到 config 的 `model`，与今天一致。
- **不改** `chat_completion` 入参：不新增 `reasoningEffortOverride`。理由：深度已经是 per-model 配置，
  再加一层"按请求覆盖"会让 `resolve_reasoning_effort` 出现两个来源，正是本仓 `model-judgment-plan.md` 里
  `K1 重复决策` 要消掉的东西。
- **不改** `resolve_reasoning_effort` 的优先级（legacy 全局回退保留）。
- **不动** `handoffForegroundTurn` 的所有权与事件时序（`backgroundTurns.ts` 的 arm/unarmed 契约一行不改）。
- **不给**子智能体 / planner / maintenance 轮次做会话级模型记录：它们没有会话，`TurnProgressSink` 给的是 `None`。
- **不改** Running 徽标的语义（计时来源、`turn-state` 的权威性、`reconcileTurns` 的自愈逻辑）。

---

## 10. 需要拍板的三条

| ID | 问题 | 推荐 | 备选 |
|---|---|---|---|
| D1 | sticky 记录是否跨"轮次结束"保留？ | **保留**。否则"后台跑完再切回"仍显示别人的模型 —— 那正是本 bug 最常被撞见的时序。 | 只在驱动存活期间生效（改动更小，但漏掉上句）。 |
| D2 | 运行中把模型/深度控件禁用？ | **禁用**。与前台的 `streaming` 完全同构，堵掉 G3 的谎话。 | 保持可点，但只写 sticky（"预选下一轮"），需要在 tooltip 里说清"对这一轮无效"。 |
| D3 | History 徽标显示模型名？ | **显示**（可截断）+ tooltip 带深度。 | 只做 tooltip，可见文本保持 `Running 1:23`；或整批 3 不做（批次 1 已修 bug）。 |
| D4 | 批次 2（Rust 加 `reasoning_effort`）本轮做？ | **做**。4 处填充 + 4 个类型 + 4 条单测，零行为变化，换来"UI 显示的深度 = 实际发出的深度"这条可证明的等式。 | 推迟；批次 1 用前端状态近似，接受第 8 节那条已知偏差。 |

---

## 11. 实施状态

**批次 1、2、3 全部落地**（工作区未提交，见末行）。

| 项 | 结果 |
|---|---|
| `node node_modules/typescript/bin/tsc --noEmit` | 干净 |
| `node node_modules/vite/bin/vite.js build` | 通过（2548 modules，34s；只有既有的 chunk >500kB 提示） |
| `cargo check --lib --tests --manifest-path src-tauri/Cargo.toml` | 通过；唯一警告是 `tools/memory.rs:2429 note_last_user_message` 未使用（并行会话的改动，与本计划无关） |
| `cargo test` | **未跑**：本机测试二进制起不来（`rust-verification-gate`），R4 的 5 条断言只有 `--tests` 的编译保证 |
| render-probe（`.data/tmp/model-probe.{html,tsx,js}`，已删） | 见下表 |

probe 实测（`/.data/tmp/model-probe.html`，372px 宽度模拟侧栏）：

| 断言 | 实测 |
|---|---|
| 运行中的两个控件禁用 | `disabled: true`，`title` = 锁定文案，`opacity: .6`，`cursor: not-allowed`（ModelSelect + Dropdown 均是） |
| 恢复出来的未知模型不空白 | 触发器文本 `mystery-model-x`（不是 placeholder） |
| 长模型名截断 | `clientWidth 88 / scrollWidth 234`，徽标 181px |
| 时间戳不被挤掉 | `created.right` 在行内，`footer` 溢出 0px；`history-meta` 仍有 66px |
| 无模型的旧轮次 | 徽标回到 88px，文案与 tooltip 与改动前一致 |

### 与计划的偏差（实现时定下的）

1. **F5 的 effect 依赖多了 `runningSessions`**：只依赖 `sessionId` 的话，"重载后重挂载"这一路（首个 `turn-state` 到达时已经在看该会话）不会重跑恢复。函数式更新保证用户手选不会反向触发。
2. **W1 记的是 `reasoningEfforts[selectedModel]` 而不是 `reasoningEffort`**：后者在源码里更靠后，而且批次 1 之后它会被"运行中冻结值"覆盖——写请求值时不能取显示值。
3. **徽标模型名上限 88px**（计划写 96px）：实测 96px 时 `history-meta` 只剩 58px，88px 保住 66px。
4. **`ModelSelect` 新增 `title` prop**（计划没写）：锁定文案要挂到触发器上，`Dropdown` 本来就有这个 prop。
5. **锁定文案的 key 定为 `app.modelSettingsLocked`**（计划里叫 `app.modelLockedTitle`），两个控件共用一条。
6. **`minimal` 深度**：后端白名单接受但前端控件不提供，徽标 tooltip 直接显示原文，不误标成 "Default"。

### 未做

- 真机手测矩阵（`#2/#3/#5/#6/#7`）需要 API key，留给桌面会话。
- 提交：工作区里 `src-tauri/src/tools/*.rs` 有并行会话的未提交改动，`events.rs` 里同时混有它的 `outcome: None` 测试字段，本计划的改动**没有提交**，避免把别人的在途工作一起带进去。
