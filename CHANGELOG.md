# Changelog

本文件由 `scripts/gen-changelog.cjs` 从两个 tag 之间的提交信息生成，请勿手工编辑：

```bash
npm run changelog -- --from <上一个 tag> --to <本次 tag> --details --write
```

条目按 Conventional Commits 的类型分组，作用域加粗；工作区里提交标题是英文、正文是中文，
所以带 `--details` 时是中英对照。

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
