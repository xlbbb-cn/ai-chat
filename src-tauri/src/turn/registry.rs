//! [`TurnRegistry`]: the running [`super::LlmTurn`] bases of one window,
//! keyed by session id.
//!
//! A new turn in the SAME session supersedes the old one (the previous cancel
//! flag goes permanently `true`, so a winding-down turn exits instead of
//! resurrecting); a turn in another session is never touched. The registry is
//! the source of truth for the History panel's running badge
//! (`list_active_turns` + the `turn-state` event) and for the snapshot a
//! webview re-attaches to (`get_turn_snapshot`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::AppHandle;

use super::progress::TurnSnapshot;
use super::LlmTurn;
use crate::AppConfig;

/// Hard ceiling for `max_concurrent_turns`; configured values are clamped.
pub const MAX_CONCURRENT_TURNS: u32 = 16;
/// Default written into a fresh `config.json`.
pub const DEFAULT_MAX_CONCURRENT_TURNS: u32 = 4;

// `serde(default = "…")` needs a function path, not a constant.
pub fn default_max_concurrent_turns() -> u32 {
    DEFAULT_MAX_CONCURRENT_TURNS
}

/// One row of `list_active_turns` — everything the History badge needs.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveTurnInfo {
    pub session_id: String,
    pub turn_id: String,
    pub model: String,
    pub started_at_ms: i64,
    pub phase: String,
}

/// Admission decision of the registry: a turn of a session that already runs
/// one supersedes it (always admitted, no new slot); a brand-new session needs
/// a free slot under the concurrency limit (`0` = unlimited).
fn admission(same_session: bool, running: usize, max_concurrent: u32) -> Result<(), String> {
    if same_session {
        return Ok(());
    }
    let limit = max_concurrent.min(MAX_CONCURRENT_TURNS);
    if limit > 0 && running as u32 >= limit {
        return Err(format!(
            "Too many conversations are running at once (limit {limit}). \
             Wait for one to finish, stop it, or raise the limit in Settings → Advanced."
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct TurnRegistry {
    inner: Mutex<HashMap<String, Arc<LlmTurn>>>,
}

impl TurnRegistry {
    /// Construct and publish the base for a new turn. The configuration is
    /// frozen into the base here — the turn never re-reads live config.
    ///
    /// Refuses the turn when the concurrency limit is spent; a superseding
    /// turn of the same session does not count as a new slot.
    pub fn begin(
        &self,
        app: AppHandle,
        session_id: &str,
        turn_id: u64,
        ui_turn_id: &str,
        model: &str,
        config: AppConfig,
        max_concurrent: u32,
    ) -> Result<Arc<LlmTurn>, String> {
        let turn = Arc::new(LlmTurn::new(
            app,
            session_id,
            turn_id,
            ui_turn_id,
            model,
            config,
        ));
        let mut map = self
            .inner
            .lock()
            .map_err(|_| "turn registry lock poisoned".to_string())?;
        if let Some(previous) = map.get(session_id) {
            previous.stop();
        } else {
            admission(false, map.len(), max_concurrent)?;
        }
        map.insert(session_id.to_string(), turn.clone());
        Ok(turn)
    }

    /// Stop the session's running turn (Stop button). Returns whether a turn
    /// was found.
    pub fn cancel(&self, session_id: &str) -> bool {
        match self.inner.lock() {
            Ok(map) => map
                .get(session_id)
                .map(|turn| {
                    turn.stop();
                })
                .is_some(),
            Err(_) => false,
        }
    }

    /// Remove the entry whose window id is `turn_id`, if it is still the
    /// published one. A winding-down turn must never remove a newer turn that
    /// already took the session over. Returns the removed base.
    pub fn remove_if(&self, turn_id: u64) -> Option<Arc<LlmTurn>> {
        let mut map = self.inner.lock().ok()?;
        let session = map
            .iter()
            .find(|(_, turn)| turn.turn_id == turn_id)
            .map(|(session, _)| session.clone())?;
        map.remove(&session)
    }

    pub fn get(&self, session_id: &str) -> Option<Arc<LlmTurn>> {
        self.inner.lock().ok()?.get(session_id).cloned()
    }

    pub fn is_active(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .map(|map| map.contains_key(session_id))
            .unwrap_or(false)
    }

    pub fn any(&self) -> bool {
        self.inner
            .lock()
            .map(|map| !map.is_empty())
            .unwrap_or(false)
    }

    /// Whether a newer turn already owns `session_id` — the winding-down turn
    /// must then leave the session's background tasks and serve processes
    /// alone (they belong to the newer turn).
    pub fn superseded(&self, session_id: &str, turn_id: u64) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).map(|turn| turn.turn_id > turn_id))
            .unwrap_or(false)
    }

    /// Running turns, oldest first — the History panel's badge source.
    pub fn list(&self) -> Vec<ActiveTurnInfo> {
        let Ok(map) = self.inner.lock() else {
            return Vec::new();
        };
        let mut turns: Vec<ActiveTurnInfo> = map.values().map(|turn| turn.info()).collect();
        turns.sort_by_key(|turn| turn.started_at_ms);
        turns
    }

    /// Full live snapshot of one session's running turn, or `None`.
    pub fn snapshot(&self, session_id: &str) -> Option<TurnSnapshot> {
        Some(self.get(session_id)?.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_supersedes_same_session_but_limits_new_ones() {
        // A same-session turn always supersedes, even at the limit.
        assert!(admission(true, 4, 4).is_ok());
        // New sessions need a free slot.
        assert!(admission(false, 3, 4).is_ok());
        let err = admission(false, 4, 4).unwrap_err();
        assert!(err.contains("limit 4"), "{err}");
        // 0 means unlimited; the hard ceiling clamps a hand-edited config.
        assert!(admission(false, 100, 0).is_ok());
        let err = admission(false, 16, 999).unwrap_err();
        assert!(err.contains("limit 16"), "{err}");
    }
}