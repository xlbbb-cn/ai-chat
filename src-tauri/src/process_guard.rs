// App-wide child-process tracking and whole-tree termination.
//
// Long-lived children (stdio MCP servers, `run_shell` serve processes, CLI
// sub-agents) must never outlive the app. Every spawn registers its pid here
// and, on Windows, is assigned to a single job object created with
// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — so even a hard crash (where Tauri's
// `RunEvent::Exit` never runs) tears the whole tree down when the OS closes
// the app's handles. On Unix, children opt into their own process group and
// the normal exit path kills `-pid`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
struct TrackedEntry {
    #[allow(dead_code)] // diagnostics / future logging
    label: String,
    /// Windows: assigned to the kill-on-close job (so `kill_all` must not also
    /// `taskkill` it by pid — that pid may already belong to another process).
    in_job: bool,
}

/// Pid -> tracking metadata (for logs / diagnostics).
fn registry() -> &'static Mutex<HashMap<u32, TrackedEntry>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u32, TrackedEntry>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// RAII tracking token. Dropping it removes the pid from the registry; the
/// process itself is left alone (callers kill it explicitly or it exits).
pub struct TrackedProcess {
    pid: Option<u32>,
}

impl TrackedProcess {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl Drop for TrackedProcess {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            if let Ok(mut registry) = registry().lock() {
                registry.remove(&pid);
            }
        }
    }
}

/// Register a freshly spawned child for app-exit cleanup. The returned token
/// must be kept alive for as long as the child may run.
pub fn track_child(child: &tokio::process::Child, label: impl Into<String>) -> TrackedProcess {
    let pid = child.id();
    if let Some(pid) = pid {
        #[cfg(windows)]
        let in_job = job::assign(child);
        #[cfg(not(windows))]
        let in_job = false;

        if let Ok(mut registry) = registry().lock() {
            registry.insert(
                pid,
                TrackedEntry {
                    label: label.into(),
                    in_job,
                },
            );
        }
    }
    TrackedProcess { pid }
}

/// Terminate a child and everything it spawned. CLI agents, MCP servers and
/// serve processes all start their own subprocesses, so killing only the
/// top-level shim is not enough.
pub fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid.filter(|pid| *pid != 0) else {
        return;
    };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut command = std::process::Command::new("taskkill");
        command
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let _ = command.status();
    }

    #[cfg(unix)]
    {
        // Negative pid = whole process group (children call `process_group(0)`).
        // Fall back to the single pid when the child was not a group leader.
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
            let _ = libc::kill(pid as i32, libc::SIGKILL);
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

/// Kill every tracked child tree. Called from the app-exit hook and when the
/// main window closes.
pub fn kill_all() {
    let entries: Vec<(u32, bool)> = registry()
        .lock()
        .map(|registry| {
            registry
                .iter()
                .map(|(pid, entry)| (*pid, entry.in_job))
                .collect()
        })
        .unwrap_or_default();

    #[cfg(windows)]
    {
        // Job members are terminated atomically by the job handle; killing
        // them again by pid risks hitting a reused pid if the child already
        // exited. Only the (rare) children whose job assignment failed (or
        // when the job itself cannot be terminated) are killed by pid.
        let job_terminated = job::terminate_all();
        for (pid, in_job) in &entries {
            if !*in_job || !job_terminated {
                kill_process_tree(Some(*pid));
            }
        }
    }

    #[cfg(not(windows))]
    for (pid, _) in &entries {
        kill_process_tree(Some(*pid));
    }

    if let Ok(mut registry) = registry().lock() {
        registry.clear();
    }
}

// ─── Windows job object ──────────────────────────────────────────────────────

#[cfg(windows)]
mod job {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Raw `HANDLE` stored as `usize` so it can live in a `OnceLock` (raw
    /// pointers are not `Send`/`Sync`).
    static JOB: OnceLock<usize> = OnceLock::new();

    fn job_handle() -> Option<HANDLE> {
        let raw = JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return 0;
            }

            job as usize
        });

        (*raw != 0).then_some(*raw as HANDLE)
    }

    pub(super) fn assign(child: &tokio::process::Child) -> bool {
        let Some(job) = job_handle() else {
            return false;
        };
        let Some(handle) = child.raw_handle() else {
            return false;
        };
        unsafe {
            // Nested jobs are supported on Windows 8+. Failure is non-fatal:
            // the pid stays tracked and `kill_all` terminates it explicitly.
            AssignProcessToJobObject(job, handle as HANDLE) != 0
        }
    }

    pub(super) fn terminate_all() -> bool {
        match job_handle() {
            Some(job) => unsafe { TerminateJobObject(job, 1) != 0 },
            None => false,
        }
    }
}
