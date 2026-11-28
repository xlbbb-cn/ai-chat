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
        add: "Add",
        all: "All",
        none: "None",
        reset: "Reset",
        refresh: "Refresh",
        retry: "Retry",
        done: "Done",
        loading: "Loading...",
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
        appMenuOpenDeliverables: "Open wsdata folder",
        appMenuSaveBackup: "Back up…",
        appMenuRestoreBackup: "Restore backup…",
        appMenuMarkdownEdit: "Markdown edit…",
        appMenuReportIssue: "Report an issue…",

        workspaceSwitch: "Switch workspace",
        workspaceSwitchAria: "Workspace {workspace} — switch workspace",
        workspaceCurrent: "Current",
        workspaceMissing: "Not found",
        workspaceRemove: "Remove from list (the folder is not deleted)",
        workspacePickOther: "Choose another folder…",
        workspacePickTitle: "Choose a workspace folder",
        workspaceBusy: "A reply is still streaming — switch when the turn ends.",
        workspaceShadowNote:
            "This window keeps its own settings; the history is shared.",

        dream: {
            hint: "Click to let me take a short nap",
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

        tip: {
            organizeMemory:
                "Save facts with the memory tool; review them in Settings → Debug.",
            archiveExperience:
                "In self-evolution mode, ask the assistant to pack a workflow as an experience.",
            skills:
                "Enable a skill in the Skills panel, then type /skill to load it.",
            subAgents:
                "Type +agent in a new chat to embed a sub-agent's role.",
            fileMentions:
                "Type @ in the composer to attach a file; PDFs become Markdown.",
            figures:
                "Ask for a diagram to get a figure card; drag 3D scenes to orbit.",
            maintenance:
                "Turn on self-evolution mode, then click the face to tidy skills and memory.",
            backup:
                "Titlebar ⋯ menu exports config, skills, memory and history as one zip.",
            createSkill:
                "Skills panel → + New Skill, then load it with /skill.",
            mcpSetup:
                "MCP panel → + Add Server: command for stdio, URL and token for SSE.",
            subAgentCreate:
                "Agents panel → + New Agent, enable it, then embed it with +agent.",
            scriptLibrary:
                "Ask the assistant to save a command as a script; run_cmd runs it by name.",
            modelDepth:
                "Switch models from the toolbar; reasoning models add a depth control.",
            workspaceSwitch:
                "Click the workspace name in the titlebar to switch workspace.",
            timerBackground:
                "Let the assistant wait on a timer; enable background execution too.",
        },

        backupExportLabel: "Exporting and compressing the backup...",
        backupExportPreparing: "Preparing the backup export...",
        backupExportHint: "Chat is locked during export and sending is disabled.",
        backupExportFailed: "Backup export failed: {error}",

        updateAvailable: "New version {version} is available — you are on {current}.",
        viewUpdate: "View update",
        dismiss: "Dismiss",
        noticeDismiss: "Dismiss notice",
        toolDisabledNotice:
            "`{tool}` failed {limit} times in a row — disabled for this session.",
        budgetExhaustedNotice:
            "Tool budget spent ({rounds} rounds, {toolCalls} calls) — wrapping up with what it has.",

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
                "Keeps the machine awake while a timer or a turn is pending.",
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
            binaryRefused:
                "{name} looks like binary, so it was not attached — read or convert it with a tool instead.",
            pdfFailed: "Could not convert the PDF {name}: {error}",
            pdfConverting: "Converting {name} to Markdown…",
            pdfConvertingPage: "page {page} / {total}",
            handoffNote:
                "[Attachment handoff — the current model cannot read these formats directly, so this message carries no bytes of them] {files}. Those paths are all you have: never guess the content. Inspect and parse each file first (e.g. `run_shell` with a parser for the format: python/pandas for xlsx, `unzip` for docx/ooxml, `pdftotext` for pdf, `tar`/`7z` for archives — install a converter when the machine has none), then continue the task with what you extracted.",
        },
        slashMention: {
            title: "Insert skill reference",
            hint: "↑↓ choose · Enter/Tab insert · Esc close",
            loading: "Loading workspace skills…",
            noMatches: "No skill matches “{query}”.",
            listFailed: "Could not list workspace skills: {error}",
        },
        agentMention: {
            title: "Embed sub-agent role",
            hint: "↑↓ choose · Enter/Tab insert · Esc close · new sessions only",
            loading: "Loading sub-agents…",
            noMatches: "No sub-agent matches “{query}”.",
            noSelectable:
                "No enabled llm sub-agent with a system prompt — enable one in the Sub Agents panel.",
            listFailed: "Could not list sub-agents: {error}",
        },
        attachmentUnsupported:
            "{names}: {model} cannot read this format; only the path is handed over.",
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
        inputPlaceholder: "Type a message… (@ file, / skill, Enter to send)",
        injections: {
            title: "Added mid-turn",
            queued: "Waiting to be delivered",
            placeholder:
                "The assistant is working — type an update to add it mid-turn (Enter to add)",
        },
        selectModel: "Select model",
        modelFilterPlaceholder: "Filter models…",
        modelFilterEmpty: "No matching models",
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
        turnLostEvent:
            "This turn ended in the backend without a terminal event, so it was closed here. What had already been generated is kept.",

        notify: {
            doneTitle: "Task completed",
            errorTitle: "Task failed",
            askTitle: "Assistant is asking",
            confirmTitle: "Approval required",
        },
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
        markdownPdf: "Export PDF",
        markdownPdfTip: "Write the preview to a PDF file (A4, light theme).",
        pdfFailed: "PDF export failed: {error}",
        pdfFallback:
            "No Edge or Chrome found — opened the print dialog instead. Choose \"Save as PDF\" there.",

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
            sudoHint: "Runs with elevated privileges and may change your system.",
            elevationHint:
                "Asks for admin elevation (UAC); may change system settings.",
            externalAgentHint:
                "This agent runs outside the app, with your permissions in the workspace.",
            deny: "Deny",
            allow: "Allow",
        },

        ask: {
            title: "The assistant is asking you",
            progress: "{answered}/{total} answered",
            tabN: "Q{n}",
            customPlaceholder: "Type your own answer…",
            incomplete: "Answer every question, then submit.",
            ready: "All questions answered — ready to submit.",
            dismiss: "Dismiss",
            submit: "Submit answers",
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
        systemNotes: "System notes",
        foldMore: "Show more",
        foldLess: "Show less",
        continue: "Continue",
        // One turn's cost, shown next to a finished reply's action buttons.
        turnMeta: "{duration} · {tokens}",
        turnMetaWithCache: "{duration} · {tokens} · cache hit {percent}%",
        turnDurationSeconds: "{seconds}s",
        turnDurationMinutes: "{minutes}m {seconds}s",
        turnTokens: "{count} tokens",
    },

    figure: {
        title: "Figure {index}",
        diagramTitle: "Diagram {index}",
        drawing: "Drawing…",
        save: "Save PNG",
        saving: "Saving…",
        saved: "Saved · {path}",
        saveFailed: "Save failed",
        resetView: "Reset view",
        saveAll: "Save all figures",
        openFolder: "Open figures folder",
        webglUnavailable: "3D needs WebGL, which is unavailable in this environment.",
        saveHtml: "Save HTML",
        htmlTooLarge: "Dynamic figure is too large to run",
        playsWhenDone: "Plays when the reply finishes",
        playDynamic: "Play dynamic figure",
        sandboxHint: "Contains script · runs sandboxed · no network",
        rememberSession: "Play automatically this session",
        navigatedAway: "This figure tried to load an outside page, so it was stopped",
        unrenderable: "This figure could not be drawn: the source did not parse.",
        copySource: "Copy source",
        showSource: "Show source",
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
        subtitle: "Manage appearance, API and runtime options.",
        navAria: "Settings sections",

        sections: {
            appearance: "Appearance",
            api: "API & Model",
            system: "System Message",
            advanced: "Advanced",
            run: "Run",
            debug: "Debug",
            about: "About & Updates",
        },

        plugins: {
            enable: "Enable the plugin system",
            enableHint:
                "Give the model the plugin tool and one tool per plugin capability.",
            manager: "Plugin manager",
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
            paletteCream: "Cream",
            paletteMacaron: "Macaron",
            paletteHint: "Applies immediately and is stored in your configuration.",
            glass: "Frosted glass",
            glassOff: "Off",
            glassSubtle: "Subtle",
            glassStandard: "Standard",
            glassStrong: "Strong",
  glassHint: "Blur the titlebar, toolbar and panel headers; off makes them opaque.",
            language: "Language",
            languageHint: "Applies immediately and is stored in your configuration.",
            chatBackground: "Chat background",
            chatBackgroundHint: "Photo behind the messages; copied into app data when chosen.",
            chatBackgroundChoose: "Choose photo…",
            chatBackgroundReplace: "Replace photo…",
            chatBackgroundDefault: "Use default wallpaper",
            chatBackgroundRemove: "Remove",
            chatBackgroundPickTitle: "Choose a chat background",
            chatBackgroundFilter: "Images",
            chatBackgroundMissing: "The saved photo could not be found. Choose another one.",
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
                "Auto-filled from the API or model catalogue; used for context budgeting.",
            capabilities: "Model capabilities",
            capabilitiesEmpty:
                "No capability data for {model} yet — use Fetch Models From API.",
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
            capabilityMaxOutput: "{tokens} max output",
            conversationHeader: "Conversation ID header",
            conversationHeaderHint:
                "Header carrying the session id. Empty = x-conversation-id.",
            customHeaders: "Custom request headers",
            customHeadersHint:
                "Extra headers on every request; an Authorization row replaces the API key.",
            customHeaderName: "Header name",
            customHeaderValue: "Value",
            addHeader: "Add header",
            removeHeader: "Remove header",
        },

        system: {
            title: "System Message",
            subtitle: "Instructions sent at the start of every conversation.",
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
                "Model requests per turn. 0 = unlimited (max 1000).",
            maxToolCalls: "Max tool calls per turn",
            maxToolCallsHint:
                "Tool calls per turn. 0 = unlimited (max 5000).",
            toolFailureLimit: "Disable a tool after N failures",
            toolFailureLimitHint:
                "Disable a tool after this many failures in a row. 0 = never (max 50).",
            maxConcurrentTurns: "Max concurrent sessions",
            maxConcurrentTurnsHint:
                "Turns that may run at once in one window. 0 = unlimited (max 16).",
            dsFormat: "DS-Format (DeepSeek)",
            dsFormatHint: "Send reasoning_content back on assistant messages, as some endpoints require.",
            dsFormatAuto: "Always on for {model}: reasoning_content is always passed back.",
            enhancedFigures: "Enhanced drawing",
            enhancedFiguresHint:
                "Let the assistant draw Mermaid, SVG and HTML figures, saved to figures/.",
            memoryHint: "Memory hint on user input",
            memoryHintHint:
                "Add keyword-matched memory titles (under 200 chars) to the newest message.",
        },

        run: {
            title: "Run",
            subtitle: "How turns run: background tools, notifications, window behavior.",
            backgroundTools: "Background execution",
            backgroundToolsHint:
                "Run slow tools in the background; the result arrives before the next request.",
            allowNotifications: "Allow notifications",
            allowNotificationsHint:
                "Sound and desktop notifications when a turn ends; stopped turns stay quiet.",
            singleInstance: "Run a single instance",
            singleInstanceHint:
                "A second launch focuses the running window instead of opening a copy.",
            selfEvolution: "Enable Self-Evolution Mode",
            selfEvolutionHint:
                "Let the assistant write its own skills, memory and MCP servers here.",
        },

        debug: {
            title: "Debug",
            subtitle: "Where logs go, how long they are kept, and the monitors.",
            loggerOutput: "Logger output",
            loggerFile: "Write to app-<date>.log",
            loggerPrintln: "Print to terminal (println)",
            loggerOutputHint: "Only a debug build writes log lines.",
            retention: "Log retention (days)",
            retentionHint:
                "Delete logs older than this at startup and on Compact DB. 0 = keep forever.",
            monitors: "Monitors",
            monitorsHint: "Each row opens a read-only dialog; a running turn shows after it ends.",
            interactionMonitor: "Interaction monitor",
            agentMissions: "Agent missions monitor",
            evolutionMemory: "Evolution memory",
            experiences: "Experience packages",
            launchMonitor: "Launch Monitor",
            monitorsEmpty: "No monitor here: open a session, or turn on self-evolution mode.",
        },

        about: {
            title: "About & Updates",
            subtitle: "Version, plus update checks against GitHub releases.",
            currentVersion: "Current version",
            checkOnStartup: "Check for updates on startup",
            checkOnStartupHint:
                "Check GitHub releases at launch and show a banner for new versions.",
            prerelease: "Include pre-releases",
            prereleaseHint:
                "Also offer pre-releases; drafts stay invisible to the API.",
            updates: "Updates",
            checkNow: "Check for updates…",
            links: "Links",
            website: "Website",
            github: "GitHub",
        },

        messageEditor: {
            aria: "Edit system message",
            title: "System Message Editor",
            placeholder: "Write your system message in Markdown...",
        },

        agentMissionsMonitorAria: "Agent missions monitor",
        evolutionMemoryMonitorAria: "Evolution memory monitor",
        experiencesAria: "Experience packages",
    },

    memoryPanel: {
        title: "Evolution memory",
        summary: "{count} entries · {injected} in the top tier",
        budget: "{chars} / {budget} chars in top-tier facts",
        loading: "Loading…",
        empty: "No evolution memories yet; recorded while self-evolution mode is on.",
        pinned: "pinned",
        auto: "auto",
        confidence: "trust {value}/5",
        conflicts: "{count}× contradicted",
        evidence: "{count}× confirmed",
        uses: "{count}× used",
        archivedTitle: "Archived into skills ({count} entries)",
        archivedMeta: "{entries} entries · {chars} chars",
        archivedMissing: " · skill file missing",
        detailFailed: "Could not load the entry.",
    },

    experiencePanel: {
        title: "Experience packages",
        summary: "{count} packages",
        loading: "Loading…",
        empty:
            "No experience package yet; create one with the experience tool's pack action.",
        loadFailed: "Could not read the experience list.",
        toolCount: "{count} tools",
        noTools: "no tools",
        labelWhen: "Matches",
        labelTools: "Tools",
        labelTrust: "Trust",
        trustOk: "confirmed",
        trustPending: "asks on first run",
        labelSource: "Source workspace",
        labelDir: "Location",
        delete: "Archive",
        deleteConfirm: "Archive {slug}?",
        deleteFailed: "Could not archive the package.",
        labelParts: "Parts",
        partsNone: "none",
        labelInstalled: "Installed here",
        installedNone: "not installed in this workspace",
        partSkill: "skill",
        partMcp: "mcp",
        labelRequires: "Requires",
        requiresNone: "none recorded",
        requiresFrom: "snapshot from {host}",
        labelReady: "This machine",
        checking: "checking…",
        ready: "everything present",
        deps: "{count} deps",
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
        parallelTools: "Parallel tool calls",
        parallelToolsHint:
            "Independent calls of one round run at the same time. Writes, memory, todos and timers stay sequential.",
        parallelCount: "Calls at once",
        parallelCountHint: "How many independent calls of one round may run at once (1–8). 1 is strictly sequential.",
        lockedBadge: "Required",
        lockedBySelfEvolution:
            "Required by self-evolution mode — it cannot be switched off.",
        lockedByDependency:
            "Required by the calculator — it cannot be switched off while the calculator is on.",
        items: {
            fileActions: {
                name: "File Actions",
                description: "Read and edit workspace files",
            },
            runCmd: {
                name: "Run Command",
                description: "Run a command or workspace script",
            },
            runShell: {
                name: "Run Shell",
                description: "Run PowerShell / Bash scripts",
            },
            calculator: {
                name: "Calculator",
                description: "Exact calculations",
            },
            memory: {
                name: "Memory",
                description: "Remember facts across chats",
            },
            todoList: {
                name: "Todo List",
                description:
                    "Track complex multi-step work with a session-scoped todo list. The assistant can add items, update statuses, check progress, clear completed items, and archive finished lists.",
            },
            timer: {
                name: "Timer",
                description: "Resume a task later",
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
            "Built by the assistant; stored in .data/mcp.conf. Script: {path}",
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
        cancelTest: "Cancel Test",
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
        enableMode: "Sub-agent system",
        moreSettings: "More settings",
        autoPlan: "Automatic planning",
        autoPlanHint:
            "Plan every message before answering; off answers directly.",
        autoConfigure: "Smart agent selection",
        autoConfigureHint:
            "Pick agents for each turn with one model call; off uses all enabled.",
        plannerModel: "Planner model",
        plannerModelInherit: "Follow the main model ({model})",
        plannerModelHint:
            "Model for agent selection and planning; empty = main chat model.",
        mbus: "Mission bus (mbus)",
        mbusHint:
            "Let the planner supervise sub-agents, and agents message each other.",
        mbusPairLimit: "Messages per agent pair",
        mbusPairLimitHint:
            "Messages between two agents, both directions. 0 = no limit.",
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
            "LLM runs in-app, CLI runs a local CLI, Cloud calls a remote endpoint.",
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
            "Prompts live in app_data/agents/*.md; runtime settings in sub_agents.json.",
        emoji: "Emoji",
        emojiHint:
            "Stored in the frontmatter and shown next to the name.",
        color: "Color",
        colorHint: "Stored in the frontmatter; not painted as a UI accent.",
        enableMcp: "MCP tools",
        enableMcpHint:
            "Inject the tools of every enabled MCP server; processes belong to the task.",
        typeLockedHint:
            "The type is fixed after creation; create a new agent to change it.",
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
            "Context window per model id; empty = 131072. Compresses at 80% of it.",
        maxTokens: "Max completion tokens",
        maxTokensPlaceholder: "Default completion limit 8192",
        temperature: "Temperature",
        temperaturePlaceholder: "Inherit default",
        maxIterations: "Max iter",
        maxIterationsHint:
            "0 = no iteration limit; external task state decides completion.",
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
                "Session todo list: add, update, clear and archive items.",
            memory: "Shared session / user / repo memory store (every sub-agent reads and writes the same one)",
        },

        // ── cli backend ──
        cliSection: "CLI settings",
        cliCommand: "Command",
        cliCommandPlaceholder: "opencode",
        cliCommandHint:
            "Executable name or full path; Windows .cmd launchers are handled.",
        cliArgs: "Arguments — one per line",
        cliArgsHint:
            "One argument per line. Placeholders: {prompt}, {cwd}, {agent_name}. Empty = default.",
        cliPromptVia: "Prompt delivery",
        cliPromptViaArg: "As an argument ({prompt})",
        cliPromptViaStdin: "Through stdin",
        cliPromptViaHint:
            "Use stdin for CLIs that read piped input or prompts with %VAR%.",
        cliModelArgs: "Model arguments — one per line",
        cliModelArgsHint:
            "Extra arguments for every run, one per line, e.g. -m then the model id.",
        cliEnv: "Environment — KEY=VALUE per line",
        cliEnvHint:
            "Environment for the run; colour output is stripped automatically.",
        timeout: "Timeout (seconds)",
        timeoutHint:
            "0 = no timeout; runs are capped at 7200 s. Empty keeps 600.",
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
            "JSON POST body; {prompt}, {agent_name}, {cwd} and image keys are filled in.",
        cloudResultPath: "Result path",
        cloudResultPathHint:
            "Dot path into the response that holds the text.",
        cloudPollUrl: "Poll URL (optional)",
        cloudPollUrlHint:
            "Poll this URL for async jobs; {id} is the job id. Empty = one response.",
        cloudPollIdPath: "Job id path",
        cloudPollStatusPath: "Status path",
        cloudPollInterval: "Poll interval (s)",
        cloudPollFirstDelay: "First poll delay (s)",
        cloudPollMethod: "Query method",
        cloudPollDoneValues: "Done status values — one per line",
        cloudPollFailedValues: "Failed status values — one per line",
        cloudUnreachable: "unreachable",
        cloudTimeoutHint:
            "Timeout for the whole job; empty = 1800 s.",
        cloudRequestTimeout: "Request timeout (s)",
        cloudRequestTimeoutHint:
            "Timeout per HTTP request. 0 = 600 s; image APIs need 360 or more.",

        // ── shared ──
        requireApproval: "Ask before running",
        requireApprovalHint:
            "Ask for approval on this agent's first run in each app session.",
        probe: "Test",
        probing: "Running…",
        probeCliHint: "Runs <command> --version to check the executable.",
        probeCloudHint: "Sends a GET request to check the endpoint is reachable.",
        noEndpoint: "No endpoint configured",
    },

    history: {
        title: "History",
        searchPlaceholder: "Search sessions by keyword",
        searchAria: "Search history sessions",
        clearSearch: "Clear search",
        running: "Running",
        runningTitle: "A turn is still running in this conversation — it keeps working in the background",
        tabAll: "All",
        tabFavorites: "Favorites",
        tabArchived: "Archived",
        empty: "No history yet.",
        noMatch: "No sessions matched \"{keyword}\".",
        noFavorites: "No favorite sessions yet.",
        noArchived: "No archived sessions.",
        renameAria: "Rename session",
        messageCount: { one: "{count} message", other: "{count} messages" },
        archived: " · archived",
        loading: "Loading…",

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
        refresh: "Refresh",
        waitForTurn: "A turn is running — the interaction log is readable once it finishes.",
        loadFailed: "Failed to load the interaction log",
        clearLogs: "Clear logs",
        clearing: "Clearing…",
        clearTitle: "Delete all request and interaction logs (chat history is kept)",
        clearConfirm: "Delete all request and interaction logs? Chat history is not affected.",
        compact: "Compact DB",
        compacting: "Compacting…",
        compactTitle: "Delete logs older than 90 days and reclaim disk space",
        compactConfirm:
            "Delete logs older than the retention window? Chat history is kept.",
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
            hint: "Aggregated from this session's logs.",
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

    plugins: {
        windowTitle: "Plugins",
        subtitle:
            "Plugins add tools that the model calls over a message bus.",
        refresh: "Refresh",
        reload: "Reload manifests",
        openManager: "Open plugin manager",
        endpoint: "Service endpoint",
        endpointHint:
            "Address a service plugin dials with its token to join the bus.",
        token: "Token",
        copy: "Copy",
        copied: "Copied",
        issues: "Manifests that could not be loaded",
        empty: "No plugins installed",
        emptyHint:
            "Add a folder with a plugin.json under .data/plugins, then reload.",
        search: "Filter plugins",
        searchEmpty: "No plugin matches “{query}”.",
        count: "{shown} of {total}",
        package: {
            import: "Import package",
            importing: "Importing…",
            importTitle: "Import a plugin package",
            export: "Export",
            exporting: "Exporting…",
            exportTitle: "Export {name}",
            filter: "Plugin package",
            importFailed: "Import failed: {message}",
            exportFailed: "Export failed: {message}",
            imported: "Imported {name} — {files} files",
            importReplace: "the previous copy moved to {dir}",
            exported: "Exported {files} files to {path}",
            exportedSecrets: "{count} secret value(s) removed",
            exportedNoSettings: "settings stayed in this workspace",
            exportedSettingsSkipped: "settings stayed here: {reason}",
            hint:
                "A package is one plugin folder as a .zip with plugin.json at its root.",
        },
        transportSpawn: "Spawned",
        transportService: "Service",
        transportBoth: "Spawned or service",
        running: "Running",
        stopped: "Stopped",
        start: "Start",
        restart: "Restart",
        stop: "Stop",
        instances: "Instances",
        state: "State",
        inflight: "In flight",
        noInstances:
            "No instance is running. Start the plugin, or start its service so it attaches.",
        stateStarting: "Starting",
        stateReady: "Ready",
        stateFailed: "Failed",
        stateStopped: "Stopped",
        instanceInflight: "{count} in flight",
        instanceRestarts: "{count} restarts",
        settings: "Settings",
        save: "Save",
        saving: "Saving…",
        saved: "Saved",
        reset: "Reset to defaults",
        advanced: "Advanced",
        schemaMissing: "This plugin declares no settings.",
        pageHint: "This plugin ships its own settings page ({page}).",
        pagePending: "That page is not hosted yet — edit config.yaml for now.",
        configModeSchema: "Form",
        configModePage: "Custom page",
        secretStored: "Stored — type a new value to replace it",
        secretEmpty: "Not set",
        logs: "Logs",
        noLogs: "Nothing logged yet.",
        bus: "Bus traffic",
        noBus: "No messages yet.",
        help: {
            open: "How to create a plugin",
            title: "Creating a plugin",
            close: "Close",
            intro:
                "A plugin is a folder whose tools the model calls over the message bus.",
            folder: "1 · Folder shape",
            folderBody:
                "The folder name is the plugin id; everything lives inside it.",
            manifest: "2 · Manifest",
            manifestBody:
                "plugin.json declares runtime, entry and tools (plugin_<id>_<tool>).",
            protocol: "3 · Protocol",
            protocolBody:
                "Line-delimited JSON on stdin and stdout; reply with the same id.",
            service: "4 · Or attach as a service",
            serviceBody:
                "The process is started elsewhere and dials the endpoint above.",
            settings: "5 · Settings page",
            settingsBody:
                "config.schema.yaml becomes this form; values are saved and pushed live.",
            agent:
                "In self-evolution mode the agent builds the whole folder for you.",
        },
    },

    mission: {
        aria: "Mission monitor",
        title: "Mission Monitor",
        subtitle:
            "Mission state, active tasks and summaries for this session.",
        autoRefresh: "Auto refresh",
        refresh: "Refresh",
        openDeliverablesFolder: "Deliverables folder",
        openDeliverablesFolderHint:
            "Open this chat's deliverable folder (.data/deliver/<session>).",
        deliverables: "Deliverables",
        deliverablesEmpty: "No deliverable files in this session yet.",
        deliverablesFiles: { one: "{count} file", other: "{count} files" },
        revealInFolder: "Show in folder",
        revealInFolderHint: "Reveal this file in the system file manager.",
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
