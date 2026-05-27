use crate::AppState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

const WORKING_STATUS_IDLE: &str = "idle";
const WORKING_STATUS_BUSY: &str = "busy";
const WORKING_TASK_KIND_TODO: &str = "todo";
const WORKING_TASK_KIND_CRON: &str = "cron";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingRuntime {
    pub uid: String,
    pub enabled: bool,
    pub status: String,
    pub status_detail: Option<String>,
    pub active_task_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingClientRecord {
    pub uid: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub updated_at_ms: u64,
    pub active_task_file: Option<String>,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingTask {
    pub uid: String,
    pub file_name: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkingLockFile {
    uid: String,
    status: String,
    status_detail: Option<String>,
    updated_at_ms: u64,
    active_task_file: Option<String>,
}

#[derive(Debug, Clone)]
struct CronTaskEntry {
    line_index: usize,
    due_at: DateTime<Utc>,
    content: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn working_lock_path(state: &AppState) -> PathBuf {
    state
        .app_data_dir
        .join(format!("app-{}.lck", state.working_uid))
}

fn todo_path(state: &AppState) -> PathBuf {
    state
        .app_data_dir
        .join(format!("todo-{}.md", state.working_uid))
}

fn todo_path_for_uid(app_data_dir: &Path, uid: &str) -> PathBuf {
    app_data_dir.join(format!("todo-{}.md", uid))
}

fn cron_path(state: &AppState) -> PathBuf {
    state
        .app_data_dir
        .join(format!("cron-{}.md", state.working_uid))
}

fn done_path(state: &AppState) -> PathBuf {
    state
        .app_data_dir
        .join(format!("todo-{}-done.md", state.working_uid))
}

fn task_file_name(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
}

fn current_runtime(state: &AppState) -> WorkingRuntime {
    let enabled = *state.working_enabled.lock().unwrap();
    let status = state.working_status.lock().unwrap().clone();
    let status_detail = state.working_status_detail.lock().unwrap().clone();
    let active_task_file = state
        .working_task_path
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|path| task_file_name(path));

    WorkingRuntime {
        uid: state.working_uid.clone(),
        enabled,
        status,
        status_detail,
        active_task_file,
    }
}

fn clear_active_task_state(state: &AppState) {
    *state.working_task_path.lock().unwrap() = None;
    *state.working_task_kind.lock().unwrap() = None;
    *state.working_cron_task_index.lock().unwrap() = None;
}

fn set_active_task_state(state: &AppState, path: PathBuf, kind: &str, cron_task_index: Option<usize>) {
    *state.working_task_path.lock().unwrap() = Some(path);
    *state.working_task_kind.lock().unwrap() = Some(kind.to_string());
    *state.working_cron_task_index.lock().unwrap() = cron_task_index;
}

fn parse_due_at(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn find_due_cron_task(path: &Path) -> Result<Option<CronTaskEntry>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = content.lines().collect();
    let now = Utc::now();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim_start();
        let Some(rest) = line.strip_prefix("- [ ] ") else {
            index += 1;
            continue;
        };

        let mut next_index = index + 1;
        let Some((due_at_raw, first_line_content)) = rest.split_once('|') else {
            while next_index < lines.len() {
                let next_line = lines[next_index].trim_start();
                if next_line.starts_with("- [ ] ") || next_line.starts_with("- [x] ") {
                    break;
                }
                next_index += 1;
            }
            index = next_index;
            continue;
        };

        let mut content_lines = Vec::new();
        if !first_line_content.trim().is_empty() {
            content_lines.push(first_line_content.trim().to_string());
        }

        while next_index < lines.len() {
            let next_line = lines[next_index];
            let next_trimmed = next_line.trim_start();
            if next_trimmed.starts_with("- [ ] ") || next_trimmed.starts_with("- [x] ") {
                break;
            }

            if next_line.starts_with("  ") {
                let continuation = next_line.trim();
                if !continuation.is_empty() {
                    content_lines.push(continuation.to_string());
                }
            }

            next_index += 1;
        }

        if let Some(due_at) = parse_due_at(due_at_raw.trim()) {
            if due_at <= now {
                return Ok(Some(CronTaskEntry {
                    line_index: index,
                    due_at,
                    content: content_lines.join("\n"),
                }));
            }
        }

        index = next_index;
    }

    Ok(None)
}

fn complete_cron_task(path: &Path, line_index: usize, success: bool, details: &str) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();

    if line_index >= lines.len() {
        return Err("cron task index out of range".to_string());
    }

    if let Some(rest) = lines[line_index].trim_start().strip_prefix("- [ ] ") {
        let leading_len = lines[line_index].len() - lines[line_index].trim_start().len();
        let indent = " ".repeat(leading_len);
        lines[line_index] = format!("{indent}- [x] {rest}");
    }

    let mut insert_at = line_index + 1;
    while insert_at < lines.len() {
        let trimmed = lines[insert_at].trim_start();
        if trimmed.starts_with("- [ ] ") || trimmed.starts_with("- [x] ") {
            break;
        }
        insert_at += 1;
    }

    let mut result_lines = vec![
        String::new(),
        format!(
            "  Result: {} at {}",
            if success { "success" } else { "failure" },
            Utc::now().to_rfc3339()
        ),
    ];

    if !details.trim().is_empty() {
        result_lines.push("  Details:".to_string());
        for line in details.trim().lines() {
            result_lines.push(format!("  > {}", line));
        }
    }

    lines.splice(insert_at..insert_at, result_lines);

    let mut updated = lines.join("\n");
    if !updated.is_empty() {
        updated.push('\n');
    }
    fs::write(path, updated).map_err(|e| e.to_string())
}

fn validate_status(status: &str) -> Result<(), String> {
    match status {
        WORKING_STATUS_IDLE | WORKING_STATUS_BUSY => Ok(()),
        _ => Err(format!("unsupported working status: {status}")),
    }
}

fn persist_lock_file(state: &AppState) -> Result<(), String> {
    if !*state.working_enabled.lock().unwrap() {
        return Ok(());
    }

    let record = WorkingLockFile {
        uid: state.working_uid.clone(),
        status: state.working_status.lock().unwrap().clone(),
        status_detail: state.working_status_detail.lock().unwrap().clone(),
        updated_at_ms: now_ms(),
        active_task_file: state
            .working_task_path
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|path| task_file_name(path)),
    };

    let content = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
    fs::write(working_lock_path(state), content).map_err(|e| e.to_string())
}

pub fn cleanup_working_lock(state: &AppState) -> Result<(), String> {
    let path = working_lock_path(state);
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_working_runtime(state: State<'_, AppState>) -> WorkingRuntime {
    current_runtime(&state)
}

#[tauri::command]
pub fn set_working_mode(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<WorkingRuntime, String> {
    *state.working_enabled.lock().unwrap() = enabled;

    if enabled {
        if state.working_status.lock().unwrap().is_empty() {
            *state.working_status.lock().unwrap() = WORKING_STATUS_IDLE.to_string();
        }
        persist_lock_file(&state)?;
    } else {
        clear_active_task_state(&state);
        *state.working_status.lock().unwrap() = WORKING_STATUS_IDLE.to_string();
        *state.working_status_detail.lock().unwrap() = None;
        cleanup_working_lock(&state)?;
    }

    Ok(current_runtime(&state))
}

#[tauri::command]
pub fn set_working_status(
    state: State<'_, AppState>,
    status: String,
    status_detail: Option<String>,
) -> Result<WorkingRuntime, String> {
    validate_status(&status)?;
    *state.working_status.lock().unwrap() = status;
    *state.working_status_detail.lock().unwrap() = status_detail;

    if *state.working_enabled.lock().unwrap() {
        persist_lock_file(&state)?;
    }

    Ok(current_runtime(&state))
}

#[tauri::command]
pub fn list_working_clients(state: State<'_, AppState>) -> Result<Vec<WorkingClientRecord>, String> {
    let mut clients = Vec::new();
    let current_uid = state.working_uid.clone();

    for entry in fs::read_dir(&state.app_data_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name().map(|name| name.to_string_lossy().to_string()) else {
            continue;
        };
        if !file_name.starts_with("app-") || !file_name.ends_with(".lck") {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<WorkingLockFile>(&content) else {
            continue;
        };

        clients.push(WorkingClientRecord {
            is_current: record.uid == current_uid,
            uid: record.uid,
            status: record.status,
            status_detail: record.status_detail,
            updated_at_ms: record.updated_at_ms,
            active_task_file: record.active_task_file,
        });
    }

    clients.sort_by(|left, right| {
        right
            .is_current
            .cmp(&left.is_current)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.uid.cmp(&right.uid))
    });

    Ok(clients)
}

#[tauri::command]
pub fn acquire_working_task(state: State<'_, AppState>) -> Result<Option<WorkingTask>, String> {
    if !*state.working_enabled.lock().unwrap() {
        return Ok(None);
    }

    if state.working_status.lock().unwrap().as_str() == WORKING_STATUS_BUSY {
        return Ok(None);
    }

    if state.working_task_path.lock().unwrap().is_some() {
        return Ok(None);
    }

    let task_path = todo_path(&state);
    if task_path.exists() {
        let content = fs::read_to_string(&task_path).map_err(|e| e.to_string())?;
        let file_name = task_file_name(&task_path).unwrap_or_else(|| format!("todo-{}.md", state.working_uid));

        set_active_task_state(&state, task_path.clone(), WORKING_TASK_KIND_TODO, None);
        *state.working_status.lock().unwrap() = WORKING_STATUS_BUSY.to_string();
        *state.working_status_detail.lock().unwrap() = Some(file_name.clone());
        persist_lock_file(&state)?;

        return Ok(Some(WorkingTask {
            uid: state.working_uid.clone(),
            file_name,
            path: task_path.to_string_lossy().to_string(),
            content,
        }));
    }

    let cron_file_path = cron_path(&state);
    let Some(cron_task) = find_due_cron_task(&cron_file_path)? else {
        return Ok(None);
    };

    let cron_file_name = task_file_name(&cron_file_path)
        .unwrap_or_else(|| format!("cron-{}.md", state.working_uid));
    let status_detail = format!("{} @ {}", cron_file_name, cron_task.due_at.to_rfc3339());

    set_active_task_state(
        &state,
        cron_file_path.clone(),
        WORKING_TASK_KIND_CRON,
        Some(cron_task.line_index),
    );
    *state.working_status.lock().unwrap() = WORKING_STATUS_BUSY.to_string();
    *state.working_status_detail.lock().unwrap() = Some(status_detail);
    persist_lock_file(&state)?;

    Ok(Some(WorkingTask {
        uid: state.working_uid.clone(),
        file_name: cron_file_name,
        path: cron_file_path.to_string_lossy().to_string(),
        content: cron_task.content,
    }))
}

#[tauri::command]
pub fn dispatch_working_task(
    state: State<'_, AppState>,
    target_uid: String,
    content: String,
) -> Result<(), String> {
    let target_uid = target_uid.trim();
    let content = content.trim();

    if target_uid.is_empty() {
        return Err("target uid is required".to_string());
    }
    if content.is_empty() {
        return Err("task content is empty".to_string());
    }

    let lock_path = state.app_data_dir.join(format!("app-{}.lck", target_uid));
    if !lock_path.exists() {
        return Err(format!("working client {target_uid} is not online"));
    }

    let lock_content = fs::read_to_string(&lock_path).map_err(|e| e.to_string())?;
    let lock_record: WorkingLockFile = serde_json::from_str(&lock_content).map_err(|e| e.to_string())?;
    if lock_record.status != WORKING_STATUS_IDLE {
        return Err(format!("working client {target_uid} is busy"));
    }

    let target_todo_path = todo_path_for_uid(&state.app_data_dir, target_uid);
    if target_todo_path.exists() {
        return Err(format!("working client {target_uid} already has a pending task"));
    }

    fs::write(target_todo_path, content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn complete_working_task(
    state: State<'_, AppState>,
    success: bool,
    details: String,
) -> Result<(), String> {
    let task_path = state
        .working_task_path
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "no active working task".to_string())?;
    let task_kind = state
        .working_task_kind
        .lock()
        .unwrap()
        .take()
        .unwrap_or_else(|| WORKING_TASK_KIND_TODO.to_string());
    let cron_task_index = state.working_cron_task_index.lock().unwrap().take();

    if task_kind == WORKING_TASK_KIND_CRON {
        let cron_task_index = cron_task_index.ok_or_else(|| "missing active cron task index".to_string())?;
        complete_cron_task(&task_path, cron_task_index, success, &details)?;
    } else {
        let done_path = done_path(&state);
        if done_path.exists() {
            fs::remove_file(&done_path).map_err(|e| e.to_string())?;
        }

        if task_path.exists() {
            fs::rename(&task_path, &done_path).map_err(|e| e.to_string())?;
        } else {
            fs::write(&done_path, "").map_err(|e| e.to_string())?;
        }

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&done_path)
            .map_err(|e| e.to_string())?;

        let status = if success { "success" } else { "failure" };
        let result_block = format!(
            "\n\n---\n## Working Result\n- uid: {}\n- status: {}\n- completed_at_ms: {}\n\n### Details\n\n{}\n",
            state.working_uid,
            status,
            now_ms(),
            details.trim()
        );
        file.write_all(result_block.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    *state.working_status.lock().unwrap() = WORKING_STATUS_IDLE.to_string();
    *state.working_status_detail.lock().unwrap() = None;

    if *state.working_enabled.lock().unwrap() {
        persist_lock_file(&state)?;
    }

    Ok(())
}