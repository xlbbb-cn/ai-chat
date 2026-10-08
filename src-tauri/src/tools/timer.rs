//! Delayed "continue the task" timers.
//!
//! The LLM calls the `timer_set` tool when it has to wait for something slow —
//! a 30-minute log collection, a long build, a rate-limit window — instead of
//! busy-waiting or polling. It schedules a one-shot timer and ends its turn; the
//! app re-injects the timer's `message` into the chat as a new user turn when the
//! timer fires, so the assistant resumes the task with the full history.
//!
//! Timers live in a process-wide registry (managed Tauri state) that is mirrored
//! to `timers.json`, so they survive an app restart: [`restore_on_startup`]
//! re-arms every pending timer and fires overdue ones immediately.
//!
//! Two events keep the UI in sync:
//! - `timer-state` — the full list of pending timers (after every mutation).
//! - `timer-fired` — the entry that just fired, so the frontend can continue
//!   the conversation it belongs to.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// Emitted with the full list of pending timers whenever it changes.
pub const TIMER_STATE_EVENT: &str = "timer-state";
/// Emitted with the [`TimerEntry`] that just fired.
pub const TIMER_FIRED_EVENT: &str = "timer-fired";

/// Shortest delay the model may request (seconds).
pub const MIN_DELAY_SECONDS: u64 = 1;
/// Longest delay the model may request (30 days, seconds).
pub const MAX_DELAY_SECONDS: u64 = 30 * 24 * 60 * 60;

/// A single pending (or just-fired) timer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerEntry {
    pub id: String,
    /// Chat session the timer belongs to. The message is injected there.
    #[serde(default)]
    pub session_id: String,
    /// Short human-readable label for the UI ("collect logs").
    #[serde(default)]
    pub label: String,
    /// Text injected into the chat when the timer fires.
    #[serde(default)]
    pub message: String,
    /// Creation time, milliseconds since the UNIX epoch.
    pub created_at: i64,
    /// Fire time, milliseconds since the UNIX epoch.
    pub fire_at: i64,
}

impl TimerEntry {
    /// Milliseconds left until this timer fires (0 when already due).
    pub fn remaining_ms(&self) -> i64 {
        (self.fire_at - now_ms()).max(0)
    }
}

/// Process-wide timer registry. Registered with `app.manage()` in `lib.rs`.
pub struct TimerRegistry {
    timers: Mutex<HashMap<String, TimerEntry>>,
    /// JSON mirror so pending timers survive an app restart.
    path: PathBuf,
}

/// Current wall-clock time in milliseconds since the UNIX epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Human-readable duration, e.g. `30m 0s`, `1h 5m`, `2d 3h`.
pub fn format_delay(seconds: u64) -> String {
    if seconds >= 86_400 {
        format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600)
    } else if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Local wall-clock rendering of a fire time, e.g. `2026-09-24 15:07:00`.
pub fn format_fire_at(fire_at_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(fire_at_ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

impl TimerRegistry {
    /// Load the persisted timers (missing or corrupt file = empty registry).
    pub fn load(path: PathBuf) -> Self {
        let timers = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<TimerEntry>>(&raw).ok())
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| (entry.id.clone(), entry))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        Self {
            timers: Mutex::new(timers),
            path,
        }
    }

    /// Write the registry to disk. Best effort — a failed write only costs the
    /// restart-survival guarantee, never the in-memory timer.
    fn persist(&self, timers: &HashMap<String, TimerEntry>) {
        let mut list: Vec<&TimerEntry> = timers.values().collect();
        list.sort_by_key(|entry| entry.fire_at);
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    /// Every pending timer, soonest first.
    pub fn list(&self) -> Vec<TimerEntry> {
        let guard = self.timers.lock().unwrap();
        let mut list: Vec<TimerEntry> = guard.values().cloned().collect();
        list.sort_by_key(|entry| entry.fire_at);
        list
    }

    /// Number of pending timers belonging to one chat session.
    #[allow(dead_code)]
    pub fn count_for_session(&self, session_id: &str) -> usize {
        self.timers
            .lock()
            .unwrap()
            .values()
            .filter(|entry| entry.session_id == session_id)
            .count()
    }

    fn insert(&self, entry: TimerEntry) {
        let mut guard = self.timers.lock().unwrap();
        guard.insert(entry.id.clone(), entry);
        self.persist(&guard);
    }

    fn remove(&self, id: &str) -> Option<TimerEntry> {
        let mut guard = self.timers.lock().unwrap();
        let removed = guard.remove(id);
        if removed.is_some() {
            self.persist(&guard);
        }
        removed
    }
}

/// Push the current timer list to the UI.
fn emit_state(app: &AppHandle) {
    let list = app.state::<TimerRegistry>().list();
    let _ = app.emit(TIMER_STATE_EVENT, list);
}

/// Wait until `entry.fire_at`, then fire it.
///
/// The task is detached: cancelling a timer only removes it from the registry,
/// and the (possibly already sleeping) task becomes a no-op when it wakes up.
pub fn arm(app: &AppHandle, entry: TimerEntry) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let delay = Duration::from_millis(entry.remaining_ms() as u64);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        fire(&app, &entry.id);
    });
}

/// Remove a due timer and notify the UI.
fn fire(app: &AppHandle, id: &str) {
    let registry = app.state::<TimerRegistry>();
    let Some(entry) = registry.remove(id) else {
        // Cancelled while the sleeper was pending — nothing to do.
        return;
    };
    drop(registry);
    emit_state(app);

    let state = app.state::<crate::AppState>();
    if let Ok(logger) = state.logger.lock() {
        logger.log(
            "INFO",
            &format!(
                "timer fired: id={}, session={}, label={}",
                entry.id, entry.session_id, entry.label
            ),
        );
    }

    let _ = app.emit(TIMER_FIRED_EVENT, entry);
}

/// Validate a requested delay, returning a user-facing error message.
pub fn validate_delay(delay_seconds: u64) -> Result<(), String> {
    if !(MIN_DELAY_SECONDS..=MAX_DELAY_SECONDS).contains(&delay_seconds) {
        return Err(format!(
            "delay_seconds must be between {MIN_DELAY_SECONDS} and {MAX_DELAY_SECONDS} (got {delay_seconds})."
        ));
    }
    Ok(())
}

/// Schedule a new one-shot timer and arm it. Returns the stored entry.
pub fn schedule(
    app: &AppHandle,
    session_id: &str,
    delay_seconds: u64,
    label: &str,
    message: &str,
) -> Result<TimerEntry, String> {
    validate_delay(delay_seconds)?;
    let message = message.trim();
    if message.is_empty() {
        return Err(
            "message is required: write the instruction the assistant should follow when the timer fires."
                .to_string(),
        );
    }

    let now = now_ms();
    let entry = TimerEntry {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        label: {
            let label = label.trim();
            if label.is_empty() {
                "timer".to_string()
            } else {
                label.to_string()
            }
        },
        message: message.to_string(),
        created_at: now,
        fire_at: now + (delay_seconds as i64) * 1000,
    };

    app.state::<TimerRegistry>().insert(entry.clone());
    emit_state(app);
    arm(app, entry.clone());
    Ok(entry)
}

/// Cancel a pending timer. Returns the entry that was removed.
pub fn cancel(app: &AppHandle, id: &str) -> Result<TimerEntry, String> {
    let removed = app
        .state::<TimerRegistry>()
        .remove(id)
        .ok_or_else(|| format!("No pending timer with id '{id}'."))?;
    emit_state(app);
    Ok(removed)
}

/// Re-arm the timers persisted by a previous run. Timers whose fire time has
/// already passed fire immediately, so a restart never silently drops a
/// "continue the task" prompt.
pub fn restore_on_startup(app: &AppHandle) {
    let pending = app.state::<TimerRegistry>().list();
    if pending.is_empty() {
        return;
    }

    let overdue = pending
        .iter()
        .filter(|entry| entry.remaining_ms() == 0)
        .count();
    if let Ok(logger) = app.state::<crate::AppState>().logger.lock() {
        logger.log(
            "INFO",
            &format!(
                "restored {} pending timer(s) ({overdue} already due)",
                pending.len()
            ),
        );
    }

    for entry in pending {
        arm(app, entry);
    }
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// List every pending timer (soonest first) for the timer bar in the UI.
#[tauri::command]
pub fn list_timers(state: State<'_, TimerRegistry>) -> Vec<TimerEntry> {
    state.list()
}

/// Cancel a pending timer by id.
#[tauri::command]
pub fn cancel_timer(app: AppHandle, id: String) -> Result<TimerEntry, String> {
    cancel(&app, &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique `timers.json` path inside the OS temp directory.
    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-timer-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("timers.json")
    }

    fn entry(id: &str, fire_at: i64) -> TimerEntry {
        TimerEntry {
            id: id.to_string(),
            session_id: "session-a".to_string(),
            label: "collect logs".to_string(),
            message: "check the log file".to_string(),
            created_at: now_ms(),
            fire_at,
        }
    }

    #[test]
    fn format_delay_is_human_readable() {
        assert_eq!(format_delay(45), "45s");
        assert_eq!(format_delay(60), "1m 0s");
        assert_eq!(format_delay(1800), "30m 0s");
        assert_eq!(format_delay(3900), "1h 5m");
        assert_eq!(format_delay(180_000), "2d 2h");
    }

    #[test]
    fn validate_delay_enforces_bounds() {
        assert!(validate_delay(0).is_err());
        assert!(validate_delay(MAX_DELAY_SECONDS + 1).is_err());
        assert!(validate_delay(MIN_DELAY_SECONDS).is_ok());
        assert!(validate_delay(1800).is_ok());
    }

    #[test]
    fn registry_persists_and_reloads_soonest_first() {
        let path = temp_path("reload");
        let _ = std::fs::remove_file(&path);

        let registry = TimerRegistry::load(path.clone());
        registry.insert(entry("later", now_ms() + 60_000));
        registry.insert(entry("sooner", now_ms() + 1_000));

        let reloaded = TimerRegistry::load(path.clone());
        let ids: Vec<String> = reloaded.list().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["sooner".to_string(), "later".to_string()]);
        assert_eq!(reloaded.count_for_session("session-a"), 2);

        assert!(reloaded.remove("sooner").is_some());
        assert!(reloaded.remove("sooner").is_none());
        assert_eq!(TimerRegistry::load(path.clone()).list().len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remaining_ms_clamps_overdue_timers_to_zero() {
        assert_eq!(entry("overdue", now_ms() - 5_000).remaining_ms(), 0);
        assert!(entry("future", now_ms() + 60_000).remaining_ms() > 0);
    }

    #[test]
    fn corrupt_registry_file_loads_empty() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(TimerRegistry::load(path.clone()).list().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
