// `run_shell` serve processes: long-lived background children (HTTP servers,
// watchers, dev daemons…) that survive the tool call and can be listed/stopped
// later with the same tool.
//
// Registry semantics:
// - `start` spawns the child, streams stdout/stderr into a bounded buffer and
//   returns immediately with a serve id.
// - `stop` terminates the whole process tree and removes the entry.
// - `list` reports running/finished entries with their recent output.
// - The Tools panel polls `list_serve_processes` for the same data and stops
//   entries through `stop_serve_process`, so the user does not need a chat turn
//   to manage background servers.
// - The pid is also registered in `crate::process_guard`, so app exit (and a
//   Windows hard crash, via the kill-on-close job object) cleans it up.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::process::{build_command, decode_hint_for_shell, decode_process_bytes};
use crate::process_guard;

/// Output kept per serve process (head is dropped first once the budget is spent).
const MAX_OUTPUT_CHARS: usize = 16_000;
/// How much of the script is echoed back in lists/start results.
const CODE_PREVIEW_CHARS: usize = 240;
/// Finished entries are kept this long for `list`, then pruned on the next start.
const FINISHED_RETENTION: Duration = Duration::from_secs(600);
/// Number of tail lines handed to the Tools panel for live monitoring.
const PANEL_OUTPUT_LINES: usize = 8;
/// How long turn-end shutdown keeps verifying that killed trees are gone.
const SHUTDOWN_VERIFY_DEADLINE: Duration = Duration::from_secs(3);
/// Poll interval while verifying killed trees.
const SHUTDOWN_VERIFY_POLL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct ServeStatus {
    state: String,
    exit_code: Option<i32>,
    output: String,
}

pub struct ServeProcess {
    pub id: String,
    pub pid: u32,
    pub shell_type: String,
    pub code: String,
    pub cwd: PathBuf,
    pub started_at: Instant,
    status: Arc<Mutex<ServeStatus>>,
}

/// Snapshot of one serve process, consumed by the Tools panel monitor.
#[derive(Debug, Clone, Serialize)]
pub struct ServeProcessInfo {
    pub id: String,
    pub pid: u32,
    pub shell_type: String,
    /// Script preview, clamped to [`CODE_PREVIEW_CHARS`].
    pub code: String,
    pub cwd: String,
    /// `running`, `exited`, `lost`, `stopped` or `unknown`.
    pub state: String,
    pub exit_code: Option<i32>,
    /// Seconds since the process was started.
    pub uptime_secs: u64,
    /// Tail of stdout/stderr, truncated to [`PANEL_OUTPUT_LINES`] lines.
    pub recent_output: String,
}

/// Registry held in `AppState`; shared by `run_shell` calls from every agent.
#[derive(Default)]
pub struct ServeRegistry {
    processes: Mutex<HashMap<String, ServeProcess>>,
}

impl ServeRegistry {
    fn prune_finished(&self, processes: &mut HashMap<String, ServeProcess>) {
        processes.retain(|_, process| {
            let running = process
                .status
                .lock()
                .map(|status| status.state == "running")
                .unwrap_or(true);
            running || process.started_at.elapsed() < FINISHED_RETENTION
        });
    }

    fn insert(&self, process: ServeProcess) {
        if let Ok(mut processes) = self.processes.lock() {
            self.prune_finished(&mut processes);
            processes.insert(process.id.clone(), process);
        }
    }

    /// `serve_id` may be a concrete id, `all` or `*` for every running process.
    pub fn stop(&self, serve_id: &str) -> String {
        let serve_id = serve_id.trim();
        if serve_id.is_empty() {
            return "Error: action 'stop' requires a non-empty 'serve_id'.".to_string();
        }

        if serve_id.eq_ignore_ascii_case("all") || serve_id == "*" {
            return self.stop_all();
        }

        let mut processes = match self.processes.lock() {
            Ok(processes) => processes,
            Err(err) => return format!("Error: serve registry is unavailable: {err}"),
        };

        let Some(process) = processes.remove(serve_id) else {
            return format!(
                "Error: no serve process with id '{serve_id}'. Use action 'list' to see the running serve processes."
            );
        };
        drop(processes);

        stop_entry(process)
    }

    fn stop_all(&self) -> String {
        let processes: Vec<ServeProcess> = match self.processes.lock() {
            Ok(mut processes) => processes.drain().map(|(_, process)| process).collect(),
            Err(err) => return format!("Error: serve registry is unavailable: {err}"),
        };

        let total = processes.len();
        let mut running = 0usize;
        for process in processes {
            let was_running = process
                .status
                .lock()
                .map(|status| status.state == "running")
                .unwrap_or(true);
            if was_running {
                running += 1;
            }
            stop_entry(process);
        }

        if total == 0 {
            "No background serve processes are running.".to_string()
        } else {
            format!(
                "✅ Stopped {running} running background serve process(es); removed {total} registry entries."
            )
        }
    }

    /// Turn-end shutdown: stop every serve process and then VERIFY that each
    /// killed tree is actually gone before returning. Escalates (re-kill of
    /// the whole tree) once for stragglers and reports anything that could
    /// still not be confirmed. The returned summary is app-log material.
    ///
    /// Synchronous by design and blocking up to
    /// [`SHUTDOWN_VERIFY_DEADLINE`] in the pathological case; call it through
    /// `spawn_blocking` from async command contexts.
    pub fn shutdown_all(&self) -> String {
        // Which entries are still running (and thus need post-kill
        // verification)? Finished entries are just registry bookkeeping.
        let running: Vec<(String, u32)> = {
            let processes = match self.processes.lock() {
                Ok(processes) => processes,
                Err(err) => return format!("Error: serve registry is unavailable: {err}"),
            };
            processes
                .values()
                .filter(|process| {
                    process
                        .status
                        .lock()
                        .map(|status| status.state == "running")
                        .unwrap_or(true)
                })
                .filter(|process| process.pid != 0)
                .map(|process| (process.id.clone(), process.pid))
                .collect()
        };

        let stopped = self.stop_all();
        if running.is_empty() {
            return stopped;
        }

        let ids: Vec<&str> = running.iter().map(|(id, _)| id.as_str()).collect();
        let unresolved = verify_trees_exited(&running);
        if unresolved.is_empty() {
            format!(
                "{stopped} Turn ended: killed serve trees [{}] — exit verified for all.",
                ids.join(", ")
            )
        } else {
            format!(
                "{stopped} Turn ended: killed serve trees [{}] — WARNING, exit NOT confirmed for: {}.",
                ids.join(", "),
                unresolved.join(", ")
            )
        }
    }

    pub fn list(&self) -> String {
        let processes = match self.processes.lock() {
            Ok(processes) => processes,
            Err(err) => return format!("Error: serve registry is unavailable: {err}"),
        };

        if processes.is_empty() {
            return "No background serve processes are running.".to_string();
        }

        let mut entries: Vec<&ServeProcess> = processes.values().collect();
        entries.sort_by_key(|process| process.started_at);

        let mut output = format!("Background serve processes ({}):\n", entries.len());
        for process in entries {
            let status = match process.status.lock() {
                Ok(status) => status,
                Err(_) => continue,
            };

            output.push_str(&format!(
                "\n- {}  [{}]  pid {}\n  shell: {}\n  started: {}s ago\n  cwd: {}\n  code: {}",
                process.id,
                if status.state.is_empty() {
                    "unknown"
                } else {
                    &status.state
                },
                process.pid,
                process.shell_type,
                process.started_at.elapsed().as_secs(),
                process.cwd.display(),
                truncate_chars(&process.code, CODE_PREVIEW_CHARS),
            ));
            if let Some(code) = status.exit_code {
                output.push_str(&format!("\n  exit code: {code}"));
            }

            let recent = recent_output(&status.output, 10);
            if !recent.trim().is_empty() {
                output.push_str("\n  recent output:\n");
                for line in recent.lines() {
                    output.push_str("    ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }

        output
    }

    /// Structured snapshot for the Tools panel's serve monitor. Finished
    /// entries are included so the user can inspect why a process exited and
    /// remove the entry without waiting for the retention window.
    pub fn snapshot(&self) -> Vec<ServeProcessInfo> {
        let mut processes = match self.processes.lock() {
            Ok(processes) => processes,
            Err(_) => return Vec::new(),
        };
        // Polled by the panel, so this is also the natural place to enforce the
        // finished-entry retention window without waiting for the next start.
        self.prune_finished(&mut processes);

        let mut entries: Vec<&ServeProcess> = processes.values().collect();
        entries.sort_by_key(|process| process.started_at);

        entries
            .into_iter()
            .filter_map(|process| {
                let status = process.status.lock().ok()?;
                Some(ServeProcessInfo {
                    id: process.id.clone(),
                    pid: process.pid,
                    shell_type: process.shell_type.clone(),
                    code: truncate_chars(&process.code, CODE_PREVIEW_CHARS),
                    cwd: process.cwd.display().to_string(),
                    state: if status.state.is_empty() {
                        "unknown".to_string()
                    } else {
                        status.state.clone()
                    },
                    exit_code: status.exit_code,
                    uptime_secs: process.started_at.elapsed().as_secs(),
                    recent_output: recent_output(&status.output, PANEL_OUTPUT_LINES),
                })
            })
            .collect()
    }
}

fn stop_entry(process: ServeProcess) -> String {
    let was_running = process
        .status
        .lock()
        .map(|mut status| {
            if status.state == "running" {
                status.state = "stopped".to_string();
                true
            } else {
                false
            }
        })
        .unwrap_or(false);

    if was_running {
        process_guard::kill_process_tree(Some(process.pid));
    }

    let verb = if was_running {
        "Stopped"
    } else {
        "Removed finished"
    };
    format!(
        "✅ {verb} serve process {} (pid {}, previous state: {})",
        process.id,
        process.pid,
        if was_running {
            "running"
        } else {
            "not running"
        }
    )
}

/// True when the pid can no longer be waited on. Platform notes:
/// - Windows: wait on a fresh `PROCESS_SYNCHRONIZE` handle; an unreachable
///   OpenProcess (invalid pid, exited and reaped) counts as gone. Signal
///   happens at process termination, independent of any reaping.
/// - Linux: `/proc/<pid>/stat` state — a zombie means the process is dead
///   (every resource freed except the pid slot), so it counts as gone.
/// - Other Unix: `kill(pid, 0)`; the serve child is reaped by this process's
///   watcher task right after the kill, so the small zombie window between
///   death and reap is the only false "alive" here.
#[cfg(windows)]
fn pid_gone(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let waited = WaitForSingleObject(handle, 0);
        let _ = CloseHandle(handle);
        waited == WAIT_OBJECT_0
    }
}

#[cfg(target_os = "linux")]
fn pid_gone(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true; // No /proc entry: the process is gone.
    };
    // Format: `pid (comm) state ...` — comm may contain spaces/parens, so
    // locate the state after the LAST ')'.
    let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
        return true;
    };
    // State is the first non-space field; 'Z' = zombie = dead.
    !rest.trim_start().starts_with('Z')
}

#[cfg(all(unix, not(target_os = "linux")))]
fn pid_gone(pid: u32) -> bool {
    let delivered = unsafe { libc::kill(pid as i32, 0) };
    delivered == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(any(windows, unix)))]
fn pid_gone(pid: u32) -> bool {
    let _ = pid;
    true
}

/// Wait until every `(id, pid)` tree is confirmed gone, re-killing the whole
/// tree once for any straggler. Returns the ids that could not be confirmed
/// within the deadline (normally empty).
fn verify_trees_exited(targets: &[(String, u32)]) -> Vec<String> {
    if targets.is_empty() {
        return Vec::new();
    }

    let deadline = Instant::now() + SHUTDOWN_VERIFY_DEADLINE;
    let mut pending: Vec<(String, u32)> = targets
        .iter()
        .filter(|(_, pid)| !pid_gone(*pid))
        .cloned()
        .collect();
    let mut escalated = false;

    while !pending.is_empty() && Instant::now() < deadline {
        std::thread::sleep(SHUTDOWN_VERIFY_POLL);
        pending.retain(|(_, pid)| !pid_gone(*pid));
        if !pending.is_empty() && !escalated {
            // One poll cycle was not enough: re-kill the whole trees once
            // (the same mechanism the initial stop used) and keep waiting.
            escalated = true;
            for (_, pid) in &pending {
                process_guard::kill_process_tree(Some(*pid));
            }
        }
    }

    pending.into_iter().map(|(id, _)| id).collect()
}

/// Start a long-lived child for `code` and return the tool result immediately.
pub async fn start(
    app: &AppHandle,
    shell_type: &str,
    code: &str,
    scoped_code: String,
    cwd: PathBuf,
) -> String {
    let mut command = match build_command(shell_type, &scoped_code, Some(cwd.clone())) {
        Ok(command) => command,
        Err(err) => return format!("Error: {err}"),
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return format!("Error: failed to start serve process: {err}"),
    };

    let pid = child.id().unwrap_or(0);
    let id = format!(
        "serve-{}",
        uuid::Uuid::new_v4()
            .to_string()
            .split('-')
            .next()
            .unwrap_or("unknown")
    );

    let status = Arc::new(Mutex::new(ServeStatus {
        state: "running".to_string(),
        exit_code: None,
        output: String::new(),
    }));
    let decode_hint = decode_hint_for_shell(shell_type);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(stdout) = stdout {
        tokio::spawn(pump(stdout, status.clone(), decode_hint, false));
    }
    if let Some(stderr) = stderr {
        tokio::spawn(pump(stderr, status.clone(), decode_hint, true));
    }

    let tracked = process_guard::track_child(&child, format!("serve {id}"));
    let watcher_status = status.clone();
    log_start(app, &id, pid, shell_type, code, &cwd);
    tokio::spawn(async move {
        let result = child.wait().await;
        // Dropping `tracked` removes the pid from the app-exit registry.
        drop(tracked);

        let mut status = match watcher_status.lock() {
            Ok(status) => status,
            Err(_) => return,
        };
        if status.state != "running" {
            return;
        }
        match result {
            Ok(exit) => {
                status.state = "exited".to_string();
                status.exit_code = exit.code();
            }
            Err(err) => {
                status.state = "lost".to_string();
                append_limited(
                    &mut status.output,
                    &format!("\n[serve process wait failed: {err}]\n"),
                );
            }
        }
    });

    let state = app.state::<crate::AppState>();
    state.serve_processes.insert(ServeProcess {
        id: id.clone(),
        pid,
        shell_type: shell_type.to_string(),
        code: code.to_string(),
        cwd,
        started_at: Instant::now(),
        status,
    });

    format!(
        "✅ Background serve process started.\n\
         serve_id: {id}\n\
         pid: {pid}\n\
         shell: {shell_type}\n\
         status: running\n\n\
         It keeps running for the rest of this chat turn. Manage it with:\n\
         - `run_shell` {{\"type\":\"{shell_type}\",\"run_type\":\"serve\",\"action\":\"list\"}}\n\
         - `run_shell` {{\"type\":\"{shell_type}\",\"run_type\":\"serve\",\"action\":\"stop\",\"serve_id\":\"{id}\"}}\n\n\
         The process tree is stopped with verified termination when this chat turn ends, and at app exit."
    )
}

fn log_start(app: &AppHandle, id: &str, pid: u32, shell_type: &str, code: &str, cwd: &PathBuf) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.logger.lock().unwrap().log(
            "INFO",
            &format!(
                "Started serve process {id} (pid {pid}, shell {shell_type}, cwd {}): {}",
                cwd.display(),
                truncate_chars(code, CODE_PREVIEW_CHARS)
            ),
        );
    }
}

async fn pump<R>(
    reader: R,
    status: Arc<Mutex<ServeStatus>>,
    decode_hint: super::process::OutputDecodeHint,
    stderr: bool,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let line = decode_process_bytes(&buffer, decode_hint);
        let mut status = match status.lock() {
            Ok(status) => status,
            Err(_) => return,
        };
        if stderr {
            append_limited(&mut status.output, "[stderr] ");
        }
        append_limited(&mut status.output, &line);
    }
}

fn append_limited(buffer: &mut String, text: &str) {
    buffer.push_str(text);
    let total = buffer.chars().count();
    if total > MAX_OUTPUT_CHARS {
        *buffer = buffer
            .chars()
            .skip(total - MAX_OUTPUT_CHARS)
            .collect::<String>();
    }
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max).collect();
    truncated.push_str(" …");
    truncated
}

fn recent_output(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

/// Every serve process the app knows about (running and recently finished),
/// oldest first. Polled by the Tools panel's serve monitor.
#[tauri::command]
pub fn list_serve_processes(state: State<'_, crate::AppState>) -> Vec<ServeProcessInfo> {
    state.serve_processes.snapshot()
}

/// Stop one serve process (or drop a finished entry). Returns the same
/// human-readable status the `run_shell` tool prints for `action: "stop"`.
#[tauri::command]
pub fn stop_serve_process(state: State<'_, crate::AppState>, serve_id: String) -> String {
    state.serve_processes.stop(&serve_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished_process(id: &str) -> ServeProcess {
        ServeProcess {
            id: id.to_string(),
            // Never killed by these tests: the state is not `running`.
            pid: u32::MAX,
            shell_type: "bash".to_string(),
            code: "echo hi".to_string(),
            cwd: PathBuf::from("."),
            started_at: Instant::now(),
            status: Arc::new(Mutex::new(ServeStatus {
                state: "exited".to_string(),
                exit_code: Some(0),
                output: "hello\nworld\n".to_string(),
            })),
        }
    }

    #[test]
    fn lists_and_removes_finished_processes() {
        let registry = ServeRegistry::default();
        registry.insert(finished_process("serve-test1"));

        let listed = registry.list();
        assert!(listed.contains("serve-test1"));
        assert!(listed.contains("[exited]"));
        assert!(listed.contains("hello"));

        let stopped = registry.stop("serve-test1");
        assert!(stopped.contains("Removed finished"));
        assert!(registry.list().contains("No background serve processes"));
    }

    #[test]
    fn snapshot_reports_panel_fields() {
        let registry = ServeRegistry::default();
        registry.insert(finished_process("serve-test2"));

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        let entry = &snapshot[0];
        assert_eq!(entry.id, "serve-test2");
        assert_eq!(entry.state, "exited");
        assert_eq!(entry.exit_code, Some(0));
        assert_eq!(entry.shell_type, "bash");
        assert!(entry.recent_output.contains("hello"));
        assert!(!entry.cwd.is_empty());
    }

    #[test]
    fn stop_requires_an_id() {
        let registry = ServeRegistry::default();
        assert!(registry.stop("  ").contains("non-empty"));
    }

    #[test]
    fn stop_unknown_id_reports_an_error() {
        let registry = ServeRegistry::default();
        assert!(registry.stop("serve-missing").contains("no serve process"));
    }

    #[test]
    fn output_buffer_keeps_only_the_tail() {
        let mut buffer = String::new();
        append_limited(&mut buffer, &"x".repeat(MAX_OUTPUT_CHARS + 100));
        assert_eq!(buffer.chars().count(), MAX_OUTPUT_CHARS);
    }

    /// The serve path uses `build_command` + `process_guard`: the child must be
    /// killed as a tree, and the registry entry must disappear with the guard.
    #[tokio::test]
    async fn spawned_process_is_killed_and_untracked() {
        #[cfg(windows)]
        let sleep_code = "ping -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let sleep_code = "sleep 30";

        let mut command = build_command("bash", sleep_code, None).expect("build sleep command");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = command.spawn().expect("spawn sleep process");
        let tracked = process_guard::track_child(&child, "serve test");
        let pid = tracked.pid().expect("child pid");

        tokio::time::sleep(Duration::from_millis(200)).await;
        process_guard::kill_process_tree(Some(pid));
        drop(tracked);

        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
        assert!(
            matches!(status, Ok(Ok(_))),
            "the process should exit after its tree is killed"
        );
    }

    /// Turn-end shutdown must kill the running tree AND verify its exit in the
    /// returned summary.
    #[tokio::test]
    async fn shutdown_all_verifies_running_trees() {
        #[cfg(windows)]
        let sleep_code = "ping -n 30 127.0.0.1 >nul";
        #[cfg(not(windows))]
        let sleep_code = "sleep 30";

        let mut command = build_command("bash", sleep_code, None).expect("build sleep command");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut child = command.spawn().expect("spawn sleep process");
        let tracked = process_guard::track_child(&child, "serve shutdown test");

        let registry = ServeRegistry::default();
        registry.insert(ServeProcess {
            id: "serve-shutdown".to_string(),
            pid: child.id().unwrap_or(0),
            shell_type: "bash".to_string(),
            code: sleep_code.to_string(),
            cwd: PathBuf::from("."),
            started_at: Instant::now(),
            status: Arc::new(Mutex::new(ServeStatus {
                state: "running".to_string(),
                exit_code: None,
                output: String::new(),
            })),
        });

        let summary = registry.shutdown_all();
        drop(tracked);

        assert!(summary.contains("serve-shutdown"), "{summary}");
        assert!(summary.contains("verified"), "{summary}");
        assert!(!summary.contains("NOT confirmed"), "{summary}");

        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
        assert!(
            matches!(status, Ok(Ok(_))),
            "the process should exit after shutdown_all"
        );
    }
}
