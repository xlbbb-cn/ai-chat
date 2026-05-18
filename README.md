# AI Chat

[中文](./README.zh.md)

A desktop AI chat client built with Tauri 2 + React + TypeScript + Rust, compatible with OpenAI APIs. It supports streaming responses, skill systems, tool invocation, MCP server integration, and session persistence.

> Goal: Combine "general AI model capabilities" with "local controllable execution" into a secure, lightweight, cross-platform desktop application.




---

## Table of Contents

- [Introduction](#introduction)
- [Core Features](#core-features)
- [First Principles Design](#first-principles-design)
- [Tech Stack & Architecture](#tech-stack--architecture)
- [Quick Start](#quick-start)
- [Build & Installation](#build--installation)
- [Configuration](#configuration)
- [Project Structure](#project-structure)
- [Security & Boundaries](#security--boundaries)
- [Development Roadmap](#development-roadmap)

---

## Introduction

AI Chat is a desktop AI assistant application that connects to any OpenAI-compatible model service. It not only supports chat but also extends capabilities through Skills and Tools. It saves configurations, session histories, and request logs locally, making it suitable for personal productivity and experimental engineering scenarios.

Use cases:

- Multi-model switching and prompt engineering
- Contextual conversations with history tracking
- Semi-automated task execution combining local commands, files, and web scraping
- MCP server integration and tool ecosystem expansion

---

## Core Features

### 1) Chat Capabilities

- Supports OpenAI-compatible Chat Completions
- Streaming responses (token-level incremental display)
- Displays reasoning content streams (reasoning tokens)
- Interruptible generation
- Model switching and parameter configuration (temperature, top_p, max_tokens, etc.)

### 2) Skill System

- Define skills via skill.md (YAML frontmatter + system prompt)
- Add, delete, enable/disable skills
- Load skills from both app and user directories
- Skills can restrict accessible tools for controlled execution boundaries

### 3) Tool System

- web_search: Internet search
- fetch_web: Web scraping
- execute_command: Local command execution (bash/cmd/powershell/python)
- file_actions: File read/write/edit/patch
- knowledge_graph: Knowledge graph queries (supports Neo4j)

### 4) MCP Management

- Add, edit, delete MCP servers
- Supports stdio / SSE transport modes
- Connectivity testing

### 5) Local Persistence

- config.json for model and tool configurations
- SQLite for chat history and API request monitoring data
- Request monitoring panel for viewing API call summaries, errors, and durations

---

## First Principles Design

From first principles, this project breaks down the desktop AI assistant into four irreducible problems:

1. How to reliably obtain model outputs
2. How to enable the model to "act" on the local world
3. How to clearly define risk boundaries
4. How to ensure reproducibility and debuggability

Design decisions:

- Minimal dependency closure:
  - Model calls are unified through OpenAI-compatible APIs to avoid vendor lock-in.
- Decoupling capabilities and permissions:
  - Chat is the default capability; tools are optional; skills can further restrict tools.
- Local-first observability:
  - Configurations, histories, and request logs are stored locally for auditing and reproducibility.
- Explicit security boundaries:
  - Skill directory isolation, tool whitelisting, and workspace constraints reduce overreach risks.
- Replaceable architecture:
  - Frontend UI, backend commands, model services, and knowledge graph backends are modular and interchangeable.

Think of it as:

- Large models handle "cognition and planning"
- Local tools handle "execution and verification"
- The application framework handles "boundaries and records"

---

## Tech Stack & Architecture

### Frontend

- React 19 + TypeScript + Vite
- Tauri JS API (event listeners and command invocations)

### Backend

- Rust + Tauri 2 command system
- reqwest (HTTP / streaming)
- rusqlite (local database)
- serde / serde_json / serde_yaml (serialization)
- Neo4j client (knowledge graph)

### Communication Model

- Frontend invokes Rust commands via `invoke`
- Rust pushes events to the frontend (chat-token / chat-done / chat-error / chat-usage)

---

## Quick Start

### 1. Prerequisites

- Node.js 18+
- Rust stable (recommended: rustup)
- Tauri CLI 2.x
- Windows: MSVC toolchain recommended

### 2. Install Dependencies

```bash
npm install
cargo install tauri-cli --version "^2"
```

### 3. Start Development Mode

```bash
npm run tauri dev
```

---

## Build & Installation

### A. Local Development Build

```bash
# Frontend build
npm run build

# Rust check
cargo check --manifest-path src-tauri/Cargo.toml
```

### B. Production Build

```bash
npm run tauri build
```

Build artifacts are located in:

- src-tauri/target/release/bundle

### C. Installation

- Windows: Run the installer in the bundle directory (e.g., NSIS / MSI)
- macOS: Use .app / .dmg (depending on the build target)
- Linux: Use .deb / .AppImage / .rpm (depending on the build target)

> On Windows, if you encounter GNU linker or WebView2 issues, switch to the MSVC toolchain.

---

## Configuration

After running, a config.json file will be generated in the system app data directory. Common fields:

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

Security recommendations:

- Do not commit real API keys
- Use separate keys and minimal privilege accounts for production
- Separate knowledge graph accounts from application accounts

---

## Project Structure

```text
ai-chat/
├─ src/                 # React frontend
├─ src-tauri/           # Rust + Tauri backend
│  ├─ src/lib.rs        # App entry and command registration
│  ├─ src/llm_complete.rs
│  ├─ src/skills.rs
│  ├─ src/tools.rs
│  ├─ src/mcp.rs
│  └─ src/db.rs
├─ skills-example/      # Skill examples
└─ README.md
```

---

## Security & Boundaries

- Tool execution has system-level capabilities; use execute_command and file_actions cautiously
- Skill execution follows directory isolation rules to prevent boundary overreach
- Validate MCP external services with minimal privileges and connectivity tests before integration
- Use separate configuration files and API credentials for development and production

---

## Development Roadmap

- Add presets for model providers (OpenAI / Azure / local gateways)
- Add tool invocation auditing and replay
- Add skill signing and publishing mechanisms
- Add multi-workspace session isolation and synchronization
- Add end-to-end testing and security baseline scans

---

If desired, I can further enhance this README with:

1. Bilingual (Chinese-English) versions
2. One-click installation scripts (Windows/macOS/Linux)
3. GitHub Actions for automated build and release