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
//! - `sleep-lock` — the state of the cross-platform system-awake lock
//!   ([`SleepLock`]), held while any timer is pending and shared with the
//!   chat-turn path in `llm_complete.rs`; the timer bar badge reflects it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    /// Shared "system awake" lock, held while at least one timer is pending.
    sleep_lock: SleepLock,
    /// Parked guard for the sleep lock; exists exactly while the timer set is
    /// non-empty.
    sleep_guard: Mutex<Option<SleepGuard>>,
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

// ─── Sleep lock ──────────────────────────────────────────────────────────────

/// Emitted with [`SleepLockState`] whenever the sleep-lock hold count changes.
/// The timer bar badge in the UI reflects this.
#[cfg_attr(test, allow(dead_code))] // test builds never emit
pub const SLEEP_LOCK_EVENT: &str = "sleep-lock";

/// Snapshot of the cross-platform "system awake" lock.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SleepLockState {
    /// True while the OS has been asked not to idle-suspend.
    pub active: bool,
    /// Number of live holders (pending timers, running chat turns, …).
    pub holders: u32,
}

/// Platform mechanism that keeps the system awake. Acquire and release are
/// only invoked on the 0→1 and 1→0 transitions, serialized by [`SleepLock`]'s
/// mutex. Every backend is best-effort: a failure degrades to no sleep
/// prevention (logged), never to a broken app.
trait SleepBackend: Send + Sync {
    fn acquire(&self) -> Result<(), String>;
    fn release(&self) -> Result<(), String>;
}

/// One struct per target OS; the `SleepBackend` impls are cfg'd so exactly one
/// compiles per target.
struct OsBackend;

#[cfg(windows)]
impl SleepBackend for OsBackend {
    fn acquire(&self) -> Result<(), String> {
        platform::apply(true)
    }
    fn release(&self) -> Result<(), String> {
        platform::apply(false)
    }
}

#[cfg(unix)]
impl SleepBackend for OsBackend {
    fn acquire(&self) -> Result<(), String> {
        platform::apply(true)
    }
    fn release(&self) -> Result<(), String> {
        platform::apply(false)
    }
}

#[cfg(not(any(windows, unix)))]
impl SleepBackend for OsBackend {
    fn acquire(&self) -> Result<(), String> {
        Err("no sleep-lock mechanism for this platform".to_string())
    }
    fn release(&self) -> Result<(), String> {
        Ok(())
    }
}

/// The backend matching the current target OS.
fn default_backend() -> Arc<dyn SleepBackend> {
    Arc::new(OsBackend)
}

#[cfg(windows)]
mod platform {
    use std::sync::{mpsc, OnceLock};
    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };

    /// `ES_CONTINUOUS` is scoped to the calling thread: the assertion only
    /// lives for as long as the same thread keeps issuing it. Every request
    /// therefore runs through one dedicated worker that stays alive for the
    /// rest of the app's lifetime.
    static REQUESTS: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

    pub fn apply(on: bool) -> Result<(), String> {
        let requests = REQUESTS.get_or_init(spawn_worker);
        requests
            .send(on)
            .map_err(|_| "sleep-lock power worker exited".to_string())
    }

    fn spawn_worker() -> mpsc::Sender<bool> {
        let (sender, receiver) = mpsc::channel::<bool>();
        // The JoinHandle is dropped (the thread stays detached): terminate
        // with the process, together with every other app-owned resource.
        // If the spawn itself fails, the closure — and with it the receiver —
        // is dropped too, so `send` fails fast from then on.
        let _ = std::thread::Builder::new()
            .name("sleep-lock".to_string())
            .spawn(move || {
                while let Ok(on) = receiver.recv() {
                    let flags = ES_CONTINUOUS | if on { ES_SYSTEM_REQUIRED } else { 0 };
                    // 0 = the call failed; any other value would be the
                    // previous state, which nobody needs.
                    if unsafe { SetThreadExecutionState(flags) } == 0 {
                        eprintln!("sleep-lock: SetThreadExecutionState({flags:#010x}) failed");
                    }
                }
            });
        sender
    }
}

#[cfg(unix)]
mod platform {
    #[cfg(target_os = "macos")]
    pub fn apply(on: bool) -> Result<(), String> {
        macos::apply(on)
    }

    #[cfg(target_os = "linux")]
    pub fn apply(on: bool) -> Result<(), String> {
        linux::apply(on)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    pub fn apply(on: bool) -> Result<(), String> {
        let _ = on;
        Err("no sleep-lock mechanism for this platform".to_string())
    }

    /// macOS: an IOKit power assertion prevents idle system sleep. Declared as
    /// plain C FFI so no objc bindings are needed; both frameworks are always
    /// linked on macOS targets.
    #[cfg(target_os = "macos")]
    mod macos {
        use std::ffi::{c_char, c_int, c_void, CString};
        use std::sync::{Mutex, OnceLock};

        /// `kCFStringEncodingUTF8`
        const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
        /// `kIOPMAssertionLevelOn`
        const IOPM_ASSERTION_LEVEL_ON: u32 = 255;
        /// `kIOPMAssertPreventUserIdleSystemSleep`: the display may still dim,
        /// but the system will not idle-suspend (lid-close etc. still sleep).
        const ASSERTION_TYPE: &str = "PreventUserIdleSystemSleep";

        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFStringCreateWithCString(
                allocator: *const c_void,
                c_str: *const c_char,
                encoding: u32,
            ) -> *mut c_void;
            fn CFRelease(cf: *mut c_void);
        }

        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IOPMAssertionCreateWithName(
                assertion_type: *const c_void,
                level: u32,
                assertion_name: *const c_void,
                assertion_id: *mut u32,
            ) -> c_int;
            fn IOPMAssertionRelease(assertion_id: u32) -> c_int;
        }

        /// Create an owned `CFString` from a Rust string.
        fn cf_string(value: &str) -> Result<*mut c_void, String> {
            let narrow =
                CString::new(value).map_err(|_| "NUL byte in sleep-lock string".to_string())?;
            // SAFETY: `narrow` outlives the call; the allocator is NULL (the
            // default allocator); the returned object is owned and released
            // by the caller.
            let created = unsafe {
                CFStringCreateWithCString(
                    std::ptr::null(),
                    narrow.as_ptr(),
                    K_CF_STRING_ENCODING_UTF8,
                )
            };
            if created.is_null() {
                Err("CFStringCreateWithCString failed".to_string())
            } else {
                Ok(created)
            }
        }

        fn release_cf(value: *mut c_void) {
            unsafe { CFRelease(value) };
        }

        /// Live assertion id. Access is effectively serialized by the
        /// coordinator's mutex, but a `Mutex` keeps this self-contained.
        static ASSERTION: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

        pub fn apply(on: bool) -> Result<(), String> {
            let slot = ASSERTION.get_or_init(|| Mutex::new(None));
            let mut slot = slot.lock().unwrap();
            if on {
                if slot.is_some() {
                    return Ok(()); // already asserted
                }
                let assertion_type = cf_string(ASSERTION_TYPE)?;
                let name = match cf_string("ai-chat long task") {
                    Ok(name) => name,
                    Err(err) => {
                        release_cf(assertion_type);
                        return Err(err);
                    }
                };
                let mut id: u32 = 0;
                let status = unsafe {
                    IOPMAssertionCreateWithName(
                        assertion_type,
                        IOPM_ASSERTION_LEVEL_ON,
                        name,
                        &mut id,
                    )
                };
                release_cf(name);
                release_cf(assertion_type);
                if status != 0 {
                    return Err(format!(
                        "IOPMAssertionCreateWithName failed (kern_return {status})"
                    ));
                }
                *slot = Some(id);
                Ok(())
            } else {
                let Some(id) = slot.take() else {
                    return Ok(());
                };
                let status = unsafe { IOPMAssertionRelease(id) };
                if status != 0 {
                    Err(format!(
                        "IOPMAssertionRelease failed (kern_return {status})"
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Linux: hold every sleep-inhibit mechanism the session offers.
    ///
    /// Two are worth having and they are not interchangeable:
    ///
    /// * `org.freedesktop.login1` (systemd-logind) — the portable one. It lives
    ///   on the *system* bus and exists on every systemd distribution whatever
    ///   the desktop, which is exactly where the desktop-specific service is
    ///   missing: Fedora MATE, for one, exports `org.mate.ScreenSaver` and
    ///   nothing under `org.freedesktop.*`.
    /// * the desktop screensaver — what keeps the display awake on GNOME, KDE,
    ///   Xfce and MATE, so it is held *in addition* when it answers.
    ///
    /// Each inhibition dies with its own handle (logind: the file descriptor it
    /// returns, closed by dropping it; screensaver: the cookie, released with
    /// `UnInhibit`; session bus: closing it drops whatever it owned), so an app
    /// crash cannot leak an awake machine. When no service answers at all,
    /// nothing is held and the log says so once — an unusual session is not an
    /// error worth stopping a long task over.
    #[cfg(target_os = "linux")]
    mod linux {
        use std::sync::{mpsc, OnceLock};
        use zbus::fdo::Result as FdoResult;
        use zbus::zvariant::OwnedFd;

        /// Desktop screensaver inhibit — returns a cookie that must pair with
        /// the matching `UnInhibit` call.
        #[zbus::proxy(
            default_service = "org.freedesktop.ScreenSaver",
            default_path = "/org/freedesktop/ScreenSaver",
            interface = "org.freedesktop.ScreenSaver"
        )]
        trait ScreenSaver {
            fn inhibit(&self, application_name: &str, reason_for_inhibit: &str) -> FdoResult<u32>;
            // The freedesktop spec spells it `UnInhibit` — the odd capital I
            // comes from the method's lifetime as `UnInhibit`, so the snake
            // name must be `un_inhibit` to land on it.
            fn un_inhibit(&self, cookie: u32) -> FdoResult<()>;
        }

        /// MATE exports the same idea under its own name and interface.
        #[zbus::proxy(
            default_service = "org.mate.ScreenSaver",
            default_path = "/org/mate/ScreenSaver",
            interface = "org.mate.ScreenSaver"
        )]
        trait MateScreenSaver {
            fn inhibit(&self, application_name: &str, reason: &str) -> FdoResult<u32>;
            fn un_inhibit(&self, cookie: u32) -> FdoResult<()>;
        }

        /// systemd-logind. `Inhibit` answers with a file descriptor and the
        /// inhibition lives exactly as long as that descriptor stays open.
        #[zbus::proxy(
            default_service = "org.freedesktop.login1",
            default_path = "/org/freedesktop/login1",
            interface = "org.freedesktop.login1.Manager"
        )]
        trait Login1Manager {
            fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> FdoResult<OwnedFd>;
        }

        const APP_NAME: &str = "ai-chat";
        const REASON: &str = "long task in progress";
        /// `idle` is the inhibit logind grants without a polkit prompt, and the
        /// one power managers consult before suspending on their own.
        const LOGIND_WHAT: &str = "idle";

        /// Which screensaver answered, so the release knows who to call.
        #[derive(Clone, Copy)]
        enum Saver {
            Freedesktop,
            Mate,
        }

        /// One mechanism actually held, together with everything releasing it
        /// needs. Dropping the value is safe: logind's descriptor closes, and a
        /// dropped session bus drops its own inhibition.
        enum Hold {
            Logind {
                /// Kept alive so the descriptor below stays valid.
                _bus: zbus::blocking::Connection,
                _fd: OwnedFd,
            },
            ScreenSaver {
                bus: zbus::blocking::Connection,
                saver: Saver,
                cookie: u32,
            },
        }

        /// One D-Bus worker owns the inhibit for the app's lifetime, so the
        /// cookie/connection pairing never races.
        static REQUESTS: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

        pub fn apply(on: bool) -> Result<(), String> {
            let requests = REQUESTS.get_or_init(spawn_worker);
            requests
                .send(on)
                .map_err(|_| "sleep-lock D-Bus worker exited".to_string())
        }

        fn spawn_worker() -> mpsc::Sender<bool> {
            let (sender, receiver) = mpsc::channel::<bool>();
            // If the spawn fails, the closure — and with it the receiver — is
            // dropped too, so `send` fails fast from then on.
            let _ = std::thread::Builder::new()
                .name("sleep-lock".to_string())
                .spawn(move || {
                    let mut holds: Vec<Hold> = Vec::new();
                    // One line per cause: a session without any inhibit service
                    // re-arms on every turn and must not repeat itself.
                    let mut reported: Option<String> = None;
                    while let Ok(on) = receiver.recv() {
                        if on {
                            if holds.is_empty() {
                                match establish_inhibit() {
                                    Ok(held) => {
                                        holds = held;
                                        reported = None;
                                    }
                                    Err(err) => {
                                        if reported.as_deref() != Some(err.as_str()) {
                                            eprintln!("sleep-lock: {err}");
                                            reported = Some(err);
                                        }
                                    }
                                }
                            }
                        } else {
                            lift_holds(std::mem::take(&mut holds));
                        }
                    }
                });
            sender
        }

        /// Acquire every mechanism that answers rather than stopping at the
        /// first: logind asks the power manager to stay up, the screensaver
        /// keeps the display on, and a desktop honouring only one of them is
        /// still covered.
        fn establish_inhibit() -> Result<Vec<Hold>, String> {
            let mut holds = Vec::new();
            let mut notes: Vec<String> = Vec::new();

            match logind_inhibit() {
                Ok(hold) => holds.push(hold),
                Err(note) => notes.push(note),
            }
            match screensaver_inhibit() {
                Ok(hold) => holds.push(hold),
                Err(note) => notes.push(note),
            }

            if holds.is_empty() {
                return Err(format!(
                    "no sleep-inhibit service answered ({})",
                    notes.join("; ")
                ));
            }
            Ok(holds)
        }

        fn logind_inhibit() -> Result<Hold, String> {
            let bus = zbus::blocking::Connection::system()
                .map_err(|err| format!("system D-Bus failed: {err}"))?;
            let manager = Login1ManagerProxyBlocking::new(&bus)
                .map_err(|err| format!("logind proxy failed: {err}"))?;
            let fd = manager
                .inhibit(LOGIND_WHAT, APP_NAME, REASON, "block")
                .map_err(|err| format!("logind Inhibit failed: {err}"))?;
            Ok(Hold::Logind { _bus: bus, _fd: fd })
        }

        fn screensaver_inhibit() -> Result<Hold, String> {
            let bus = zbus::blocking::Connection::session()
                .map_err(|err| format!("session D-Bus failed: {err}"))?;
            let mut notes: Vec<String> = Vec::new();

            match ScreenSaverProxyBlocking::new(&bus) {
                Ok(proxy) => match proxy.inhibit(APP_NAME, REASON) {
                    Ok(cookie) => {
                        return Ok(Hold::ScreenSaver {
                            bus,
                            saver: Saver::Freedesktop,
                            cookie,
                        })
                    }
                    Err(err) => notes.push(err.to_string()),
                },
                Err(err) => notes.push(err.to_string()),
            }

            match MateScreenSaverProxyBlocking::new(&bus) {
                Ok(proxy) => match proxy.inhibit(APP_NAME, REASON) {
                    Ok(cookie) => {
                        return Ok(Hold::ScreenSaver {
                            bus,
                            saver: Saver::Mate,
                            cookie,
                        })
                    }
                    Err(err) => notes.push(err.to_string()),
                },
                Err(err) => notes.push(err.to_string()),
            }

            Err(format!("screensaver unavailable ({})", notes.join("; ")))
        }

        fn lift_holds(holds: Vec<Hold>) {
            for hold in holds {
                if let Err(err) = release(hold) {
                    eprintln!("sleep-lock: {err}");
                }
            }
        }

        fn release(hold: Hold) -> Result<(), String> {
            match hold {
                // Dropping the value closes the descriptor, which is the whole
                // release as far as logind is concerned.
                Hold::Logind { .. } => Ok(()),
                Hold::ScreenSaver { bus, saver, cookie } => match saver {
                    Saver::Freedesktop => ScreenSaverProxyBlocking::new(&bus)
                        .map_err(|err| format!("ScreenSaver proxy failed: {err}"))?
                        .un_inhibit(cookie)
                        .map_err(|err| format!("ScreenSaver UnInhibit failed: {err}")),
                    Saver::Mate => MateScreenSaverProxyBlocking::new(&bus)
                        .map_err(|err| format!("MATE ScreenSaver proxy failed: {err}"))?
                        .un_inhibit(cookie)
                        .map_err(|err| format!("MATE ScreenSaver UnInhibit failed: {err}")),
                },
            }
        }
    }
}

/// Reference-counted handle around the platform sleep lock. `0→1` acquires
/// the OS mechanism, `1→0` releases it; the [`SleepGuard`] is RAII, so a panic
/// or an early return can never leak the lock. Cheap to clone.
#[derive(Clone)]
pub struct SleepLock {
    /// App handle for the `sleep-lock` event.
    ///
    /// Deliberately absent from test builds: the timer tests construct and
    /// clone this type, and carrying an `AppHandle<Wry>` would instantiate its
    /// deep clone/drop glue inside the unit-test binary. That glue walks into
    /// the whole Tauri window stack (`tao`/`muda`), whose comctl32 **v6**
    /// imports cannot load without an embedded manifest — a `cargo test` run
    /// would then die before `main` with STATUS_ENTRYPOINT_NOT_FOUND.
    #[cfg(not(test))]
    app: AppHandle,
    backend: Arc<dyn SleepBackend>,
    state: Arc<Mutex<u32>>,
}

impl SleepLock {
    /// Production lock: platform backend + event emission.
    #[cfg(not(test))]
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            backend: default_backend(),
            state: Arc::default(),
        }
    }

    /// Test-build variant of [`SleepLock::new`]: `run()` is never executed by
    /// the test harness, so the handle is accepted and dropped without being
    /// stored (and its glue is stripped along with the unreferenced caller).
    #[cfg(test)]
    pub fn new(_app: AppHandle) -> Self {
        Self {
            backend: default_backend(),
            state: Arc::default(),
        }
    }

    /// Unit-test lock: no events, caller-supplied counting backend.
    #[cfg(test)]
    fn for_tests(backend: Arc<dyn SleepBackend>) -> Self {
        Self {
            backend,
            state: Arc::default(),
        }
    }

    /// Take one hold of the sleep lock. The returned guard releases it when
    /// dropped; failures to engage the OS mechanism are logged, not errors —
    /// the app keeps working either way.
    #[must_use]
    pub fn acquire(&self) -> SleepGuard {
        let holders = {
            let mut state = self.state.lock().unwrap();
            *state += 1;
            if *state == 1 {
                if let Err(err) = self.backend.acquire() {
                    self.log_failure(true, &err);
                }
            }
            *state
        };
        self.emit(holders);
        SleepGuard { lock: self.clone() }
    }

    /// Drop one hold. Runs when a [`SleepGuard`] is dropped.
    fn release_one(&self) {
        let holders = {
            let mut state = self.state.lock().unwrap();
            *state = state.saturating_sub(1);
            if *state == 0 {
                if let Err(err) = self.backend.release() {
                    self.log_failure(false, &err);
                }
            }
            *state
        };
        self.emit(holders);
    }

    #[cfg(not(test))]
    fn emit(&self, holders: u32) {
        let _ = self.app.emit(
            SLEEP_LOCK_EVENT,
            SleepLockState {
                active: holders > 0,
                holders,
            },
        );
    }

    #[cfg(test)]
    fn emit(&self, _holders: u32) {}

    /// Current state, for the UI's initial render (events only report
    /// changes, and the webview may subscribe after the lock was taken at
    /// startup — e.g. restored timers).
    pub fn snapshot(&self) -> SleepLockState {
        let holders = *self.state.lock().unwrap();
        SleepLockState {
            active: holders > 0,
            holders,
        }
    }

    /// Surface backend failures where they are actionable: the app log, or
    /// stderr when there is no app (tests).
    fn log_failure(&self, acquiring: bool, err: &str) {
        let message = format!(
            "sleep lock {} failed (system may suspend anyway): {err}",
            if acquiring { "acquire" } else { "release" }
        );
        #[cfg(not(test))]
        if let Some(state) = self.app.try_state::<crate::AppState>() {
            if let Ok(logger) = state.logger.lock() {
                logger.log("WARN", &message);
                return;
            }
        }
        eprintln!("sleep-lock: {message}");
    }
}

/// RAII handle: as long as this value lives, one hold on the sleep lock is
/// active. Dropping it is the only way to release.
#[must_use]
pub struct SleepGuard {
    lock: SleepLock,
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        self.lock.release_one();
    }
}

impl TimerRegistry {
    /// Load the persisted timers (missing or corrupt file = empty registry).
    pub fn load(path: PathBuf, sleep_lock: SleepLock) -> Self {
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
            sleep_lock,
            sleep_guard: Mutex::new(None),
        }
    }

    /// Keep the sleep guard in sync with the registry contents: parked while
    /// at least one timer is pending, dropped once the set empties. Idempotent
    /// and cheap, so callers can invoke it after any mutation.
    fn sync_sleep_guard(&self) {
        let has_timers = !self.timers.lock().unwrap().is_empty();
        let mut slot = self.sleep_guard.lock().unwrap();
        if has_timers {
            if slot.is_none() {
                *slot = Some(self.sleep_lock.acquire());
            }
        } else if let Some(guard) = slot.take() {
            drop(guard);
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
        drop(guard);
        self.sync_sleep_guard();
    }

    fn remove(&self, id: &str) -> Option<TimerEntry> {
        let removed = {
            let mut guard = self.timers.lock().unwrap();
            let removed = guard.remove(id);
            if removed.is_some() {
                self.persist(&guard);
            }
            removed
        };
        if removed.is_some() {
            self.sync_sleep_guard();
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
    let registry = app.state::<TimerRegistry>();
    let pending = registry.list();
    if pending.is_empty() {
        return;
    }
    // Re-take the sleep lock: the registry was refilled from disk directly,
    // not through `insert`, so nothing else has parked the guard yet.
    registry.sync_sleep_guard();

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

/// Current sleep-lock state, so the timer bar badge is correct on first
/// render (the `sleep-lock` event only reports changes).
#[tauri::command]
pub fn get_sleep_lock_state(state: State<'_, SleepLock>) -> SleepLockState {
    state.snapshot()
}

/// Cancel a pending timer by id.
#[tauri::command]
pub fn cancel_timer(app: AppHandle, id: String) -> Result<TimerEntry, String> {
    cancel(&app, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique `timers.json` path inside the OS temp directory.
    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai-chat-timer-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("timers.json")
    }

    /// Test backend counting acquire/release calls without touching the OS.
    struct CountingBackend(Arc<AtomicU32>);

    impl SleepBackend for CountingBackend {
        fn acquire(&self) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn release(&self) -> Result<(), String> {
            self.0.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A counting backend pair: (lock backend, live-transition counter).
    fn counting_backend() -> (Arc<dyn SleepBackend>, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        (Arc::new(CountingBackend(count.clone())), count)
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

        let registry =
            TimerRegistry::load(path.clone(), SleepLock::for_tests(counting_backend().0));
        registry.insert(entry("later", now_ms() + 60_000));
        registry.insert(entry("sooner", now_ms() + 1_000));

        let reloaded =
            TimerRegistry::load(path.clone(), SleepLock::for_tests(counting_backend().0));
        let ids: Vec<String> = reloaded.list().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec!["sooner".to_string(), "later".to_string()]);
        assert_eq!(reloaded.count_for_session("session-a"), 2);

        assert!(reloaded.remove("sooner").is_some());
        assert!(reloaded.remove("sooner").is_none());
        assert_eq!(
            TimerRegistry::load(path.clone(), SleepLock::for_tests(counting_backend().0))
                .list()
                .len(),
            1
        );

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
        assert!(
            TimerRegistry::load(path.clone(), SleepLock::for_tests(counting_backend().0))
                .list()
                .is_empty()
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Guards on one lock share the OS acquire: it happens on the first hold
    /// and releases only when the last guard drops.
    #[test]
    fn sleep_lock_refcounts_across_guards() {
        let (backend, transitions) = counting_backend();
        let lock = SleepLock::for_tests(backend);

        let first = lock.acquire();
        assert_eq!(transitions.load(Ordering::SeqCst), 1);
        let second = lock.acquire();
        assert_eq!(transitions.load(Ordering::SeqCst), 1);

        drop(first);
        assert_eq!(transitions.load(Ordering::SeqCst), 1);
        drop(second);
        assert_eq!(transitions.load(Ordering::SeqCst), 0);
    }

    /// The registry parks the sleep guard exactly while timers are pending.
    #[test]
    fn timer_registry_parks_the_sleep_lock_while_timers_pending() {
        let (backend, transitions) = counting_backend();
        let lock = SleepLock::for_tests(backend);
        let registry = TimerRegistry::load(temp_path("sleep-lock"), lock);

        registry.insert(entry("a", now_ms() + 60_000));
        registry.insert(entry("b", now_ms() + 60_000));
        assert_eq!(transitions.load(Ordering::SeqCst), 1);

        registry.remove("a");
        assert_eq!(transitions.load(Ordering::SeqCst), 1);
        registry.remove("b");
        assert_eq!(transitions.load(Ordering::SeqCst), 0);
    }
}
