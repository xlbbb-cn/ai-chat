技能是可复用的工作流：一个目录加一个 `SKILL.md`（或 `skill.md`）。YAML frontmatter 描述元数据；正文是加载该技能时追加进系统提示词的片段。技能适合代码评审、发布检查、数据分析这类重复流程。

技能是**按工作区隔离**的：每个工作区有自己的一套技能与启用状态。

## 技能放在哪

```text
<workspace>/.data/skills/
  code-review/
    SKILL.md          ← 小写 skill.md 也可以
    templates/
      review.md        ← 可选模板/数据
  release-notes/
    skill.md
```

- 唯一的技能根目录是 `<workspace>/.data/skills`。有且只有这一个——它在后端只有一处计算，因此不可能出现第二个位置。
- 每个技能一个子目录；目录名可以与 frontmatter 的 `name` 不同。
- 解析时先按目录名匹配，再扫描所有技能的 frontmatter `name`。从 zip 解出的 `gracker-writing-main/` 只要声明 `name: gracker-writing`，仍按 `gracker-writing` 解析。
- `skill.md` 与 `SKILL.md` 都接受。
- 删除留下的备份目录（`demo.bak.1`）不会被当作技能扫描。

## 用面板新建技能

1. 点顶部工具栏的**技能**。
2. 点 **+ 新建技能**。
3. 填写：
   - **名称**：给模型看的 id，例如 `code-reviewer`；目录用这个名字创建。
   - **描述**：一句话说明能力与触发条件；这是模型判断是否加载它的主要信号。
   - **系统提示词**：点**编辑**打开带实时预览的 Markdown 编辑器。
   - **版本**：可选，例如 `1.0.0`。
   - **允许的命令**：逗号分隔的可执行文件名，例如 `git, rg, npm`；留空表示不限制。
4. 点**保存**。应用写入 `.data/skills/<name>/skill.md`。
5. 在列表里打开开关，为当前工作区启用它。

> 新技能默认关闭。启用状态存在 `.data/skills.status`，重启后保持。

### 启用 / 停用

- 开关只影响当前工作区。
- 停用的技能不会列给模型，也无法用 `use_skill` 加载。
- 删除技能目录不会移除 `skills.status` 里的记录；记录会保留到目录重新出现。
- 面板按 frontmatter 的 `name` 记录启用状态。

## SKILL.md 格式

```markdown
---
name: code-reviewer
description: Review code changes in the current workspace and output issues ordered by severity. Use when the user mentions code review, reviewing a diff, or checking a PR.
version: 1.0.0
author: your-name
allowed-commands:
  - git
  - rg
  - npm
allowed-tools:
  - file_actions
  - run_cmd
license: MIT
---

You are a rigorous code reviewer. Follow this process:

1. Use `file_actions` to inspect the files involved in `git diff`.
2. Check correctness, edge cases, concurrency and error handling first.
3. For each issue, give the file, line, cause and a fix.
4. End with three buckets: blocking / recommended / optional.

Do not modify code unless the user explicitly asks.
```

### Frontmatter 字段

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `name` | 是 | 面向模型与面板的技能 id |
| `description` | 是 | 能力与触发条件；显示在可用技能列表里 |
| `version` | 否 | 版本字符串，仅作说明 |
| `author` | 否 | 作者 |
| `allowed-commands` | 否 | 可执行文件白名单；留空表示不限制 |
| `allowed-tools` | 否 | 保留元数据；实际工具暴露仍由**工具**面板控制 |
| `context` | 否 | 可选上下文说明，保留字段 |
| `agent` | 否 | 可选关联智能体，保留字段 |
| `license` | 否 | 许可证 |

`---` 之后的正文就是 `system_prompt`，支持 Markdown，会原样插入模型上下文。

### 路径

技能内部的引用相对于**技能根目录**（含 `SKILL.md` 的那个目录）：

```markdown
Use the template in `templates/review.md`.
Read `templates/review.md` with `skill_read`.
```

## 技能如何到达模型

启用技能不会把整段提示词塞进每次请求。流程是：

1. 系统提示词里得到一份**可用技能**列表，只含 `name` 与 `description`。
2. 模型判断任务需要它时，调用 `use_skill`。
3. 后端把完整正文、技能根目录与 `SKILL.md` 路径作为内部上下文追加进去。
4. 会话记住该技能已加载，不会重复加载。
5. 同时注入 `skill_read`，模型可以读取技能目录里的模板/数据。

也可以显式要求：

```text
{skill:code-reviewer} Review this change.
```

系统提示词会把这种写法当作直接请求，要求模型先加载该技能。

### `skill_read`

- 动作：`read` / `list` / `search`
- 路径相对于技能根目录；`.` 就是根目录
- 只能访问已启用/已加载的技能目录；`../` 越界会被拒绝
- `read` 支持 `start_line` / `end_line`
- `search` 支持 `query`、`recursive`、`case_sensitive`、`use_regex`、`glob`、`max_results`

## 命令白名单

`allowed-commands` 按**并集**合并：多个已启用/已加载的技能都声明了命令时，全部进入本轮允许集合；比较不区分大小写并去重。

当前版本在 `run_cmd` 上生效：第一个可执行文件名（去掉扩展名）必须在白名单里，否则调用被拒。`run_shell` 仍走风险评估与确认，不受这个列表过滤。

留空表示不限制。一旦任何已启用/已加载的技能声明了非空白名单，未列出的可执行文件都会被拒绝。尽量收紧：

```yaml
allowed-commands:
  - git
  - rg
  - npm
  - node
```

## 自我进化模式

**设置 → 运行 → 启用自我进化模式**：

- 让助手通过 `file_actions` **自行安装、扩展、编辑、删除技能**——`mkdir` 建目录，`write` / `patch` 写 `skill.md`，`move` 重组，`delete` 删除；
- 唯一的技能根目录是 `<workspace>/.data/skills`——没有第二个技能位置，助手被告知这一点；
- 修改或删除该根目录下的任何内容前，会创建 `<文件名>.bak.N` 备份（整个目录则是 `demo.bak.1`）；
- 在 `skills.status` 里把来源标记为 `self-evolution`；
- **强制打开 `file_actions` 与 `memory` 工具**——它们在工具面板里被锁定，无法关闭：面板关不掉、备份关不掉、手改 `config.json` 也关不掉；
- 打开专用的 `evolution` 记忆作用域存放已验证事实（见 [记忆待办与定时器](记忆待办与定时器)）；
- 助手可以用 `use_skill` 加载自己刚写的技能，不必先在面板里启用；
- 模型还会拿到 `mcp_self` 工具，可以搭建自己的 MCP 服务器（见 [MCP服务器](MCP服务器)）；
- 可以运行一次「打盹」维护，把积累的技能与记忆收敛整理：运行期间空状态问候会被进度行（阶段、轮次、工具调用数）替代，完成后留下可点击的报告气泡，中途停下会写一条续跑备注，下一轮接着做而不是从头开始。

这是高风险开关：模型能改写自己的指令。请只在受控工作区、短时使用，事后检查 `.bak` 文件与 `git diff`。

### 体验包（跨工作区）

技能属于一个工作区（`.data/skills`），**体验包**属于整个应用——它存在应用数据目录的 `experiences/<slug>/`，切换工作区不会丢。一个包就是一个可移植目录：

```text
experiences/<slug>/
  guide.md      方法：怎么用、要注意什么
  skill/        可选的技能部件（装进工作区后 use_skill 可用）
  scripts/      可选脚本（装进 .data/scripts 后 run_cmd 按裸名解析）
  mcp/          可选 MCP 服务器（装进 .data/mcp 后由 mcp_self 托管）
```

`experience` 工具（只在自我进化模式下出现）负责它，且**从不执行任何东西**：

| 动作 | 作用 |
| --- | --- |
| `search` | 按标题与 matchers（`ext:docx`、`mime:…`、`magic:…`、`name:*.pdf`、`tag:…`）搜索，先搜再动手 |
| `load` | 读 `guide.md`——方法本身，不含任何执行 |
| `install` | 把部件复制进当前工作区的 `.data/{skills,scripts,mcp}`（同名覆盖时旧副本留 `.bak.n`），之后由 `use_skill` / `run_cmd` / `run_shell` / `mcp_self` 照常运行 |
| `pack` | 把工作区里已有的技能 / 脚本 / MCP 服务器快照成一个包 |
| `distill` | 把几个包折成一个（默认归档源包，`keep_sources` 可保留） |
| `forget` | 归档一个包 |

面板在 **设置 → 调试 → 监视器 → 全局经验包**（只在自我进化模式开着时出现）。每行给出部件（skill / scripts / mcp）、matchers、工具数、来源工作区、`requires`（打包那台机器的快照）以及「本机」检查：缺依赖时显示差几项，而依赖装不装是你的活——包自己从不跑检查。这里也能**归档**一个包。

与技能的关系：包可以**包含**技能，但两者不是一回事——技能没有跨工作区的位置，经验包才有。

## 导入、导出与迁移

- **手动导入**：把技能目录复制进 `<workspace>/.data/skills/`，重开技能面板。
- **完整备份**：标题栏 `⋯` → **导出备份…** 包含 `workspace/skills/` 与 `workspace/skills.status`；恢复时写回当前工作区。
- **搬到别的工作区**：复制 `.data/skills` 与 `skills.status`，或重新启用这些技能。
- **版本管理**：把技能源放在 Git 仓库里再复制进 `.data/skills`；`.data` 本身应加进 `.gitignore`。

## 示例：发布检查技能

```markdown
---
name: release-check
description: Check versions, builds, changelogs and installers before a release. Use when the user mentions release, shipping or packaging.
version: 1.0.0
allowed-commands:
  - git
  - npm
  - node
  - cargo
---

You are the release owner. Do this in order:

1. Read `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`; confirm the versions match.
2. Run `npx tsc --noEmit` and `cargo check`; record failures.
3. Confirm the newest git tag is the version being released.
4. Summarize the changelog and list breaking changes.
5. Output a release checklist and mark failed items.
```

## 常见问题

| 现象 | 检查 |
| --- | --- |
| 列表里没有技能 | `.data/skills` 存在吗？技能是不是在工作区外？ |
| 启用了但模型从不用 | `description` 是否写清了触发条件？任务真的需要它吗？ |
| `use_skill` 说找不到 | 目录名与 frontmatter `name` 不一致；当前工作区对不对？ |
| 同一会话里改动不生效 | 已加载的技能不会重载；新开对话或重新启用 |
| 命令被拒绝 | 该可执行文件不在 `allowed-commands` 并集里 |
| `run_cmd` 拒绝某个程序 | `run_cmd` 只跑 PATH 里能解析的名字——带路径请用 `run_shell` |
| 助手自己改写了技能 | 自我进化模式开着；检查 `.data/skills/*/*.bak.N` |
| 工具面板里文件操作/记忆变灰 | 这两个工具由自我进化模式接管，开启期间无法关闭 |
| 导入 zip 后名字不对 | 以 frontmatter `name` 为准，目录名可以不同 |