//! Unit tests for `agents.rs`.

use super::*;

#[test]
fn provider_balance_errors_are_recognized() {
    for text in [
        "Upstream request failed: Insufficient account funds",
        "insufficient_quota",
        "Free usage limit reached",
        "This request requires more credits, or fewer max_tokens",
    ] {
        assert!(is_provider_balance_error(text), "{text}");
    }
    assert!(!is_provider_balance_error("Model is unavailable"));
    assert!(!is_provider_balance_error("connection reset by peer"));
}

/// The user's pre-typed config: an `llm` agent plus the legacy standalone
/// CLI-agent array. Loading it must yield two typed agents and drop the
/// legacy array.
#[test]
fn legacy_cli_agents_are_migrated_into_typed_agents() {
    let raw = r#"{
        "agents": [
            {
                "id": "code-writer",
                "name": "code-writer",
                "description": "reviews code",
                "system_prompt": "be precise",
                "allowed_tools": ["file_actions"],
                "max_iterations": 10,
                "enabled": true
            }
        ],
        "orchestration": { "use_agents": true, "auto_configure": false, "max_concurrent": 3, "mode": "parallel" },
        "cli_agents": [
            {
                "id": "opencode-agent",
                "name": "opencode",
                "description": "",
                "command": "C:\\Users\\x\\AppData\\Roaming\\npm\\opencode2.ps1",
                "args": ["run", "{prompt}"],
                "prompt_via": "arg",
                "model_args": [],
                "env": [],
                "default_timeout_seconds": 600,
                "enabled": true
            }
        ]
    }"#;

    let mut config: AgentsConfig = serde_json::from_str(raw).expect("legacy config parses");
    normalize_config(&mut config);

    assert_eq!(config.agents.len(), 2);
    assert!(config.cli_agents.is_empty());

    let llm_agent = config
        .agents
        .iter()
        .find(|agent| agent.id == "code-writer")
        .expect("llm agent kept");
    assert_eq!(llm_agent.agent_type, AGENT_TYPE_LLM);
    assert!(llm_agent.require_approval);

    let cli_agent = config
        .agents
        .iter()
        .find(|agent| agent.id == "opencode-agent")
        .expect("legacy CLI agent migrated");
    assert_eq!(cli_agent.agent_type, AGENT_TYPE_CLI);
    assert_eq!(
        cli_agent.cli.args,
        vec!["run".to_string(), "{prompt}".to_string()]
    );
    assert_eq!(cli_agent.cli.default_timeout_seconds, 600);
    assert!(cli_agent.cli.command.ends_with("opencode2.ps1"));

    // The legacy array must not come back when the config is serialized.
    let serialized = serde_json::to_string(&config).expect("serializes");
    assert!(!serialized.contains("\"cli_agents\""));
}

#[test]
fn failed_and_timed_out_runs_are_not_successes() {
    assert_eq!(task_status_for_run("success"), "success");
    assert_eq!(task_status_for_run("cancelled"), "stopped");
    // A non-zero exit or a timeout must not be reported as "success": the
    // mission would be marked accomplished and the panel would show a
    // green "done" badge for a failed run.
    assert_eq!(task_status_for_run("failed"), "error");
    assert_eq!(task_status_for_run("timeout"), "error");
}

#[test]
fn unknown_agent_types_fall_back_to_llm() {
    assert_eq!(normalize_agent_type("CLI"), AGENT_TYPE_CLI);
    assert_eq!(normalize_agent_type(" cloud "), AGENT_TYPE_CLOUD);
    assert_eq!(normalize_agent_type("banana"), AGENT_TYPE_LLM);
    assert_eq!(normalize_agent_type(""), AGENT_TYPE_LLM);
}

#[test]
fn zero_timeout_means_no_timeout() {
    // `0` disables the deadline for CLI and cloud agents.
    assert!(resolve_agent_timeout(0).is_none());
    assert_eq!(resolve_agent_timeout(600), Some(Duration::from_secs(600)));
    // Positive values stay capped at the documented maximum.
    assert_eq!(
        resolve_agent_timeout(u32::MAX),
        Some(Duration::from_secs(MAX_AGENT_TIMEOUT_SECS))
    );
}

#[test]
fn legacy_max_tokens_migrates_to_max_complete_tokens() {
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"agents":[
            {"id":"legacy","max_tokens":16384},
            {"id":"explicit","max_tokens":8192,"max_complete_tokens":2048},
            {"id":"zero","max_tokens":4096,"max_complete_tokens":0}
        ]}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);

    // A legacy-only value is adopted and the dead field is cleared.
    assert_eq!(config.agents[0].max_complete_tokens, Some(16_384));
    assert_eq!(config.agents[0].max_tokens, None);
    // An explicit new value wins.
    assert_eq!(config.agents[1].max_complete_tokens, Some(2_048));
    assert_eq!(config.agents[1].max_tokens, None);
    // `0` is "unset", so the legacy value still applies.
    assert_eq!(config.agents[2].max_complete_tokens, Some(4_096));
    assert_eq!(config.agents[2].max_tokens, None);
}

#[test]
fn max_iterations_defaults_to_unlimited_and_is_capped() {
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"agents":[{"id":"default"},{"id":"huge","max_iterations":99999}]}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);

    // Default changed from 10 to 0: the external task state ends a mission,
    // not an arbitrary round count.
    assert_eq!(config.agents[0].max_iterations, 0);
    // A hand-edited file cannot exceed the documented ceiling.
    assert_eq!(config.agents[1].max_iterations, MAX_AGENT_ITERATIONS);
}

#[test]
fn completion_limit_prefers_the_new_field() {
    let mut agent: SubAgent = serde_json::from_str(r#"{"id":"a"}"#).expect("agent parses");
    assert_eq!(
        resolve_completion_limit(&agent),
        LLM_DEFAULT_MAX_COMLETE_TOKENS
    );

    agent.max_tokens = Some(16_384);
    assert_eq!(resolve_completion_limit(&agent), 16_384);

    agent.max_complete_tokens = Some(2_048);
    assert_eq!(resolve_completion_limit(&agent), 2_048);

    // A zero in the current field means "unset", not "zero output".
    agent.max_complete_tokens = Some(0);
    assert_eq!(resolve_completion_limit(&agent), 16_384);
}

#[test]
fn v2_config_adopts_smart_selection_once() {
    // A v2 file always carried an explicit `false` (old default): adopt the
    // new default once …
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":2,"agents":[],"orchestration":{"use_agents":true,"auto_configure":false}}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);
    assert!(config.orchestration.auto_configure);
    assert_eq!(config.version, 5);

    // … while a v3 file keeps whatever the user chose.
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":3,"agents":[],"orchestration":{"use_agents":true,"auto_configure":false}}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);
    assert!(!config.orchestration.auto_configure);
}

#[test]
fn v3_config_migrates_the_old_iteration_default_once() {
    // The old panel always serialized `10`, so that is the old default, not
    // a deliberate cap. Any other value is the user's choice and survives.
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":3,"agents":[
            {"id":"old-default","max_iterations":10},
            {"id":"explicit","max_iterations":25}
        ]}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);

    assert_eq!(config.agents[0].max_iterations, 0);
    assert_eq!(config.agents[1].max_iterations, 25);
    assert_eq!(config.version, 5);
}

#[test]
fn v4_config_adds_shared_memory_to_llm_agents_once() {
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":4,"agents":[
            {"id":"llm","agent_type":"llm","allowed_tools":["file_actions"]},
            {"id":"cli","agent_type":"cli","allowed_tools":[]}
        ]}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);

    assert!(config.agents[0]
        .allowed_tools
        .iter()
        .any(|tool| tool == "memory"));
    assert!(!config.agents[1]
        .allowed_tools
        .iter()
        .any(|tool| tool == "memory"));
    assert_eq!(config.version, 5);

    // A v5 file keeps a deliberate opt-out.
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":5,"agents":[
            {"id":"llm","agent_type":"llm","allowed_tools":["file_actions"]}
        ]}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);
    assert!(!config.agents[0]
        .allowed_tools
        .iter()
        .any(|tool| tool == "memory"));
}

fn roster_ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn planner_model_defaults_to_the_main_model_and_wins_when_set() {
    let mut orchestration = AgentOrchestration::default();
    // Before the field existed every planner call ran on the main chat model.
    assert_eq!(orchestration.planner_model_or("gpt-main"), "gpt-main");

    orchestration.planner_model = Some("cheap-planner".to_string());
    assert_eq!(orchestration.planner_model_or("gpt-main"), "cheap-planner");
}

#[test]
fn planner_model_is_trimmed_and_blank_means_inherit() {
    let mut config: AgentsConfig = serde_json::from_str(
        r#"{"version":5,"orchestration":{"planner_model":"  cheap-planner  "}}"#,
    )
    .expect("config parses");
    normalize_config(&mut config);
    assert_eq!(
        config.orchestration.planner_model.as_deref(),
        Some("cheap-planner")
    );

    // Blank (a hand-edited file, or a form that sent "") inherits the main
    // model instead of pinning an empty id.
    let mut config: AgentsConfig =
        serde_json::from_str(r#"{"version":5,"orchestration":{"planner_model":"   "}}"#)
            .expect("config parses");
    normalize_config(&mut config);
    assert_eq!(config.orchestration.planner_model, None);
}

fn expect_mission(decision: PlanDecision) -> TaskPlan {
    match decision {
        PlanDecision::Mission(plan) => plan,
        PlanDecision::Direct { why } => {
            panic!("expected a mission plan, got a direct answer: {why}")
        }
    }
}

#[test]
fn plan_parser_strips_fences_and_preamble() {
    let raw = "Here is the plan you asked for:\n```json\n{\"tasks\":[{\"id\":\"t1\",\
               \"agent_id\":\"a\",\"description\":\"do it\"}],\"execution_mode\":\"parallel\"}\n```\nEnjoy!";
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert_eq!(plan.tasks.len(), 1);
    assert_eq!(plan.tasks[0].id, "t1");
    assert_eq!(plan.execution_mode, "parallel");
    assert!(issues.is_empty(), "unexpected issues: {issues:?}");
}

#[test]
fn plan_parser_repairs_near_json() {
    let raw = r#"{
        "tasks": [
            { "id": "t1", "agent_id": "a", "description": "one", }, // trailing comma
            { "id": "t2", "agent_id": "a", "description": "two" }
        ],
    }"#;
    let (decision, _) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert_eq!(plan.tasks.len(), 2);
}

#[test]
fn plan_parser_drops_unknown_agents_and_renames_duplicates() {
    let raw = r#"{"tasks":[
        {"id":"t1","agent_id":"a","description":"keep me"},
        {"id":"t1","agent_id":"a","description":"duplicate id"},
        {"id":"t2","agent_id":"ghost","description":"unknown agent"},
        {"id":"t3","agent_id":"a","description":"   "}
    ]}"#;
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    // t2 (unknown agent) and t3 (empty description) are dropped; the
    // duplicate t1 is renamed so mission ids stay unique.
    assert_eq!(plan.tasks.len(), 2);
    assert_eq!(plan.tasks[0].id, "t1");
    assert_eq!(plan.tasks[1].id, "t1-2");
    assert!(issues.iter().any(|issue| issue.contains("ghost")));
    assert!(issues.iter().any(|issue| issue.contains("duplicate")));
}

#[test]
fn plan_parser_reserves_the_planner_id() {
    let raw = r#"{"tasks":[
        {"id":"planner","agent_id":"a","description":"model used the reserved id"},
        {"id":"t1","agent_id":"a","description":"normal"}
    ]}"#;
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert_eq!(plan.tasks.len(), 2);
    assert_eq!(plan.tasks[0].id, "task");
    assert_eq!(plan.tasks[1].id, "t1");
    assert!(issues
        .iter()
        .any(|issue| issue.contains("reserved") || issue.contains("planner")));
}

#[test]
fn plan_parser_breaks_dependency_cycles() {
    let raw = r#"{"tasks":[
        {"id":"a1","agent_id":"a","description":"first","dependencies":["b1"]},
        {"id":"b1","agent_id":"a","description":"second","dependencies":["a1"]}
    ],"execution_mode":"parallel"}"#;
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert_eq!(plan.tasks.len(), 2);
    // At least one edge is gone, so the wave scheduler can start somewhere.
    assert!(
        plan.tasks.iter().any(|task| task.dependencies.is_empty()),
        "cycle was not broken: {plan:?}"
    );
    assert!(issues.iter().any(|issue| issue.contains("cycle")));
}

#[test]
fn plan_parser_drops_dangling_dependencies() {
    let raw = r#"{"tasks":[
        {"id":"t1","agent_id":"a","description":"only task","dependencies":["missing","t1"]}
    ]}"#;
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert!(plan.tasks[0].dependencies.is_empty());
    assert!(issues.iter().any(|issue| issue.contains("dependency")));
}

#[test]
fn plan_parser_keeps_and_normalizes_task_cwd() {
    let raw = r#"{"tasks":[
        {"id":"t1","agent_id":"a","description":"one","cwd":"  apps/web  "},
        {"id":"t2","agent_id":"a","description":"two","cwd":"./"},
        {"id":"t3","agent_id":"a","description":"three","cwd":null},
        {"id":"t4","agent_id":"a","description":"four"}
    ]}"#;
    let (decision, _) = parse_task_plan(raw, &roster_ids(&["a"])).expect("plan parses");
    let plan = expect_mission(decision);
    assert_eq!(plan.tasks[0].cwd.as_deref(), Some("apps/web"));
    assert_eq!(plan.tasks[1].cwd, None, "\"./\" means the workspace root");
    assert_eq!(plan.tasks[2].cwd, None, "null means the workspace root");
    assert_eq!(
        plan.tasks[3].cwd, None,
        "missing cwd means the workspace root"
    );
}

#[test]
fn resolve_task_cwd_is_sandboxed_and_creates_missing_directories() {
    let root = std::env::temp_dir().join(format!("rabbit-chat-studio-cwd-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("temp workspace");

    // Unset / "." → the workspace root itself.
    assert_eq!(resolve_task_cwd(None, &root).unwrap(), root);
    assert_eq!(resolve_task_cwd(Some("."), &root).unwrap(), root);

    // An existing subdirectory is returned as-is.
    let existing = root.join("apps").join("web");
    std::fs::create_dir_all(&existing).expect("existing task dir");
    let resolved = resolve_task_cwd(Some("apps/web"), &root).expect("existing cwd");
    assert_eq!(
        resolved.canonicalize().unwrap(),
        existing.canonicalize().unwrap()
    );

    // A directory that does not exist yet is created (scaffolding tasks).
    let resolved = resolve_task_cwd(Some("./apps/new"), &root).expect("new cwd");
    assert!(resolved.is_dir(), "missing task dir was not created");
    assert!(resolved.ends_with("apps/new"));

    // Escapes and outside absolute paths are rejected.
    assert!(resolve_task_cwd(Some("../outside"), &root).is_err());
    let outside = std::env::temp_dir().display().to_string();
    assert!(resolve_task_cwd(Some(&outside), &root).is_err());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn plan_parser_rejects_unusable_plans() {
    // Not JSON at all.
    assert!(parse_task_plan("no plan here", &roster_ids(&["a"])).is_err());
    // Valid JSON, but every task is unusable.
    let raw = r#"{"tasks":[{"id":"t1","agent_id":"ghost","description":"x"}]}"#;
    let err = parse_task_plan(raw, &roster_ids(&["a"])).expect_err("no usable tasks");
    assert!(err.contains("ghost"), "unexpected error: {err}");
    // `delegate:true` without tasks is unusable.
    assert!(parse_task_plan(
        r#"{"delegate":true,"execution_mode":"parallel"}"#,
        &roster_ids(&["a"])
    )
    .is_err());
}

#[test]
fn plan_parser_honours_a_direct_answer() {
    let (decision, issues) = parse_task_plan(
        r#"{"delegate":false,"why":"simple question"}"#,
        &roster_ids(&["a"]),
    )
    .expect("decline parses");
    match decision {
        PlanDecision::Direct { why } => assert_eq!(why, "simple question"),
        PlanDecision::Mission(_) => panic!("expected a direct answer"),
    }
    assert!(issues.is_empty());

    // An explicit false wins even when the model also emitted tasks.
    let raw = r#"{"delegate":false,"why":"no","tasks":[
        {"id":"t1","agent_id":"a","description":"ignored"}
    ]}"#;
    let (decision, issues) = parse_task_plan(raw, &roster_ids(&["a"])).expect("decline parses");
    assert!(matches!(decision, PlanDecision::Direct { .. }));
    assert!(issues.iter().any(|issue| issue.contains("ignored")));
}

#[test]
fn json_extraction_handles_braces_inside_strings() {
    let raw =
        r#"prefix {"tasks":[{"id":"t1","agent_id":"a","description":"a } brace"}]} suffix"#;
    let extracted = extract_json_object(raw).expect("object found");
    assert!(extracted.starts_with('{'));
    assert!(extracted.ends_with('}'));
    assert!(extracted.contains("a } brace"));
}

#[test]
fn repaired_json_roundtrips_through_serde() {
    let raw = r#"{"tasks":[{"id":"t1","agent_id":"a","description":"x",}],}"#;
    let repaired = repair_json(raw);
    let _: Value = serde_json::from_str(&repaired).expect("repaired JSON parses");
}

#[test]
fn task_result_skipped_carries_reason() {
    let task = Task {
        id: "t1".to_string(),
        agent_id: "a".to_string(),
        description: "x".to_string(),
        context: String::new(),
        cwd: None,
        dependencies: Vec::new(),
    };
    let result = TaskResult::skipped(&task, "agent a", "agent 'a' is not available");
    assert_eq!(result.status, "skipped");
    assert_eq!(result.task_id, "t1");
    assert!(result.content.contains("not available"));
}

#[test]
fn compact_list_caps_long_capability_lists() {
    let items: Vec<String> = (0..10).map(|i| format!("tool{i}")).collect();
    let text = compact_list(&items);
    assert!(text.ends_with(",…"), "unexpected list: {text}");
    assert!(text.len() < items.join(",").len());
    assert_eq!(compact_list(&[]), "-");
}

#[test]
fn main_helper_tools_drop_chat_only_tools() {
    let mut config = AppConfig::default();
    config.selected_tools = vec![
        "run_cmd".to_string(),
        "file_actions".to_string(),
        "timer".to_string(),
    ];
    let ids = main_helper_tool_ids(&config);
    assert!(ids.iter().any(|id| id == "run_cmd"));
    assert!(ids.iter().any(|id| id == "file_actions"));
    assert!(
        !ids.iter().any(|id| id == "timer"),
        "timer is a chat-session tool and must not reach the helper"
    );
}

#[test]
fn main_helper_history_is_budgeted_and_keeps_the_newest_entries() {
    let records: Vec<crate::db::HistoryRecord> = (0..20)
        .map(|i| crate::db::HistoryRecord {
            id: i,
            session_id: "s".to_string(),
            role: if i % 2 == 0 {
                "user".to_string()
            } else {
                "assistant".to_string()
            },
            content: format!("message {i}"),
            timestamp: String::new(),
            tool_calls: None,
            reasoning_content: None,
            attachments: None,
            injections: None,
            error: None,
            status: None,
            usage_meta: None,
        })
        .collect();

    let transcript = build_main_helper_history(&records);
    assert!(
        transcript.starts_with("[user]: message 8"),
        "got: {transcript}"
    );
    assert!(
        transcript.ends_with("[assistant]: message 19"),
        "got: {transcript}"
    );
    assert!(
        !transcript.contains("message 7"),
        "older entry was not dropped"
    );
}

#[test]
fn main_helper_history_respects_the_byte_budget() {
    let records: Vec<crate::db::HistoryRecord> = (0..20)
        .map(|i| crate::db::HistoryRecord {
            id: i,
            session_id: "s".to_string(),
            role: "user".to_string(),
            content: format!("{i}:{}", "x".repeat(5_000)),
            timestamp: String::new(),
            tool_calls: None,
            reasoning_content: None,
            attachments: None,
            injections: None,
            error: None,
            status: None,
            usage_meta: None,
        })
        .collect();

    let transcript = build_main_helper_history(&records);
    let limit = MAIN_HELPER_HISTORY_BUDGET_BYTES + MAIN_HELPER_HISTORY_MESSAGE_BYTES + 20;
    assert!(
        transcript.len() <= limit,
        "transcript exceeded the budget: {} bytes",
        transcript.len()
    );
}

#[test]
fn main_helper_tool_results_are_capped() {
    let short = "ok".to_string();
    assert_eq!(cap_helper_tool_result(short.clone()), short);

    let long = "x".repeat(MAIN_HELPER_TOOL_RESULT_MAX_BYTES + 100);
    let capped = cap_helper_tool_result(long);
    assert!(capped.len() <= MAIN_HELPER_TOOL_RESULT_MAX_BYTES + 100);
    assert!(capped.contains("truncated"));
}

#[test]
fn mission_snapshot_clips_pathological_task_lists() {
    let tasks: Vec<MissionTaskRecord> = (0..SNAPSHOT_MAX_TASKS + 5)
        .map(|i| MissionTaskRecord {
            task_id: format!("t{i}"),
            name: format!("task {i}"),
            description: "x".repeat(2_000),
            status: "pending".to_string(),
        })
        .collect();
    let snapshot = build_active_tasks_snapshot(&tasks);
    assert!(snapshot.contains("more task(s)"));
    for line in snapshot.lines() {
        assert!(
            line.chars().count() <= SNAPSHOT_TASK_DESC_MAX_CHARS + 10,
            "line not clipped: {} chars",
            line.chars().count()
        );
    }

    let mission = MissionStateSnapshot {
        mission_id: "m1".to_string(),
        root_task_description: "root".to_string(),
        root_task_context: "c".repeat(10_000),
        episodic_summary: String::new(),
        mission_accomplished: false,
        final_report: String::new(),
        active_tasks: Vec::new(),
    };
    let prompt = build_mission_snapshot_prompt(&mission);
    assert!(
        prompt.chars().count() < SNAPSHOT_CONTEXT_MAX_CHARS + 400,
        "snapshot not clipped: {} chars",
        prompt.chars().count()
    );
}

fn temp_agents_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rabbit-chat-studio-agents-{label}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn llm_agent(id: &str, name: &str) -> SubAgent {
    SubAgent {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        agent_type: AGENT_TYPE_LLM.to_string(),
        system_prompt: String::new(),
        model: None,
        max_complete_tokens: None,
        max_tokens: None,
        temperature: None,
        allowed_tools: Vec::new(),
        allowed_skills: Vec::new(),
        max_iterations: 10,
        enable_mcp: false,
        emoji: None,
        color: None,
        file: None,
        parse_error: None,
        configured: true,
        cli: Default::default(),
        cloud: Default::default(),
        require_approval: true,
        enabled: false,
    }
}

#[test]
fn saving_config_keeps_only_runtime_config_for_file_backed_agents() {
    let dir = temp_agents_dir("save");
    let path = dir.join("sub_agents.json");

    let mut virtual_agent = llm_agent("engineering/virtual", "Virtual");
    virtual_agent.file = Some("engineering/virtual.md".to_string());
    virtual_agent.configured = false;

    let mut configured = llm_agent("custom/configured", "Configured");
    configured.file = Some("custom/configured.md".to_string());
    configured.system_prompt = "be precise".to_string();
    configured.description = "reviews code".to_string();
    configured.emoji = Some("🤖".to_string());
    configured.enable_mcp = true;

    let mut cli: SubAgent =
        serde_json::from_str(r#"{"id":"cli","name":"OpenCode","description":"","agent_type":"cli","cli":{"command":"opencode"}}"#)
            .expect("cli agent parses");
    cli.configured = true;

    let config = AgentsConfig {
        version: 99,
        agents: vec![virtual_agent, configured, cli],
        orchestration: AgentOrchestration::default(),
        cli_agents: Vec::new(),
    };
    save_agents_config_file(&path, &config).expect("save");

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
            .expect("json");
    assert_eq!(saved["version"], 5);
    let agents = saved["agents"].as_array().expect("agents array");
    assert_eq!(agents.len(), 2, "virtual agents are never persisted");

    let llm = agents
        .iter()
        .find(|agent| agent["id"] == "custom/configured")
        .expect("configured llm agent");
    assert!(llm.get("name").is_none());
    assert!(llm.get("description").is_none());
    assert!(llm.get("system_prompt").is_none());
    assert!(llm.get("emoji").is_none());
    assert!(llm.get("cli").is_none());
    assert!(llm.get("cloud").is_none());
    assert!(llm.get("require_approval").is_none());
    assert_eq!(llm["file"], "custom/configured.md");
    assert_eq!(llm["enable_mcp"], true);
    assert!(llm.get("configured").is_none());

    let cli = agents
        .iter()
        .find(|agent| agent["id"] == "cli")
        .expect("cli agent");
    assert_eq!(cli["name"], "OpenCode");
    assert!(cli.get("enable_mcp").is_none());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn indexed_agents_serialize_with_the_fields_the_panel_reads() {
    let dir = temp_agents_dir("serialize");
    let path = dir.join("sub_agents.json");
    let root = dir.join("agents");
    std::fs::write(&path, r#"{ "agents": [] }"#).expect("write config");
    std::fs::create_dir_all(root.join("custom")).expect("agents dir");
    std::fs::write(
        root.join("custom/reviewer.md"),
        "---\nname: Reviewer\ndescription: reviews code\nemoji: 👀\n---\n\nbe precise\n",
    )
    .expect("write agent");
    // The panel lists discovered agents from the manifest, not a scan.
    super::files::write_index(&root, &super::files::build_index(&root)).expect("write index");

    let resolved = load_sub_agent_list(&path);
    let value = serde_json::to_value(
        resolved
            .iter()
            .find(|agent| agent.file.as_deref() == Some("custom/reviewer.md"))
            .expect("indexed agent"),
    )
    .expect("serialize");

    // The panel's draft conversion reads `cli`/`cloud` for every type and
    // the enable checkbox reads `require_approval`, so these must stay in
    // the API payload even though the config file omits them.
    assert_eq!(value["name"], "Reviewer");
    assert_eq!(value["description"], "reviews code");
    assert_eq!(value["emoji"], "👀");
    assert!(value["cli"].is_object());
    assert!(value["cloud"].is_object());
    assert_eq!(value["require_approval"], true);
    assert_eq!(value["allowed_skills"], serde_json::json!([]));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn configured_load_migrates_inline_prompts_and_panel_reads_the_index() {
    let dir = temp_agents_dir("resolved");
    let path = dir.join("sub_agents.json");
    let root = dir.join("agents");
    std::fs::write(
        &path,
        r#"{
            "agents": [{
                "id": "legacy",
                "name": "Legacy",
                "description": "old",
                "system_prompt": "be precise",
                "enabled": true
            }],
            "orchestration": { "use_agents": false }
        }"#,
    )
    .expect("write config");

    std::fs::create_dir_all(root.join("engineering")).expect("agents dir");
    std::fs::write(
        root.join("engineering/library.md"),
        "---\nname: Library\ndescription: from disk\nemoji: 📚\n---\n\nlibrary prompt\n",
    )
    .expect("write library agent");

    // Runtime resolution sees configured agents only; the library file must
    // not be discovered until Refresh writes the manifest.
    let configured = load_configured_agents(&path);
    assert_eq!(configured.agents.len(), 1);
    let legacy = configured
        .agents
        .iter()
        .find(|agent| agent.id == "legacy")
        .expect("legacy agent");
    assert_eq!(legacy.file.as_deref(), Some("custom/legacy.md"));
    assert!(legacy.system_prompt.is_empty());
    assert!(root.join("custom/legacy.md").is_file());

    // Refresh (index build + replace) makes the file visible to the panel.
    super::files::write_index(&root, &super::files::build_index(&root)).expect("write index");
    let resolved = load_sub_agent_list(&path);
    let library = resolved
        .iter()
        .find(|agent| agent.file.as_deref() == Some("engineering/library.md"))
        .expect("indexed library agent");
    assert!(!library.configured);
    assert!(!library.enabled);
    assert_eq!(library.name, "Library");
    assert_eq!(library.emoji.as_deref(), Some("📚"));
    assert_eq!(
        library.allowed_tools,
        vec!["file_actions".to_string(), "memory".to_string()]
    );
    assert!(!library.enable_mcp);

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read config"))
            .expect("json");
    let agents = saved["agents"].as_array().expect("agents array");
    assert_eq!(
        agents.len(),
        1,
        "only the migrated config entry is persisted"
    );
    assert_eq!(agents[0]["id"], "legacy");
    assert_eq!(agents[0]["file"], "custom/legacy.md");
    assert!(agents[0].get("system_prompt").is_none());

    std::fs::remove_dir_all(&dir).ok();
}

