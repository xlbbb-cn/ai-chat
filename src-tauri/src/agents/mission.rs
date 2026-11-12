//! Mission state: tasks, their persisted status and the snapshot handed to
//! the main agent.

use super::*;

// ─── Main-agent help (`ask_main_agent`) ──────────────────────────────────────
/// Tool rounds the helper may run before it must answer.
pub(crate) const MAIN_HELPER_MAX_ITERATIONS: u32 = 8;
/// Wall-clock budget for one `ask_main_agent` exchange.
pub(crate) const MAIN_HELPER_TIMEOUT_SECS: u64 = 180;
/// How many times one sub-agent task may ask the main agent.
pub(crate) const MAIN_HELPER_MAX_CALLS: u32 = 3;
/// Session history handed to the helper: entries, per-entry bytes, total bytes.
pub(crate) const MAIN_HELPER_HISTORY_MESSAGES: usize = 12;
pub(crate) const MAIN_HELPER_HISTORY_MESSAGE_BYTES: usize = 2_000;
pub(crate) const MAIN_HELPER_HISTORY_BUDGET_BYTES: usize = 12_000;
/// Hard cap for the material returned to the sub-agent.
pub(crate) const MAIN_HELPER_ANSWER_MAX_CHARS: usize = 20_000;
/// Cap for one tool result inside the helper loop, so a huge file read or MCP
/// response cannot blow up the helper's context before it answers.
pub(crate) const MAIN_HELPER_TOOL_RESULT_MAX_BYTES: usize = 32_000;

#[derive(Debug, Clone, Serialize)]
pub struct MissionTaskRecord {
    pub task_id: String,
    pub name: String,
    pub description: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissionStateView {
    pub mission_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub agent_name: String,
    /// Mission message bus of the orchestration batch that produced this
    /// mission. `None` for direct `call_subagent` runs and old rows.
    pub bus_id: Option<String>,
    pub root_task_description: String,
    pub root_task_context: String,
    pub status: String,
    pub mission_accomplished: bool,
    pub episodic_summary: String,
    pub final_report: String,
    /// One line per `ask_main_agent` exchange, newest last.
    pub help_notes: String,
    pub active_tasks: Vec<MissionTaskRecord>,
    pub active_task_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct MissionStateSnapshot {
    pub(crate) mission_id: String,
    pub(crate) root_task_description: String,
    pub(crate) root_task_context: String,
    pub(crate) episodic_summary: String,
    pub(crate) mission_accomplished: bool,
    pub(crate) final_report: String,
    pub(crate) active_tasks: Vec<MissionTaskRecord>,
}

pub(crate) fn mission_id_for_task(task: &Task) -> &str {
    &task.id
}

pub(crate) fn normalize_task_name(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        fallback.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn normalize_task_status(status: &str) -> Result<&'static str, String> {
    match status.trim() {
        "pending" => Ok("pending"),
        "in_progress" => Ok("in_progress"),
        "completed" => Ok("completed"),
        other => Err(format!(
            "Unsupported task status '{}'. Expected one of: pending, in_progress, completed.",
            other
        )),
    }
}

pub fn initialize_mission_state(
    db: &rusqlite::Connection,
    session_id: &str,
    agent: &SubAgent,
    task: &Task,
    bus_id: Option<&str>,
) -> Result<(), String> {
    let mission_id = mission_id_for_task(task);
    // Planner task ids (`t1`, `t2`, …) repeat across turns in one session, so
    // the upsert must reset the previous run's state: otherwise a new task
    // would start "accomplished" with the old final report and help log.
    db.execute(
        "INSERT INTO agent_missions (mission_id, session_id, agent_id, root_task_description, root_task_context, bus_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(mission_id) DO UPDATE SET \
           session_id = excluded.session_id, \
           agent_id = excluded.agent_id, \
           root_task_description = excluded.root_task_description, \
           root_task_context = excluded.root_task_context, \
           bus_id = excluded.bus_id, \
           status = 'running', \
           mission_accomplished = 0, \
           episodic_summary = '', \
           final_report = '', \
           help_notes = '', \
           created_at = CURRENT_TIMESTAMP, \
           updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![mission_id, session_id, agent.id, task.description, task.context, bus_id],
    )
    .map_err(|e| e.to_string())?;

    // A planner task id repeats across turns (`t1`, `t2`, …). The root task is
    // re-seeded below; stale sub-tasks from the previous run must not leak into
    // the new mission's active list (the planner supervisor reads it too).
    db.execute(
        "DELETE FROM agent_tasks WHERE mission_id = ?1 AND task_id != ?1",
        rusqlite::params![mission_id],
    )
    .map_err(|e| e.to_string())?;

    db.execute(
        "INSERT INTO agent_tasks (task_id, mission_id, name, description, status) \
         VALUES (?1, ?2, ?3, ?4, 'in_progress') \
         ON CONFLICT(task_id) DO UPDATE SET \
           mission_id = excluded.mission_id, \
           name = excluded.name, \
           description = excluded.description, \
           status = 'in_progress', \
           created_at = CURRENT_TIMESTAMP, \
           updated_at = CURRENT_TIMESTAMP, \
           completed_at = NULL",
        rusqlite::params![
            mission_id,
            mission_id,
            normalize_task_name(&task.description, "Primary task"),
            task.description,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub(crate) fn get_mission_task(
    db: &rusqlite::Connection,
    mission_id: &str,
    task_id: &str,
) -> Result<MissionTaskRecord, String> {
    let mut stmt = db
        .prepare(
            "SELECT task_id, name, description, status \
             FROM agent_tasks WHERE mission_id = ?1 AND task_id = ?2 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;

    stmt.query_row(rusqlite::params![mission_id, task_id], |row| {
        Ok(MissionTaskRecord {
            task_id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            status: row.get(3)?,
        })
    })
    .map_err(|e| e.to_string())
}

pub fn add_mission_task(
    db: &rusqlite::Connection,
    mission_id: &str,
    name: &str,
    description: &str,
) -> Result<MissionTaskRecord, String> {
    let trimmed_description = description.trim();
    if trimmed_description.is_empty() {
        return Err("Task description cannot be empty.".to_string());
    }

    let task = MissionTaskRecord {
        task_id: Uuid::new_v4().to_string(),
        name: normalize_task_name(name, trimmed_description),
        description: trimmed_description.to_string(),
        status: "pending".to_string(),
    };

    db.execute(
        "INSERT INTO agent_tasks (task_id, mission_id, name, description, status) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            task.task_id,
            mission_id,
            task.name,
            task.description,
            task.status,
        ],
    )
    .map_err(|e| e.to_string())?;

    Ok(task)
}

pub fn update_mission_task_status(
    db: &rusqlite::Connection,
    mission_id: &str,
    task_id: &str,
    status: &str,
) -> Result<MissionTaskRecord, String> {
    let normalized_status = normalize_task_status(status)?;
    let updated = db
        .execute(
            "UPDATE agent_tasks SET \
               status = ?1, \
               updated_at = CURRENT_TIMESTAMP, \
               completed_at = CASE WHEN ?1 = 'completed' THEN CURRENT_TIMESTAMP ELSE NULL END \
             WHERE mission_id = ?2 AND task_id = ?3",
            rusqlite::params![normalized_status, mission_id, task_id],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Err(format!(
            "Task '{}' was not found in mission '{}'.",
            task_id, mission_id
        ));
    }

    get_mission_task(db, mission_id, task_id)
}

pub fn get_active_mission_tasks(
    db: &rusqlite::Connection,
    mission_id: &str,
) -> Result<Vec<MissionTaskRecord>, String> {
    let mut stmt = db
        .prepare(
            "SELECT task_id, name, description, status \
             FROM agent_tasks \
             WHERE mission_id = ?1 AND status NOT IN ('completed', 'failed', 'stopped') \
             ORDER BY CASE status WHEN 'in_progress' THEN 0 ELSE 1 END, created_at, task_id",
        )
        .map_err(|e| e.to_string())?;

    let tasks = stmt
        .query_map(rusqlite::params![mission_id], |row| {
            Ok(MissionTaskRecord {
                task_id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(tasks)
}

pub fn save_mission_summary(
    db: &rusqlite::Connection,
    mission_id: &str,
    summary: &str,
) -> Result<(), String> {
    db.execute(
        "UPDATE agent_missions \
         SET episodic_summary = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1",
        rusqlite::params![mission_id, summary.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn save_mission_final_report(
    db: &rusqlite::Connection,
    mission_id: &str,
    final_report: &str,
) -> Result<(), String> {
    db.execute(
        "UPDATE agent_missions \
         SET final_report = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1",
        rusqlite::params![mission_id, final_report.trim()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn mark_mission_accomplished(
    db: &rusqlite::Connection,
    mission_id: &str,
    final_report: Option<&str>,
) -> Result<(), String> {
    let report = final_report.unwrap_or("").trim().to_string();
    let updated = db
        .execute(
            "UPDATE agent_missions SET \
               status = 'completed', \
               mission_accomplished = 1, \
               final_report = CASE WHEN ?2 != '' THEN ?2 ELSE final_report END, \
               updated_at = CURRENT_TIMESTAMP \
             WHERE mission_id = ?1",
            rusqlite::params![mission_id, report],
        )
        .map_err(|e| e.to_string())?;

    if updated == 0 {
        return Err(format!("Mission '{}' was not found.", mission_id));
    }

    let _ = db.execute(
        "UPDATE agent_tasks SET \
           status = 'completed', \
           updated_at = CURRENT_TIMESTAMP, \
           completed_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1 AND task_id = ?1 AND status != 'completed'",
        rusqlite::params![mission_id],
    );

    Ok(())
}

/// Closes a mission that ended without `mark_mission_accomplished` (max
/// iterations reached, user cancellation, hard error, …) so the mission
/// monitor never keeps showing a stale "running" status.
pub(crate) fn close_mission(app: &AppHandle, mission_id: &str, status: &str) {
    let state = app.state::<AppState>();
    let db = match state.db.lock() {
        Ok(db) => db,
        Err(_) => return,
    };
    let updated = db.execute(
        "UPDATE agent_missions SET status = ?2, updated_at = CURRENT_TIMESTAMP \
         WHERE mission_id = ?1 AND mission_accomplished = 0 AND status = 'running'",
        rusqlite::params![mission_id, status],
    );
    // A failed/stopped mission must not leave its root task (or any internal
    // sub-task) showing as `in_progress` in the Mission panel. `agent_tasks`
    // only knows pending/in_progress/completed in the tool API, so the terminal
    // failed/stopped values are written internally and excluded by
    // `get_active_mission_tasks`.
    if updated.unwrap_or(0) > 0 {
        let _ = db.execute(
            "UPDATE agent_tasks SET status = ?2, updated_at = CURRENT_TIMESTAMP \
             WHERE mission_id = ?1 \
               AND status NOT IN ('completed', 'failed', 'stopped')",
            rusqlite::params![mission_id, status],
        );
    }
}

pub(crate) fn load_mission_state(
    db: &rusqlite::Connection,
    mission_id: &str,
) -> Result<MissionStateSnapshot, String> {
    let mut stmt = db
        .prepare(
            "SELECT mission_id, root_task_description, root_task_context, episodic_summary, \
                    mission_accomplished, final_report \
             FROM agent_missions WHERE mission_id = ?1 LIMIT 1",
        )
        .map_err(|e| e.to_string())?;

    let mut snapshot = stmt
        .query_row(rusqlite::params![mission_id], |row| {
            Ok(MissionStateSnapshot {
                mission_id: row.get(0)?,
                root_task_description: row.get(1)?,
                root_task_context: row.get(2)?,
                episodic_summary: row.get(3)?,
                mission_accomplished: row.get::<_, i64>(4)? != 0,
                final_report: row.get(5)?,
                active_tasks: Vec::new(),
            })
        })
        .map_err(|e| e.to_string())?;

    snapshot.active_tasks = get_active_mission_tasks(db, mission_id)?;
    Ok(snapshot)
}

pub(crate) fn build_active_tasks_snapshot(tasks: &[MissionTaskRecord]) -> String {
    if tasks.is_empty() {
        return "- none".to_string();
    }

    let mut lines: Vec<String> = tasks
        .iter()
        .take(SNAPSHOT_MAX_TASKS)
        .map(|task| {
            format!(
                "- [{}] {} (id: {})\n  {}",
                task.status,
                task.name,
                task.task_id,
                preview_text(&task.description, SNAPSHOT_TASK_DESC_MAX_CHARS)
            )
        })
        .collect();
    if tasks.len() > SNAPSHOT_MAX_TASKS {
        // The snapshot is re-sent every iteration; a long list is pure weight.
        // The full list stays one `get_active_tasks` call away.
        lines.push(format!(
            "- … {} more task(s) — call get_active_tasks for the full list",
            tasks.len() - SNAPSHOT_MAX_TASKS
        ));
    }
    lines.join("\n")
}

pub(crate) fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(max_chars).collect::<String>()
}

pub(crate) fn summarize_message_for_memory(message: &Value) -> Option<String> {
    let role = message
        .get("role")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");

    if let Some(tool_calls) = message.get("tool_calls").and_then(|value| value.as_array()) {
        let tool_names = tool_calls
            .iter()
            .filter_map(|call| call.get("function"))
            .filter_map(|function| function.get("name"))
            .filter_map(|name| name.as_str())
            .collect::<Vec<_>>();
        if !tool_names.is_empty() {
            return Some(format!(
                "- assistant requested tools: {}",
                tool_names.join(", ")
            ));
        }
    }

    let content = message
        .get("content")
        .map(content_to_text)
        .map(|value| truncate_for_summary(&value, 400))
        .unwrap_or_default();

    if content.is_empty() {
        None
    } else {
        Some(format!("- {role}: {content}"))
    }
}

pub(crate) fn format_memory_excerpt(messages: &[Value]) -> String {
    messages
        .iter()
        .filter_map(summarize_message_for_memory)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Static part of the sub-agent prompt — computed once per mission and reused
/// across iterations so the LLM prompt cache keeps hitting on the prefix.
pub(crate) fn build_agent_static_system_prompt(agent: &SubAgent, self_evolution_context: &str) -> String {
    let self_evolution_block = if self_evolution_context.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n{}", self_evolution_context)
    };
    format!(
        "{}\n\nAutonomous execution sub-agent. External mission state is authoritative for task tracking.\n\
         Rules: track tasks via add_task/update_task_status/get_active_tasks/mark_mission_accomplished \
         (not chat history); keep outputs execution-focused; create tasks for branches; \
         clear active tasks before finishing. \
         The latest mission snapshot is appended as the final system message every turn — \
         always treat it as authoritative.{}",
        agent.system_prompt, self_evolution_block,
    )
}

/// Dynamic mission snapshot — refreshed every iteration and appended as the
/// LAST message so changes never invalidate the cached prefix before it.
pub(crate) fn build_mission_snapshot_prompt(mission: &MissionStateSnapshot) -> String {
    let episodic_summary = if mission.episodic_summary.trim().is_empty() {
        "(empty)".to_string()
    } else {
        mission.episodic_summary.clone()
    };

    let completion_state = if mission.mission_accomplished {
        "Mission completion flag is already set. Unless a final cleanup step is still required, produce the final answer."
    } else {
        "Mission completion flag is not set. Keep executing until you explicitly call mark_mission_accomplished."
    };

    format!(
        "MISSION SNAPSHOT (refreshed each turn):\n\
         Mission ID: {}\nPrimary task: {}\nContext: {}\n{}\n\
         Active tasks:\n{}\nEpisodic memory:\n{}",
        mission.mission_id,
        mission.root_task_description,
        preview_text(&mission.root_task_context, SNAPSHOT_CONTEXT_MAX_CHARS),
        completion_state,
        build_active_tasks_snapshot(&mission.active_tasks),
        episodic_summary,
    )
}

pub(crate) fn build_agent_messages(
    static_system_prompt: &str,
    working_memory: &[Value],
    snapshot_prompt: &str,
) -> Vec<Value> {
    let mut messages = Vec::with_capacity(working_memory.len() + 2);
    messages.push(json!({ "role": "system", "content": static_system_prompt }));
    messages.extend(working_memory.iter().cloned());
    messages.push(json!({ "role": "system", "content": snapshot_prompt }));
    messages
}

pub(crate) fn estimate_message_tokens(messages: &[Value]) -> usize {
    if let Ok(encoding) = cl100k_base() {
        return messages
            .iter()
            .map(|message| {
                let serialized = serde_json::to_string(message).unwrap_or_default();
                encoding.encode_with_special_tokens(&serialized).len() + 4
            })
            .sum::<usize>()
            + 2;
    }

    messages
        .iter()
        .map(|message| serde_json::to_string(message).unwrap_or_default().len() / 4)
        .sum()
}

pub(crate) async fn summarize_agent_memory(
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    current_summary: &str,
    messages_to_compress: &[Value],
) -> Result<String, String> {
    let excerpt = format_memory_excerpt(messages_to_compress);
    if excerpt.trim().is_empty() {
        return Ok(current_summary.trim().to_string());
    }

    let raw = call_llm_once(
        client,
        url,
        api_key,
        headers,
        vec![
            json!({
                "role": "system",
                "content": "You compress autonomous agent working memory. Preserve durable facts, decisions, blockers, evidence, file paths, and unresolved threads. Do not reproduce full tool transcripts or exact TODO lists because tasks are tracked externally. Return plain text bullet points only."
            }),
            json!({
                "role": "user",
                "content": format!(
                    "Current episodic summary:\n{}\n\nNew interaction excerpt to compress:\n{}\n\nMerge them into one concise episodic memory.",
                    if current_summary.trim().is_empty() {
                        "(empty)"
                    } else {
                        current_summary.trim()
                    },
                    excerpt
                )
            }),
        ],
        model,
        AGENT_SUMMARY_MAX_TOKENS,
        None,
    )
    .await?;

    Ok(raw.trim().to_string())
}

pub(crate) async fn compress_working_memory_if_needed(
    client: &Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    model: &str,
    static_system_prompt: &str,
    snapshot_prompt: &str,
    working_memory: &mut Vec<Value>,
    episodic_summary: &mut String,
    context_budget: usize,
) -> Result<bool, String> {
    let candidate_messages =
        build_agent_messages(static_system_prompt, working_memory, snapshot_prompt);
    let estimated_tokens = estimate_message_tokens(&candidate_messages);
    let threshold = (context_budget as f32 * AGENT_CONTEXT_COMPRESSION_THRESHOLD) as usize;
    let should_compress = working_memory.len() > AGENT_WORKING_MEMORY_MESSAGES
        || estimated_tokens >= threshold.max(1);

    if !should_compress {
        return Ok(false);
    }

    // Always keep the recent window verbatim, whether the trigger was the token
    // budget or the message ceiling. Summarising down to the last 60 messages
    // after a 61-message runaway would leave the summary far too large.
    let keep_count = AGENT_MIN_RECENT_MESSAGES.min(working_memory.len());

    if working_memory.len() <= keep_count || keep_count == 0 {
        return Ok(false);
    }

    let split_index = working_memory.len() - keep_count;
    let older_messages: Vec<Value> = working_memory.drain(..split_index).collect();
    let merged_summary = summarize_agent_memory(
        client,
        url,
        api_key,
        headers,
        model,
        episodic_summary,
        &older_messages,
    )
    .await?;

    *episodic_summary = merged_summary;
    Ok(true)
}
