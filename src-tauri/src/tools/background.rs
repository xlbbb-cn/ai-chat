// Background tool tasks: `run_cmd` / `run_shell` / `file_actions search` with
// `run_in_background: true` return a task id immediately instead of blocking
// the chat turn, and their result is injected into the conversation at the
// next request boundary.
//
// Why a registry and not a bare `tokio::spawn`:
// - the main tool loop must be able to *drain* finished results once and only
//   once (the OpenAI protocol has no place for a late `tool` message, so the
//   real output travels as a `user` message — see `llm_complete.rs`);
// - the turn stays open while something is still running, which needs a
//   wait/notify primitive rather than a busy loop;
// - the user and the model can cancel one task, a list of them, or every task
//   of a session, and the runner must observe that flag to kill its process
//   tree.
//
// Lifetime: tasks are turn-scoped, exactly like `run_shell` serve processes.
// `end_turn_cleanup` shuts a session down, which sets every cancel flag and
// drops the entries; the runners then kill their children and find no entry to
// report into.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::Notify;

/// Emitted after every state transition with the full snapshot, so the UI can
/// upsert the matching tool-call entry idempotently.
pub const BACKGROUND_TASK_STATE_EVENT: &str = "background-task-state";
/// Emitted when the turn is about to be held open waiting for results.
pub const BACKGROUND_WAITING_EVENT: &str = "background-waiting";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BgStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl BgStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BgStatus::Running => "running",
            BgStatus::Done => "done",
            BgStatus::Failed => "failed",
            BgStatus::Cancelled => "cancelled",
        }
    }
}

/// Serializable view of one task. `preview` is a short single-line excerpt so
/// the UI can show something without carrying the whole (possibly huge) body.
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskSnapshot {
    pub id: String,
    pub session_id: String,
    pub tool: String,
    pub label: String,
    pub status: BgStatus,
    pub started_at_ms: i64,
    pub elapsed_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// One finished result handed to the conversation (or to the `background_task`
/// control tool). Taking it marks the task delivered, so the same output can
/// never be injected twice.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub id: String,
    pub tool: String,
    pub label: String,
    pub status: BgStatus,
    pub elapsed_ms: i64,
    pub text: String,
}

impl Delivery {
    /// Full block for the model. `header` is false when several deliveries are
    /// concatenated by the control tool under one intro.
    pub fn render(&self) -> String {
        format!(
            "task: {} ({})\nlabel: {}\nstatus: {}\nelapsed: {:.1}s\noutput:\n{}",
            self.id,
            self.tool,
            self.label,
            self.status.as_str(),
            self.elapsed_ms as f64 / 1000.0,
            self.text
        )
    }
}

struct BackgroundTask {
    id: String,
    session_id: String,
    tool: String,
    label: String,
    started_at: Instant,
    started_at_ms: i64,
    finished_at: Option<Instant>,
    status: BgStatus,
    result: Option<String>,
    cancel: Arc<AtomicBool>,
    delivered: bool,
}

impl BackgroundTask {
    fn elapsed_ms(&self) -> i64 {
        self.finished_at
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started_at)
            .as_millis() as i64
    }

    fn snapshot(&self) -> BackgroundTaskSnapshot {
        BackgroundTaskSnapshot {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            tool: self.tool.clone(),
            label: self.label.clone(),
            status: self.status,
            started_at_ms: self.started_at_ms,
            elapsed_ms: self.elapsed_ms(),
            preview: self.result.as_deref().and_then(preview_of),
        }
    }

    fn delivery(&self) -> Option<Delivery> {
        Some(Delivery {
            id: self.id.clone(),
            tool: self.tool.clone(),
            label: self.label.clone(),
            status: self.status,
            elapsed_ms: self.elapsed_ms(),
            text: self.result.clone().unwrap_or_default(),
        })
    }
}

/// First non-empty line of a result, trimmed to a UI-friendly length.
fn preview_of(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut preview: String = line.chars().take(160).collect();
    if line.chars().count() > 160 {
        preview.push('…');
    }
    Some(preview)
}

/// Turn-scoped registry of background tool tasks. Every method is synchronous
/// (short critical sections); only the waits are async.
#[derive(Default)]
pub struct BackgroundRegistry {
    tasks: Mutex<HashMap<String, BackgroundTask>>,
    notify: Arc<Notify>,
}

impl BackgroundRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new running task and return its id plus the cancel flag the
    /// runner must poll. Prunes long-finished entries so the map cannot grow
    /// without bound over a long session.
    pub fn insert(&self, session_id: &str, tool: &str, label: &str) -> (String, Arc<AtomicBool>) {
        self.prune(Duration::from_secs(600));

        let id = format!(
            "bg-{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("unknown")
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);

        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.insert(
                id.clone(),
                BackgroundTask {
                    id: id.clone(),
                    session_id: session_id.to_string(),
                    tool: tool.to_string(),
                    label: label.to_string(),
                    started_at,
                    started_at_ms,
                    finished_at: None,
                    status: BgStatus::Running,
                    result: None,
                    cancel: cancel.clone(),
                    delivered: false,
                },
            );
        }

        (id, cancel)
    }

    /// Record the terminal state. A task already marked `Cancelled` (by a
    /// cancel request) keeps that status: the runner finishing afterwards must
    /// not turn a cancelled task into a green one.
    pub fn finish(
        &self,
        id: &str,
        status: BgStatus,
        result: Option<String>,
    ) -> Option<BackgroundTaskSnapshot> {
        let snapshot = {
            let mut tasks = self.tasks.lock().ok()?;
            let task = tasks.get_mut(id)?;
            if task.status != BgStatus::Cancelled {
                task.status = status;
            }
            task.finished_at = Some(Instant::now());
            task.result = result;
            task.snapshot()
        };
        self.notify.notify_waiters();
        Some(snapshot)
    }

    /// Finished, not-yet-delivered tasks of a session, marked delivered.
    pub fn take_deliverable(&self, session_id: &str) -> Vec<Delivery> {
        self.take_results(session_id, None)
    }

    /// Finished, not-yet-delivered tasks, optionally restricted to `ids`.
    /// Cancelled tasks are never delivered: the model already knows (it either
    /// cancelled them itself or the turn is ending).
    pub fn take_results(&self, session_id: &str, ids: Option<&[String]>) -> Vec<Delivery> {
        let Ok(mut tasks) = self.tasks.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for task in tasks.values_mut() {
            if task.session_id != session_id || task.delivered {
                continue;
            }
            if task.status == BgStatus::Running || task.status == BgStatus::Cancelled {
                continue;
            }
            if ids.is_some_and(|ids| !ids.iter().any(|id| id == &task.id)) {
                continue;
            }
            task.delivered = true;
            if let Some(delivery) = task.delivery() {
                out.push(delivery);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Running tasks of a session.
    pub fn pending(&self, session_id: &str) -> usize {
        self.tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| {
                        task.session_id == session_id && task.status == BgStatus::Running
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Running tasks plus finished results that have not been delivered yet.
    /// This is what keeps a turn open before its final answer.
    pub fn unsettled(&self, session_id: &str) -> usize {
        self.tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| {
                        task.session_id == session_id
                            && task.status != BgStatus::Cancelled
                            && (task.status == BgStatus::Running || !task.delivered)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn list(&self, session_id: &str) -> Vec<BackgroundTaskSnapshot> {
        let mut out: Vec<BackgroundTaskSnapshot> = self
            .tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| task.session_id == session_id)
                    .map(BackgroundTask::snapshot)
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms).then(a.id.cmp(&b.id)));
        out
    }

    pub fn get(&self, id: &str) -> Option<BackgroundTaskSnapshot> {
        self.tasks
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(id).map(BackgroundTask::snapshot))
    }

    /// Request cancellation. The runner observes the flag, kills its process
    /// tree and reports back; the status flips immediately so the UI reacts
    /// without waiting for the kill to finish.
    pub fn cancel(&self, id: &str) -> Option<BackgroundTaskSnapshot> {
        let snapshot = {
            let mut tasks = self.tasks.lock().ok()?;
            let task = tasks.get_mut(id)?;
            if task.status == BgStatus::Running {
                task.cancel.store(true, Ordering::SeqCst);
                task.status = BgStatus::Cancelled;
                task.finished_at = Some(Instant::now());
            }
            task.snapshot()
        };
        self.notify.notify_waiters();
        Some(snapshot)
    }

    /// Cancel every running task of a session. Returns how many were running.
    pub fn cancel_session(&self, session_id: &str) -> usize {
        let ids: Vec<String> = self
            .tasks
            .lock()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|task| {
                        task.session_id == session_id && task.status == BgStatus::Running
                    })
                    .map(|task| task.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let mut cancelled = 0;
        for id in ids {
            if self.cancel(&id).is_some() {
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Turn-end teardown: set every running cancel flag, then drop all of the
    /// session's entries. The runners still complete (and kill their children)
    /// but find nothing to report into, which is exactly what "the turn is
    /// over" means.
    pub fn shutdown_session(&self, session_id: &str) -> usize {
        let Ok(mut tasks) = self.tasks.lock() else {
            return 0;
        };
        for task in tasks.values() {
            if task.session_id == session_id && task.status == BgStatus::Running {
                task.cancel.store(true, Ordering::SeqCst);
            }
        }
        let before = tasks.len();
        tasks.retain(|_, task| task.session_id != session_id);
        let removed = before - tasks.len();
        drop(tasks);
        self.notify.notify_waiters();
        removed
    }

    /// Wait until the session has no *running* task, or the timeout elapses.
    ///
    /// Finished-but-undelivered results do not count: they are already ready
    /// and the caller drains them at the next request boundary. Waiting on
    /// `unsettled` here would hang until the timeout on a result that arrived
    /// a moment ago.
    pub async fn wait_until_idle(&self, session_id: &str, timeout: Duration) {
        self.wait_until(|| self.pending(session_id) == 0, timeout)
            .await;
    }

    /// Wait until every given id is finished (or the timeout elapses).
    pub async fn wait_for_tasks(&self, ids: &[String], timeout: Duration) {
        self.wait_until(
            || {
                let Ok(tasks) = self.tasks.lock() else {
                    return true;
                };
                ids.iter().all(|id| {
                    tasks
                        .get(id)
                        .map(|task| task.status != BgStatus::Running)
                        .unwrap_or(true)
                })
            },
            timeout,
        )
        .await;
    }

    async fn wait_until<F>(&self, ready: F, timeout: Duration)
    where
        F: Fn() -> bool,
    {
        if ready() {
            return;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            let notified = self.notify.notified();
            // The 200 ms cap guarantees forward progress even if a state
            // change lands between `ready()` and the waiter registration.
            let slice = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(200));
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(slice) => {}
            }
            if ready() {
                return;
            }
        }
    }

    /// Drop delivered entries older than `max_age` so a long-lived app does not
    /// accumulate every task it ever ran.
    fn prune(&self, max_age: Duration) {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain(|_, task| {
                task.finished_at
                    .map(|finished| finished.elapsed() < max_age)
                    .unwrap_or(true)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_results_are_delivered_exactly_once() {
        let registry = BackgroundRegistry::new();
        let (id, _cancel) = registry.insert("s1", "run_shell", "sleep 60");
        assert_eq!(registry.pending("s1"), 1);
        assert_eq!(registry.unsettled("s1"), 1);

        registry.finish(&id, BgStatus::Done, Some("hello".to_string()));
        assert_eq!(registry.pending("s1"), 0);
        assert_eq!(registry.unsettled("s1"), 1, "undelivered result keeps the turn open");

        let first = registry.take_deliverable("s1");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text, "hello");
        assert!(registry.take_deliverable("s1").is_empty());
        assert_eq!(registry.unsettled("s1"), 0);
    }

    #[tokio::test]
    async fn wait_until_idle_does_not_wait_for_an_undelivered_result() {
        let registry = BackgroundRegistry::new();
        let (id, _) = registry.insert("s1", "run_cmd", "x");
        registry.finish(&id, BgStatus::Done, Some("ok".to_string()));

        let started = Instant::now();
        registry
            .wait_until_idle("s1", Duration::from_secs(5))
            .await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a finished result must be ready to drain immediately"
        );
        assert_eq!(registry.unsettled("s1"), 1);
    }

    #[test]
    fn cancelled_tasks_are_never_delivered() {
        let registry = BackgroundRegistry::new();
        let (id, cancel) = registry.insert("s1", "run_shell", "sleep 60");

        let snapshot = registry.cancel(&id).unwrap();
        assert_eq!(snapshot.status, BgStatus::Cancelled);
        assert!(cancel.load(Ordering::SeqCst));

        // The runner finishing afterwards must not overwrite the cancellation.
        let snapshot = registry.finish(&id, BgStatus::Done, Some("late".to_string())).unwrap();
        assert_eq!(snapshot.status, BgStatus::Cancelled);
        assert!(registry.take_deliverable("s1").is_empty());
        assert_eq!(registry.unsettled("s1"), 0);
    }

    #[test]
    fn sessions_are_isolated() {
        let registry = BackgroundRegistry::new();
        let (a, _) = registry.insert("s1", "run_cmd", "a");
        let (b, _) = registry.insert("s2", "run_cmd", "b");
        registry.finish(&a, BgStatus::Failed, Some("Error: boom".to_string()));
        registry.finish(&b, BgStatus::Done, Some("ok".to_string()));

        assert_eq!(registry.list("s1").len(), 1);
        assert_eq!(registry.list("s2").len(), 1);
        assert_eq!(registry.take_deliverable("s1").len(), 1);
        assert_eq!(registry.take_deliverable("s2").len(), 1);
    }

    #[test]
    fn take_results_can_be_restricted_to_ids() {
        let registry = BackgroundRegistry::new();
        let (a, _) = registry.insert("s1", "run_cmd", "a");
        let (b, _) = registry.insert("s1", "run_cmd", "b");
        registry.finish(&a, BgStatus::Done, Some("A".to_string()));
        registry.finish(&b, BgStatus::Done, Some("B".to_string()));

        let picked = registry.take_results("s1", Some(std::slice::from_ref(&b)));
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, b);
        // The other one is still deliverable at the next boundary.
        let rest = registry.take_deliverable("s1");
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, a);
    }

    #[test]
    fn shutdown_session_drops_everything_and_flags_running_tasks() {
        let registry = BackgroundRegistry::new();
        let (running, cancel) = registry.insert("s1", "run_shell", "long");
        let (done, _) = registry.insert("s1", "run_cmd", "short");
        registry.finish(&done, BgStatus::Done, Some("ok".to_string()));

        assert_eq!(registry.shutdown_session("s1"), 2);
        assert!(cancel.load(Ordering::SeqCst));
        assert!(registry.list("s1").is_empty());
        assert!(registry.get(&running).is_none());
    }

    #[tokio::test]
    async fn wait_until_idle_returns_when_the_last_task_finishes() {
        let registry = Arc::new(BackgroundRegistry::new());
        let (id, _) = registry.insert("s1", "run_cmd", "x");

        let waiter = {
            let registry = registry.clone();
            tokio::spawn(async move {
                registry
                    .wait_until_idle("s1", Duration::from_secs(5))
                    .await;
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        registry.finish(&id, BgStatus::Done, Some("ok".to_string()));
        waiter.await.unwrap();
        assert_eq!(registry.pending("s1"), 0);
    }

    #[tokio::test]
    async fn wait_until_idle_gives_up_after_the_timeout() {
        let registry = BackgroundRegistry::new();
        let (_id, _) = registry.insert("s1", "run_cmd", "x");
        let started = Instant::now();
        registry
            .wait_until_idle("s1", Duration::from_millis(50))
            .await;
        assert!(started.elapsed() >= Duration::from_millis(40));
    }
}
