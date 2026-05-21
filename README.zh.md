# AI Chat

[English](./README.md)

一个基于 Tauri 2 + React + TypeScript + Rust 的桌面 AI 对话客户端，面向 OpenAI 兼容接口，支持流式回复、技能系统、工具调用、MCP 服务接入和会话持久化。

> 目标：把“通用大模型能力”与“本地可控执行能力”融合到同一个安全、轻量、跨平台桌面应用中。

---



## 目录

- [项目介绍](#项目介绍)
- [核心功能](#核心功能)
- [第一性设计原理](#第一性设计原理)
- [技术栈与架构](#技术栈与架构)
- [快速开始](#快速开始)
- [编译与安装](#编译与安装)
- [配置说明](#配置说明)
- [项目结构](#项目结构)
- [安全与边界](#安全与边界)
- [开发路线建议](#开发路线建议)

---

## 项目介绍

AI Chat 是一个桌面端 AI 助手应用，支持接入任意 OpenAI 兼容模型服务。它不仅能聊天，还能通过技能（Skills）和工具（Tools）扩展能力，并且在本地保存配置、历史会话与请求日志，适合个人效率场景与工程化实验场景。

适用场景：

- 多模型切换与提示词实验
- 带上下文的持续对话与历史回溯
- 结合本地命令、文件、网页抓取的半自动任务执行
- MCP 服务接入与工具生态扩展

---

## 核心功能

### 1) 对话能力

- 支持 OpenAI 兼容 Chat Completions
- 支持流式返回与 usage 事件
- 支持推理内容流（reasoning token）展示
- 支持中断生成
- 支持消息文件附件展示
- 支持模型切换与模型参数配置（temperature、top_p、max_tokens 等）

### 2) 子代理编排（Sub-Agents）

- 支持子代理新增、编辑、启用与删除
- 支持编排配置与子代理模式开关
- 对话中支持任务级编排事件

### 3) 技能系统（Skills）

- 基于 skill.md（YAML frontmatter + system prompt）定义技能
- 支持技能的新增、删除、启用/停用
- 支持从应用目录和用户目录加载技能
- 技能目录隔离，约束文件与命令作用范围
- 多技能下命令白名单可合并

### 4) 工具系统（Tools）

- run_cmd / run_shell：本地命令执行，内置危险命令确认
- file_actions：文件读写、编辑、补丁，支持 mkdir / rename / move / delete
- knowledge_graph：知识图谱查询（支持 Neo4j）

### 5) MCP 管理

- 支持 MCP Server 的增删改查
- 支持 stdio / sse 两种传输模式，并改进相对路径处理
- 支持连通性测试

### 6) 本地持久化与运维

- config.json 保存模型与工具配置
- SQLite 保存聊天历史与 API 请求监控数据
- 请求监控面板支持列表、详情与删除
- 支持配置快照的保存与恢复（Profile）
- 支持在设置中管理工作目录

---

## 第一性设计原理

从第一性出发，本项目将桌面 AI 助手拆解为四个不可再简化的基本问题：

1. 如何稳定获得模型输出
2. 如何让模型对本地世界“有手有脚”
3. 如何把风险边界明确化
4. 如何保证可复现与可调试

对应设计决策如下：

- 最小依赖闭环：
  - 模型调用统一走 OpenAI 兼容接口，避免供应商锁定。
- 能力与权限解耦：
  - 对话是默认能力，工具是可选能力；技能可进一步限制工具。
- 本地优先可观察：
  - 配置、历史、请求日志落地本地，方便审计与复现。
- 安全边界显式化：
  - 技能目录隔离、工具白名单、工作目录约束，降低越权风险。
- 架构可替换：
  - 前端 UI、后端命令、模型服务、知识图谱后端均可替换，不耦合单点实现。

你可以把它理解为：

- 大模型负责“认知与规划”
- 本地工具负责“执行与验证”
- 应用框架负责“边界与记录”

---

## 技术栈与架构

### 前端

- React 19 + TypeScript + Vite
- Tauri JS API（事件监听与命令调用）

### 后端

- Rust + Tauri 2 命令系统
- reqwest（HTTP / 流式）
- rusqlite（本地数据库）
- serde / serde_json / serde_yaml（序列化）
- Neo4j 客户端（知识图谱）

### 通信模型

- 前端 invoke 调用 Rust 命令
- Rust 通过事件向前端推送 chat-token / chat-done / chat-error / chat-usage

---

## 快速开始

### 1. 环境要求

- Node.js 18+
- Rust stable（建议 rustup 管理）
- Tauri CLI 2.x
- Windows 建议使用 MSVC 工具链

### 2. 安装依赖

```bash
npm install
cargo install tauri-cli --version "^2"
```

### 3. 启动开发模式

```bash
npm run tauri dev
```

---

## 编译与安装

### A. 本地开发编译

```bash
# 前端构建
npm run build

# Rust 检查
cargo check --manifest-path src-tauri/Cargo.toml
```

### B. 生产构建

```bash
npm run tauri build
```

构建产物位于：

- src-tauri/target/release/bundle

### C. 安装方式

- Windows：运行 bundle 目录中的安装包（如 NSIS / MSI）
- macOS：使用 .app / .dmg（按构建目标）
- Linux：使用 .deb / .AppImage / .rpm（按构建目标）

> Windows 上如果遇到 GNU linker 与 WebView2 相关问题，建议切换到 MSVC 工具链构建。

---

## 配置说明

运行后会在系统应用数据目录生成配置文件 config.json，常见字段：

```json
{
  "api_base_url": "https://api.openai.com/v1",
  "api_key": "sk-xxx",
  "model": "gpt-4o-mini",
  "model_catalog": ["gpt-4o-mini", "gpt-4.1-mini"],
  "model_settings": {
    "temperature": 0.7,
    "top_p": 0.95,
    "reasoning_effort": "medium",
    "max_tokens": 4096
  },
  "selected_tools": ["web_search", "file_actions", "knowledge_graph"],
  "search_engine": "duckduckgo",
  "kg_engine": "neo4j",
  "neo4j_uri": "bolt://localhost:7687",
  "neo4j_user": "neo4j",
  "neo4j_password": "your_password"
}
```

安全建议：

- 不要提交真实 API Key
- 生产环境使用独立密钥与最小权限账号
- 将知识图谱账号与应用账号分离

---

## 项目结构

```text
ai-chat/
├─ src/                 # React 前端
├─ src-tauri/           # Rust + Tauri 后端
│  ├─ src/lib.rs        # 应用入口与命令注册
│  ├─ src/llm_complete.rs
│  ├─ src/skills.rs
│  ├─ src/tools.rs
│  ├─ src/mcp.rs
│  └─ src/db.rs
├─ skills-example/      # 技能示例
└─ README.md
```

---

## 安全与边界

- 工具执行具备系统能力，应谨慎开启 execute_command 与 file_actions
- 技能执行遵循技能目录隔离原则，避免越界访问
- MCP 外部服务接入前应先做最小权限与连通性验证
- 建议在开发与生产中使用不同配置文件与 API 凭据

---

## 开发路线建议

- 增加模型提供方预设（OpenAI / Azure / 本地网关）
- 增加工具调用审计与回放
- 增加技能签名与发布机制
- 增加多工作区会话隔离与同步
- 增加端到端测试与安全基线扫描

---

如果你希望，我可以继续为这个 README 增加：

1. 中英文双语版本
2. 一键安装脚本（Windows/macOS/Linux）
3. GitHub Actions 自动构建发布配置
