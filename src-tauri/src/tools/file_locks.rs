//! Cross-conversation file race protection for `file_actions`.
//!
//! Several chat sessions can run turns at once in one workspace, and two of
//! them editing the same file blind is how silent overwrites happen. Two
//! complementary defences, both scoped to the structured `file_actions` tool:
//!
//! * **Path locks** — every mutating action (write / patch / delete / move)
//!   takes per-path async mutexes first, so two mutating calls on the same
//!   path can never interleave their backup + write + note sequence, and a
//!   `.bak` copy never snapshots a half-applied state. Multiple paths (move)
//!   are locked in sorted order, so two moves in opposite directions cannot
//!   deadlock.
//! * **Stale-write rejection** — the registry remembers, per path, who wrote
//!   it last and when, plus the last time each session read it. When a session
//!   writes a file that ANOTHER session modified after this session's last
//!   read, the write is refused with an explicit "re-read and retry" error
//!   instead of clobbering the other conversation's change. `patch` still has
//!   its own context matching as a second line of defence; `write` is a blind
//!   overwrite, so it is the case this guard exists for.
//!
//! Known boundary: `run_shell` / `run_cmd` can modify any file without going
//! through here — they are the user's free-form escape hatch by design. The
//! registry is in-memory only and clears on restart, so a session that
//! outlives the app starts clean.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Who wrote a path last, and when (monotonic clock).
struct PathRecord {
    last_write: Option<(String, Instant)>,
}

#[derive(Default)]
pub struct FileLockRegistry {
    /// One lock per path, created on demand. The map itself is short-lived
    /// state (`std::sync::Mutex`); the values are `tokio` mutexes because the
    /// actions run inside async commands.
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    records: Mutex<HashMap<PathBuf, PathRecord>>,
    /// `(session, path)` → last read instant. Pruned by writes and deletes;
    /// entries are small and bounded by sessions × paths actually touched.
    reads: Mutex<HashMap<(String, PathBuf), Instant>>,
}

/// Stable key for a path: components instead of the raw spelling, so `a/b`,
/// `a//b` and `./a/b` share one entry. Paths that reach here already resolved
/// through `resolve_input_path` (absolute, workspace-contained); the
/// components pass is just the last normalisation step.
fn lock_key(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Session key used when the caller has no chat session (a panel-driven call,
/// a background task without one). Such callers do not participate in the
/// staleness check — their writes are recorded under this key and never count
/// as "another conversation modified the file".
const ANONYMOUS_SESSION: &str = "(no session)";

impl FileLockRegistry {
    /// Record that `session` read `path` (a successful read / list / diff).
    pub fn record_read(&self, session: Option<&str>, path: &Path) {
        let session = session.unwrap_or(ANONYMOUS_SESSION);
        if session == ANONYMOUS_SESSION {
            return;
        }
        if let Ok(mut reads) = self.reads.lock() {
            reads.insert((session.to_string(), lock_key(path)), Instant::now());
        }
    }

    /// Record that `session` successfully wrote `path`.
    pub fn note_write(&self, session: Option<&str>, path: &Path) {
        let session = session.unwrap_or(ANONYMOUS_SESSION).to_string();
        let key = lock_key(path);
        if let Ok(mut records) = self.records.lock() {
            records.insert(
                key.clone(),
                PathRecord {
                    last_write: Some((session.clone(), Instant::now())),
                },
            );
        }
        // This session has seen the newest state of the path; its read stamp
        // would only age the comparison.
        if let Ok(mut reads) = self.reads.lock() {
            reads.remove(&(session, key));
        }
    }

    /// Forget the write record of a path that no longer exists (delete / move
    /// source), so a recreated file does not inherit a stale-history verdict.
    pub fn forget(&self, path: &Path) {
        let key = lock_key(path);
        if let Ok(mut records) = self.records.lock() {
            records.remove(&key);
        }
        if let Ok(mut reads) = self.reads.lock() {
            reads.retain(|(_, p), _| p != &key);
        }
    }

    /// Lock `paths` (sorted, deadlock-free) and run `f` inside the guards
    /// after the staleness check passed for every path.
    ///
    /// The check-then-write happens entirely inside the locks: two sessions
    /// writing the same path are serialised, and the second one sees the
    /// first one's `note_write` — so the only way through is a fresh read.
    /// `f` must be synchronous file IO (no awaits between taking the guards
    /// and dropping them) and returns the tool result; a result that reads
    /// like an error (the same `tool_output_is_error` convention every tool
    /// layer uses) did NOT touch the file, so no new "last write" is
    /// published for it.
    pub async fn with_write_guard(
        &self,
        session: Option<&str>,
        paths: &[PathBuf],
        f: impl FnOnce() -> String,
    ) -> Result<String, String> {
        let session = session.unwrap_or(ANONYMOUS_SESSION);
        let mut entries: Vec<(PathBuf, PathBuf)> =
            paths.iter().map(|p| (lock_key(p), p.clone())).collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);

        // Collect the locks in sorted order. Holding the map lock only while
        // looking up / inserting keeps contention off the actions; the Arcs
        // outlive their guards, so the guards can be held across the action.
        let pending: Vec<Arc<tokio::sync::Mutex<()>>> = {
            let mut map = self
                .locks
                .lock()
                .map_err(|_| "file lock registry is unavailable".to_string())?;
            entries
                .iter()
                .map(|(key, _)| map.entry(key.clone()).or_default().clone())
                .collect()
        };
        let mut guards: Vec<tokio::sync::MutexGuard<'_, ()>> =
            Vec::with_capacity(pending.len());
        for lock in &pending {
            guards.push(lock.lock().await);
        }

        for (key, _) in &entries {
            if let Some(reason) = self.stale_reason(session, key) {
                return Err(reason);
            }
        }

        let result = f();
        // A failed action leaves the file untouched (its result reads like an
        // error, the same convention `tool_output_is_error` uses everywhere),
        // so no new "last write" must be published for it.
        if !crate::turn_guard::tool_output_is_error(&result) {
            for (_, path) in &entries {
                self.note_write(Some(session), path);
            }
        }
        drop(guards);
        Ok(result)
    }

    /// Why this write must be refused, or `None` when it is safe.
    fn stale_reason(&self, session: &str, key: &Path) -> Option<String> {
        if session == ANONYMOUS_SESSION {
            return None;
        }
        let records = self.records.lock().ok()?;
        let last_write = records.get(key)?.last_write.as_ref()?;
        let (writer, at) = last_write;
        if writer == session {
            // Our own last write: the session already owns the newest state.
            return None;
        }
        let read_at = self.reads.lock().ok()?.get(&(session.to_string(), key.to_path_buf())).copied();
        let display = key.to_string_lossy().into_owned();
        Some(match read_at {
            // The session read the file after the other conversation's write:
            // it has seen the newest state, so its write is safe.
            Some(read_at) if *at <= read_at => return None,
            Some(_) => format!(
                "Error: STALE WRITE — '{display}' was modified by another conversation after you last \
                 read it. Re-read the file and apply your change again; do not repeat the same write.",
            ),
            None => format!(
                "Error: STALE WRITE — '{display}' was modified by another conversation and you have not \
                 read it since. Read the file first, then apply your change on top of its current content."
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/ws").join(name)
    }

    #[tokio::test]
    async fn stale_write_is_refused_and_a_fresh_read_clears_it() {
        let registry = FileLockRegistry::default();
        let p = path("doc.md");
        registry.record_read(Some("a"), &p);
        // Session b writes after a's read.
        registry
            .with_write_guard(Some("b"), &[p.clone()], || String::new())
            .await
            .unwrap();

        // a writes blind → refused, with a re-read instruction.
        let err = registry
            .with_write_guard(Some("a"), &[p.clone()], || "nope".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("STALE WRITE"), "{err}");
        assert!(err.contains("Re-read"), "{err}");

        // After re-reading, the write goes through and becomes the newest.
        registry.record_read(Some("a"), &p);
        let result = registry
            .with_write_guard(Some("a"), &[p.clone()], || "ok".to_string())
            .await
            .unwrap();
        assert_eq!(result, "ok");
    }

    #[tokio::test]
    async fn a_sessions_own_writes_never_count_as_stale() {
        let registry = FileLockRegistry::default();
        let p = path("own.md");
        registry.record_read(Some("a"), &p);
        registry
            .with_write_guard(Some("a"), &[p.clone()], || String::new())
            .await
            .unwrap();
        // Same session writes again without re-reading: allowed.
        registry
            .with_write_guard(Some("a"), &[p.clone()], || String::new())
            .await
            .unwrap();
        // A different path the session never read is also allowed (new file).
        registry
            .with_write_guard(Some("a"), &[path("new.md")], || String::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn deleting_forgets_the_history_and_anonymous_never_blocked() {
        let registry = FileLockRegistry::default();
        let p = path("gone.md");
        registry.record_read(Some("a"), &p);
        registry
            .with_write_guard(Some("b"), &[p.clone()], || String::new())
            .await
            .unwrap();
        registry.forget(&p);
        // The stale verdict is gone; a fresh write of a recreated file passes.
        registry
            .with_write_guard(Some("a"), &[p.clone()], || String::new())
            .await
            .unwrap();
        // Panel calls (no session) are never refused.
        registry
            .with_write_guard(None, &[p.clone()], || String::new())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_writes_serialise_and_the_later_one_is_the_newest() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let registry = Arc::new(FileLockRegistry::default());
        let p = path("shared.md");
        let inside = Arc::new(AtomicUsize::new(0));
        let max_inside = Arc::new(AtomicUsize::new(0));

        // Two writers without reads between them: the first write records
        // itself, so the second one MUST be refused instead of interleaving.
        let r1 = registry.clone();
        let p1 = p.clone();
        let i1 = inside.clone();
        let m1 = max_inside.clone();
        let first = tokio::spawn(async move {
            r1.with_write_guard(Some("a"), &[p1], move || {
                let now = i1.fetch_add(1, Ordering::SeqCst) + 1;
                m1.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(20));
                i1.fetch_sub(1, Ordering::SeqCst);
                String::new()
            })
            .await
        });
        let r2 = registry.clone();
        let p2 = p.clone();
        let second = tokio::spawn(async move {
            r2.with_write_guard(Some("b"), &[p2], || String::new())
                .await
        });
        let (a, b) = tokio::join!(first, second);
        let a = a.unwrap();
        let b = b.unwrap();
        // Exactly one of the two succeeded; whichever failed did so with the
        // stale error, never by racing inside the guard.
        assert!(a.is_ok() ^ b.is_ok(), "a={a:?} b={b:?}");
        if let Err(err) = a {
            assert!(err.contains("STALE WRITE"), "{err}");
        }
        if let Err(err) = b {
            assert!(err.contains("STALE WRITE"), "{err}");
        }
        assert_eq!(max_inside.load(Ordering::SeqCst), 1, "guards overlapped");
    }

    #[tokio::test]
    async fn opposite_moves_lock_in_sorted_order_and_do_not_deadlock() {
        let registry = Arc::new(FileLockRegistry::default());
        let a = path("one");
        let b = path("two");

        let r1 = registry.clone();
        let (a1, b1) = (a.clone(), b.clone());
        let first = tokio::spawn(async move {
            r1.with_write_guard(Some("s1"), &[a1, b1], || String::new()).await
        });
        let r2 = registry.clone();
        let (a2, b2) = (a, b);
        let second = tokio::spawn(async move {
            r2.with_write_guard(Some("s2"), &[b2, a2], || String::new()).await
        });
        // The join resolves on both sides: sorted locking prevents the
        // lock-order inversion a naive two-lock take would create.
        let (x, y) = tokio::join!(first, second);
        let x = x.unwrap();
        let y = y.unwrap();
        // The first mover wins; the second one is refused (both paths carry
        // the first mover's write) — exactly the staleness verdict, never a
        // deadlock. Either task may run first, so assert the shape, not the
        // winner.
        assert!(x.is_ok() ^ y.is_ok(), "x={x:?} y={y:?}");
        if let Err(err) = x {
            assert!(err.contains("STALE WRITE"), "{err}");
        }
        if let Err(err) = y {
            assert!(err.contains("STALE WRITE"), "{err}");
        }
    }
}