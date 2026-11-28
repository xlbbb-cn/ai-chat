插件把一段**常驻程序**变成模型可以直接调用的工具。它补的是内置工具、脚本与 MCP 之外的位置：技能是提示词，脚本是一次性命令，MCP 是另一套协议；插件则是工作区里的一组常驻实例加一条消息总线。

链路是固定的：模型调用工具 → 工具把消息发到**插件消息总线** → 总线把请求交给该插件的某个实例 → 实例的回复沿同一个请求 id 回来。

## 打开与入口

| 位置 | 作用 |
| --- | --- |
| 设置 → 运行 → **启用插件系统** | `config.plugins_enabled`，默认关闭。开启后模型才会拿到 `plugin` 工具和每个插件声明的 `plugin_<id>_<tool>` |
| 设置 → 运行 → **插件管理器** | 打开一个**独立窗口**（不是设置面板里的分区）；关闭插件系统时这一行不显示 |
| 插件管理器内 | 带筛选框的插件列表、实例表、设置表单、日志尾部、总线审计条；标题旁的书籍图标打开「如何创建插件」说明，启动/停止旁的**导出**与工具栏的**导入插件包**见下文*导出与导入* |

服务式插件需要的**服务端点**（`127.0.0.1:<端口>`）和**令牌**也在管理器窗口里，可一键复制。

插件是**工作区内容**：换工作区就换一整套插件，切走时运行中的实例全部停止（它们属于旧目录）。

## 目录与清单

```text
<workspace>/.data/plugins/<id>/plugin.json   # 清单；<id> 必须等于文件夹名
<workspace>/.data/plugins/<id>/<entry>       # 入口脚本（server.mjs / server.py / …）
<workspace>/.data/plugins/<id>/config.schema.yaml  # 可选：设置表单
<workspace>/.data/plugins/<id>/config.yaml         # 设置值（由管理器写入）
```

```json
{
  "name": "Clock",
  "version": "1.0.0",
  "description": "面板里的一行说明。",
  "runtime": "node",
  "entry": "server.mjs",
  "instances": 2,
  "enabled": true,
  "auto_start": true,
  "call_timeout_ms": 30000,
  "transport": "spawn",
  "subscriptions": ["clock/#"],
  "tools": [
    { "name": "now", "description": "当前时间。", "parameters": { "type": "object", "properties": {} } }
  ]
}
```

| 字段 | 说明 |
| --- | --- |
| `name` / `version` / `description` | 面板与工具描述展示用 |
| `runtime` | `auto` / `python` / `node` / `shell` / `exec`；`auto` 按 `entry` 后缀推断 |
| `entry` | 相对插件目录的入口文件 |
| `command` + `args` | 编译好的二进制或自带解释器时的逃生口，优先于 `runtime` |
| `env` | 追加给子进程的环境变量 |
| `instances` | 常驻实例数，默认 1，上限 8 |
| `enabled` | 是否参与加载；停用的插件既不占实例也不给工具 |
| `auto_start` | 打开工作区时是否预热实例池（不预热时**第一次调用**仍会拉起） |
| `call_timeout_ms` | 单次回复期限，`0` = 默认 30 秒，上限 600 秒 |
| `transport` | `spawn`（默认）/ `service` / `both`，见下文 |
| `subscriptions` | 订阅的总线主题，支持 `+`（一级通配）与结尾 `#`；留空 = `<id>/#` |
| `tools` | 声明给模型的工具；每个都会变成 `plugin_<id>_<tool>`（单项可带 `topic`，留空 = 插件 id） |
| `config_page` | 自带设置页面时的 HTML 文件名（否则用 schema 表单） |

清单损坏不会拖垮整个加载：这个插件变成管理器「无法加载的清单」里的一条，其它插件照常工作。

## 导出与导入

插件管理器用**一个 `.zip`** 在工作室之间搬运插件：

| 操作 | 实际行为 |
| --- | --- |
| **导出**（启动/停止旁） | 打包选中的插件：`plugin.json` 位于压缩包根目录，其后是入口脚本、设置架构与资源文件。隐藏文件、`*.bak.*`、`node_modules/` 与各类缓存目录不打包。只有插件声明了 `config.schema.yaml` 时才带上 `config.yaml`，且先移除所有 `secret` 字段——导入后需要重新填写令牌 |
| **导入插件包**（工具栏） | 选中一个 `.zip`（本应用导出的包，或手工压缩、内含单个 `plugin.json` 的文件夹），解包到当前工作区并选中新插件。若同名插件已安装，先停止它的实例，再把旧目录移到 `.data/plugin-backups/<id>-<时间戳>/` |
| 直接拒绝，不留痕迹 | 压缩包内没有 `plugin.json`、一个包里有两个插件、声明的 `entry` 或 `config_schema` 未随包携带，或工作区已有 32 个插件 |

插件包也是迁移手段：[配置与备份](配置与备份) 的备份包不含 `.data/plugins`，换工作区前先把需要的插件导出。

## 协议

stdio，**一行一个 JSON 对象**（不是 JSON-RPC 帧）：

```text
host -> {"type":"hello","plugin":"clock","name":"Clock","instance":0,"instances":2,
         "workspace":"…","dir":"…/.data/plugins/clock","topics":["clock/#"],"protocol":1}
node -> {"type":"ready","plugin":"clock","version":"1.0.0","tools":["now"]}
host -> {"type":"request","id":"r1","topic":"clock","tool":"now","args":{},"session_id":"…","timeout_ms":30000}
node -> {"type":"reply","id":"r1","ok":true,"result":{"time":"12:00"}}
node -> {"type":"reply","id":"r1","ok":false,"error":"失败原因"}
```

- `hello` 之后 **15 秒**内没有 `ready`，该实例判为失败；
- 宿主还会发 `event`（`{"type":"event","topic":"…","payload":{…}}`）、`ping`、`shutdown`；
- 实例可以主动发 `event`（进总线审计，管理器可见）、`log`（`{"level":"info","message":"…"}`）与 `pong`；
- **stdout 只写协议行**：非 JSON 行会被记日志并丢弃，`print` 不会让插件死掉；诊断信息写 stderr，进插件自己的日志环（保留最近 200 条）。

## 实例、调度与生命周期

- 一个插件跑 N 个长驻实例，**在飞请求最少的实例接单**，平局按轮询；一次请求写不进去时先失败转移到兄弟实例一次，再报错。
- 实例按需启动（`ensure_started`）：缺的补齐、死的剔除，下次调用重试整个池。
- 子进程以「解释器 + 入口路径」启动，**永远不是 shell 字符串**，工作目录是插件目录，环境里带 `PLUGIN_ID` / `PLUGIN_INSTANCE` / `PLUGIN_DIR` / `PLUGIN_WORKSPACE`。
- 子进程加入进程守卫（Windows job object / Unix 进程组），退出应用会杀掉整棵树。
- 切换工作区会 `stop_all()`；应用退出由守卫兜底。

## 服务式插件

`transport` 为 `service`（或 `both`）时，进程由你自己的守护方式启动，不归应用管：

1. 在管理器里读出**服务端点**与**令牌**（令牌存在 `.data/plugins/.link-token`）；
2. 进程主动连上 `127.0.0.1:<端口>`，第一行发：

```json
{"type":"hello","plugin":"clock","token":"<面板里的令牌>"}
```

应用回 `{"type":"welcome","instance":"clock#link0","config":{…}}`，之后就是同一套协议。

- **一条连接 = 一个实例**：要 3 个 worker 就连 3 次；断开只移除对应实例。
- 清单里的 `instances` 只约束 spawn 池，不约束服务连接数。
- 关闭附着的实例只是断 socket，**不会**杀掉不是应用启动的进程。

## 设置

两种方式二选一：

| 方式 | 做法 | 界面 |
| --- | --- | --- |
| 声明式表单 | 放一个 `config.schema.yaml`（`sections[] → fields[]`） | 管理器按字段类型渲染：`text` / `multiline` / `number` / `integer` / `boolean` / `enum` / `path` / `secret` / `json`，支持 `default`、`required`、`min` / `max` / `max_length`、`options`、`advanced`、`reveal_when` |
| 自带页面 | 清单里写 `config_page` 指向插件目录里的 HTML | 管理器内嵌该页面 |

**每个 section 和每个 field 都要写 `key`**：它是存进 `config.yaml` 的名字，也是表单里的锚点；`title` 才是用户读到的文字。下面这个形状是能加载的：

```yaml
title: 时钟设置                # 可选：表单标题
sections:
  - key: main                  # section 必填
    title: 时钟
    fields:
      - key: timezone          # field 必填 —— 存进 config.yaml 的名字
        type: text
        title: 时区
        default: UTC
      - key: format
        type: enum
        title: 制式
        options:
          - { value: 24h, label: 24 小时 }
          - { value: 12h, label: 12 小时 }
      - key: token
        type: secret           # 表单里打码，导出插件包时被剔除
        title: API 令牌
        advanced: true
```

schema 解析失败时，管理器会把解析器原文显示在表单的位置上，插件本身照常运行。报错带的是精确路径（按 `sections[<i>].fields[<j>]` 读）：

| 报错 | 含义 |
| --- | --- |
| `sections[0]: missing field 'key'` | 第一个 section 对象少了 `key:` 一行 |
| `sections[1].fields[2]: missing field 'key'` | 该 field 少了 `key`；只有 `title` 不算有身份 |
| `... .type: unknown variant 'textbox'` | `type` 不是上面九种之一 |
| `... .options: invalid type: string` | `options` 是 `{value, label}` 映射的列表，不能写成裸字符串 |
| `did not find expected key` | YAML 缩进问题：每级两空格，`fields:` 挂在它的 section 下，不要用 Tab |

`label` / `help` 可以作为 `title` / `description` 的别名。

- 值存在插件目录的 `config.yaml` 里，插件因此是**自包含、可整体复制**的；
- `secret` 字段保存后只回显掩码，填同一掩码表示「不修改」；
- 保存时逐字段校验，失败把字段级错误还给表单；
- 保存成功后**热推送** `{"type":"config","config":{…}}` 给所有就绪实例，不用重启；实例也能在 `welcome.config` 或自己的 `config.yaml` / `PLUGIN_DIR` 里拿到设置。
- **模型没有写设置的入口**：`plugin` 工具刻意没有改配置的动作，避免被提示词注入的回合把插件指向别处。

## 模型看到什么

两套工具面，一条总线：

- **`plugin` 控制工具**：`list`（先看这个）、`call`（`{plugin, tool, args}`，可带 `topic` / `timeout_ms`）、`publish`（向订阅某主题的所有实例扇出，无回复）、`events`（读实例主动推的事件，可选 `drain`）、`status`（实例状态、pid、在飞数、重启次数、最后错误）、`logs`（插件日志环）、`start` / `stop` / `reload`（生命周期）。
- **每个声明的工具**直接以 `plugin_<id>_<tool>` 出现在工具表里（上限 64 个，按函数名排序，因为它属于缓存的请求前缀）。两条路径最终都走 `PluginHost::call`。
- 清单在**回合中途**写入或 `reload` 后，下一次请求就能调用（与 MCP 的 `mcp_revision` 同样的重建条件）。
- **没有确认弹窗**：插件是你装进工作区的文件，信任级别同技能的脚本，而且它拿不到 shell；真正按风险打分并要求确认的仍是 `run_cmd` / `run_shell`。

## 写一个最小插件

```json
{ "name": "Echo", "version": "1.0.0", "runtime": "node", "entry": "server.mjs",
  "tools": [{ "name": "echo", "description": "原样返回文本。",
              "parameters": { "type": "object", "properties": { "text": { "type": "string" } } } }] }
```

```js
// server.mjs —— stdout 只写协议行，诊断写 stderr
import { createInterface } from "node:readline";

const send = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.type === "hello") return send({ type: "ready", version: "1.0.0" });
  if (msg.type === "request") {
    const text = msg.args?.text ?? "";
    return send({ type: "reply", id: msg.id, ok: true, result: { text } });
  }
  if (msg.type === "ping") send({ type: "pong" });
});
```

流程：

1. 用 `file_actions` 建目录与文件（工作区相对路径 `./.data/plugins/<id>/…`）；
2. `plugin` 动作 `reload` 重读清单，`list` 确认已加载，`start` 拉起实例池；
3. 聊天里直接调用 `plugin_<id>_<tool>`，或用 `plugin` 的 `call`；
4. 记得先在设置 → 运行里**启用插件系统**，否则模型拿不到任何插件工具。

## 边界与禁忌

- 插件目录里**不要放凭证**：密钥读设置值（`secret` 字段）。
- 子进程不是 shell：需要 shell 语义请把逻辑写进脚本，由插件自己调用。
- `.js` 入口在 `"type": "module"` 的工作区里按 ESM 解释——用 `server.mjs`，或 `server.cjs` 走 CommonJS。
- 一个插件只做一件事：它的工具共享同一个实例池和同一张设置页。
- 回复是模型读的文本：保持小、结构化、不带 ANSI 噪声。
- **备份暂不含 `.data/plugins`**：换工作区前，先把需要的插件导出成插件包（见*导出与导入*）。

## 常见问题

| 现象 | 原因与处理 |
| --- | --- |
| 模型完全看不到插件工具 | 设置 → 运行里的插件系统没开；开启后不需要重启，下一次请求生效 |
| 管理器里插件显示「无法加载」 | `plugin.json` 解析失败或 `<id>` 与文件夹名不一致；看管理器的 issues 与日志 |
| 实例一直是「启动中」然后「失败」 | 15 秒内没收到 `ready`：入口脚本报错，或 stdout 被别的东西污染——看插件日志 |
| 调用超时 | 请求期限默认 30 秒（`call_timeout_ms`，上限 600 秒）；长任务自己做进度或拆调用 |
| 服务式插件连不上 | 端点/令牌过期或端口未监听；重新复制令牌，确认进程连的是当前工作区的 `127.0.0.1:<端口>` |
| 设置改了但插件没反应 | 插件没有监听 `{"type":"config"}`，或实例当时不是就绪状态；`stop` 后 `start` 一次 |
| 换工作区后插件全没了 | 正常：插件按工作区隔离，新工作区需要自己那一份 |

## 相关页面

- [工具总览](工具总览) —— 工具开关与风险等级
- [MCP 服务器](MCP服务器) —— 另一种外部工具接入方式
- [工作区与本地数据](工作区与本地数据) —— `.data` 布局
- [配置与备份](配置与备份) —— 备份包内容与迁移
