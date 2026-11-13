# Changelog

本文件由 `scripts/gen-changelog.cjs` 从两个 tag 之间的提交信息生成，请勿手工编辑：

```bash
npm run changelog -- --from <上一个 tag> --to <本次 tag> --details --write
```

条目按 Conventional Commits 的类型分组，作用域加粗；工作区里提交标题是英文、正文是中文，
所以带 `--details` 时是中英对照。

## [v3.3.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.3.0) — 2026-11-13

### 新增

- **site**: hero web create ([`ba26871`](https://github.com/xlbbb-cn/ai-chat/commit/ba2687183591658aa2017f9dc20b60c988afcdc2))
- **ui**: unify sidebar list rows and fold orchestration settings ([`d3ce53c`](https://github.com/xlbbb-cn/ai-chat/commit/d3ce53c0aadf9a655245072404f46084227bfba4))
  - Give History, Skills, Tools, MCP and Agents one flat row language: transparent rows, inset hairlines and a --c-hover hover/focus fill.
  - Share the 36x20 toggle switch, neutralise agent type/tool badges and flatten the Tools auto-accept footer.
  - Add a leading History search glyph and clear action, one segmented tab control, and relative timestamps with a full-date tooltip.
  - Fold the orchestration rows from smart selection through max concurrency behind a shared .section-toggle; Enable mode stays visible.
  - Document the side-panel list rows and disclosure primitive in DESIGN.md.
- **models**: store the catalogue flat and match normalised names exactly ([`89789ed`](https://github.com/xlbbb-cn/ai-chat/commit/89789ed835e9aa31ba0ef34fa2fb8f5229c010f8))
  - model_metadata.rs: the merge writes the normalisation key back into `model_name`, so every stored entry is lowercase and flattened (`gpt-4o`, `glm-5-3`) and the file holds one entry per normalised name; the three parsers stay shape-specific (models.dev's nested providers, OpenRouter's `data[]` with `architecture` / `top_provider`, llm-metadata's `{data:[...]}` with comma-separated `tags`)
  - the catalogue format is bumped to 2: an older file still parses and serves as the offline fallback, but `catalog_is_fresh` (and the memory cache) treat a version mismatch as expired, so a new build re-downloads instead of matching against pre-normalisation names for the rest of the TTL
  - modelMetadata.ts: `matchModelCatalog` indexes entries by normalised name and returns the direct hit with `match_score: 1`; only unknown ids reach the existing similarity scorer (`fallbackMatch`, MIN_MATCH_SCORE unchanged), so `deepseek-chat-v3.1` still finds `deepseek-chat`
  - SettingsPanel: the Fetch Models button shows a Font Awesome spinner plus `aria-busy` while the model list and the catalogue build load (the first build downloads ~7 MB), and `.settings-btn` gains a 6px gap for the icon
  - tests: the merged catalogue must equal its own normalisation with unique keys, llm-metadata is read wrapped or bare, OpenRouter falls back to `top_provider.context_length`, and a version-1 file never counts as fresh; the parser tests now assert the stored flattened names
  - Verified: `cargo test model_metadata::` 19 passed, the ignored live merge passes against the public feeds, `tsc --noEmit` clean, `vite build` succeeds.
- **models**: merge three capability catalogues into one weekly file ([`964cf20`](https://github.com/xlbbb-cn/ai-chat/commit/964cf20d57ec1434ba7d30766a7cbf3593bebde7))
  - `src-tauri/src/model_metadata.rs`: three parsers behind `build_catalog`, a `<app_data>/model_metadata.json` snapshot (`version`, `updated_at`, `entries`) with a 7-day TTL, an atomic `.json.tmp` + rename write and a `tokio::sync::Mutex` around the refresh so two Settings panels download once. Resolution is memory -> file -> network; a single failing feed logs a WARN and the merge continues, and when every feed fails the stale file is still served, so the model list never depends on this feature. `force` bypasses both caches.
  - Real data: 2930 entries (was ~370), 2806 with a context window and 2682 with a max-output limit, 1.2 MB on disk; llm-metadata keeps 62 entries the others miss.
  - OpenRouter writes `"hugging_face_id": ""` for closed models, so the parser must require a non-empty id -- `gpt-4o` was being flagged as open weights.
  - `src/types.ts`, `SettingsPanel.tsx`: `max_output_tokens` and `sources` are surfaced as an output-limit badge plus the source list in the match hint; en / zh-CN / zh-TW catalogues and the doc comments of `api.ts`, `modelMetadata.ts`, `ModelSelect.tsx` and `App.tsx` follow the merged feed.
  - AGENTS.md, config.example.md, the store privacy policy (three hosts instead of one), `docs/gen.py` + the generated architecture page and the wiki pages no longer describe the feature as a single llm-metadata download.
  - Verified: `cargo test` 406 passed / 2 ignored (live-network), the ignored `live_merge_produces_a_usable_catalogue` passes against the public feeds, `tsc --noEmit` clean, `vite build` succeeds.
- **agents**: let the planner run on its own model ([`6be2a48`](https://github.com/xlbbb-cn/ai-chat/commit/6be2a48f9ce84a95f01cebb6b54d23cea6ce6c23))
  - agents/config.rs: `AgentOrchestration.planner_model` (missing or blank = inherit) is normalized in `normalize_config`, so load and save agree, and `planner_model_or` is the single resolver for every planner call
  - agents/planning.rs: orchestrate() resolves the model once and hands the same one to select_agents, plan_tasks and the supervisor
  - AgentsPanel: a picker after "Smart agent selection" offers "Follow the main model" plus the composer's catalogue, and keeps a configured id that has since left the catalogue
  - AGENTS.md: document the field next to Agent orchestration
- **i18n**: adopt the OS UI language on first launch ([`47b3d53`](https://github.com/xlbbb-cn/ai-chat/commit/47b3d5341ed9674b32fee71ea8ce119f4a098f97))
  - src-tauri/src/system_locale.rs: detect() reads the OS user-interface language (Windows GetUserDefaultUILanguage -> LCIDToLocaleName, macOS `defaults read -g AppleLocale`, Linux LC_ALL / LC_MESSAGES / LANG) and maps it with the same rule as the frontend's detectSystemLocale() (Hant/TW/HK/MO -> zh-TW, any other Chinese tag -> zh-CN, else -> en)
  - lib.rs setup: when config.language is missing or not one of the three, claim the detected locale and persist it in the single startup write that already seeds workspace_history; a pick in Settings or an imported backup is never overwritten, and a shadow window writes only its own copy
  - detectSystemLocale() stays the fallback for the very first paint
  - AGENTS.md: document the rule next to Theming
- **site**: add web site ([`c3e42df`](https://github.com/xlbbb-cn/ai-chat/commit/c3e42df315bbd02b9e0ad5995cfa8dc377f26085))
- **settings**: fetch models before saving; workspace switcher polish ([`03d9bc4`](https://github.com/xlbbb-cn/ai-chat/commit/03d9bc4d7ce3bcf0a8f254541105cd6bf0a3be20))
  - `fetch_models` takes the endpoint the settings form currently holds (base url, api key, custom headers) instead of reading the saved config, so "Fetch Models From API" works before "Save Changes". Nothing is written back, and an empty base url fails with a clear message.
  - Titlebar workspace list: the folder name is never truncated (it is the identity of the row), the parent path is the only part allowed to ellipsize, and the popup ceiling grew to 480px so a whole name fits.
  - Switching workspace closes the sidebar first (`onBeforeWorkspaceSwitch`), so no panel repaints against the workspace being replaced.
- **ui**: switch workspaces from the titlebar, one dropdown, no named profiles ([`d7fd4fe`](https://github.com/xlbbb-cn/ai-chat/commit/d7fd4fe3d600973cf6aa34c101cfcbb531238038))
  - The workspace indicator is the picker now: clicking it lists the recent workspaces (`workspace_history_entries`, current/missing marked) and applies one immediately through `set_workspace_dir`, which writes that single field onto the live config and is refused while a reply is streaming. The menu also offers "choose another folder" and notes that a shadow window keeps its own settings copy while sharing the history.
  - `src/components/Dropdown.tsx` is the one listbox primitive (portal, viewport clamp, keyboard contract); ModelSelect, the agents panel and the MCP inspector use it instead of their bespoke selects, and the duplicated CSS is gone.
  - The workspace row is removed: the titlebar owns `workspace_dir`, and every save path (`handleSave`, `handleLanguageChange`, `applyMessageEditor`) takes it from a fresh `getConfig()`, so a switch made while the panel is open can no longer be rolled back by "Save Changes". Self-evolution mode moved to Runtime.
  - Named profiles deleted end to end: `list_profiles` / `save_profile_config` / `delete_profile_config` / `apply_profile_config`, `ProfilesFile`, `AppState::profiles_path`, `AppState::profiles.json`, the unused `materialize_agent`, the Settings section, `ProfilePanel`, the `Profile` type and the i18n keys.
  - The backup archive no longer packs or restores `profiles.json`, and the export/import wording, menu ids and default file name say "backup".

### 修复

- **site**: generate the architecture page in the Pages deploy ([`9287bac`](https://github.com/xlbbb-cn/ai-chat/commit/9287bac2e855066151b4669cb337dfd61892a1e4))
- **i18n**: the app menu entry opens the tmp folder, not deliverables ([`720a778`](https://github.com/xlbbb-cn/ai-chat/commit/720a7783839bb22c9c52dd7f67d04a47bffd0c43))
- **ui**: keep the orchestration picker off the row label ([`e1adb75`](https://github.com/xlbbb-cn/ai-chat/commit/e1adb7563a9bf9933b9225f529095113877316e1))

### 性能

- **chat**: stop re-rendering the whole transcript on every streamed token ([`b799225`](https://github.com/xlbbb-cn/ai-chat/commit/b79922541394589bc9f5589c1138c3af7c747aab))
  - 根因：流式 token 通过 `prev.map` 只替换增长的那一行（未变行对象身份不变），但 `ChatMessage` 没做 memo，且 `handleDeleteMessage` / `handleForkMessage` 的依赖数组里有 `messages`——每来一个 token 就换一次身份，于是整段历史每个 token 重建一次
  - 新增 `messagesDataRef`（`useLayoutEffect` 同步，handler 永远读不到比已提交渲染更旧的 transcript），两个 handler 改读它并从依赖数组里去掉 `messages`：删除/分叉的语义与原先的闭包取值一致，只是不再让回调身份跟着 token 变
  - `onRetry` 只传给真正会显示重试按钮的那一行（其余为 `undefined`），它的依赖里仍有 `messages`
  - `ChatMessage` 改成 `memo(ChatMessageRow)`，另两个 memo 化 props（`backgroundTasks`/`backgroundWaiting`）只在最新一行传入，`todoMessageId` 是字符串、`skillNames`/`agentTokens` 的依赖与流式无关
  - 逐项核对：delete [t]、fork [sessionId, resetEmbeddedAgents, t]、confirm [updateActiveAssistantToolCalls([]), t]、cancelBackground [input, streaming, sessionId, backupExporting]——流式期间都不变
  - 验证：`tsc --noEmit` 无错、`vite build` 通过；本机没有 jsdom / testing-library，故未做渲染次数量化，Memo 的安全前提（props 身份稳定）由上面的依赖数组审计保证
- **db**: index the two log tables ([`e05c0e3`](https://github.com/xlbbb-cn/ai-chat/commit/e05c0e3ee0f2f5eb223a7f012ba91d61d854a42f))
  - `api_requests` 与 `interaction_log` 只有表没有索引，而它们被三类查询反复扫：Monitor 列表按 session_id 取并按 id 倒序、会话报告按 session_id 取、保留策略按 timestamp 删。加 4 个幂等索引（每表 `(session_id, id)` 与 `timestamp`），`CREATE INDEX IF NOT EXISTS` 对老库同样安全
  - 20 万行 interaction_log 实测：Monitor 列表 32 ms → 1 ms（SCAN → SEARCH USING COVERING INDEX）；保留策略 DELETE 89 ms → 0.3 ms，而这条在每次启动都会跑（lib.rs prune_old_logs）；api_requests 按会话查询 5 ms → <0.5 ms
  - cargo test 389 passed / 1 ignored

### 重构

- **site**: move the landing page into docs/site/ ([`def58d7`](https://github.com/xlbbb-cn/ai-chat/commit/def58d7e190db33212d22a2d7341fac1fdd6669e))
  - site/* -> docs/site/* (everything inside the page uses relative paths, so no markup changed; git records the moves as renames)
  - pages.yml publishes docs/site and still copies docs/architecture.html in as docs/site/architecture.html; triggers now watch docs/site/** and docs/architecture.html
  - .gitignore: drop the local preview copy of architecture.html
  - AGENTS.md, READMEs: new paths, and the note that only docs/site/ is published
  - docs/store-publishing.md -> docs/store/store-publishing.md, with the Pages section corrected: the site is deployed by Actions from docs/site/, so the privacy policy needs its own public URL
- split the three god files into topic modules ([`6079d0a`](https://github.com/xlbbb-cn/ai-chat/commit/6079d0acfde547ee9dae3d4f96cec496edf8a7b0))
  - `agents.rs` 6613 → 656 行 + `agents/{config,mission,planning,execution,helper,tests}.rs`。`#[tauri::command]` 全部留在模块根：属性会同时生成 `__cmd__*` / `__tauri_command_name_*`，`generate_handler!` 按 `agents::<name>` 取名，搬走就连不上；公开类型保持 `pub`，只有原本私有的项放宽到 `pub(crate)`，crate 外可见性没变
  - `llm_complete.rs` 5677 → 342 行 + `llm_complete/{stream,background,context,conclusion,turn,tests}.rs`。`chat_completion` 同理留在根模块
  - `tools/mod.rs` 3499 → 74 行 + `execute.rs`（run_cmd 与分发）/ `schema.rs`（工具 JSON 目录）/ `tests.rs`；`split_command_line`、`machine_label` 等被兄弟模块引用的辅助函数从 `mod.rs` 重新导出，`super::name` 路径不变
  - `instance.rs` 的测试同样移出到 `instance/tests.rs`
  - 纯搬运，没有改一行逻辑：函数名集合逐模块比对前后完全一致（agents 364、llm_complete 123、tools 544、instance 38）；`cargo check --all-targets` 零 warning，`cargo test` 389 passed / 1 ignored
- finish the profile → backup rename and dedupe drifted helpers ([`d67b058`](https://github.com/xlbbb-cn/ai-chat/commit/d67b0585561c9934290e9a84f675a4906bdb179b))
  - `profile` → `backup` 收尾：命令 app_export_profile / app_import_profile → app_export_backup / app_import_backup（Rust 命令 + TitleBar/api 包装），事件 profile-export-* / profile-restored → backup-export-* / backup-restored，前端 exportProfileTo / importProfileFrom、profileExporting|Phase|Status、CSS .profile-export-progress* 一并改名，AGENTS.md 的命令清单同步；界面早就叫 Backup，代码里的旧名到此为止
  - 新增 src-tauri/src/util.rs：now_ms（原来 5 份拷贝，外加 mcp.rs 的 u64 变体 → now_ms_u64）、truncate_chars / truncate_with_ellipsis / truncate_with_trailing_ellipsis（三种语义此前共用 truncate_chars 一个名字，差异只写在各自的函数体里）、slugify(text, max)；agents/files.rs 与 tools/mcp_self.rs 只保留各自的空值兜底（agent-<uuid> / server）
  - 新增 src/utils/format.ts：formatBytes（MonitorPanel 固定 0/1 位小数、UpdatePanel 自适应，同一尺寸在两处读数不同）、formatDate、escapeHtml（markdownMath 与 printDocument 各一份）
  - 硬编码色进 token：App.css 六个 token 块补 --c-knob / --c-favorite，三个暗色块另加 --c-syn-* 八色；ChatMessage.css 的 hljs 覆盖、三处 toggle 滑块、收藏星、三处 mask 渐变全部改走 token

### 文档

- drop winget/brew/scoop install commands until the packages exist ([`403a747`](https://github.com/xlbbb-cn/ai-chat/commit/403a747c908f84213b0a05db9a658d8799311d18))
- **store**: link the Microsoft Store listing from README, site and wiki-facing docs ([`41fcebf`](https://github.com/xlbbb-cn/ai-chat/commit/41fcebf162efd5957f901f1cec748521db7d4bc9))
- redraw the README banner with the current product name ([`bdad7f7`](https://github.com/xlbbb-cn/ai-chat/commit/bdad7f7590b123c746c5dbda911728dab121b95d))
  - docs/banner.png 重绘为 1672×941、8-bit、193 KB：暗色渐变 + 强调色光晕 + 应用图标 + 新名称 + 中英副标题 + 版本页脚，配色取自界面 token（#141E38/#3E6AE1/#9DB7FF）
  - 生成脚本可重复运行：`.data/tmp/banner/build.sh`（先量文本宽度再排版，中文用 Noto Sans CJK）
  - store-publishing.md 的待办同步：banner 已处理，商店截图仍需在装好新名称窗口的机器上重拍

### 杂项

- **icons**: refresh the app, store and favicon artwork ([`a5bb7f7`](https://github.com/xlbbb-cn/ai-chat/commit/a5bb7f7dddeb196a4a9f49b6a41d4f9dff26dcb6))
- drop the dead CSS, IPC commands and exports left by the panel rework ([`0afdbaa`](https://github.com/xlbbb-cn/ai-chat/commit/0afdbaa18bedf3c53513ded9857ba188199e22e4))
  - 死 CSS：.message-export-*（ChatMessage，已被 .message-action-buttons 取代）、.history-delete-btn 五条（HistoryPanel，已由 .history-action-btn 取代）、AgentsPanel 里退役的 “CLI agents” 列表（.cli-agents-block / -header / -intro / -empty、.cli-agent-item / -info / -command / -hint / -probe-hint），保留仍在用的 .cli-agent-probe-row 与 -probe-result*
  - 不可达的 api_requests 监控通道：db.rs 的 ApiRequestRecord / ApiRequestDetail 与 list/get/delete/clear_api_requests 四个命令、lib.rs 注册、api.ts 的两个类型与四个包装。Monitor 面板改读 interaction_log 之后没有任何调用方；api_requests 表本身仍在（llm_complete 写入、会话统计与日志保留策略在读），因此只退通道不退表
  - db::delete_session_meta 与 api/db::clear_interactions 一并删除：命令注册齐全但前端没有入口，面板的“清除”走 clear_logs
  - 未引用导出清空（审计脚本现为 total: 0）：FILE_EXTENSIONS、getActiveLocale、LegacyCliAgent、AgentMbusDeliveredEvent
  - docs/gen.py 的实测计数同步为 92 命令 · 48 事件（删掉 6 个命令后的真实值），architecture.html 重新生成
  - 验证：tsc 无错、vite build 通过（主 chunk 1 908.71 kB / gzip 601.10 kB 不变）、cargo check --all-targets 零 warning、cargo test 389 passed / 1 ignored
- **build**: trim the main chunk and refresh stale docs after the merge ([`38851b1`](https://github.com/xlbbb-cn/ai-chat/commit/38851b1bf2989baf1c2b87e86296a67cb672ace4))
  - highlight.js 改走 lib/common，再补 12 个常用语法（dockerfile、powershell、nginx、cmake、dart、elixir 等）；主 chunk 2692 kB → 1909 kB，gzip 851 kB → 601 kB
  - 删掉无引用的字体资源：.eot/.svg/.ttf、重复的 RazerF5.ttf、Web Fonts Demo.html、react.svg，共 1.39 MB；@font-face 只留 woff2/woff
  - .gitignore 里的 docs\store\screenshots/ 用了反斜杠，任何平台都匹配不到，改成正斜杠
  - CHANGELOG 用 gen-changelog.cjs 补齐 v2.9.6 → v3.2.0 六个版本（共 150 行）
  - docs/gen.py 仍写着旧名 AI Chat 与 GPL-2.0-only，重跑会把 architecture.html 改回旧值；同步为 Rabbit Chat Studio / MIT，并按源码实测刷新计数

### 其它

- gitignore ([`b81e932`](https://github.com/xlbbb-cn/ai-chat/commit/b81e9320cac9efea3e32c6dc08fab2438c4b46db))

**完整对比**：[v3.2.0...v3.3.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.2.0...v3.3.0)

## [v3.2.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.2.0) — 2026-11-12

### 新增

- add Microsoft Store (MSIX) packaging pipeline ([`88bedb5`](https://github.com/xlbbb-cn/ai-chat/commit/88bedb581601a21725941507bbb7753990fa4b9e))
  - scripts/build-msix.ps1 + 'npm run msix': makeappx packaging, 4-part version, optional dev cert
  - src-tauri/msix/: full-trust Desktop Bridge manifest, Partner Center identity in store.config.json
  - update: skip the GitHub updater on Store installs (WindowsApps path detection)
  - CI: build the .msix in the windows job, keep it as an Actions artifact only
  - docs/store-publishing.md + docs/store/: policy mapping, certification notes, privacy policy
  - scripts/capture-store-screenshots.ps1 for store screenshots

### 重构

- rename AI Chat to Rabbit Chat Studio, relicense to MIT ([`18e27e1`](https://github.com/xlbbb-cn/ai-chat/commit/18e27e100cd1296e0c471b791d8bac75d71225ce))
  - display name, productName, crate and binary become Rabbit Chat Studio / rabbit-chat-studio
  - MSIX identity target: XLBBB.RabbitChatStudio, CI stable assets renamed to Rabbit-Chat-Studio-*
  - identifier com.leonard.aichat, repo slug and the ai-chat.locale key stay put (user config continuity)
  - GPL-2.0-only -> MIT: LICENSE, Cargo/package manifests, README badges, architecture doc, store copy
  - winget id follows to XLBBB.RabbitChatStudio (winget-pkgs#458061 for Leonard.AIChat was closed)

**完整对比**：[v3.1.1...v3.2.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.1.1...v3.2.0)

## [v3.1.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.1.1) — 2026-11-12

### 修复

- **figures**: keep a diagram out of the code path in the transcript ([`a513b0e`](https://github.com/xlbbb-cn/ai-chat/commit/a513b0e8abf1b187866c93b8796767976293fc6e))

### 杂项

- **icons**: regenerate the app icons ([`4ead3ca`](https://github.com/xlbbb-cn/ai-chat/commit/4ead3ca2884d8013b1f980d3b17fbcacbbb11129))

**完整对比**：[v3.1.0...v3.1.1](https://github.com/xlbbb-cn/ai-chat/compare/v3.1.0...v3.1.1)

## [v3.1.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.1.0) — 2026-11-12

### 新增

- **chat**: write a reply's PDF export to a file, not through the print dialog ([`9028336`](https://github.com/xlbbb-cn/ai-chat/commit/9028336c152640a299b44c885a02b4c2fce2a9b3))
- **markdown**: export the editor draft to a real PDF file ([`154b5f9`](https://github.com/xlbbb-cn/ai-chat/commit/154b5f9bf4f7deb2fcaa3e14db8720196cae0bd3))
- **tools**: fix run_shell's shell at build time and carry the host OS in its schema ([`2c56c61`](https://github.com/xlbbb-cn/ai-chat/commit/2c56c61a1b24acaf479dde3cca9b065ecc870be7))
- **figures**: declarative flow charts and scene-diagram vocabulary for carve ([`dbea81e`](https://github.com/xlbbb-cn/ai-chat/commit/dbea81e4fcbf477df9ced1ec712327caffe121c6))
  - `flow` fence (or raw <flow> tag): JSON spec of nodes (6 shapes, 5 tones), labelled edges, LR/TB direction. Node sizes are measured text, positions are dagre's, colours are baked interface tokens — light/dark and every palette follow on re-render. Wide graphs scroll past 2.5x the card; tall TB charts never letterbox. Lazy dagre chunk.
  - Motion without script: anim "flow" (dashes + an SMIL packet per edge) and "step" (rank-order reveal) play on the settle commit without the script-consent gate, honour prefers-reduced-motion twice (build time and inside the emitted style), and still save as PNG.
  - carve: `edge` objects (line + cone arrowhead, dashed option), `label` sprites with a surface halo, a soft contact shadow by default, and MeshStandardMaterial shading — a scene diagram, not floating solids.
  - prompt: flow is the flowchart path, html-fig narrows to touch/ interaction, carve documents edge/label/shadow; the prompt's own flow example is test-pinned as valid JSON. #name also parses at the end of a fence line.
- **figures**: render dynamic HTML5 figures as sandboxed html-fig cards ([`398e91a`](https://github.com/xlbbb-cn/ai-chat/commit/398e91adbce01b3fcde41b2d688bf1402a9a72fd))
  - utils/htmlFigure.ts (new): token projection into the frame document, the CSP, the height reporter, buildFrameDocument/buildStandaloneDocument
  - figures.rs: save_figure_html + check_html_document, dynamic-figure bullet in the drawing prompt
  - figureCard.ts: buildHtmlCard, the execution gate and the navigation guard, FigureHandle.beforeMount/dispose
  - ChatMessage.tsx: settle-commit rebuild so a deferred frame can mount
- **glass**: keep the frost on platforms with no window backdrop ([`2a01081`](https://github.com/xlbbb-cn/ai-chat/commit/2a01081d277b12a50a2cc2cefa5c7be04b015c2b))

### 修复

- **chat**: truncate the failed turn before retrying ([`b7d3d6b`](https://github.com/xlbbb-cn/ai-chat/commit/b7d3d6bce3d5fffa0d74297acc3f77d7b7b3e0b7))
- **ui**: keep the [goal:DONE] control marker out of the chat bubble ([`89f3c51`](https://github.com/xlbbb-cn/ai-chat/commit/89f3c51148f8a7c7681a77c25898d08bf24b8fc2))
- **llm**: stop the chat turn from dying silently mid-task ([`887519a`](https://github.com/xlbbb-cn/ai-chat/commit/887519a22305c365c2944441ebcdf559c81b9258))
  - stream requests without a total timeout (read_timeout 120s instead of the 300s wall that killed every long thinking stream)
  - execute streamed tool calls also when finish_reason is "stop"/absent (relay dialects); "length" truncations are regenerated, never executed
  - continue once or twice after finish_reason "length" instead of silently ending the turn with a half answer
  - treat a stream that closes without finish_reason/[DONE] as an error instead of truncating silently
  - retry transient HTTP statuses (429/5xx) honoring Retry-After
  - retry mid-stream disconnects even when tokens already streamed: a new chat-retry event lets the frontend drop the dead attempt tail first; after 3 attempts the turn fails with the error visible
  - goal loop: with the todos tool on, a final answer does not end the turn while the list has open items; the model ends explicitly with [goal:DONE] (bounded at 4 continuations)

### 杂项

- **figuers**: op figuers system prompt ([`f2a72a5`](https://github.com/xlbbb-cn/ai-chat/commit/f2a72a57a33719a276853d14c17d77473b69ec97))

**完整对比**：[v3.0.0...v3.1.0](https://github.com/xlbbb-cn/ai-chat/compare/v3.0.0...v3.1.0)

## [v3.0.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v3.0.0) — 2026-11-11

### 新增

- **composer**: embed a sub-agent role with +agent in a new conversation ([`f0f30d9`](https://github.com/xlbbb-cn/ai-chat/commit/f0f30d925d63ba343fe901d5d4f3f1530bd54e29))
- **figures**: draw with Mermaid when the enhanced toggle is off ([`260dfe9`](https://github.com/xlbbb-cn/ai-chat/commit/260dfe9fc9c4be5a922ee0e1dcb0a7200d529d04))
- **ui**: report an issue from the titlebar, above About ([`2c1c790`](https://github.com/xlbbb-cn/ai-chat/commit/2c1c790234a003cb9e2614036dd16f2b5ddb9b2a))
- **composer**: address tmp files as @@name and name attachments instead of listing them ([`00fc6a9`](https://github.com/xlbbb-cn/ai-chat/commit/00fc6a9dce17d257b270c345cae134b8c8234ef7))
- **composer**: a pasted or dropped file becomes an @ mention under .data/tmp ([`406edd5`](https://github.com/xlbbb-cn/ai-chat/commit/406edd5dda4a0e95623b8efca2ad42180bab4dbd))
- **ui**: the reply can draw, in two dimensions and in three ([`b895af2`](https://github.com/xlbbb-cn/ai-chat/commit/b895af20af3b5a25d515e7bb024753b5b178d372))
- **ui**: the transcript as glass under the desktop ([`bf865d7`](https://github.com/xlbbb-cn/ai-chat/commit/bf865d7a555f0ec0dd8292824290c9141ecccf3e))
- **ui**: spring-driven sidebar motion with framer-motion ([`1156002`](https://github.com/xlbbb-cn/ai-chat/commit/1156002e1b4adabe7ec6c04c135492cf67aa5817))
- **ui**: frosted glass as a window-level DWM backdrop ([`04553a5`](https://github.com/xlbbb-cn/ai-chat/commit/04553a5f1e480643d93f6192df91ea247a9ae95a))
  - the window is transparent and the desktop behind it is blurred by DWM, not by CSS: window-vibrancy applies the Windows 11 system backdrop DWMSBT_TRANSIENTWINDOW (Acrylic) rather than Mica, because Mica only samples the wallpaper and reads as a flat pane; Mica stays as the fallback, and apply_mica still runs first for DWMWA_USE_IMMERSIVE_DARK_MODE so the Acrylic tint follows the app theme instead of the system one
  - new set_window_vibrancy(level, dark) command, applied in setup and from Settings, returns the kind that is now in effect; <html data-vibrancy> is written only when a backdrop really is applied, so a host without one keeps the opaque canvas instead of showing a raw or black window
  - --c-window (24 / 40 / 58% of --c-surface for subtle / standard / strong) takes over --glass-backdrop under data-vibrancy: titlebar, sidebar, toolbar, input area and panel headers turn translucent over the blurred desktop while .messages keeps its opaque --c-bg, so reading and scrolling are untouched
  - Settings -> Appearance: the glass level drives the native backdrop live, and a theme change re-applies it so the Mica/Acrylic light-dark variant follows
  - docs: DESIGN.md (Window vibrancy section + the --c-window token row), config.example.md, AGENTS.md and the three i18n glass hints

### 修复

- **figures**: clean up a failed Mermaid draw and an export rejection ([`f076b32`](https://github.com/xlbbb-cn/ai-chat/commit/f076b3293a662aa1990bd2168a456639f0bde442))
- **figures**: keep a drawing inside its card and code out of the scan ([`23bbfba`](https://github.com/xlbbb-cn/ai-chat/commit/23bbfbafe61dc1fc91dd11535fdda0e4a50bd9ef))
- **chat**: broadcast the end of background tasks their turn tore down ([`aeba54c`](https://github.com/xlbbb-cn/ai-chat/commit/aeba54c877568ea51bfac7d54bf4ea53b88e8f28))
- **ui**: readable select rows and chevrons in the dark palettes ([`9c782bc`](https://github.com/xlbbb-cn/ai-chat/commit/9c782bcdfb28d097fa3a724507b74c82673fd4fb))

### 文档

- add a self-contained architecture map ([`8b0ea99`](https://github.com/xlbbb-cn/ai-chat/commit/8b0ea994391aee90084bb3430ae96e9917ab8a62))
  - docs/architecture.html: layers, one chat turn and the sub-agent pipeline as inline SVG, coloured by the design tokens in src/App.css
  - docs/gen.py: regenerates the page with no dependencies (python3 docs/gen.py)
  - README + README.zh.md: link the map from Documentation, Tech Stack and Architecture, and the project tree

### 其它

- add： update README ([`1c0fd8b`](https://github.com/xlbbb-cn/ai-chat/commit/1c0fd8bf3c32e62acaa51dee3741b8bcf11f9946))

**完整对比**：[v2.9.7...v3.0.0](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.7...v3.0.0)

## [v2.9.7](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.7) — 2026-11-10

### 新增

- show maintenance progress and results on the empty state ([`49f7618`](https://github.com/xlbbb-cn/ai-chat/commit/49f7618e7eacffcd0f3f912ed969aa006ed1c67e))
  - running events carry phase/round/toolCalls and drive the status line (Dreaming Scene N/M) that replaces the empty-state greeting
  - a completed pass leaves a clickable result bubble with the model's report; clicking it restores the greeting
  - partial passes persist a resume note in the evolution memory scope and the next pass continues from it; budget wrap-up ends with a report instead of an error

### 杂项

- bump version to 2.9.7 ([`cff16d0`](https://github.com/xlbbb-cn/ai-chat/commit/cff16d0d0b96565ab0690019ff0622b8975955f0))

**完整对比**：[v2.9.6...v2.9.7](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.6...v2.9.7)

## [v2.9.6](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.6) — 2026-11-10

### 新增

- add nap-face evolution maintenance with resumable progress ([`70a36a4`](https://github.com/xlbbb-cn/ai-chat/commit/70a36a499467f7ee41e86e71c9d1b7b9602f1902))
- **chat**: LLM-structured context compression with cross-turn replacement ([`0f52872`](https://github.com/xlbbb-cn/ai-chat/commit/0f52872502d953fd4dfc7dc0a56ddd72b471605c))
  - Summarize older context with a 9-section compact prompt (intent, concepts, files, errors/fixes, problem solving, all user messages, pending, current work, next step) via the main model; mechanical line summarizer stays as a fallback so compression never fails a turn.
  - Preserve tool traces: tool rounds render as a one-line ledger (name(args) -> result preview) in both the summarizer input and the fallback; the recent window only starts on a clean round boundary.
  - Inject the summary as a user message with continuation framing, placed before the remaining history (stable prompt-cache prefix).
  - Replace the covered history across turns: session_summaries now stores covered_messages + boundary_text; the next turn verifies the boundary and drops the summarized prefix (stale summaries are discarded safely).
  - Add a predictive pre-check (~4 chars/token, tools included, saturation guard) and reactive recovery on provider context-overflow errors (one hard compression + single retry).
  - db migration for the new coverage columns; 12 new unit tests.

### 文档

- update READMEs and CHANGELOG for v2.9.5 ([`e19dff4`](https://github.com/xlbbb-cn/ai-chat/commit/e19dff43b965809b8df27f3d234a9acb8fef88de))

### 杂项

- bump version to 2.9.6 ([`ec21d20`](https://github.com/xlbbb-cn/ai-chat/commit/ec21d209a5c178feb67d1126cbe9fd5ddb89dcd9))

**完整对比**：[v2.9.5...v2.9.6](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.5...v2.9.6)

## [v2.9.5](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.5) — 2026-11-10

### 新增

- add hopping smiley face animation to the empty chat state ([`5b7b4a5`](https://github.com/xlbbb-cn/ai-chat/commit/5b7b4a5b265bf17dde7a25652a7b4246aac3650b))
- **monitor**: give the panel a light theme and a session report ([`5222b34`](https://github.com/xlbbb-cn/ai-chat/commit/5222b34549a0a04a270dae3d75c8aa3ae122a28a))
- **instance**: run extra windows on forked configs and scope timers per workspace ([`6db0fbc`](https://github.com/xlbbb-cn/ai-chat/commit/6db0fbcb83680f561fbb99d04929e15f61a8bd70))
  - shadow windows copy config.json into shadow/config-<ts>-<id>/ with a private workspace and merge only workspace_history back on exit
  - record workspace_history in apply_config and expose it as a Settings dropdown
  - move timers.json into <workspace>/.data and rebase the registry on workspace switch
  - fix the pre-Tauri app-data lookup to dirs::data_dir() (Roaming on Windows) so startup reads the real config.json again; setup re-claims if it ever diverges
  - keep a shadow's cleared workspace_dir inside its own fork

### 重构

- **palette**: rename the fig palette to sakura ([`26f083d`](https://github.com/xlbbb-cn/ai-chat/commit/26f083dcf4b146ef7d9b3d3607e30f24b8053e41))

### 杂项

- bump version to 2.9.5 ([`307624a`](https://github.com/xlbbb-cn/ai-chat/commit/307624a14221c4154e65dcc84fdfbe5f8122f2c6))

### 其它

- **chat**: reveal streamed replies character by character ([`65940df`](https://github.com/xlbbb-cn/ai-chat/commit/65940df786f433dd6c42f2445e575668111ae057))
- **panel**: unify the side panel header chrome ([`ddb3891`](https://github.com/xlbbb-cn/ai-chat/commit/ddb389152fe1947ff0b23177aa625ed7fb45aea9))
- **sidebar**: crossfade panels instead of sliding the frosted surface ([`a284336`](https://github.com/xlbbb-cn/ai-chat/commit/a284336501fb099ad5bc5c57f2b01ee5251ae174))
- **chat**: slide the panel selection and deepen the typewriter fade ([`2a7eb51`](https://github.com/xlbbb-cn/ai-chat/commit/2a7eb51787cc84b035b1494dee06b0e1d00e0fd5))
- **titlebar**: underline the active workspace label in red ([`04cd90a`](https://github.com/xlbbb-cn/ai-chat/commit/04cd90a676078b4a43537f925cdd59ebf83792e2))

**完整对比**：[v2.9.4...v2.9.5](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.4...v2.9.5)

## [v2.9.4](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.4) — 2026-11-09

### 新增

- **release**: generate release notes and drop the Windows MSI bundle ([`4701f4b`](https://github.com/xlbbb-cn/ai-chat/commit/4701f4bcdbd50612affe9df1af377024bcc8ab41))
- **agent**: cap the turn budget and disable a failing tool for the session ([`fdf28bd`](https://github.com/xlbbb-cn/ai-chat/commit/fdf28bd26cbde4fd5eee20c3e19e1ecc2e1a0b0e))
  - turn_guard.rs: TurnBudget caps requests and tool calls per turn, ToolFailureGuard disables a tool after N consecutive failures for the rest of the session. Both are pure logic and unit tested.
  - The main loop, the sub-agent loop and run_main_agent_helper share the guard, so a tool the chat gave up on stays disabled for its sub-agents too.
  - When the budget is spent the turn sends one wrap-up request that keeps the identical tool list and only sets tool_choice=none, so the prompt prefix and its KV cache survive; a rejected wrap-up ends the turn with a fallback answer instead of an error.
  - Tools disabled mid-turn stay advertised until the next turn and are refused by the tool loop, for the same prefix-cache reason.
  - Settings -> Advanced gains main_agent_max_rounds (40), main_agent_max_tool_calls (120) and tool_failure_limit (3); 0 means unlimited and every value is clamped on save.

**完整对比**：[v2.9.3...v2.9.4](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.3...v2.9.4)

## [v2.9.3](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.3) — 2026-11-08

### 新增

- **mcp**: let the assistant build and run its own MCP servers ([`9656a3c`](https://github.com/xlbbb-cn/ai-chat/commit/9656a3c0fa8a97d22d0a4053010152252964700d))
  - `mcp_self` tool (`tools/mcp_self.rs`): `list` / `write` / `enable` / `disable` / `test` / `restart` / `logs` / `remove`. The script lives in `<workspace>/.data/mcp/<slug>/` (`.bak.<n>` on update); the interpreter is picked by the backend from a whitelist (`python` / `node` / `shell`), never named by the model, so the tool can only turn workspace code into a server, not register an arbitrary command. Writes, enabling and removal ask the user once (`user_software`, pre-approvable via `auto_accept_confirm_kinds`).
  - Two rosters merged on read: manually created servers stay in the app-data `mcp_servers.json`, self-built ones live next to their script in `<workspace>/.data/mcp.conf` (YAML, JSON tolerated). `load_effective_servers` merges them with the panel entry winning on an id clash; panel save/delete route by provenance, so a self-built row is edited through the workspace file and is never duplicated into app data. Startup migrates entries an earlier build wrote into app data.
  - `AppState::mcp_revision` + `build_turn_tools`: the tool payload is rebuilt at each request boundary once the registry changed, so write -> enable -> call works inside one turn instead of the next conversation turn. Sub-agents and the main-agent helper drop `mcp_self` (`agents::drop_mcp_self`): they would race on the roster, an `AgentMcpSession` only reads servers at task start, and the confirmation dialog would block a mission.
  - A start that fails `tools/list` leaves the server disabled so it cannot stall later requests, and the error carries the log tail; `logs` returns the tail on demand so the model can fix its own script.
  - Profile export/import carries `workspace/mcp.conf` and now really restores the app-data `mcp_servers.json` (it was exported but ignored on import), then retires live stdio children so the restored roster takes effect.
  - Docs: AGENTS.md (tool family, `.data` layout, self-evolution bullets, MCP warmup), DESIGN.md (provenance badge spec), the self-evolution system prompt, and the MCP panel provenance badge plus a refresh on `mcp-servers-changed`.

### 性能

- **prompt**: shrink the first request and stop preloading evolution facts ([`2c29981`](https://github.com/xlbbb-cn/ai-chat/commit/2c29981521279a9c35a35acb9eff73dcf0737db7))
  - Tool schemas (`get_all_tools`, `mcp_self`, `skill_read`): rendered JSON for the 13 main-agent tools drops 28 731 -> ~23 600 characters (-18%) with the parameter shapes unchanged. `run_shell` reuses `run_cmd`'s risk-level wording, the MCP stdio contract is documented once in the `mcp_self` schema, and the timer/todo/background descriptions keep the mechanics while the system prompt keeps the policy.
  - System prompt: `self_evolution_prompt` 4 475 -> 3 175 characters (-29%), plus compacted skills/agents lists (one line per entry), memory/timer/todo guidance, the explicit-invocation line and the active-skill context.
  - Orchestration: selector, planner, supervisor and the sub-agent static/mission prompts lose duplicated sentences (-8%..-17%); the JSON contracts and the capability-fit rules stay verbatim.
  - Evolution memory is no longer preloaded into any request (it could take the whole ~4,000-character budget on every turn): the prompt only tells the model to search the scope when a problem needs earlier experience. `render_evolution_context` stays as the `#[cfg(test)]` reference renderer; `EVOLUTION_CONTEXT_BUDGET_CHARS` still drives the panel's top tier and the `consolidate` plan.
  - Memory panel and i18n (en / zh-CN / zh-TW): "in prompt" becomes "top tier"; AGENTS.md documents the on-demand policy instead of the VERIFIED FACTS block.

### 重构

- **kg**: extract the knowledge graph client into its own crate ([`fa6516f`](https://github.com/xlbbb-cn/ai-chat/commit/fa6516faa1ba7faa0058b7b399a4dec006a78b22))
  - The app crate drops `neo4rs` and `async-trait` (only that client used them), so the binary no longer links a Neo4j driver for code it never calls.
  - `lib.rs` stops declaring the module through `#[path = "utils/knowledge_graph/mod.rs"]`: `src/utils/` is a plain folder holding standalone crates, not a module.
  - The crate's tests are targeted by package name (`cargo test -p knowledge-graph`), because the app crate's default build only covers the root package.

### 文档

- **changelog**: add the v2.9.2 section ([`b77a190`](https://github.com/xlbbb-cn/ai-chat/commit/b77a190c4ef92830ebeaf3995c35bbb0c1531422))

**完整对比**：[v2.9.2...v2.9.3](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.2...v2.9.3)

## [v2.9.2](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.2) — 2026-11-08

### 新增

- **release**: generate the changelog and release notes from commits ([`874a572`](https://github.com/xlbbb-cn/ai-chat/commit/874a572b3c4c83c9f42c4f61678ced6a43768bfd))
  - `--write` upserts the section into CHANGELOG.md and replaces a section for the same version, so re-running a release is idempotent.
  - `--format body --output <file>` produces the short list the workflow needs.
  - `release.yml` gains a "Generate release notes" step: it resolves the previous tag with `git describe --tags --abbrev=0 "${GITHUB_REF_NAME}^"`, renders the body, and hands it over through `$GITHUB_OUTPUT` with a heredoc delimiter (a plain `echo` would flatten the newlines). `releaseBody` embeds it above the install notes.

### 重构

- **tools**: retire the knowledge graph tool, keep its client in utils ([`fd38515`](https://github.com/xlbbb-cn/ai-chat/commit/fd38515e09b8fe3088000fb137b75630bf362cd8))
  - `src-tauri/src/utils/knowledge_graph/` is a self-contained module library: `mod.rs` (the `KnowledgeGraph` trait), `settings.rs` (`Engine` + `Settings`) and `neo4j.rs` (`Neo4jRepo`). It is registered from the crate root with `#[path]`, so `src/utils/` stays a plain folder next to the `pdf2md-rs` crate instead of growing a `mod.rs` of its own.
  - `Settings` owns the connection defaults (`bolt://localhost:7687`, user `neo4j`, empty password) that used to live in `AppConfig::default()`, so re-enabling the tool is a tool definition plus a dispatch arm calling `Neo4jRepo::connect(&Settings)`.
  - `AppConfig` loses `kg_engine` / `neo4j_uri` / `neo4j_user` / `neo4j_password`. A `config.json` written by an older version still parses — serde ignores the unknown keys — and the next save drops them.
  - `RETIRED_TOOL_IDS` with `AppConfig::strip_retired_tools()` prunes the stale id on startup, on every save, in `get_config` and in `effective_tools()`, so a selection carried over from an old config cannot reach the prompt.
  - The tool definition, the dispatch arm, the Tools-panel row, the engine selector, its CSS and the i18n keys are gone. `neo4rs` stays a dependency of the library, and `build_config` is split out so the settings → driver mapping is covered by a test that never touches the network.

### 杂项

- keep the local test book PDF out of the repository ([`5d108f2`](https://github.com/xlbbb-cn/ai-chat/commit/5d108f219b342e4792f55ccfc21294e64ece8447))

**完整对比**：[v2.9.1...v2.9.2](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.1...v2.9.2)

## [v2.9.1](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.1) — 2026-11-08

### 新增

- **composer**: show PDF mention conversion progress, store it by path ([`850a6bd`](https://github.com/xlbbb-cn/ai-chat/commit/850a6bd784ff04913d0cbb3ac9fe81f617ac5d8f))
  - `convert_pdf_mention` takes a `tauri::ipc::Channel<PdfMentionProgress>` and forwards the pipeline's own ticks (stage, page/total, percent), deduped on stage + page + total so a fast conversion stays a few frames;
  - `convertPdfMention` / `resolveFileMentions` take an optional callback and tag every tick with the file it belongs to; `sendMessage` renders the strip and clears it in a `finally`, so a failed or cancelled mention leaves nothing behind;
  - the strip is one muted 12px row with a page counter and the same 2px accent track as the context bar, in flow next to the injection bar (DESIGN.md).
  - `compactAttachmentsForDb` drops `text_content` from any attachment that has a `path` — the Markdown already reaches every later request through the message `content` — so a stored PDF mention keeps one string plus the cache path;
  - live attachments keep their text, so sending never depends on a cache file that may since have been pruned, and a text slice without `path` is untouched.
- **composer**: convert `@paper.pdf#2` mentions to Markdown ([`c703f4e`](https://github.com/xlbbb-cn/ai-chat/commit/c703f4eaa28ca7421efbbe5f64adf2ab81ad2400))
  - `pdf_mentions.rs`: page planning (a bare mention converts a 60-page preview, `#2-` runs to the last page), 48 MiB and 60k-character guards, cache keyed by path + range + file size + converter version, and a provenance comment that names the cache file so a later tool call can read the rest. Conversion runs in `spawn_blocking`, so a hundred-page mention cannot freeze the UI.
  - The cache keeps the *full* conversion (`prompt_markdown` only budgets the copy that goes into the prompt, note included).
  - `pipeline::page_count` lets the range be clamped before converting, which is what makes "page 9 of 30" a better error than a failed range.
  - `supportsMentionName` keeps PDFs in the popup for models without `supports_files`; `fenceLanguage` fences the inlined Markdown as `markdown` instead of letting the `kk.pdf#2` label become the fence tag.
  - AGENTS.md documents the flow next to the other attachment rules.
- **pdf2md**: ship pdf2md-rs as an embeddable document library ([`1ce2257`](https://github.com/xlbbb-cn/ai-chat/commit/1ce2257e5978dc1a9f858f3ed93471e85eff464c))
  - read `/FirstChar`, `/Widths` and `/MissingWidth` and advance the pen by the font's own metrics. Runs that resume from an explicit `Tm` no longer overlap the text before them (`the GNU` used to interleave into `tGhNeU` on real papers).
  - decode literal strings that are valid multi-byte UTF-8 as UTF-8 before falling back to the font encoding, so bullets stop turning into `¢`.
  - group lines into paragraphs by the page's own leading (smallest gap in the column, capped) instead of a fixed fraction of the font size, and drop the "short line ends a paragraph" rule that split ragged-right text at every line break.
  - grow the measured text box to the mirrored margin so a centred title or equation no longer defines the right edge of the page — which is what the formula centring test compares against.
  - ruled tables assign words, not lines, to cells; all-empty trailing columns are trimmed and a grid with a single column is not a table.
  - aligned-text tables must repeat the same number of cells at the same x on most rows, so a schematic or a poster is no longer one giant table.
  - the "by A. Author" line becomes metadata only, and list markers are stripped before the IR keeps the item text.
  - inline math requires a short variable and one short subscript, so identifiers such as `Conn_02x10_Odd_Even` stay plain text; `*` and `/` are operators, which keeps `y[n] = h[n] * x[n]` in one math run.
- **pdf2md**: add the pdf2md-rs extractor crate as a workspace member ([`328a14a`](https://github.com/xlbbb-cn/ai-chat/commit/328a14acd59ebd57aeab3a527129d1f5e507e64e))
  - `src-tauri/Cargo.toml` gains `[workspace] members = ["src/utils/pdf2md-rs"]`, so the extractor is built and tested by the same toolchain as the app; the root `Cargo.lock` picks up its dependencies (lopdf, image/png, thiserror, serde, and the optional GROBID stack: ureq + quick-xml).
  - The crate holds the PDFOxide text-extraction layer (font tables and ToUnicode CMaps, image re-encoding, content-stream decoding), a line/word layout pass with reading-order sorting, the unified IR, a Markdown router with heading and table parsers, formula handling and optional GROBID layout backend.
  - Landing it as-is keeps the scaffolding reviewable: `src/lib.rs` is still the placeholder and declares none of the modules, and `src/parse/` has no `mod.rs`, so `cargo check -p pdf2md-rs` passes without compiling any of the new code. Wiring the module tree is the follow-up commit.
- **chat**: float warnings and errors above the transcript ([`2cf5104`](https://github.com/xlbbb-cn/ai-chat/commit/2cf51045575e13009096a231c5be8061e24383cb))
  - Warnings and errors were a bar inside the message flow: it pushed the conversation around and stayed on screen until the next request. `ChatNotice` renders them at the top-left of the chat window instead, in a zero-height anchor strip below the toolbar, so nothing is reserved and nothing shifts. A notice leaves on a click and on its own after five seconds (opacity on `--t-dur` / `--t-ease`); one that carries an action waits instead, so its button cannot vanish under the pointer reaching for it.
  - The failed turn's own failure row is gone — the notice is the only report, and it carries "continue" when the turn left something to resume. `resumeAction()` withholds that action from turns that produced nothing at all (an immediate rate limit has nothing to push forward). A failed turn also stops offering PDF export: there is no finished reply to hand out, and with the failure row gone that icon was all that remained at the bottom.
  - Injections longer than three lines fold to three with a measured `Show more` / `Show less` (the `.inline-edit-btn` primitive at message density), so one turn stays scannable. The fold is measured after paint rather than counted from newlines: a single long line wraps past three rows and a short note may sit on two, so a note that fits never gets a dead toggle.
  - Size overrides on `.inline-edit-btn` now use compound selectors (`.inline-edit-btn.xxx`): the bundled sheet emits App.css after the component CSS, so single-class overrides lost the specificity tie and rendered at the primitive's 28px / 13px.
  - New i18n keys `chat.foldMore`, `chat.foldLess`, `app.noticeDismiss`; `app.errorPrefix` dropped along with the row that used it. DESIGN.md documents the notice surface and the three-line fold.

### 修复

- **pdf2md**: read the font's own CMap, and fall forward when there is none ([`c9ac2ed`](https://github.com/xlbbb-cn/ai-chat/commit/c9ac2edbad30e8de2e34bc4b348228b1ef0b6a95))
  - `cmap.rs` reads `ToUnicode` itself, tolerantly: code space ranges, `bfchar`, all three `bfrange` forms, comments, odd-length hex, several sections per CMap, and headers that do not parse at all. Codes are matched longest-first; codes the CMap does not cover are counted, not fatal, so a partial map still decodes with holes instead of collapsing into mojibake.
  - `fallback.rs` covers fonts that genuinely carry no CMap. A predefined CMap name states the code space outright (`Uni*-UCS2/UTF16` → UTF-16BE, `GBK-EUC`/`GBK` → GBK, `BigFive`/`CNS`/`ETen` → Big5, `*RKSJ` → Shift_JIS, `EUC` → EUC-JP, `KSC*` → EUC-KR), and so does a `UCS` CID ordering, where the code *is* the code point. With neither, the CJK byte encodings are tried in the order the font name suggests and kept only when the text could not plausibly be anything else: ≥40% CJK under a CJK name, 100% without one, and never a replacement or control character — a Latin run is never "rescued".
  - `fonts.rs` folds that into one ladder: own CMap → declared code space → UTF-8 sniff for simple fonts → gated guess → `lopdf`'s table → Latin-1, so no byte is dropped silently.
  - `pdf2md-rs` 0.2.0: the PDF mention cache digests the converter version, and this changes what the converter returns, so conversions cached from the mojibake build are invalidated instead of replayed.
  - `encoding_rs` supplies the GBK/Big5/Shift_JIS/EUC-KR tables; it was already in the lock as a transitive dependency.
- **pdf-mentions**: expire cached conversions after a week ([`6ab8b27`](https://github.com/xlbbb-cn/ai-chat/commit/6ab8b27396f912087dbc29fe7f8598d7618b3797))
- **test**: refuse Windows paths on every host ([`1c8eea1`](https://github.com/xlbbb-cn/ai-chat/commit/1c8eea1d433bd0eaa01e4e756b08c5344a2b7c1f))

### 文档

- **pdf2md**: describe the crate as a library in its front page ([`7ba75a1`](https://github.com/xlbbb-cn/ai-chat/commit/7ba75a10605534e51430cc8301f2ed2c914e8e7e))

**完整对比**：[v2.9.0...v2.9.1](https://github.com/xlbbb-cn/ai-chat/compare/v2.9.0...v2.9.1)

## [v2.9.0](https://github.com/xlbbb-cn/ai-chat/releases/tag/v2.9.0) — 2026-11-07

### 新增

- **composer**: add a /skill picker beside @file mentions ([`111e228`](https://github.com/xlbbb-cn/ai-chat/commit/111e228a1aa93125ac8e23e9fd3107c882abbd92))
  - `src/slashMention.ts` mirrors the `@` grammar: the `/` must open the line or follow whitespace and may not cross whitespace, `@` or `#`, so at most one popup can be open.
  - The skill list is read from the workspace at most every 15s and ranked by relevance with enabled skills first. Nothing is filtered out: `use_skill` resolves any workspace skill, enabling only preloads its description.
  - Accepting inserts a plain `/skill-name` token and leaves resolution to the model. Known skill tokens in a user message wear the `@file` accent, so `/usr/bin` and `1/2` stay plain text.
  - Also threads the session id and the only-newest-assistant flag into the working-plan panel, which shares these two files.
- **chat**: mirror the session working plan in the transcript ([`9f3405c`](https://github.com/xlbbb-cn/ai-chat/commit/9f3405c1390b7a497e42973295dd2be01a1939f0))
  - Read the session's active list on mount (`get_active_todo_list`) and keep the event stream on top of it. A sequence guard stops an in-flight fetch from overwriting a fresher event snapshot, and events are filtered by session so parallel sessions cannot mirror each other's list.
  - Snapshot the list once per turn in `run_chat_turn` and feed both channels from that one snapshot: the `ACTIVE TODO LIST` prompt block and a single `todo-state` event. Finished lists are still pushed — that event is what makes a 100% panel disappear.
  - Collapsed panel is two lines: title with the completed/total counter, then the running task (or the first pending one when nothing is running). The description line is gone.
  - Nothing pending and nothing running hides the panel entirely, the same rule the prompt block already followed.
  - Only the newest assistant message mirrors the list; every other message renders nothing, which also stops historic replies from each drawing the same plan.
- **chat**: group background tasks and serve processes in the transcript ([`095b29d`](https://github.com/xlbbb-cn/ai-chat/commit/095b29dc9df998a7bdcbb5a1a8fa85420ba3caff))
- **chat**: keep background task results out of the transcript ([`7594a0f`](https://github.com/xlbbb-cn/ai-chat/commit/7594a0f684204d9f8dfe06ec0ef4de8520dc2a14))
- **chat**: accent the @file mention instead of listing attachments ([`3bbc0d1`](https://github.com/xlbbb-cn/ai-chat/commit/3bbc0d1d85cd903291222a87a325311bb6fffb9e))
- **composer**: @file mentions with line ranges in the input box ([`e1007d7`](https://github.com/xlbbb-cn/ai-chat/commit/e1007d7585cf21df90c957118708afa2450ffdd7))

### 修复

- **sleep-lock**: hold logind and the desktop screensaver ([`32b9a86`](https://github.com/xlbbb-cn/ai-chat/commit/32b9a8613882519dabe56e6647c36a5c6b199820))
- **window**: keep the native menu bar to macOS only ([`7cc145f`](https://github.com/xlbbb-cn/ai-chat/commit/7cc145ff0f675ab013ff4e85f3a0d7665542de24))

### 重构

- **composer**: drop the paperclip picker in favour of @file mentions ([`a6fa5eb`](https://github.com/xlbbb-cn/ai-chat/commit/a6fa5ebdf5d416f8ab426de742585f5d1d7d5763))

### 杂项

- **deps**: re-align package-lock with package.json ([`24aa10c`](https://github.com/xlbbb-cn/ai-chat/commit/24aa10c84fccccaf1907845789d3cf65b96e4572))

### 其它

- add .data git ignore ([`b9fb537`](https://github.com/xlbbb-cn/ai-chat/commit/b9fb537b10fb6f891b51e664c37a29bef13e6ef8))

**完整对比**：[v2.8.9...v2.9.0](https://github.com/xlbbb-cn/ai-chat/compare/v2.8.9...v2.9.0)
