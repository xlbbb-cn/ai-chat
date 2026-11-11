/**
 * English strings — the source of truth for the whole i18n catalogue.
 *
 * Conventions
 * - Nested groups keep related strings together; components read them with a
 *   dot path, e.g. `t("settings.appearance.title")`.
 * - `{name}` placeholders are replaced by the values passed as the second
 *   argument of `t()`.
 * - A `{ one, other }` value selects the branch matching `t(key, { count })`
 *   (see `src/i18n/index.tsx`). Locales that do not inflect for plurals use a
 *   plain string instead.
 */
const en = {
    common: {
        save: "Save",
        saving: "Saving...",
        saveChanges: "Save Changes",
        saved: "Saved",
        cancel: "Cancel",
        close: "Close",
        collapse: "Collapse panel",
        delete: "Delete",
        edit: "Edit",
        apply: "Apply",
        applying: "Applying...",
        add: "Add",
        all: "All",
        none: "None",
        reset: "Reset",
        refresh: "Refresh",
        retry: "Retry",
        done: "Done",
        loading: "Loading...",
        browse: "Browse…",
        picking: "Picking…",
        copy: "Copy",
        copied: "Copied!",
        enable: "Enable",
        disable: "Disable",
        unnamed: "(unnamed)",
        empty: "(empty)",
    },

    app: {
        newChat: "New Chat",
        newChatTitle: "New chat",
        toolbarAgents: "Agents",
        toolbarAgentsTitle: "Sub Agents",
        toolbarAgentsTitleEnabled: "Sub Agents (Enabled)",
        toolbarSkills: "Skills",
        toolbarMcp: "MCP",
        toolbarMcpTitle: "MCP Servers",
        toolbarTools: "Tools",
        toolbarHistory: "History",
        toolbarSettings: "Settings",

        windowMinimize: "Minimize",
        windowMaximize: "Maximize",
        windowRestore: "Restore",
        windowClose: "Close",
        appMenu: "App menu",
        appMenuOpenDataDir: "Open app data directory",
        appMenuOpenWorkspaceDir: "Open workspace directory",
        appMenuOpenDeliverables: "Open deliverables folder",
        appMenuSaveProfile: "Save profile…",
        appMenuRestoreProfile: "Restore profile…",
        appMenuMarkdownEdit: "Markdown edit…",
        appMenuAbout: "About AI Chat",

        emptyStart: "Start a conversation",
        emptyHint: "Use {skills} to set a system prompt, or configure the API in {settings}.",
        dream: {
            hint: "Click to let me take a short nap",
            sleeping: "Napping while tidying skills and memory…",
            cancelHint: "Double-click to stop (not recommended)",
            statusTitle: "Tidying skills and memory…",
            statusDetail: "Dreaming Scene {round}/{tools}",
            phase: {
                inventory: "Taking stock of skills and memory",
                files: "Checking skill files",
                skills: "Merging overlapping skills",
                memory: "Consolidating memory entries",
                wrap_up: "Wrapping up the report",
            },
            resultTitle: "Maintenance finished",
            resultDismiss: "Click to dismiss",
            ariaStart: "Click to let me take a short nap and tidy skills and memory",
            ariaRunning: "Napping while tidying skills and memory; double-click to stop, not recommended",
            failed: "Memory maintenance failed: {error}",
        },

        profileExportLabel: "Exporting and compressing profile...",
        profileExportPreparing: "Preparing profile export...",
        profileExportHint: "Chat is locked during export and sending is disabled.",
        profileExportFailed: "Profile export failed: {error}",

        updateAvailable: "New version {version} is available — you are on {current}.",
        viewUpdate: "View update",
        dismiss: "Dismiss",
        noticeDismiss: "Dismiss notice",
        toolDisabledNotice:
            "`{tool}` failed {limit} times in a row — disabled for this session.",
        budgetExhaustedNotice:
            "Tool budget spent ({rounds} rounds, {toolCalls} tool calls) — the assistant is wrapping up with the material it already has.",

        timers: {
            title: "Timers",
            untitled: "timer",
            firesIn: "in {time}",
            cancel: "Cancel timer",
            otherSession:
                "Timer “{label}” fired in another chat session. Insert it here to continue that task.",
            insert: "Insert into input",
            dismiss: "Dismiss",
            sleepLock: "Sleep blocked",
            sleepLockRunning: "Sleep blocked · turn {time}",
            sleepLockHint:
                "A timer is pending or this turn is still running, so the system is kept awake.",
        },

        background: {
            cancel: "Cancel task",
            toolName: "Background",
        },

        fileMention: {
            title: "Insert file reference",
            hint: "↑↓ choose · Enter/Tab insert · #12-24 for lines, #2 for a PDF page · Esc close",
            loading: "Loading workspace files…",
            noMatches: "No file matches “{query}” for this model.",
            listFailed: "Could not list workspace files: {error}",
            readFailed: "Could not read {name}: {error}",
            rangeNotSupported:
                "{name} cannot be sliced by lines — attached the whole file.",
            pdfFailed: "Could not convert the PDF {name}: {error}",
            pdfConverting: "Converting {name} to Markdown…",
            pdfConvertingPage: "page {page} / {total}",
        },
        slashMention: {
            title: "Insert skill reference",
            hint: "↑↓ choose · Enter/Tab insert · Esc close",
            loading: "Loading workspace skills…",
            noMatches: "No skill matches “{query}”.",
            listFailed: "Could not list workspace skills: {error}",
        },
        attachmentUnsupported:
            "Skipped {names}: “{model}” cannot read that file type.",
        composer: {
            dropHint: "Drop to attach — copied to .data/tmp (or referenced in place) and inserted as @",
            busy: "The assistant is replying: add files once this turn ends.",
            tooLarge: "{name} is too large to attach (24 MB limit).",
            empty: "{name} has no content (the clipboard provided no file data).",
            unreadable: "Could not read {name}: {error}",
            saveFailed: "Could not save {name}: {error}",
            dropFailed: "Could not attach the dropped file: {error}",
        },
        messageRenderFailed: "This message could not be rendered: {error}",
        inputPlaceholder: "Type a message… (@ to reference a file, / for a skill, Enter to send)",
        injections: {
            title: "Added mid-turn",
            queued: "Waiting to be delivered",
            placeholder:
                "The assistant is working — type an update to add it mid-turn (Enter to add)",
        },
        selectModel: "Select model",
        reasoningDepth: "Thinking depth",
        reasoningDepthTitle:
            "How much the model should think before answering — sent as reasoning_effort.",
        reasoningDefault: "Default",
        reasoningLow: "Low",
        reasoningMedium: "Medium",
        reasoningHigh: "High",
        send: "Send",
        stop: "Stop",
        contextUsage: "Context usage: {percent}%",
        contextUsageWithCache: "Context usage: {percent}% · cache hit {cachePercent}%",

        generationStopped: "[Generation stopped]",
        sessionSwitchBlocked:
            "A reply is currently being generated. Please stop it before switching history sessions.",
        deleteFailed: "Failed to delete message: {error}",
        forkFailed: "Failed to fork session: {error}",
        markdownPathEmpty: "Markdown file path is empty.",
        markdownSaveFailed: "Failed to save markdown file: {error}",
        continueUnfinished:
            "Continue the unfinished reply above without repeating what you already generated.",

        markdownEditorAria: "Markdown editor",
        markdownEditorTitle: "Markdown Edit",
        markdownColumn: "Markdown",
        previewColumn: "Preview",
        markdownPlaceholder: "Write markdown here...",
        markdownPreviewEmpty: "Markdown preview will appear here.",

        confirm: {
            titleSudo: "Privileged operation (sudo)",
            titleElevation: "Privileged operation (administrator)",
            titleExternalAgent: "External agent request",
            titleRisk: "Risk Level {level} — confirm execution",
            titleDangerous: "Dangerous command detected",
            badgeSudo: "SUDO",
            badgeAdmin: "ADMIN",
            badgeExternalAgent: "AGENT",
            badgeRisk: "RISK",
            badgeDangerous: "DANGEROUS",
            reason: "Reason:",
            type: "Type:",
            score: "Score: {score}/100",
            blacklistHits: "Blacklist hits",
            penaltyItems: "Penalty items",
            username: "Username (optional)",
            usernamePlaceholder: "leave blank for current user",
            password: "Password",
            passwordPlaceholder: "required",
            sudoHint: "This will run with elevated privileges and may modify your system.",
            elevationHint:
                "This will request administrator elevation (UAC) and may modify system settings.",
            externalAgentHint:
                "This CLI / cloud agent runs outside the app: a CLI runs with your user permissions in the workspace, a cloud agent runs the task on a remote endpoint. It is approved once per agent for this app run.",
            deny: "Deny",
            allow: "Allow",
        },

        agents: {
            planner: "Planner",
            selector: "Selector",
            aggregator: "Aggregator",
            tool: "Tool",
            planDone: "Plan completed — {count} tasks total",
            plannerMonitoring: "Planner monitoring — {count} tasks",
            plannerActionSchedule: "Planner scheduled tasks",
            plannerActionSend: "Planner request → {target}",
            plannerActionStatus: "Planner checked agent status",
            plannerActionHistory: "Planner read bus history",
            plannerActionSubscribe: "Planner subscribed to {target}",
            plannerActionUnsubscribe: "Planner unsubscribed from {target}",
            plannerActionDirect: "Planner directive → {target}",
            plannerActionEndTask: "Planner ended {target}",
            plannerActionFinish: "Planner finished supervision",
            plannerActionUnknown: "Planner action: {action}",
            selectionDone: {
                one: "Selected {count} agent — {names}",
                other: "Selected {count} agents — {names}",
            },
            aggregated: {
                one: "Aggregated {count} subtask result",
                other: "Aggregated {count} subtask results",
            },
            loadingSkill: "Loading skill: {name}",
        },
    },

    chat: {
        thoughtProcess: "Thought Process",
        tokenUsage: "Token usage",
        retryTitle: "Retry this unfinished user message",
        exportTitle: "Export this reply to PDF",
        deleteTitle: "Delete this message",
        forkTitle: "Fork conversation from this message",
        injected: "Added mid-turn",
        foldMore: "Show more",
        foldLess: "Show less",
        continue: "Continue",
    },

    figure: {
        title: "Figure {index}",
        drawing: "Drawing…",
        save: "Save PNG",
        saving: "Saving…",
        saved: "Saved · {path}",
        saveFailed: "Save failed",
        resetView: "Reset view",
        saveAll: "Save all figures",
        openFolder: "Open figures folder",
        webglUnavailable: "3D needs WebGL, which is unavailable in this environment.",
    },

    toolGroup: {
        completed: { one: "{count} tool call completed", other: "{count} tool calls completed" },
        completedWithErrors: {
            one: "{count} step: {done} completed, {errors} failed",
            other: "{count} steps: {done} completed, {errors} failed",
        },
        completedWithSkipped: {
            one: "{count} step: {done} completed, {skipped} skipped",
            other: "{count} steps: {done} completed, {skipped} skipped",
        },
        steps: { one: "{count} step", other: "{count} steps" },
        running: "{running} running · {done} completed",
        thinkingLive: "Thinking · {count} chars · {seconds}s",
        waitingOutput: "Waiting for output · {seconds}s",
        confirmTitle: "Allow this shell command?",
        rejected: "Rejected by user",
        mainAgentHelp: "Main agent",
        mainAgentHelpChars: "→ {count} chars",
        mainAgentHelpFailed: "Main-agent help failed",
        mbusCounts: "mbus · {sent} sent · {received} received",
    },

    backgroundGroup: {
        title: "Background",
        running: "{count} running",
        waiting: "{count} waiting",
        serves: "Serve processes",
        tasks: "Background tasks",
        hint: "Waiting for a background result…",
        cancelTask: "Cancel task",
        stopServe: "Stop",
        serveMeta: "pid {pid} · {uptime}",
    },

    settings: {
        aria: "Settings",
        title: "Settings",
        subtitle: "Manage appearance, workspace, API and runtime options.",
        navAria: "Settings sections",

        sections: {
            appearance: "Appearance",
            workspace: "Workspace & Profiles",
            api: "API & Model",
            system: "System Message",
            advanced: "Advanced",
            runtime: "Runtime & Debug",
            about: "About & Updates",
        },

        appearance: {
            title: "Appearance",
            subtitle: "Pick how the interface follows your system theme.",
            colorMode: "Color mode",
            themeAuto: "Auto",
            themeLight: "Light",
            themeDark: "Dark",
            palette: "Color scheme",
            paletteDefault: "Default",
            paletteSakura: "Sakura",
            paletteCream: "Cream",
            paletteHint: "Applies immediately and is stored in your configuration.",
            glass: "Frosted glass",
            glassOff: "Off",
            glassSubtle: "Subtle",
            glassStandard: "Standard",
            glassStrong: "Strong",
  glassHint: "Blur strength for the titlebar, toolbar and panel headers. Off makes those bars opaque instead of blurred — cheaper on weak GPUs and remote sessions. On Windows the level also makes the window frame translucent, so the blurred desktop shows through it (Acrylic); the transcript stays solid.",
            language: "Language",
            languageHint: "Applies immediately and is stored in your configuration.",
        },

        workspace: {
            title: "Workspace & Profiles",
            subtitle: "Directory for skills, tools and files, plus saved configuration profiles.",
            dirLabel: "Workspace directory",
            defaultDir: "Default workspace directory",
            browseTitle: "Browse for a folder",
            reset: "Reset",
            resetTitle: "Use the default workspace directory",
            resetAria: "Reset workspace directory to default",
            historyPlaceholder: "Recent workspaces…",
            historyTitle: "Switch to a workspace you used before",
            selfEvolution: "Enable Self-Evolution Mode",
            selfEvolutionHint:
                "Let the assistant evolve its own skills inside the workspace. The file actions and memory tools stay enabled while the mode is on, and the assistant keeps verified facts (runtime environment, correct tool usage, file locations) in a dedicated evolution memory. The assistant may also build its own MCP servers (`mcp_self`) — scripts under .data/mcp, started as local processes after your confirmation.",
            profilesTitle: "Configuration Profiles",
            profileNamePlaceholder: "Profile name",
            saveCurrent: "Save Current",
            profileMeta: "{tools} tools · {agents} agents",
            applyTitle: "Apply this profile",
            deleteAria: "Delete profile {name}",
            instancePrimaryHint:
                "“Run a single instance” is off: launching AI Chat again opens a separate window with its own workspace and its own copy of this configuration.",
            instanceShadowHint:
                "This window runs on a temporary copy of the configuration — its workspace and settings are its own, and changes made here are discarded when you close it.",
        },

        api: {
            title: "API & Model",
            subtitle: "Connect any OpenAI-compatible endpoint and pick a model.",
            baseUrl: "Base URL",
            apiKey: "API Key",
            model: "Model",
            modelCatalog: "Model catalog",
            fetchModels: "Fetch Models From API",
            loadingModels: "Loading models…",
            addModelManually: "Add model manually",
            addModelPlaceholder: "gpt-4.1-mini",
            addModelTitle: "Add this model to the catalog",
            contextWindow: "Context window for “{model}” (tokens)",
            contextPlaceholder: "auto (provider-reported or default)",
            contextHint:
                "Auto-filled when the API reports a context window (OpenRouter, Groq, vLLM…), and otherwise from the matched llm-metadata catalogue entry. OpenAI-compatible endpoints that expose neither can be configured manually here — the value is used for agent context budgeting.",
            capabilities: "Model capabilities",
            capabilitiesEmpty:
                "No capability data for “{model}” yet — use “Fetch Models From API” to load it.",
            capabilitiesMatched: "matched “{name}” · {percent}% similar",
            capabilitiesSummary: "Capability metadata matched for {matched}/{total} models.",
            capabilitiesError: "Capability metadata unavailable: {error}",
            capabilityTools: "Tools",
            capabilityReasoning: "Thinking",
            capabilityVision: "Vision",
            capabilityFiles: "Files",
            capabilityAudio: "Audio",
            capabilityOpenWeights: "Open weights",
            capabilityDeprecated: "Deprecated",
            capabilityContext: "{tokens} context",
            conversationHeader: "Conversation ID header",
            conversationHeaderHint:
                "Header name that carries the chat session id in every request. Leave empty to use the default “x-conversation-id”.",
            customHeaders: "Custom request headers",
            customHeadersHint:
                "Extra headers sent with every model request — chat turns, sub-agent planning and the model listing. Blank rows are ignored, invalid names or values are skipped, and an “Authorization” row replaces the API key.",
            customHeaderName: "Header name",
            customHeaderValue: "Value",
            addHeader: "Add header",
            removeHeader: "Remove header",
        },

        system: {
            title: "System Message",
            subtitle: "Instructions sent to the model at the start of every conversation.",
            content: "Content",
            placeholder: "You are a helpful assistant…",
        },

        advanced: {
            title: "Model Advanced Settings",
            subtitle: "Sampling parameters — leave empty to use API defaults.",
            temperature: "Temperature",
            topP: "Top P",
            maxCompletionTokens: "Max Completion Tokens",
            placeholderDefault: "default",
            placeholderDefaultLimit: "default limit",
            maxRounds: "Max rounds per turn",
            maxRoundsHint:
                "Model requests one chat turn may make. 0 = unlimited (hard cap 1000). When the budget is spent the turn sends a final request without tools, so the assistant answers with what it already has.",
            maxToolCalls: "Max tool calls per turn",
            maxToolCallsHint:
                "Tool calls executed in one chat turn. 0 = unlimited (hard cap 5000). Calls beyond it are answered with a notice instead of running.",
            toolFailureLimit: "Disable a tool after N failures",
            toolFailureLimitHint:
                "A tool (MCP tools included) that fails this many times in a row is disabled for the rest of the session and the model is told to switch approach. 0 = never disable (hard cap 50). One successful call resets the count.",
            dsFormat: "DS-Format (DeepSeek)",
            dsFormatHint: "Pass reasoning_content back on assistant messages as DeepSeek-family thinking models require. DeepSeek-family models enable this automatically; turn it on only for other endpoints that need the same pass-through.",
            dsFormatAuto: "Always on for {model}: DeepSeek-family models get DS-Format automatically, so reasoning_content is always passed back.",
        },

        runtime: {
            title: "Runtime & Debug",
            subtitle: "Execution behavior, where logs are written, how long they are kept, and how requests are monitored.",
            loggerOutput: "Logger Output (debug build only)",
            loggerFile: "Write to app.log",
            loggerPrintln: "Print to terminal (println)",
            retention: "Log retention (days)",
            retentionHint:
                "Request and interaction logs older than this are deleted at startup and by the Interaction Monitor's “Compact DB”. 0 keeps logs forever. Chat history is never pruned. Change it and press Save to apply.",
            singleInstance: "Run a single instance",
            singleInstanceHint:
                "Launching AI Chat again brings the running window to the front instead of opening a second copy. When off, every launch gets its own window on a temporary copy of this configuration. Applies after restarting the app.",
            backgroundTools: "Background execution",
            backgroundToolsHint:
                "Let the assistant run slow tools as background tasks (`run_in_background: true`): `run_cmd`, `run_shell`, `file_actions` search, every MCP tool and `call_subagent`. The result is delivered automatically before the next request and the turn stays open until it arrives. Sub-agents never receive the flag.",
            subAgentMainHelp: "Sub-agent main-agent help",
            subAgentMainHelpHint:
                "Let `llm` sub-agents call `ask_main_agent` while they run. A bounded helper run with the main model, the real conversation history, enabled skills, MCP servers and the main tool set collects the material and hands it back, then the sub-agent continues its own task. `cli`/`cloud` agents cannot use it.",
            interactionMonitor: "Interaction monitor",
            agentMissions: "Agent missions monitor",
            evolutionMemory: "Evolution memory",
            launchMonitor: "Launch Monitor",
        },

        about: {
            title: "About & Updates",
            subtitle: "Running version, plus update checks against this repository's GitHub releases.",
            currentVersion: "Current version",
            checkOnStartup: "Check for updates on startup",
            checkOnStartupHint:
                "Queries the GitHub Releases API once per launch and shows a banner when a newer version exists.",
            prerelease: "Include pre-releases",
            prereleaseHint:
                "Also consider releases flagged as pre-release. Draft releases are never visible to the API.",
            updates: "Updates",
            checkNow: "Check for updates…",
        },

        messageEditor: {
            aria: "Edit system message",
            title: "System Message Editor",
            placeholder: "Write your system message in Markdown...",
        },

        agentMissionsMonitorAria: "Agent missions monitor",
        evolutionMemoryMonitorAria: "Evolution memory monitor",
    },

    memoryPanel: {
        title: "Evolution memory",
        summary: "{count} entries · {injected} in the top tier",
        budget: "{chars} / {budget} chars in top-tier facts",
        loading: "Loading…",
        empty: "No evolution memories yet. They are recorded while self-evolution mode is on.",
        pinned: "pinned",
        auto: "auto",
        evidence: "{count}× confirmed",
        uses: "{count}× used",
        archivedTitle: "Archived into skills ({count} entries)",
        archivedMeta: "{entries} entries · {chars} chars",
        archivedMissing: " · skill file missing",
        detailFailed: "Could not load the entry.",
    },

    skills: {
        title: "Skills",
        empty: "No skills yet. Click the button below to create one.",
        newSkill: "+ New Skill",
        name: "Name",
        namePlaceholder: "e.g. code-reviewer",
        description: "Description",
        descriptionPlaceholder: "Short description and trigger condition",
        systemPrompt: "System Prompt",
        systemPromptPlaceholder: "You are a helpful assistant that…",
        version: "Version",
        versionPlaceholder: "e.g. 1.0.0 (optional)",
        allowedCommands: "Allowed Commands",
        allowedCommandsPlaceholder: "e.g. curl, wget, git  (empty = unrestricted)",
        allowedCommandsHint: "Comma-separated executable names. Leave empty to allow all.",
        promptEditor: {
            aria: "Edit system prompt",
            title: "System Prompt Editor",
            placeholder: "Write your system prompt in Markdown...",
        },
    },

    tools: {
        title: "Tools",
        builtIn: "Built-In Tool Set",
        autoAccept: "AutoAccept Mode",
        lockedBadge: "Required",
        lockedBySelfEvolution:
            "Required by self-evolution mode — it cannot be switched off.",
        items: {
            fileActions: {
                name: "File Actions",
                description: [
                    "Read, write, list, search, patch, rename, move, create, and delete files only inside the current workspace root — including the `.data` state folder (skills, scripts, memory, todos).",
                    "Use `./...` paths such as `./src/App.tsx`, `./.data/skills/demo/skill.md`, or `.` for the workspace root.",
                    "Paths such as `workspace/...` and `../...` are rejected. The workspace database (`.data/chat.db`) is managed by the app and cannot be modified through this tool.",
                ].join("\n"),
            },
            runCmd: {
                name: "Run Command",
                description: [
                    "Run a program or a workspace script directly (without a shell). The process starts in the workspace root.",
                    "Action `run` (default) executes `command`. Its first word is resolved against the workspace script library `./.data/scripts` by bare name with the extension optional (`build-frontend`, `deploy.py`); otherwise it must be a bare program name that PATH resolves (`python`, `git`, `curl`). The library is prepended to the child's PATH, so a script wins over a program with the same name. Absolute and relative paths are rejected; use Run Shell when a path is really needed.",
                    "Action `list` shows the library scripts with their interpreter, `@description`, `@usage` and use count. Description and usage are read only from header comments (`#`, `//`, `REM` or `::`). A script whose body contains destructive or sensitive patterns asks for confirmation once; the approval is remembered for that exact content until the file changes.",
                    "Privileged operations and dangerous commands require explicit confirmation.",
                ].join("\n"),
            },
            runShell: {
                name: "Run Shell",
                description: [
                    "Execute a script in a shell (PowerShell or Bash). The script has full freedom: it starts in the workspace root but may change directory anywhere and run anything, and every script is audited by the risk engine before it runs.",
                    "Supports pipes, loops, variables, and other shell features. `direct` blocks until the script exits and returns its output; `serve` starts a long-lived background process (for example an HTTP server), returns its id immediately, and supports list / stop, with automatic cleanup when the app exits.",
                    "Privileged operations and dangerous commands require explicit confirmation.",
                ].join("\n"),
            },
            memory: {
                name: "Memory",
                description: [
                    "Persistent memory system with three scopes for storing notes and information across conversations.",
                    "",
                    "**session** — Short-term, in-memory only. Survives for the current chat session. Use for task-specific context and in-progress notes.",
                    "**user** — Long-term, file-backed. Cross-session persistent. Use for user preferences, patterns, and general insights.",
                    "**repo** — Long-term, file-backed. Repository-scoped. Use for codebase conventions, build commands, and project facts.",
                    "**evolution** — Long-term, file-backed. The self-evolution area, available while self-evolution mode is on. Use for verified facts that must survive across sessions: the default interpreter and runtime environment, the correct way to operate a tool, the file locations you use often. Each entry carries a title, kind, tags, importance and an evidence count, and the scope is queried on demand (never preloaded into the prompt) in ranked order — keep each entry to one dense fact.",
                    "",
                    "Supports add, get, list, search, delete, and consolidate. `consolidate` folds a large related cluster of evolution entries into a skill archive (plan first, then apply with the full skill markdown and the same keys).",
                ].join("\n"),
            },
            todoList: {
                name: "Todo List",
                description:
                    "Track complex multi-step work with a session-scoped todo list. The assistant can add items, update statuses, check progress, clear completed items, and archive finished lists.",
            },
            timer: {
                name: "Timer",
                description: [
                    "Schedule a one-shot delay timer so the assistant resumes a task later instead of polling for it.",
                    "Use it when a step must wait for something slow — a 30-minute log collection, a long build or download, an external job.",
                    "When the timer fires, its message is injected into the same chat as a new user turn, so the assistant continues with the full conversation history. Pending timers show up above the input box with a countdown and a cancel button.",
                ].join("\n"),
            },
        },
        autoAcceptItems: {
            all: {
                label: "Auto-approve everything",
                description:
                    "One click: skip every approval dialog, including confirmation kinds added in future versions.",
            },
            dangerous: {
                label: "Dangerous commands (L0)",
                description:
                    "Skip the approval prompt for L0 system-risk run_cmd / run_shell requests (risk score ≥ 85).",
            },
            systemConfig: {
                label: "System configuration changes (L1)",
                description:
                    "Skip the approval prompt for L1 requests that modify system configuration or system software (score 70–84).",
            },
            userSoftware: {
                label: "User software changes (L2)",
                description:
                    "Skip the approval prompt for L2 requests that install, uninstall, or modify user applications (score 55–69).",
            },
            userData: {
                label: "User data changes (L3)",
                description:
                    "Skip the approval prompt for L3 requests that modify or delete user data files (score 40–54).",
            },
        },
    },

    mcp: {
        title: "MCP Servers",
        addTitle: "Add MCP Server",
        editTitle: "Edit MCP Server",
        description:
            "Model Context Protocol (MCP) servers extend the AI with external tools and data sources.",
        empty: "No servers configured yet.",
        selfBuilt: "self-built",
        selfBuiltHint:
            "Built by the assistant in self-evolution mode. Stored in the workspace roster .data/mcp.conf, merged with this list. Script: {path}",
        addServer: "+ Add Server",
        name: "Name",
        namePlaceholder: "My MCP Server",
        transport: "Transport",
        transportStdio: "stdio (subprocess)",
        transportSse: "SSE (HTTP + Server-Sent Events)",
        transportHttp: "HTTP (plain JSON-RPC)",
        transportStreamHttp: "Streamable HTTP (recommended)",
        command: "Command",
        commandPlaceholder: "npx / python / ./server",
        arguments: "Arguments",
        argumentsHint: "(space-separated)",
        argumentsPlaceholder: "-m my_mcp_server --port 8080",
        envVars: "Environment Variables",
        envVarsHint: "(KEY=value, one per line)",
        url: "URL",
        authToken: "Auth Token",
        authTokenHint: "(optional, Bearer)",
        testConnection: "Test connection",
        viewLogs: "View diagnostic logs",
        testing: "testing…",
        testingTitle: "Testing connection…",
        toggleTitle: "Click to {action} {name}",
        logsTitle: "Logs: {name}",
        cancelTest: "⏹ Cancel Test",
        testInProgress: "Test in progress — logs auto-refresh every 1.5s",
        noLogs: "No log entries yet. Run a test to populate.",
        logCount: {
            one: "{count} entry (auto-refresh every 1.5s)",
            other: "{count} entries (auto-refresh every 1.5s)",
        },
        logPlaceholder: "Diagnostic info will appear here once you run a test.",
    },

    agents: {
        title: "Sub Agents",
        orchestration: "Orchestration settings",
        enableMode: "Automatic orchestration",
        autoConfigure: "Smart agent selection",
        mbus: "Mission bus (mbus)",
        mbusHint:
            "Let the planner supervise dispatched sub-agents and let agents exchange messages. Adds planner model calls while tasks run.",
        mbusPairLimit: "Messages per agent pair",
        mbusPairLimitHint:
            "Both directions combined. When two agents reach this limit they can no longer message each other and the planner is asked to coordinate them (0 = no limit).",
        executionMode: "Execution mode",
        modeParallel: "Parallel",
        modeSequential: "Sequential",
        maxConcurrency: "Max concurrency",
        empty: "No agents yet. Click the button below to create one.",
        newAgent: "+ New Agent",
        typeLabel: "Agent type",
        typeLlm: "LLM",
        typeCli: "CLI",
        typeCloud: "Cloud",
        typeHint:
            "LLM runs an autonomous tool-using loop inside the app. CLI launches a headless coding CLI installed on this machine. Cloud sends the task to a remote HTTP endpoint.",
        searchPlaceholder: "Search agents by name, role or category",
        filterByStatus: "Filter agents by enablement",
        filterAll: "All",
        filterEnabled: "Enabled",
        filterDisabled: "Disabled",
        enabledCount: "{enabled} of {total} enabled",
        groupCustom: "Custom",
        groupUncategorized: "Uncategorized",
        rescan: "Refresh agent index",
        openFolder: "Open agents folder",
        noResults: "No agents match these filters.",
        sourceFile: "Definition file: agents/{file}",
        definitionStorageHint:
            "LLM agent definitions live in app_data/agents — name, description, emoji, color and the prompt are read from the .md file; runtime settings stay in sub_agents.json.",
        emoji: "Emoji",
        emojiHint:
            "Stored in the frontmatter and shown next to the name (content, not a UI icon).",
        color: "Color",
        colorHint: "Stored in the frontmatter; not painted as a UI accent.",
        enableMcp: "MCP tools",
        enableMcpHint:
            "Inject the tools of every enabled MCP server. This agent starts its own server processes for the task and stops them when the task ends (never shared with the main agent).",
        typeLockedHint:
            "The agent type is fixed after creation; create a new agent to use another backend.",
        deleteCustomConfirm:
            'Delete agent "{name}"? Its markdown file is removed from agents/custom (a .bak copy is kept).',
        parseError: "Definition error: {error}",
        name: "Name",
        namePlaceholder: "e.g. code-analyzer",
        description: "Description",
        descriptionPlaceholder: "Briefly describe this agent's role",
        systemPrompt: "System prompt",
        systemPromptPlaceholder: "You are an expert in... focused on...",
        modelLimits: "Model & limits",
        model: "Model (optional, overrides main config)",
        modelPlaceholder: "Leave empty to inherit the main model",
        modelHint: "Overrides the main model for this agent's LLM requests.",
        contextWindow: "Context window (tokens)",
        contextWindowPlaceholder: "Default 131072",
        contextWindowHint:
            "Stored per model id and shared with the main chat. Empty = default (131072 for agents). The conversation is compressed at 80% of this value.",
        maxTokens: "Max completion tokens",
        maxTokensPlaceholder: "Default completion limit 8192",
        temperature: "Temperature",
        temperaturePlaceholder: "Inherit default",
        maxIterations: "Max iter",
        maxIterationsHint:
            "0 (default) means no iteration limit; mission completion is controlled by external task state.",
        capabilities: "Capabilities",
        allowedTools: "Allowed tools",
        statusRunning: "Running",
        statusDone: "Done",
        statusError: "Error",
        promptEditor: {
            aria: "Edit system prompt",
            title: "System Prompt Editor",
            placeholder: "Write your system prompt in Markdown...",
        },
        toolHints: {
            fileActions: "Read, write, patch and search files inside the workspace",
            runCmd: "Execute a single command or workspace script with risk assessment",
            runShell: "Run PowerShell / bash scripts (direct or background serve)",
            todos:
                "The session todo list via the `todos` tool (list / add / update_status / clear_completed / archive). The plan is shared with the chat session and every parallel agent.",
            memory: "Shared session / user / repo memory store (every sub-agent reads and writes the same one)",
        },

        // ── cli backend ──
        cliSection: "CLI settings",
        cliCommand: "Command",
        cliCommandPlaceholder: "opencode",
        cliCommandHint:
            "Executable name (searched on PATH) or a full path. Windows .cmd/.bat/.ps1 launchers are handled automatically.",
        cliArgs: "Arguments — one per line",
        cliArgsHint:
            "One argument per line; placeholders: {prompt}, {cwd}, {agent_name}. Example: `run` + `{prompt}` runs `opencode run \"…\"`. If {prompt} is missing, the prompt is appended as the last argument. Leave empty to use the built-in headless form (opencode: `run`, claude/gemini/cursor-agent/copilot: `-p`, codex/droid: `exec`, crush: `run`, amp: `-x`).",
        cliPromptVia: "Prompt delivery",
        cliPromptViaArg: "As an argument ({prompt})",
        cliPromptViaStdin: "Through stdin",
        cliPromptViaHint:
            "Use stdin for CLIs that read piped input, and on Windows when a prompt could contain %VAR% text.",
        cliModelArgs: "Model arguments — one per line",
        cliModelArgsHint:
            "Appended to every run, after the arguments above. One argument per line — put the flag and its value on separate lines, e.g. `-m` then `opencode-go/deepseek-v4.1-flash`. The model is not inherited from the main config.",
        cliEnv: "Environment — KEY=VALUE per line",
        cliEnvHint:
            "ANSI colour codes are stripped from captured output. Children start with NO_COLOR=1, CLICOLOR=0, CLICOLOR_FORCE=0, FORCE_COLOR=0 and TERM=dumb; entries above override those defaults.",
        timeout: "Timeout (seconds)",
        timeoutHint:
            "0 = no timeout: the run keeps going until it finishes, the turn is stopped, or the app exits. Positive values are capped at 7200 seconds. Leaving the field empty keeps the default (600).",
        noCommand: "No command configured",
        inheritModel: "Inherits the main model",

        // ── cloud backend ──
        cloudSection: "Cloud settings",
        cloudEndpoint: "Endpoint URL",
        cloudEndpointPlaceholder: "https://api.example.com/agent",
        cloudToken: "Token",
        cloudTokenPlaceholder: "optional",
        cloudTokenHint: "Sent as `Authorization: Bearer <token>`.",
        cloudHeaders: "Extra headers — KEY=VALUE per line",
        cloudBodyTemplate: "Request body template (JSON)",
        cloudBodyTemplateHint:
            "JSON sent as the POST body. Placeholders {prompt}, {agent_name} and {cwd} are inserted already JSON-escaped.",
        cloudResultPath: "Result path",
        cloudResultPathHint:
            "Dot path into the response holding the agent's text, e.g. `choices.0.message.content` or `result.output`.",
        cloudPollUrl: "Poll URL (optional)",
        cloudPollUrlHint:
            "For asynchronous jobs: poll this URL (use `{id}` for the job id) until the status matches. Leave empty for a single response.",
        cloudPollIdPath: "Job id path",
        cloudPollStatusPath: "Status path",
        cloudPollInterval: "Poll interval (s)",
        cloudPollDoneValues: "Done status values — one per line",
        cloudPollFailedValues: "Failed status values — one per line",
        cloudUnreachable: "unreachable",
        cloudTimeoutHint:
            "0 = no overall timeout: polling continues until the job finishes or the turn is stopped. Each HTTP request is still capped at 300 seconds, positive values at 7200. Leaving the field empty keeps the default (1800).",

        // ── shared ──
        requireApproval: "Ask before running",
        requireApprovalHint:
            "When on, this agent asks for approval on its first run per app session; off means the enabled switch is the standing authorization.",
        probe: "Test",
        probing: "Running…",
        probeCliHint: "Runs `<command> --version` to verify the executable resolves.",
        probeCloudHint: "Sends a GET request to check the endpoint is reachable.",
        noEndpoint: "No endpoint configured",
    },

    history: {
        title: "History",
        searchPlaceholder: "Search sessions by keyword",
        searchAria: "Search history sessions",
        switchHint: "A reply is being generated, switching sessions is temporarily unavailable",
        tabAll: "All",
        tabFavorites: "Favorites",
        tabArchived: "Archived",
        empty: "No history yet.",
        noMatch: "No sessions matched \"{keyword}\".",
        noFavorites: "No favorite sessions yet.",
        noArchived: "No archived sessions.",
        renameAria: "Rename session",
        messageCount: { one: "{count} message", other: "{count} messages" },
        current: " · current",
        archived: " · archived",
        loading: "Loading…",
        created: "Created {date}",
        addFavorite: "Add to favorites",
        removeFavorite: "Remove from favorites",
        rename: "Rename session",
        archive: "Archive session",
        unarchive: "Unarchive session",
        delete: "Delete session",
        titleEmpty: "(empty)",
        titleAttachment: "(attachment)",
    },

    update: {
        aria: "Software update",
        title: "Software Update",
        subtitleInstalled: "Installed v{version} · {platform}",
        subtitleUnknown: "Checks the newest published GitHub release for this repository.",
        checkAgain: "Check again",
        checking: "Checking…",
        statusChecking: "Checking for updates…",
        latest: "You are on the latest published version (v{version})",
        latestWithDate: "You are on the latest published version (v{version}), released {date}.",
        available: "Version {version} is available",
        prerelease: "pre-release",
        recommended: "recommended",
        noPlatformAsset:
            "This release has no bundle for {platform}. Choose a file below or open the release page.",
        download: "Download",
        downloading: "Downloading…",
        noAssets: "No downloadable files attached to this release.",
        runInstaller: "Run installer",
        showInFolder: "Show in folder",
        releaseNotes: "Release notes",
        noNotes: "This release has no notes.",
        openReleasePage: "Open release page",
        draftsSkipped: "Draft releases are not published and are skipped.",
    },

    monitor: {
        aria: "Interaction Monitor",
        title: "Interaction Monitor",
        autoRefresh: "Auto Refresh",
        clearLogs: "Clear logs",
        clearing: "Clearing…",
        clearTitle: "Delete all request and interaction logs (chat history is kept)",
        clearConfirm: "Delete all request and interaction logs? Chat history is not affected.",
        compact: "Compact DB",
        compacting: "Compacting…",
        compactTitle: "Delete logs older than 90 days and reclaim disk space",
        compactConfirm:
            "Compact the database? Logs older than the retention window (Settings → Runtime & Debug) are deleted and the file is rewritten to reclaim space. Chat history is kept.",
        compacting2: "Compacting… this can take a few seconds",
        interactions: "Interactions ({count})",
        cacheChip: "Cache {percent}%",
        thinkingChip: "thought {duration}",
        cacheAvg: "avg cache {percent}%",
        noInteractions: "No interactions recorded",
        selectPrompt: "Select an interaction to view details",
        details: "Details",
        type: "Type",
        actor: "Actor",
        action: "Action",
        timestamp: "Timestamp",
        duration: "Duration",
        error: "Error",
        input: "Input",
        output: "Output",
        metadata: "Metadata",
        removed: {
            one: "Removed {count} log row — compact to free the space",
            other: "Removed {count} log rows — compact to free the space",
        },
        clearFailed: "Clear failed: {error}",
        compactFailed: "Compact failed: {error}",
        compacted: "Compacted: {before} → {after}",
        freed: " (freed {bytes})",
        pruned: " · pruned {count} expired rows",
        window: " · window {days}d",
        retentionDisabled: " · retention disabled",
        report: {
            aria: "Session report",
            title: "Session report",
            hint: "Aggregated from this session's request and interaction logs",
            empty: "No requests recorded for this session yet",
            totalTokens: "Total tokens",
            tokenSplit: "{prompt} prompt · {completion} completion",
            cacheHit: "Cache hit",
            cacheMiss: "Cache miss",
            cacheReported: "{hits} of {reported} requests reported a cache",
            noCacheData: "Provider reports no cache metrics",
            context: "Context used",
            contextDetail: "{used} / {window}",
            contextUnknown: "Context window unknown — set it in Settings → Model",
            requests: "Requests",
            requestDetail: "{failed} failed · avg {avg}",
            reasoning: "Thinking per round",
            reasoningDetail: "{count} rounds · {total} in total · first token {ttft}",
            reasoningNone: "Model answers without reasoning",
            toolCalls: "Tool calls",
            toolDetail: "{failed} failed · avg {avg}",
            toolRanking: "Busiest tools",
            noTools: "No tool calls in this session",
            toolRow: "{count} calls",
            toolFailures: "{count} failed",
        },
    },

    mission: {
        aria: "Mission monitor",
        title: "Mission Monitor",
        subtitle:
            "Track external mission state, active tasks, and episodic summaries for this chat session.",
        autoRefresh: "Auto refresh",
        refresh: "Refresh",
        openExchangeFolder: "Deliverables folder",
        openExchangeFolderHint:
            "Open the sub-agent deliverable exchange area (.data/tmp) for manual cleanup.",
        missions: "Missions ({count})",
        empty: "No mission state recorded for this session yet.",
        activeTasks: { one: "{count} active task", other: "{count} active tasks" },
        mission: "Mission",
        agent: "Agent",
        status: "Status",
        created: "Created",
        updated: "Updated",
        context: "Context",
        activeTasksLabel: "Active Tasks",
        mainAgentHelp: "Main-agent help",
        messageBus: "Message bus (mbus)",
        noBusMessages: "No bus messages recorded for this mission batch.",
        episodicSummary: "Episodic Summary",
        finalReport: "Final Report",
        noActiveTasks: "No active tasks.",
        selectPrompt: "Select a mission to inspect its external state.",
        statusCompleted: "completed",
        statusRunning: "running",
        statusFailed: "failed",
        statusStopped: "stopped",
    },

    profile: {
        title: "Profiles",
        newPlaceholder: "New profile name",
        saveCurrent: "Save Current Config",
        empty: "No profiles saved",
        metaTools: { one: "{count} tool", other: "{count} tools" },
        metaAgents: { one: "{count} agent", other: "{count} agents" },
        metaMcp: { one: "{count} MCP server", other: "{count} MCP servers" },
        updated: "Updated: {date}",
        deleteConfirm: "Delete profile \"{name}\"?",
    },
} as const;

/** A pluralised message: `one` is used when `t()` receives `count: 1`. */
export interface PluralValue {
    one: string;
    other: string;
}

/**
 * Recursively widens a message tree so a locale only has to satisfy the
 * *shape* of the English catalogue, not its literal string values.
 *
 * Pluralised entries accept either form: a locale without plural inflection
 * can supply a single plain string.
 */
export type LocaleMessages<T> = {
    [K in keyof T]: T[K] extends string
    ? string
    : T[K] extends PluralValue
    ? string | PluralValue
    : LocaleMessages<T[K]>;
};

export type Messages = LocaleMessages<typeof en>;

/** Every dot path of the catalogue, e.g. `"settings.appearance.title"`. */
export type Leaves<T> = {
    [K in keyof T & string]: T[K] extends string | PluralValue
    ? K
    : `${K}.${Leaves<T[K]>}`;
}[keyof T & string];

export type MessageKey = Leaves<Messages>;

export default en;
