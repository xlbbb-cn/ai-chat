#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Generate docs/architecture.html — a self-contained architecture map for
Rabbit Chat Studio.

Static inline SVG, computed coordinates, no external assets. Colours come from
the design tokens in src/App.css so the document matches the product.

Usage:  python3 docs/gen.py      (writes docs/architecture.html, no dependencies)
"""

import html
import pathlib

OUT = pathlib.Path(__file__).resolve().parent / "architecture.html"
W = 1480
PAD = 48


# ── text metrics ────────────────────────────────────────────────────────────
def esc(s):
    return html.escape(str(s), quote=True)


NARROW = set("iljtfr.,'!:;|()[]·")


def tw(s, size):
    w = 0.0
    for c in s:
        o = ord(c)
        if o >= 0x2E80:
            w += size
        elif c in NARROW:
            w += size * 0.34
        elif c in "mwMW@":
            w += size * 0.88
        elif c.isupper():
            w += size * 0.67
        elif c == " ":
            w += size * 0.30
        else:
            w += size * 0.56
    return w


# ── svg builder ─────────────────────────────────────────────────────────────
class Svg:
    def __init__(self, w, uid):
        self.w, self.uid = w, uid
        self.body, self.defs = [], []
        self.h = 0

    def add(self, s):
        self.body.append("  " + s)

    def marker(self, name, color):
        self.defs.append(
            f'<marker id="{name}" viewBox="0 0 8 8" refX="6.6" refY="4" '
            f'markerWidth="7.5" markerHeight="7.5" orient="auto-start-reverse">'
            f'<path d="M0.6 0.8 L6.4 4 L0.6 7.2" fill="none" stroke="{color}" '
            f'stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>'
            f"</marker>"
        )

    def rect(self, x, y, w, h, fill="var(--c-surface)",
             stroke="var(--c-border-i)", rx=4, sw=1, dash=None):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.add(f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" '
                 f'rx="{rx}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"{d}/>')

    def text(self, x, y, s, size=12.5, fill="var(--c-text)", weight=400,
             anchor="start", mono=False):
        fam = "var(--mono)" if mono else "inherit"
        w = f' font-weight="{weight}"' if weight != 400 else ""
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        self.add(f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}"'
                 f'{w}{a} style="font-family:{fam}">{esc(s)}</text>')

    def line(self, x1, y1, x2, y2, stroke="var(--c-border)", sw=1, dash=None):
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.add(f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" '
                 f'stroke="{stroke}" stroke-width="{sw}"{d}/>')

    def path(self, d, stroke="var(--c-accent)", sw=1.2, marker=None, dash=None):
        m = f' marker-end="url(#{marker})"' if marker else ""
        ds = f' stroke-dasharray="{dash}"' if dash else ""
        self.add(f'<path d="{d}" fill="none" stroke="{stroke}" stroke-width="{sw}" '
                 f'stroke-linejoin="round"{m}{ds}/>')

    def dot(self, x, y, color="var(--c-border-i)", r=2.6):
        self.add(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r}" fill="var(--c-bg)" '
                 f'stroke="{color}" stroke-width="1.3"/>')

    def render(self):
        self.h = round(self.h)
        defs = f"<defs>{''.join(self.defs)}</defs>" if self.defs else ""
        return (f'<svg viewBox="0 0 {self.w} {self.h}" role="img" '
                f'style="font-family:var(--font)">\n{defs}\n'
                + "\n".join(self.body) + "\n</svg>")


# ── diagram 1: layer map ────────────────────────────────────────────────────
CHIP_H = 46
CHIP_GAP = 10

BANDS = [
    dict(num="01", name="交互层", path="用户可见",
         groups=[
             ("窗口与输入", [
                 ("TitleBar", "窗口控制 · 应用菜单 · 运行状态", "n"),
                 ("Composer", "文本 · @文件 · /技能 · 模型选择", "n"),
                 ("空状态", "自进化「小憩」入口", "n"),
             ]),
             ("对话呈现", [
                 ("ChatMessage", "Markdown · KaTeX · 代码高亮 · 流式", "n"),
                 ("ToolCallGroup", "工具卡片 · 折叠 · 结果预览", "n"),
                 ("TodoList", "任务清单", "n"),
                 ("BackgroundTaskGroup", "轮内后台任务", "n"),
             ]),
         ]),
    dict(num="02", name="前端外壳", path="src/",
         groups=[
             ("外壳与桥接", [
                 ("App.tsx", "状态与事件中枢 · 3.9k 行", "core"),
                 ("api.ts", "invoke 包装 + listen 订阅", "core"),
                 ("types.ts", "前后端共享类型", "n"),
                 ("i18n", "en · zh-CN · zh-TW", "n"),
             ]),
             ("渲染", [
                 ("MarkdownPreview", "markdown-it · KaTeX · highlight.js", "n"),
                 ("ChatMessage 组件", "流式打字机 · 代码块复制", "n"),
                 ("Portal", "覆盖层挂载", "n"),
             ]),
             ("输入解析", [
                 ("fileMention", "@文件 / @目录 展开", "n"),
                 ("slashMention", "/技能 提及", "n"),
                 ("attachments", "图片与二进制转 base64", "n"),
             ]),
             ("侧边栏面板（11）", [
                 ("SettingsPanel", "设置 · 模型 · 主题", "n"),
                 ("SkillsPanel", "技能", "n"),
                 ("ToolsPanel", "工具", "n"),
                 ("McpPanel", "MCP", "n"),
                 ("AgentsPanel", "子智能体", "n"),
                 ("AgentMissionPanel", "任务监视", "n"),
                 ("HistoryPanel", "历史", "n"),
                 ("MemoryPanel", "记忆", "n"),
                 ("MonitorPanel", "请求审计", "n"),
                 ("UpdatePanel", "更新", "n"),
                 ("ProfilePanel", "档案", "n"),
             ]),
         ]),
    dict(num="03", name="IPC 桥", path="Tauri 2",
         groups=[
             ("前后端契约", [
                 ("invoke", "90 个 #[tauri::command]（lib.rs 注册）", "n"),
                 ("emit / listen", "45 个事件通道：chat-* · tool-* · agent-*", "n"),
                 ("capabilities", "default.json 运行时权限白名单", "n"),
                 ("plugins", "opener · dialog · clipboard-manager", "n"),
             ]),
         ]),
    dict(num="04", name="后端核心", path="src-tauri/src/",
         groups=[
             ("", [
                 ("main.rs", "Tauri 启动入口", "n"),
                 ("lib.rs", "AppState · 菜单 · 命令注册 · setup", "core"),
                 ("llm_complete.rs", "chat_completion 流式主循环 · 5.1k 行", "core"),
                 ("turn_guard.rs", "轮次 / 工具 / 失败守卫", "n"),
                 ("instance.rs", "单实例锁 · shadow 窗口", "n"),
                 ("process_guard.rs", "子进程登记 · 进程树终止", "n"),
                 ("logger.rs", "app.log · 保留天数", "n"),
                 ("utils/", "knowledge_graph · pdf2md-rs 两个独立 crate", "n"),
             ]),
         ]),
    dict(num="05", name="工具系统", path="src-tauri/src/tools/",
         groups=[
             ("分发与边界", [
                 ("mod.rs", "16 个核心工具 + 按开关注入", "core"),
                 ("risk.rs", "L0–L6 · 语法闸门 · 加权评分", "core"),
                 ("paths.rs", "工作区沙箱 · 拒绝 `..` 与符号链接逃逸", "n"),
             ]),
             ("能力", [
                 ("run_cmd", "PATH 单程序 · serve 常驻", "n"),
                 ("run_shell", "任意脚本 · 语法闸门", "n"),
                 ("file_actions", "读写 · patch · diff · .bak 备份", "n"),
                 ("search.rs", "ignore · globset · 结果截断", "n"),
                 ("scripts.rs", "脚本库 · 审批 + 预览", "n"),
                 ("memory", "session · user · repo · evolution", "n"),
                 ("todos", "任务清单与归档", "n"),
                 ("timer_*", "delay timer · 到点入队", "n"),
                 ("background_task", "轮内后台任务 · 结果作 user 消息", "n"),
                 ("skill_read / use_skill", "按需加载指令", "n"),
                 ("mcp_self", "自建 stdio MCP 服务器", "n"),
                 ("skill_merge", "维护轮专用（仅自进化）", "n"),
             ]),
         ]),
    dict(num="06", name="子智能体", path="src-tauri/src/agents.rs",
         groups=[
             ("编排", [
                 ("agents.rs", "SubAgent · Task · Mission · 6.5k 行", "core"),
                 ("选择器", "按工作区与对话挑出可用 agent", "n"),
                 ("plan_tasks", "拆任务 · 依赖 · 每任务 cwd", "n"),
                 ("ask_main_agent", "子智能体回调主模型", "n"),
             ]),
             ("三类后端", [
                 ("llm", "内部工具循环 · 分节记忆摘要", "n"),
                 ("cli", "无头编码 CLI 子进程 · 流式", "n"),
                 ("cloud", "远程 HTTP Agent 端点", "n"),
             ]),
             ("运行时", [
                 ("files.rs", "llm 型 agent 的 markdown 定义 + index.yaml", "n"),
                 ("mbus.rs", "进程内 pub/sub 任务总线", "n"),
                 ("supervisor.rs", "事件驱动监督者 · 最多 10 次激活", "n"),
                 ("exchange.rs", ".data/tmp 交付物交换区", "n"),
             ]),
         ]),
    dict(num="07", name="能力层", path="独立模块",
         groups=[
             ("", [
                 ("skills.rs", "SKILL.md · skills.status 唯一真源", "n"),
                 ("mcp.rs", "stdio / SSE / HTTP · 主池 + 每任务会话", "core"),
                 ("db.rs", "SQLite 访问层", "core"),
                 ("evolution_maintenance.rs", "静默维护轮 · 20 轮 / 60 调用 / 15 分钟", "n"),
                 ("model_metadata.rs", "三源能力目录 · 上下文与能力", "n"),
                 ("pdf_mentions.rs", "@paper.pdf#2 → Markdown（pdf2md-rs）", "n"),
                 ("workspace_files.rs", "文件浏览 · 分片读取 · 二进制", "n"),
                 ("backup.rs", "profile 导出 / 导入", "n"),
                 ("update.rs", "GitHub Releases 检查与下载", "n"),
             ]),
         ]),
    dict(num="08", name="持久化", path="随工作区 / 随应用",
         groups=[
             ("工作区 · 会话状态（切工作区即切换）", [
                 ("chat.db", "8 张表：历史 / 会话 / 摘要 / 审计 / 任务", "n"),
                 ("memory/", "user · repo · evolution（session 在内存）", "n"),
                 ("todos/", "任务清单", "n"),
                 ("timers.json", "未到点的 delay timer", "n"),
             ]),
             ("工作区 · 技能与交付", [
                 ("skills/ + skills.status", "技能唯一根 + 启用状态", "n"),
                 ("scripts/", "run_cmd 脚本库", "n"),
                 ("mcp/ + mcp.conf", "自建 MCP 服务器", "n"),
                 ("tmp/", "交付物交换区 · 永久保留", "n"),
             ]),
             ("应用数据", [
                 ("config.json", "30 个配置字段", "n"),
                 ("mcp_servers.json", "用户面板维护的 MCP 花名册", "n"),
                 ("sub_agents.json", "子智能体清单", "n"),
                 ("profiles.json", "档案：配置 + 技能启用状态", "n"),
                 ("agents/", "定义 + index.yaml 发现清单", "n"),
                 ("shadow/", "每窗口一份配置 fork", "n"),
                 ("app.log", "日志 · 按保留天数清理", "n"),
             ]),
         ]),
    dict(num="09", name="外部依赖", path="进程之外",
         groups=[
             ("", [
                 ("OpenAI 兼容 API", "SSE 流式 chat / models", "ext"),
                 ("MCP 服务器", "stdio 子进程 · SSE · HTTP", "ext"),
                 ("远程 Agent 端点", "cloud 子智能体", "ext"),
                 ("本机 CLI", "opencode · Claude Code · Codex", "ext"),
                 ("Neo4j", "knowledge-graph crate", "ext"),
                 ("GitHub Releases", "更新检查与下载", "ext"),
                 ("模型能力目录", "模型能力元数据", "ext"),
             ]),
         ]),
]


def chip_w(title, note):
    return min(max(max(tw(title, 12.5), tw(note, 11.0)) + 26, 150), 430)


def diagram_layers():
    s = Svg(W, 1)
    rail_w = 176
    cx0 = PAD + rail_w
    avail = W - PAD - cx0
    label_h, rgap, ggap, vpad = 20, 10, 18, 22

    y = 0.0
    warn = []
    for i, band in enumerate(BANDS):
        if i:
            s.line(PAD, y + 0.5, W - PAD, y + 0.5)
        plan = []
        for gname, chips in band["groups"]:
            widths = [chip_w(t, n) for t, n, _ in chips]
            rows, cur, curw = [], [], 0.0
            for chip, w in zip(chips, widths):
                need = w if not cur else CHIP_GAP + w
                if cur and curw + need > avail:
                    rows.append(cur)
                    cur, curw = [(chip, w)], w
                else:
                    cur.append((chip, w))
                    curw += need
            if cur:
                rows.append(cur)
            plan.append((gname, rows))

        content_h = 0.0
        for gi, (gname, rows) in enumerate(plan):
            if gi:
                content_h += ggap
            if gname:
                content_h += label_h
            content_h += len(rows) * CHIP_H + (len(rows) - 1) * rgap
        band_h = max(content_h, 56) + 2 * vpad

        # rail
        tx = PAD
        s.text(tx, y + vpad + 8, band["num"], 11, "var(--c-placeholder)", mono=True)
        s.text(tx, y + vpad + 32, band["name"], 15, "var(--c-text)", 500)
        s.text(tx, y + vpad + 51, band["path"], 10.5, "var(--c-muted)", mono=True)

        cy = y + vpad
        for gi, (gname, rows) in enumerate(plan):
            if gi:
                cy += ggap
            if gname:
                s.text(cx0, cy + 12, gname, 11, "var(--c-muted)")
                cy += label_h
            for ri, row in enumerate(rows):
                x = cx0
                for (title, note, kind), w in row:
                    if tw(note, 11.0) + 26 > w + 0.5:
                        warn.append(f"{band['num']}/{title}: note overflows")
                    if tw(title, 12.5) + 26 > w + 0.5:
                        warn.append(f"{band['num']}/{title}: title overflows")
                    if kind == "ext":
                        s.rect(x, cy, w, CHIP_H, fill="none", dash="4 3")
                    else:
                        s.rect(x, cy, w, CHIP_H)
                    if kind == "core":
                        s.add(f'<rect x="{x + 0.5:.1f}" y="{cy + 8:.1f}" width="2.5" '
                              f'height="{CHIP_H - 16}" fill="var(--c-accent)"/>')
                    s.text(x + 12, cy + 20, title, 12.5, "var(--c-text)", 500)
                    s.text(x + 12, cy + 35, note, 11, "var(--c-muted)")
                    x += w + CHIP_GAP
                cy += CHIP_H + (rgap if ri < len(rows) - 1 else 0)
        y += band_h

    s.h = y + 4
    for w in warn:
        print("WARN", w)
    return s.render()


# ── diagram 2: one chat turn ────────────────────────────────────────────────
TURN_STEPS = [
    ("提交", "Composer 提交", "回车 → sendMessage()：乐观渲染 user 气泡，分配本轮 turn id", "n"),
    ("装配请求", "invoke(\u201cchat_completion\u201d)", "session_id · 完整历史 · selected_tools · 模型与推理设置", "n"),
    ("装配请求", "lib.rs 组装请求", "AppState 配置 + 工具 schema + 技能目录 + @提及已展开", "n"),
    ("工具轮次循环", "stream_llm_request", "reqwest SSE → 上游 OpenAI 兼容 API（带取消 flag 与超时）", "loop"),
    ("工具轮次循环", "流式回传", "chat-token / chat-reasoning-token / chat-usage → 逐字渲染", "loop"),
    ("工具轮次循环", "判定 tool_calls", "空 id 补 call_xxx，保证 assistant(tool_calls) 与 tool 结果配对", "loop"),
    ("工具轮次循环", "风险闸门 risk::assess", "L0/L1 人工复核 · L2/L3 确认 · L4 审计 · L5/L6 放行", "loop"),
    ("工具轮次循环", "执行工具", "沙箱路径校验 → 子进程 / 文件 / 网络 / MCP / 子智能体", "loop"),
    ("工具轮次循环", "回填结果", "tool-call 事件 + tool 消息；失败计入 tool_failure_limit", "loop"),
    ("工具轮次循环", "边界注入与压缩", "turn_inbox 插话 · 后台任务 · mbus；超阈值先压缩再发下一轮", "loop"),
    ("收束", "chat-done", "无 tool_calls：写 history / session_summaries / memory，关注入窗口", "n"),
    ("收束", "end_turn_cleanup", "回收后台任务与进程登记，本轮注入窗口关闭", "n"),
]


def diagram_turn():
    W2 = 1440
    s = Svg(W2, 2)
    s.marker("ar2", "var(--c-accent)")
    s.marker("ar2m", "var(--c-border-i)")
    X, CW, CHH, GAP = 330, 720, 58, 26
    TL = X - 40          # timeline x
    y = 24.0
    centers = []
    first_of_phase = []
    seen = set()

    for i, (phase, title, note, kind) in enumerate(TURN_STEPS):
        if phase not in seen:
            seen.add(phase)
            first_of_phase.append((phase, y + CHH / 2))
        if kind == "loop" and TURN_STEPS[i - 1][3] != "loop":
            y += 34                      # room for the loop-box label
        if kind != "loop" and TURN_STEPS[i - 1][3] == "loop":
            y += 26                      # breathing room after the loop box
        centers.append(y + CHH / 2)
        y += CHH + GAP

    # loop box geometry
    loop_idx = [i for i, st in enumerate(TURN_STEPS) if st[3] == "loop"]
    box_top = centers[loop_idx[0]] - CHH / 2 - 15
    box_bot = centers[loop_idx[-1]] + CHH / 2 + 15
    box_left, box_right = X - 16, X + CW + 16
    box_mid = (box_top + box_bot) / 2
    ret_x = 1180
    target = centers[3]                  # stream_llm_request

    # return path first (sits behind the cards)
    s.path(f"M{box_right} {box_mid} H{ret_x} V{target} H{X + CW + 2}",
           "var(--c-accent)", 1.2, marker="ar2")
    s.text(ret_x + 16, (box_mid + target) / 2, "下一轮请求（tool 结果已配对）",
           11, "var(--c-accent-text)", anchor="start")

    # loop box
    s.rect(box_left, box_top, box_right - box_left, box_bot - box_top,
           fill="none", stroke="var(--c-border-i)", dash="5 4")
    s.text(box_left + 4, box_top - 9, "工具轮次循环 · 每轮一次上游请求，直到没有 tool_calls",
           11, "var(--c-accent-text)")

    # timeline + cards
    prev = None
    for i, (phase, title, note, kind) in enumerate(TURN_STEPS):
        cy = centers[i]
        if prev is not None:
            col = "var(--c-accent)" if kind == "loop" else "var(--c-border-i)"
            s.path(f"M{TL} {prev + 7} V{cy - 11}", col, 1.1,
                   marker="ar2" if kind == "loop" else "ar2m")
        s.dot(TL, cy, "var(--c-accent)" if kind == "loop" else "var(--c-border-i)")
        prev = cy

        s.rect(X, cy - CHH / 2, CW, CHH)
        s.text(X + 14, cy + 4, f"{i + 1:02d}", 11, "var(--c-placeholder)", mono=True)
        s.text(X + 44, cy - 2, title, 12.5, "var(--c-text)", 500,
               mono=not any(ord(c) >= 0x2E80 for c in title))
        s.text(X + 44, cy + 17, note, 11, "var(--c-muted)")
        if tw(note, 11) + 44 + 14 > CW:
            print("WARN turn note overflow:", title, int(tw(note, 11)))

    for phase, py in first_of_phase:
        s.text(TL - 18, py + 4, phase, 11, "var(--c-muted)", anchor="end")

    s.h = y + 18
    return s.render()


# ── diagram 3: sub-agent orchestration ──────────────────────────────────────
def diagram_agents():
    s = Svg(W, 3)
    s.marker("ar3", "var(--c-accent)")
    s.marker("ar3m", "var(--c-border-i)")
    CX, CHH = 620, 58
    ACC, MUT = "var(--c-accent)", "var(--c-border-i)"

    def card(x, w, cy, title, note, core=False):
        s.rect(x, cy - CHH / 2, w, CHH)
        if core:
            s.add(f'<rect x="{x + 0.5:.1f}" y="{cy - CHH / 2 + 8:.1f}" width="2.5" '
                  f'height="{CHH - 16}" fill="var(--c-accent)"/>')
        s.text(x + 14, cy - 2, title, 12.5, "var(--c-text)", 500,
               mono=not any(ord(c) >= 0x2E80 for c in title))
        s.text(x + 14, cy + 17, note, 11, "var(--c-muted)")
        if tw(note, 11) + 28 > w:
            print("WARN agent note overflow:", title, int(tw(note, 11)))

    def fan(xt, yt, xbs, yb, color=ACC, mk="ar3"):
        ym = (yt + yb) / 2
        if len(xbs) == 1:
            s.path(f"M{xt} {yt} V{yb - 2}", color, 1.2, marker=mk)
            return
        s.path(f"M{xt} {yt} V{ym}", color, 1.2)
        s.path(f"M{min(xbs)} {ym} H{max(xbs)}", color, 1.2)
        for xb in xbs:
            s.path(f"M{xb} {ym} V{yb - 2}", color, 1.2, marker=mk)

    def merge(xs, yt, xb, yb, color=ACC, mk="ar3"):
        ym = (yt + yb) / 2
        if len(xs) == 1:
            s.path(f"M{xs[0]} {yt} V{yb - 2}", color, 1.2, marker=mk)
            return
        for x in xs:
            s.path(f"M{x} {yt} V{ym}", color, 1.2)
        s.path(f"M{min(xs)} {ym} H{max(xs)}", color, 1.2)
        s.path(f"M{xb} {ym} V{yb - 2}", color, 1.2, marker=mk)

    yA = 44
    yC = yA + CHH + 48
    yD = yC + CHH + 48
    yE = yD + CHH + 52
    yF = yE + CHH + 48
    yG = yF + CHH + 44

    # A — main agent
    card(CX - 260, 520, yA, "主智能体", "call_subagent 工具 / 显式编排入口 · 只有主模型能派活", core=True)

    # C — selector + planner
    sel_x, sel_w = 190, 400
    pl_x, pl_w = 650, 400
    card(sel_x, sel_w, yC, "选择器", "按工作区与对话挑出可用 agent")
    card(pl_x, pl_w, yC, "规划器 plan_tasks", "拆任务 · 依赖 · 每任务 cwd 与后端")
    fan(CX, yA + CHH / 2, [sel_x + sel_w / 2, pl_x + pl_w / 2], yC - CHH / 2)
    # selector → planner
    s.path(f"M{sel_x + sel_w + 2} {yC} H{pl_x - 6}", ACC, 1.2, marker="ar3")

    # D — mission + tasks
    mw = 700
    card(CX - mw / 2, mw, yD, "Mission · Task 列表", "任务落 agent_tasks · 状态变化经 mbus 广播")
    merge([sel_x + sel_w / 2, pl_x + pl_w / 2], yC + CHH / 2, CX, yD - CHH / 2)

    # annotations on the right
    nx, nw = 1090, 330
    s.rect(nx, yD - CHH / 2, nw, CHH, fill="none", dash="4 3")
    s.text(nx + 14, yD - 2, "mbus", 12.5, "var(--c-text)", 500)
    s.text(nx + 14, yD + 17, "进程内 pub/sub 任务总线", 11, "var(--c-muted)")
    s.path(f"M{CX + mw / 2 + 2} {yD} H{nx - 2}", MUT, 1.1, marker="ar3m", dash="4 3")

    s.rect(nx, yD + CHH / 2 + 14, nw, CHH, fill="none", dash="4 3")
    s.text(nx + 14, yD + CHH / 2 + 30, "supervisor", 12.5, "var(--c-text)", 500)
    s.text(nx + 14, yD + CHH / 2 + 49, "只持 mbus 工具 · 最多 10 次激活", 11, "var(--c-muted)")

    # E — three backends
    bw, bgap = 260, 24
    bx0 = CX - (3 * bw + 2 * bgap) / 2
    bxs = [bx0 + i * (bw + bgap) for i in range(3)]
    card(bxs[0], bw, yE, "llm 后端", "内部工具循环 + 记忆摘要")
    card(bxs[1], bw, yE, "cli 后端", "无头编码 CLI 子进程")
    card(bxs[2], bw, yE, "cloud 后端", "远程 HTTP Agent 端点")
    fan(CX, yD + CHH / 2, [x + bw / 2 for x in bxs], yE - CHH / 2)

    # ask_main_agent — dashed callback from the llm backend up the left side
    lx = 150
    cy_llm = yE
    s.path(f"M{bxs[0] - 2} {cy_llm} H{lx} V{yA} H{CX - 260 - 2}",
           ACC, 1.1, marker="ar3", dash="4 3")
    s.add(f'<text font-size="10.5" fill="var(--c-accent-text)" text-anchor="middle" '
          f'transform="translate({lx - 16} {(cy_llm + yA) / 2}) rotate(-90)" '
          f'style="font-family:inherit">ask_main_agent · 子智能体回问主模型</text>')

    # F — exchange area
    card(CX - mw / 2, mw, yF, ".data/tmp/<batch>/<task>/", "交付物按任务分目录落地 · 永久保留")
    merge([x + bw / 2 for x in bxs], yE + CHH / 2, CX, yF - CHH / 2)

    # G — report
    card(CX - mw / 2, mw, yG, "MissionReport", "文件清单 + 摘要 → 主智能体用 file_actions 读取")
    fan(CX, yF + CHH / 2, [CX], yG - CHH / 2)

    s.text(nx, yF + 4, "交付物不进 profile 备份", 11, "var(--c-muted)")
    s.text(nx, yF + 22, "⋯ 应用菜单可打开该目录", 11, "var(--c-muted)")

    s.h = yG + CHH / 2 + 26
    return s.render()


# ── html ────────────────────────────────────────────────────────────────────
CSS = """
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root,html[data-theme="light"]{
  color-scheme:light;
  --c-bg:#FFFFFF;--c-surface:#FFFFFF;--c-surface-2:#F4F4F4;--c-hover:rgba(23,26,32,.04);
  --c-text:#171A20;--c-body:#393C41;--c-muted:#5C5E62;--c-placeholder:#8E8E8E;
  --c-border:#EEEEEE;--c-border-s:#F4F4F4;--c-border-i:#D0D1D2;
  --c-accent:#3E6AE1;--c-accent-soft:rgba(62,106,225,.10);--c-accent-text:#3E6AE1;
  --c-danger:#C0392B;--c-success:#1F8A3B;--c-warning:#B07500;
  --r-sm:4px;--t-dur:.33s;--t-ease:cubic-bezier(.5,0,0,.75);
  --font:"RazerF5",-apple-system,BlinkMacSystemFont,"Segoe UI",Arial,"Microsoft YaHei",
    "PingFang SC","Noto Sans CJK SC",sans-serif;
  --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace;
}
@media (prefers-color-scheme:dark){
  :root:not([data-theme="light"]){
    color-scheme:dark;
    --c-bg:#171A20;--c-surface:#1E2127;--c-surface-2:#23262D;--c-hover:rgba(255,255,255,.06);
    --c-text:#F4F4F4;--c-body:#D0D1D2;--c-muted:#8E8E93;--c-placeholder:#6B6E73;
    --c-border:rgba(255,255,255,.08);--c-border-s:rgba(255,255,255,.05);
    --c-border-i:rgba(255,255,255,.16);
    --c-accent:#5B7EF0;--c-accent-soft:rgba(91,126,240,.16);--c-accent-text:#5B7EF0;
    --c-danger:#E5655C;--c-success:#3BB55C;--c-warning:#E0A33C;
  }
}
html[data-theme="dark"]{
  color-scheme:dark;
  --c-bg:#171A20;--c-surface:#1E2127;--c-surface-2:#23262D;--c-hover:rgba(255,255,255,.06);
  --c-text:#F4F4F4;--c-body:#D0D1D2;--c-muted:#8E8E93;--c-placeholder:#6B6E73;
  --c-border:rgba(255,255,255,.08);--c-border-s:rgba(255,255,255,.05);
  --c-border-i:rgba(255,255,255,.16);
  --c-accent:#5B7EF0;--c-accent-soft:rgba(91,126,240,.16);--c-accent-text:#5B7EF0;
  --c-danger:#E5655C;--c-success:#3BB55C;--c-warning:#E0A33C;
}
html{background:var(--c-bg)}
body{background:var(--c-bg);color:var(--c-body);font-family:var(--font);font-size:14px;
  line-height:1.65;-webkit-font-smoothing:antialiased}
.page{max-width:1620px;margin:0 auto;padding:56px 40px 88px}
button{font:inherit;color:inherit;cursor:pointer;background:none;border:1px solid var(--c-border-i);
  border-radius:4px;padding:5px 12px;transition:border-color var(--t-dur) var(--t-ease),
  background var(--t-dur) var(--t-ease)}
button:hover{border-color:var(--c-text);background:var(--c-hover)}

.top{display:flex;justify-content:flex-end;margin-bottom:34px}
.eyebrow{font-family:var(--mono);font-size:11px;color:var(--c-placeholder);letter-spacing:normal}
h1{font-size:40px;font-weight:500;color:var(--c-text);line-height:1.15;margin:8px 0 0}
.lede{font-size:15px;color:var(--c-body);max-width:760px;margin-top:14px}
.meta{display:flex;flex-wrap:wrap;gap:10px;margin-top:22px}
.meta span{font-family:var(--mono);font-size:11px;color:var(--c-muted);
  border:1px solid var(--c-border);border-radius:4px;padding:4px 9px}

.stats{display:grid;grid-template-columns:repeat(4,1fr);margin-top:44px;
  border-top:1px solid var(--c-border);border-bottom:1px solid var(--c-border)}
.stat{padding:20px 0 22px;border-left:1px solid var(--c-border);padding-left:20px}
.stat:first-child{border-left:0;padding-left:0}
.stat b{display:block;font-size:26px;font-weight:500;color:var(--c-text);line-height:1.2}
.stat span{font-size:11.5px;color:var(--c-muted)}

.toc{display:flex;flex-wrap:wrap;gap:22px;margin-top:26px;font-size:12px}
.toc a{color:var(--c-muted);text-decoration:none;border-bottom:1px solid transparent;
  transition:color var(--t-dur) var(--t-ease),border-color var(--t-dur) var(--t-ease)}
.toc a:hover{color:var(--c-text);border-color:var(--c-border-i)}

section{margin-top:76px}
.sec-head{display:flex;align-items:baseline;gap:14px;border-top:1px solid var(--c-border);
  padding-top:22px}
.sec-num{font-family:var(--mono);font-size:11px;color:var(--c-placeholder)}
h2{font-size:21px;font-weight:500;color:var(--c-text)}
.sec-note{font-size:13px;color:var(--c-muted);max-width:820px;margin-top:10px}
.figure{margin-top:26px;overflow-x:auto;overflow-y:hidden;padding-bottom:4px}
.figure svg{display:block;width:100%;height:auto;min-width:1040px}

.legend{display:flex;flex-wrap:wrap;gap:20px;margin-top:18px;font-size:11.5px;
  color:var(--c-muted)}
.legend li{display:flex;align-items:center;gap:7px;list-style:none}
.sw{width:16px;height:12px;border:1px solid var(--c-border-i);border-radius:2px;
  background:var(--c-surface)}
.sw--core{border-left:2.5px solid var(--c-accent)}
.sw--ext{border-style:dashed}
.sw--accent{border:0;height:1.5px;background:var(--c-accent)}

.disk{display:grid;grid-template-columns:1fr 1fr;gap:28px;margin-top:26px}
.disk h3{font-size:12.5px;font-weight:500;color:var(--c-text);margin-bottom:4px}
.disk p{font-size:11.5px;color:var(--c-muted);margin-bottom:12px}
pre.tree{font-family:var(--mono);font-size:11.5px;line-height:1.85;color:var(--c-body);
  border:1px solid var(--c-border);border-radius:4px;padding:16px 18px;
  background:var(--c-surface);overflow-x:auto;white-space:pre;tab-size:2}
pre.tree b{font-weight:400;color:var(--c-text)}
pre.tree .cm{color:var(--c-placeholder)}
pre.tree .ac{color:var(--c-accent-text)}

.rules{display:grid;grid-template-columns:1fr 1fr;gap:22px 44px;margin-top:26px}
.rule{border-top:1px solid var(--c-border);padding-top:12px}
.rule h3{font-size:12.5px;font-weight:500;color:var(--c-text)}
.rule p{font-size:12px;color:var(--c-muted);margin-top:3px}
.rule code{font-family:var(--mono);font-size:11.5px;color:var(--c-body)}

footer{margin-top:84px;border-top:1px solid var(--c-border);padding-top:18px;
  font-family:var(--mono);font-size:11px;color:var(--c-placeholder);
  display:flex;flex-wrap:wrap;justify-content:space-between;gap:12px}

@media (max-width:1080px){
  .page{padding:40px 22px 64px}
  .stats{grid-template-columns:1fr 1fr}
  .stat:nth-child(3){border-left:0;padding-left:0}
  .disk,.rules{grid-template-columns:1fr}
}
@media print{
  .top{display:none}
  .figure{overflow:visible}
  section{break-inside:avoid}
}
"""

JS = """
(function(){
  var el=document.documentElement, btn=document.getElementById('themeToggle');
  var modes=['auto','light','dark'], label={auto:'跟随系统',light:'浅色',dark:'深色'};
  function apply(m){
    if(m==='auto'){ el.removeAttribute('data-theme'); } else { el.dataset.theme=m; }
    btn.textContent=label[m];
    btn.setAttribute('aria-label','主题：'+label[m]);
  }
  var cur='auto';
  btn.addEventListener('click',function(){
    cur=modes[(modes.indexOf(cur)+1)%modes.length];
    apply(cur);
  });
  apply('auto');
})();
"""


def build_html(d1, d2, d3):
    return f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Rabbit Chat Studio · 架构图</title>
<style>{CSS}</style>
</head>
<body>
<div class="page">

  <div class="top"><button id="themeToggle" type="button" aria-label="主题">跟随系统</button></div>

  <header>
    <p class="eyebrow">architecture map · v3.2.0</p>
    <h1>Rabbit Chat Studio</h1>
    <p class="lede">Tauri 2 桌面客户端：React 19 前端 + Rust 后端，把任意 OpenAI 兼容 API 变成带技能、
      工具与子智能体的工作台。下图自上而下是一条调用链——界面只通过 invoke 与事件和 Rust 通信，
      所有「能做事」的能力都在后端，虚线框是项目之外的依赖。</p>
    <div class="meta">
      <span>Tauri 2 · React 19 · Rust</span>
      <span>Rust 78 文件 / 6.22 万行</span>
      <span>前端 54 文件 / 1.44 万行（.ts/.tsx）</span>
      <span>SQLite · 单文件</span>
      <span>MIT</span>
    </div>
  </header>

  <div class="stats">
    <div class="stat"><b>90</b><span>Tauri 命令（invoke 入口）</span></div>
    <div class="stat"><b>45</b><span>事件通道（emit → listen）</span></div>
    <div class="stat"><b>8</b><span>SQLite 表</span></div>
    <div class="stat"><b>3</b><span>子智能体后端：llm / cli / cloud</span></div>
  </div>

  <nav class="toc">
    <a href="#map">01 分层架构</a>
    <a href="#turn">02 一次对话轮</a>
    <a href="#agents">03 子智能体编排</a>
    <a href="#disk">04 磁盘与数据库</a>
    <a href="#rules">05 关键约束</a>
  </nav>

  <section id="map">
    <div class="sec-head"><span class="sec-num">01</span><h2>分层架构</h2></div>
    <p class="sec-note">九个层次，每一层只依赖它下面的那一层。想读懂这个项目，先看三个文件：
      <code style="font-family:var(--mono);font-size:12px">lib.rs</code>（状态与命令）、
      <code style="font-family:var(--mono);font-size:12px">llm_complete.rs</code>（对话主循环）、
      <code style="font-family:var(--mono);font-size:12px">agents.rs</code>（派活与汇总）。</p>
    <div class="figure">
{d1}
    </div>
    <ul class="legend">
      <li><span class="sw sw--core"></span>核心入口（先读这几个）</li>
      <li><span class="sw"></span>项目内模块</li>
      <li><span class="sw sw--ext"></span>外部依赖</li>
      <li><span class="sw sw--accent"></span>主链路 / 循环</li>
    </ul>
  </section>

  <section id="turn">
    <div class="sec-head"><span class="sec-num">02</span><h2>一次对话轮</h2></div>
    <p class="sec-note">一轮 = 多次上游请求。工具结果必须紧邻它的
      <code style="font-family:var(--mono);font-size:12px">assistant(tool_calls)</code>，
      所以 mid-turn 插话只在「轮首」这个唯一安全边界注入，上下文压缩也只在这里做。</p>
    <div class="figure">
{d2}
    </div>
  </section>

  <section id="agents">
    <div class="sec-head"><span class="sec-num">03</span><h2>子智能体编排</h2></div>
    <p class="sec-note">规划器把一条请求拆成若干 Task，交给三类后端并行执行；交付物走
      <code style="font-family:var(--mono);font-size:12px">.data/tmp</code>
      而不是塞进答案，监督者只在旁边观察、不碰文件。</p>
    <div class="figure">
{d3}
    </div>
  </section>

  <section id="disk">
    <div class="sec-head"><span class="sec-num">04</span><h2>磁盘与数据库</h2></div>
    <p class="sec-note">应用数据与工作区状态分开存放：工作区可以切换，
      <code style="font-family:var(--mono);font-size:12px">.data</code>
      跟着工作区走，于是同一份配置能开多个互不干扰的项目。</p>
    <div class="disk">
      <div>
        <h3>&lt;app data&gt;/</h3>
        <p>应用自己的文件，与工作区无关</p>
        <pre class="tree"><b>config.json</b>          <span class="cm"># 30 个配置字段：模型 / 主题 / 工作区 / 守卫阈值</span>
<b>mcp_servers.json</b>    <span class="cm"># 用户面板维护的 MCP 花名册</span>
<b>sub_agents.json</b>     <span class="cm"># 子智能体清单</span>
<b>profiles.json</b>       <span class="cm"># 档案：配置 + 技能启用状态</span>
<b>agents/</b>             <span class="cm"># llm 型 agent 的 markdown 定义 + index.yaml</span>
<b>shadow/</b>             <span class="cm"># 每个窗口一份 config fork</span>
<b>workspace/</b>          <span class="cm"># 默认工作区</span>
<b>app.log</b>             <span class="cm"># 日志，按 log_retention_days 清理</span></pre>
      </div>
      <div>
        <h3>&lt;workspace&gt;/.data/</h3>
        <p>随工作区走的隐藏状态，不自动创建的一律按需生成</p>
        <pre class="tree"><b>chat.db</b>              <span class="cm"># history · session_meta · session_summaries</span>
                       <span class="cm"># api_requests · interaction_log · agent_missions</span>
                       <span class="cm"># agent_tasks · agent_bus_messages</span>
<b>memory/</b>             <span class="cm"># user/ · repo/ · evolution/（session 在内存）</span>
<b>todos/</b>              <span class="cm"># 任务清单</span>
<b>skills/</b>             <span class="cm"># 技能唯一根，每个技能一个 skill.md</span>
<b>skills.status</b>       <span class="cm"># 启用状态的唯一真源</span>
<b>scripts/</b>            <span class="cm"># run_cmd 脚本库（审批 + 预览）</span>
<b>mcp/ + mcp.conf</b>     <span class="cm"># 自建 MCP 服务器</span>
<b>tmp/&lt;batch&gt;/&lt;task&gt;/</b>  <span class="cm"># 子智能体交付物，永久保留</span>
<b>timers.json</b>         <span class="cm"># 未到点的 delay timer</span></pre>
      </div>
    </div>
  </section>

  <section id="rules">
    <div class="sec-head"><span class="sec-num">05</span><h2>关键约束</h2></div>
    <p class="sec-note">这些是读代码时最容易踩空的地方，也是这个架构真正的形状。</p>
    <div class="rules">
      <div class="rule">
        <h3>三个执行工具，三种边界</h3>
        <p><code>file_actions</code> 只能在工区内（<code>chat.db</code> 例外）；
           <code>run_cmd</code> 只能跑一个 PATH 里的程序；<code>run_shell</code> 能到任意位置，
           但先过语法闸门。</p>
      </div>
      <div class="rule">
        <h3>风险七级，处置不同</h3>
        <p>L0/L1 人工复核（≥90 强制拒绝）· L2/L3 警告 + 确认 · L4 放行 + 审计 ·
           L5/L6 直接放行。确认可被 <code>auto_accept_confirm_kinds</code> 按类别放开。</p>
      </div>
      <div class="rule">
        <h3>一轮 = 多次请求</h3>
        <p>带 <code>tool_calls</code> 的 assistant 消息必须紧邻它的 tool 结果，所以注入只发生在
           轮首；窗口已关的插话由前端丢掉并作为下一轮重发。</p>
      </div>
      <div class="rule">
        <h3>上下文四层压缩</h3>
        <p>结构化摘要（用量 0.8）· 工具台账（每轮 name(args)→结果）· 摘要接管
           <code>session_summaries</code> · 预判 90% + 反应式重试；压缩失败自动退回机械摘要，
           永远不会因此中断一次对话。</p>
      </div>
      <div class="rule">
        <h3>技能只有一个根</h3>
        <p><code>skill.md</code> 所在目录即根，<code>..</code> 一律拒绝。技能文件夹消失后
           记录仍留在 <code>skills.status</code>，只是不再进模型。</p>
      </div>
      <div class="rule">
        <h3>交付物不清理</h3>
        <p><code>.data/tmp</code> 永久保留，且刻意不进 profile 备份——它是交换区，不是状态。</p>
      </div>
      <div class="rule">
        <h3>单实例 + shadow 窗口</h3>
        <p>一个进程一个实例（<code>instance.rs</code>），每个窗口在
           <code>shadow/</code> 下拿一份配置 fork，互不覆盖。</p>
      </div>
      <div class="rule">
        <h3>自进化的护栏</h3>
        <p>维护轮可改 skills 与 memory，但 <code>chat.db</code> 与 <code>skill.md</code>
           受保护：<code>file_actions</code> 只能读、只能写辅助文件，
           <code>delete/rename/move</code> 被拒绝。</p>
      </div>
    </div>
  </section>

  <footer>
    <span>依据 v3.2.0 源码实测：92 命令 · 48 事件 · 8 表 · 78 个 Rust 文件</span>
    <span>颜色取自 src/App.css 设计令牌</span>
  </footer>

</div>
<script>{JS}</script>
</body>
</html>
"""


def main():
    d1 = diagram_layers()
    d2 = diagram_turn()
    d3 = diagram_agents()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(build_html(d1, d2, d3), encoding="utf-8")
    print("wrote", OUT, OUT.stat().st_size, "bytes")


if __name__ == "__main__":
    main()
