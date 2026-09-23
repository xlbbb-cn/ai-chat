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
        saved: "Saved ✓",
        cancel: "Cancel",
        close: "Close",
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

        emptyStart: "Start a conversation",
        emptyHint: "Use {skills} to set a system prompt, or configure the API in {settings}.",

        profileExportLabel: "Exporting and compressing profile...",
        profileExportPreparing: "Preparing profile export...",
        profileExportHint: "Chat is locked during export and sending is disabled.",
        profileExportFailed: "Profile export failed: {error}",

        updateAvailable: "New version {version} is available — you are on {current}.",
        viewUpdate: "View update",
        dismiss: "Dismiss",

        attachFiles: "Attach files",
        inputPlaceholder: "Type a message… (Enter to send, Shift+Enter for newline)",
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
        attachedFiles: "Attached Files:",

        generationStopped: "[Generation stopped]",
        sessionSwitchBlocked:
            "A reply is currently being generated. Please stop it before switching history sessions.",
        deleteFailed: "Failed to delete message: {error}",
        forkFailed: "Failed to fork session: {error}",
        markdownPathEmpty: "Markdown file path is empty.",
        markdownSaveFailed: "Failed to save markdown file: {error}",
        errorPrefix: "Error: ",

        markdownEditorAria: "Markdown editor",
        markdownEditorTitle: "Markdown Edit",
        markdownColumn: "Markdown",
        previewColumn: "Preview",
        markdownPlaceholder: "Write markdown here...",
        markdownPreviewEmpty: "Markdown preview will appear here.",

        confirm: {
            titleSudo: "⚠️ Privileged operation (sudo)",
            titleElevation: "⚠️ Privileged operation (administrator)",
            titleExternalPath: "⚠️ External file access request",
            titleRisk: "⚠️ Risk Level {level} — confirm execution",
            titleDangerous: "⚠️ Dangerous command detected",
            badgeSudo: "SUDO",
            badgeAdmin: "ADMIN",
            badgePath: "PATH",
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
            externalPathHint:
                "This file action wants to access an absolute path outside the current workspace root. Allow it only if that external location is intentional.",
            deny: "Deny",
            allow: "Allow",
        },

        agents: {
            planner: "Planner",
            aggregator: "Aggregator",
            tool: "Tool",
            planDone: "Plan completed — {count} tasks total",
            aggregating: "Aggregating all subtask results...",
            loadingSkill: "🧠 Loading skill: {name}",
        },
    },

    chat: {
        thoughtProcess: "Thought Process",
        attachedFiles: "Attached Files:",
        retryTitle: "Retry this unfinished user message",
        exportTitle: "Export this reply to PDF",
        deleteTitle: "Delete this message",
        forkTitle: "Fork conversation from this message",
    },

    toolGroup: {
        completed: { one: "{count} tool call completed", other: "{count} tool calls completed" },
        completedWithErrors: {
            one: "{count} step: {done} completed, {errors} failed",
            other: "{count} steps: {done} completed, {errors} failed",
        },
        steps: { one: "{count} step", other: "{count} steps" },
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
            selfEvolution: "Enable Self-Evolution Mode",
            selfEvolutionHint:
                "Let the assistant evolve its own skills and tools inside the workspace.",
            profilesTitle: "Configuration Profiles",
            profileNamePlaceholder: "Profile name",
            saveCurrent: "Save Current",
            profileMeta: "{skills} skills · {tools} tools · {agents} agents",
            applyTitle: "Apply this profile",
            deleteAria: "Delete profile {name}",
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
            dsFormat: "DS-Format (DeepSeek)",
            dsFormatHint: "Pass reasoning_content back on assistant messages as DeepSeek-family thinking models require. Enable only for such models; leave off for other providers.",
        },

        runtime: {
            title: "Runtime & Debug",
            subtitle: "Where logs are written, how long they are kept, and how requests are monitored.",
            loggerOutput: "Logger Output (debug build only)",
            loggerFile: "Write to app.log",
            loggerPrintln: "Print to terminal (println)",
            retention: "Log retention (days)",
            retentionHint:
                "Request and interaction logs older than this are deleted at startup and by the Interaction Monitor's “Compact DB”. 0 keeps logs forever. Chat history is never pruned. Change it and press Save to apply.",
            interactionMonitor: "Interaction monitor",
            agentMissions: "Agent missions monitor",
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
        kgEngine: "Knowledge Graph Engine",
        kgEngineTitle: "Default engine for knowledge graph",
        kgEngineDisabled: "Enable Knowledge Graph tool first",
        autoAccept: "AutoAccept Mode",
        items: {
            fileActions: {
                name: "File Actions",
                description: [
                    "Read, write, list, search, patch, rename, move, create, and delete files only inside the current workspace root.",
                    "Use `./...` paths such as `./src/App.tsx`, `./skills/demo/skill.md`, or `.` for the workspace root.",
                    "Paths such as `workspace/...` and `../...` are rejected. Absolute paths outside the workspace require an explicit approval dialog before access is allowed.",
                ].join("\n"),
            },
            runCmd: {
                name: "Run Command",
                description:
                    "Run an executable program directly (without a shell). The process starts in the workspace root. Preferred for simple commands like curl, git, or wget. Privileged operations and dangerous commands require explicit confirmation.",
            },
            runShell: {
                name: "Run Shell",
                description:
                    "Execute a script in a shell (PowerShell or Bash). The shell starts in the workspace root, and directory changes must stay inside that workspace. Supports pipes, loops, variables, and other shell features. Privileged operations and dangerous commands require explicit confirmation.",
            },
            memory: {
                name: "Memory",
                description: [
                    "Persistent memory system with three scopes for storing notes and information across conversations.",
                    "",
                    "**session** — Short-term, in-memory only. Survives for the current chat session. Use for task-specific context and in-progress notes.",
                    "**user** — Long-term, file-backed. Cross-session persistent. Use for user preferences, patterns, and general insights.",
                    "**repo** — Long-term, file-backed. Repository-scoped. Use for codebase conventions, build commands, and project facts.",
                    "",
                    "Supports add, get, list, search, and delete operations.",
                ].join("\n"),
            },
            todoList: {
                name: "Todo List",
                description:
                    "Track complex multi-step work with a session-scoped todo list. The assistant can add items, update statuses, check progress, clear completed items, and archive finished lists.",
            },
            knowledgeGraph: {
                name: "Knowledge Graph",
                description: "Connect to a knowledge graph and perform queries",
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
            externalPath: {
                label: "External absolute paths",
                description:
                    "Allow file_actions to access absolute paths outside the workspace without prompting.",
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
        noLogs: "No log entries yet. Run a test (⚡) to populate.",
        logCount: {
            one: "{count} entry (auto-refresh every 1.5s)",
            other: "{count} entries (auto-refresh every 1.5s)",
        },
        logPlaceholder: "Diagnostic info will appear here once you run a test.",
    },

    agents: {
        title: "Sub Agents",
        orchestration: "Orchestration settings",
        enableMode: "Enable sub-agent mode",
        autoConfigure: "Auto-configure mode",
        executionMode: "Execution mode",
        modeParallel: "Parallel",
        modeSequential: "Sequential",
        maxConcurrency: "Max concurrency",
        empty: "No sub-agents yet. Click the button below to create one.",
        newAgent: "+ New Agent",
        name: "Name",
        namePlaceholder: "e.g. code-analyzer",
        description: "Description",
        descriptionPlaceholder: "Briefly describe this agent's role",
        systemPrompt: "System prompt",
        systemPromptPlaceholder: "You are an expert in... focused on...",
        modelLimits: "Model & limits",
        model: "Model (optional, overrides main config)",
        modelPlaceholder: "Leave empty to inherit the main model",
        maxTokens: "Max completion tokens",
        maxTokensPlaceholder: "Default completion limit 8192",
        temperature: "Temperature",
        temperaturePlaceholder: "Inherit default",
        maxIterations: "Max iter",
        maxIterationsHint:
            "0 means no iteration limit; mission completion is controlled by external task state.",
        capabilities: "Capabilities",
        allowedTools: "Allowed tools",
        statusRunning: "⚙ Running",
        statusDone: "✓ Done",
        statusError: "✕ Error",
        promptEditor: {
            aria: "Edit system prompt",
            title: "System Prompt Editor",
            placeholder: "Write your system prompt in Markdown...",
        },
        toolHints: {
            fileActions: "Read, write, patch and search files inside the workspace",
            runCmd: "Execute a single command with risk assessment",
            runShell: "Run PowerShell / bash scripts",
            knowledgeGraph: "Neo4j-backed knowledge graph queries",
            todos:
                "todo_add / todo_update_status / todo_list / todo_clear_completed / todo_archive. The plan is shared with the chat session and every parallel agent.",
            memory: "Session / user / repo memory store",
        },
    },

    history: {
        title: "History",
        searchPlaceholder: "Search sessions by keyword",
        searchAria: "Search history sessions",
        switchHint: "A reply is being generated, switching sessions is temporarily unavailable",
        tabAll: "All",
        tabFavorites: "★ Favorites",
        tabArchived: "🗄 Archived",
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
    },

    mission: {
        aria: "Mission monitor",
        title: "Mission Monitor",
        subtitle:
            "Track external mission state, active tasks, and episodic summaries for this chat session.",
        autoRefresh: "Auto refresh",
        refresh: "Refresh",
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
        episodicSummary: "Episodic Summary",
        finalReport: "Final Report",
        noActiveTasks: "No active tasks.",
        selectPrompt: "Select a mission to inspect its external state.",
        statusCompleted: "completed",
        statusRunning: "running",
    },

    profile: {
        title: "Profiles",
        newPlaceholder: "New profile name",
        saveCurrent: "Save Current Config",
        empty: "No profiles saved",
        metaSkills: { one: "{count} skill", other: "{count} skills" },
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
