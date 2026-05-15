---
name: wiki-memory
description: ' 用于在 wikijs-store/ 中快速检索、更新和管理知识库笔记的标准化流程。   包含了文件搜索、内容修改、索引维护及 Git 版本控制的标准操作规范。'
allowed-tools:
- Bash
---


## 📂 笔记库基础信息
- **根目录**: `wikijs-store/`
- **文件格式**: Markdown (`.md`)
- **核心索引**: `wikijs-store/index.md` (总目录与大纲)

## 🔍 核心工作流 1：快速检索笔记
当用户询问某个概念、过去记录的信息，或要求查找笔记时：
使用 `ripgrep` 工具在 `wikijs-store/` 目录下进行全文搜索，快速定位相关笔记文件和行号。
linux/macOS:

```bash
cd  wikijs-store/
rg "搜索关键词" --with-filename --line-number
```
or
windows PowerShell:

```powershell
cd  wikijs-store\
rg "搜索关键词" --with-filename --line-number
```

## ✍️ 核心工作流 2：更新与新建笔记
当需要记录新信息或修改现有笔记时：

1. **局部修改**：优先使用 `patch` 工具对现有文件进行精准的文本替换（Find & Replace）。
2. **覆盖或新建**：使用 `write_file` 工具创建新文件或重写较短的文件。
3. **规范结构**：
   - 保持清晰的 Markdown 层级（`#`, `##`, `###`）。
   - 重要的列表数据考虑使用 Markdown 表格。
   - 如有必要，在文件头部添加 YAML Frontmatter（如 `title`, `date`, `tags`）。

## 🔗 核心工作流 3：维护与同步 (Git 同步法则)
**凡是修改或新增笔记，必须执行此收尾流程：**

1. **更新索引 (Index)**：
   如果是**新建**了笔记，必须使用 `patch` 或 `write_file` 更新相关目录下的 `index.md` 或总目录 `wikijs-store/index.md`，加上指向新文件的链接。
2. **版本控制同步**：
   所有变更完成后，使用 `execute_command` 工具在后台执行 Git 提交与推送，确保数据持久化且多端同步。

   ```
   cd wikijs-store
   git add .
   git commit -m "docs: 更新笔记 <简述修改内容>"
   git push
   ```

## ⚠️ 避坑指南 (Pitfalls)
- **不要猜内容**：修改前必须先 `read_file` 确认文件的实际行和上下文，否则 `patch` 极易失败。
- **防止幽灵文件**：建了新文件但不更新 `index.md`，会导致文件在知识库中“隐身”，不易被系统化查阅。
- **Git 冲突**：如果 `git push` 失败，通常是因为远程有更新。先 `git pull --rebase`，然后再 `git push`。
