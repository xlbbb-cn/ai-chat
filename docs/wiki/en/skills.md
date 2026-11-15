A skill is a reusable workflow: a folder plus a `SKILL.md` (or `skill.md`). YAML frontmatter describes the metadata; the body is the system-prompt fragment appended when the skill is loaded. Skills are ideal for code review, release checks, data analysis and other repeated processes.

Skills are **workspace-scoped**: each workspace has its own set and enablement state.

## Where skills live

```text
<workspace>/.data/skills/
  code-review/
    SKILL.md          ← lowercase skill.md also works
    templates/
      review.md        ← optional templates/data
  release-notes/
    skill.md
```

- The only skill root is `<workspace>/.data/skills`. There is exactly one — it is computed in a single place in the backend, so a second location cannot be introduced.
- One subdirectory per skill; the folder name may differ from the frontmatter `name`.
- Resolution tries the folder name first, then scans every skill's frontmatter `name`. A zip-extracted `gracker-writing-main/` declaring `name: gracker-writing` still resolves as `gracker-writing`.
- Both `skill.md` and `SKILL.md` are accepted.
- Backup folders left behind by a deletion (`demo.bak.1`) are not scanned as skills.

## Create a skill with the panel

1. Click **Skills** in the top toolbar.
2. Click **+ New Skill**.
3. Fill in:
   - **Name**: the model-facing id, e.g. `code-reviewer`; the folder is created with this name.
   - **Description**: one sentence describing the capability and trigger conditions; this is the main signal the model uses to load it.
   - **System prompt**: click **Edit** for a Markdown editor with live preview.
   - **Version**: optional, e.g. `1.0.0`.
   - **Allowed commands**: comma-separated executable names, e.g. `git, rg, npm`; empty = unrestricted.
4. Click **Save**. The app writes `.data/skills/<name>/skill.md`.
5. Toggle the skill on in the list to enable it for the workspace.

> A new skill is disabled by default. Enablement is stored in `.data/skills.status` and survives restarts.

### Enable / disable

- The toggle only affects the current workspace.
- A disabled skill is not listed to the model and cannot be loaded with `use_skill`.
- Deleting the skill folder does not remove its `skills.status` entry; the record stays until the folder returns.
- The panel keys enablement by the frontmatter `name`.

## SKILL.md format

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

### Frontmatter fields

| Field | Required | Description |
| --- | --- | --- |
| `name` | Yes | Model- and panel-facing skill id |
| `description` | Yes | Capability and trigger conditions; shown in the available-skills list |
| `version` | No | Version string, informational |
| `author` | No | Author |
| `allowed-commands` | No | Executable allowlist; empty = unrestricted |
| `allowed-tools` | No | Reserved metadata; the actual tool exposure is still controlled by the **Tools** toggles |
| `context` | No | Optional context note, reserved |
| `agent` | No | Optional associated agent, reserved |
| `license` | No | License |

The body after `---` is `system_prompt`. It supports Markdown and is inserted verbatim into the model context.

### Paths

Inside a skill, reference files relative to the **skill root** (the folder containing `SKILL.md`):

```markdown
Use the template in `templates/review.md`.
Read `templates/review.md` with `skill_read`.
```

## How a skill reaches the model

Enabling a skill does not put the whole prompt into every request. The flow is:

1. The system prompt gets an **Available skills** list with only `name` and `description`.
2. When the model decides the task needs it, it calls `use_skill`.
3. The backend appends the full body, skill root and `SKILL.md` path as internal context.
4. The session remembers the skill was loaded; it is not loaded twice.
5. `skill_read` is injected so the model can read templates/data inside the skill directory.

You can also request it explicitly:

```text
{skill:code-reviewer} Review this change.
```

The system prompt treats this as a direct request and asks the model to load the skill first.

### `skill_read`

- Actions: `read` / `list` / `search`
- Path relative to the skill root; `.` is the root
- Only enabled/loaded skill directories are accessible; `../` escapes are rejected
- `read` supports `start_line` / `end_line`
- `search` supports `query`, `recursive`, `case_sensitive`, `use_regex`, `glob`, `max_results`

## Command allowlist

`allowed-commands` is merged as a **union**: if several enabled/loaded skills allow commands, all of them enter the allowed set for the turn; comparison is case-insensitive and deduplicated.

In the current version it is enforced on `run_cmd`: the first executable name (extension stripped) must be in the allowlist, otherwise the call is rejected. `run_shell` still goes through risk assessment and confirmation and is not filtered by this list.

Empty means unrestricted. Once any enabled/loaded skill declares a non-empty allowlist, unlisted executables are rejected. Keep it tight:

```yaml
allowed-commands:
  - git
  - rg
  - npm
  - node
```

## Self-evolution mode

**Settings → Workspace & Profiles → Enable Self-Evolution Mode**:

- lets the assistant **install, extend, edit and delete skills itself** through `file_actions` — `mkdir` a folder, `write` / `patch` its `skill.md`, `move` to reorganise, `delete` to remove;
- the one skill root is `<workspace>/.data/skills` — there is no second skills location, and the assistant is told so;
- creates `<filename>.bak.N` backups before modifying or deleting anything under that root (`demo.bak.1` for a whole folder);
- marks the source as `self-evolution` in `skills.status`;
- **forces the `file_actions` and `memory` tools on** — they are locked in the Tools panel and cannot be switched off, not by the panel, not by a profile, not by a hand-edited `config.json`;
- opens a dedicated `evolution` memory scope for verified facts (see [Memory, Todos and Timers](Memory-Todos-and-Timers));
- the assistant can load a skill it just wrote with `use_skill`, even before you enable it in the panel;
- the model also receives the `mcp_self` tool and can build its own MCP servers (see [MCP Servers](MCP-Servers));
- a maintenance pass ("nap") can consolidate the skill and memory it has accumulated: while it runs the empty-state greeting is replaced by a progress line (phase, round, tool calls), a finished pass leaves a clickable report bubble, and a partial pass writes a resume note so the next pass continues instead of starting over.

This is a high-risk switch: the model can change its own instructions. Use it briefly in a controlled workspace and inspect the `.bak` files and `git diff` afterwards.

## Import, export and migration

- **Manual import**: copy a skill folder into `<workspace>/.data/skills/`; reopen the Skills panel.
- **Full backup**: title-bar `⋯` → **Export Profile…** includes `workspace/skills/` and `workspace/skills.status`; restore writes them into the current workspace.
- **Move to another workspace**: copy `.data/skills` and `skills.status`, or enable the skills again.
- **Version control**: keep skill sources in a Git repository and copy them into `.data/skills`; `.data` itself should be gitignored.

## Example: release-check skill

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

## FAQ

| Symptom | Check |
| --- | --- |
| No skills listed | Does `.data/skills` exist? Is the skill outside the workspace? |
| Enabled but the model never uses it | Is `description` specific about triggers? Does the task really need it? |
| `use_skill` says not found | Folder/frontmatter `name` mismatch; are you in the current workspace? |
| Changes do not take effect in the same session | Loaded skills are not reloaded; start a new chat or re-enable |
| Command rejected | The executable is not in the `allowed-commands` union |
| `run_cmd` refuses a program | `run_cmd` only runs PATH-resolved names — use `run_shell` for a path |
| The assistant rewrote a skill on its own | Self-evolution mode is on; check `.data/skills/*/*.bak.N` |
| The Tools panel greys out File Actions / Memory | Self-evolution mode owns these two tools; they cannot be switched off while it is on |
| Wrong name after importing a zip | The frontmatter `name` is authoritative; the folder can differ |