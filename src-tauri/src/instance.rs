//! Multiple simultaneously running instances ("shadow" windows).
//!
//! `single_instance` decides whether a second launch focuses the running
//! window (the plugin does that) or starts a second real process. In the second
//! case two processes would otherwise share one `config.json`, and whichever
//! saved last would move the other window's workspace. The two roles below fix
//! that without touching the single-instance path:
//!
//! * the **primary** is the first process: it owns `shadow/primary.lock`,
//!   keeps using `<app_data>/config.json` and behaves exactly like a single
//!   instance;
//! * every later process is a **shadow**: it copies the current config into
//!   `<app_data>/shadow/config-<timestamp>-<id>/config.json`, points its own
//!   workspace at the copy's `workspace/`, and deletes that directory when it
//!   exits. Its settings never reach the main config, so no window can move
//!   another window's workspace.
//!
//! The only write a shadow makes to the main config is merging its
//! `workspace_history` back on exit — so the "recent workspaces" dropdown stays
//! useful in every window. That merge is serialised by
//! `shadow/main-config.lock` and leaves every other field untouched.
//!
//! Liveness is judged by pid, not by an open handle: the lock file records the
//! owner's pid, and a pid that no longer names a running process makes the lock
//! reclaimable. That keeps a crashed run (`taskkill /F`) from blocking the
//! primary role forever.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::AppConfig;

/// Directory below the app data root holding the primary lock and every shadow
/// copy.
pub const SHADOW_DIR: &str = "shadow";
/// Lock file marking the process that owns the main config.
const PRIMARY_LOCK_FILE: &str = "primary.lock";
/// Short-lived lock serialising writes to the main config.
const MAIN_CONFIG_LOCK_FILE: &str = "main-config.lock";
/// Prefix of one shadow's directory (`config-<timestamp>-<random>`).
const SHADOW_PREFIX: &str = "config-";
/// Bookkeeping file inside a shadow directory.
const OWNER_FILE: &str = ".owner";
/// A shadow directory without `.owner` is treated as leftover only after this
/// grace period, so a sibling that is creating its copy right now survives.
const ORPHAN_GRACE: Duration = Duration::from_secs(60);
/// Claim attempts (and their delay): a lock file that exists but cannot be read
/// yet means another process is between `create_new` and writing its pid.
const CLAIM_ATTEMPTS: u32 = 3;
const CLAIM_RETRY_DELAY: Duration = Duration::from_millis(30);
/// Main-config write lock: wait a little longer, the holder only ever writes.
const WRITE_LOCK_ATTEMPTS: u32 = 20;
const WRITE_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Which role this process plays among the running windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Owns the shared `config.json` (there is at most one).
    Primary,
    /// Runs on a private copy of the config for its whole lifetime.
    Shadow,
}

/// What the UI needs to know about the running process (`get_instance_info`).
#[derive(Debug, Clone, Serialize)]
pub struct InstanceInfo {
    /// `primary` or `shadow`.
    pub role: Role,
    /// Shadow id (`20260101T101530-a1b2c3d4`), empty for the primary.
    pub shadow_id: String,
    /// Config file this process actually reads and writes.
    pub config_path: String,
    /// Shared config file, the same no matter how many windows are open.
    pub main_config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Owner {
    pid: u32,
    started_at_ms: i64,
}

/// One process's claim on the persisted configuration.
pub struct Instance {
    data_dir: PathBuf,
    role: Role,
    /// Created by [`Instance::prepare`]; `None` for the primary.
    shadow_dir: Option<PathBuf>,
    /// Set by [`Instance::shutdown`], so the two exit hooks clean up once.
    finished: AtomicBool,
}

impl Instance {
    /// Take the primary role when it is free, otherwise mark this process as a
    /// shadow. The shadow's copy is created later, in [`Instance::prepare`]:
    /// a process that is aborted by the single-instance plugin must not leave a
    /// directory behind.
    pub fn claim(data_dir: PathBuf) -> Self {
        Self::claim_with(data_dir.clone(), &pid_is_alive)
    }

    /// [`Instance::claim`] with the liveness probe injected, so tests can
    /// simulate a live sibling instead of spawning one.
    fn claim_with(data_dir: PathBuf, is_alive: &dyn Fn(u32) -> bool) -> Self {
        let root = shadow_root(&data_dir);
        let _ = fs::create_dir_all(&root);
        let lock = root.join(PRIMARY_LOCK_FILE);
        let primary = match claim_file(&lock, CLAIM_ATTEMPTS, CLAIM_RETRY_DELAY, is_alive) {
            Ok(claimed) => claimed,
            // Cannot tell (permissions, exotic file system): behave as a shadow.
            // Two primaries writing one config is the failure this module exists
            // to prevent, a shadow window is merely less convenient.
            Err(_) => false,
        };
        Self {
            data_dir,
            role: if primary { Role::Primary } else { Role::Shadow },
            shadow_dir: None,
            finished: AtomicBool::new(false),
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// App-data directory this claim belongs to.
    ///
    /// `setup` compares it with Tauri's `app_data_dir()`: the pre-Tauri claim
    /// uses the same resolution, but a divergence must re-claim instead of
    /// running on a config the app never reads.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn is_primary(&self) -> bool {
        self.role == Role::Primary
    }

    /// The shared config file every window would use.
    pub fn main_config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    /// The config this process actually reads and writes.
    pub fn config_path(&self) -> PathBuf {
        match &self.shadow_dir {
            Some(dir) => dir.join("config.json"),
            None => self.main_config_path(),
        }
    }

    /// Shadow id (`20260101T101530-a1b2c3d4`); `None` for the primary.
    pub fn shadow_id(&self) -> Option<String> {
        let name = self.shadow_dir.as_ref()?.file_name()?.to_string_lossy().to_string();
        Some(name.strip_prefix(SHADOW_PREFIX).unwrap_or(&name).to_string())
    }

    /// Materialise this process's config file and return its path.
    ///
    /// The primary keeps using the shared file and nothing is created. A shadow
    /// gets `shadow/config-<ts>-<id>/config.json`: a snapshot of the shared
    /// config whose `workspace_dir` points at that directory's own
    /// `workspace/`, so two windows never open the same database by accident.
    pub fn prepare(&mut self) -> Result<PathBuf, String> {
        let main = self.main_config_path();
        if self.role == Role::Primary {
            return Ok(main);
        }
        if let Some(dir) = &self.shadow_dir {
            return Ok(dir.join("config.json"));
        }

        let dir = shadow_root(&self.data_dir).join(new_shadow_name());
        fs::create_dir_all(&dir).map_err(|e| format!("failed to create '{}': {e}", dir.display()))?;
        // The owner file comes first: the startup GC only removes a directory
        // it can attribute to a dead process (or one old enough that the
        // creation was interrupted).
        write_owner(&dir, std::process::id())?;

        let workspace = dir.join("workspace");
        fs::create_dir_all(&workspace)
            .map_err(|e| format!("failed to create '{}': {e}", workspace.display()))?;

        // Snapshot the shared config. A missing or corrupt file falls back to
        // the defaults, exactly like the primary's own startup does.
        let mut config: AppConfig = fs::read_to_string(&main)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        // The fork that keeps two windows off one SQLite file. The history is
        // copied as-is, so the Settings dropdown can still jump to a workspace
        // another window used.
        config.workspace_dir = Some(workspace.to_string_lossy().into_owned());
        let json = serde_json::to_string_pretty(&config)
            .map_err(|e| format!("failed to serialize the shadow config: {e}"))?;
        let config_path = dir.join("config.json");
        fs::write(&config_path, json)
            .map_err(|e| format!("failed to write '{}': {e}", config_path.display()))?;

        self.shadow_dir = Some(dir);
        Ok(config_path)
    }

    /// Persist `json` to `path`, serialising writers of the shared main config.
    ///
    /// Only writes to the main config need the lock (a shadow's copy has a
    /// single writer); the lock is best effort, a save is never dropped because
    /// a sibling held it for a moment.
    pub fn write_config(&self, path: &Path, json: &str) -> io::Result<()> {
        if path == self.main_config_path() {
            if let Some(_guard) = MainConfigLock::acquire(&self.data_dir) {
                return fs::write(path, json);
            }
        }
        fs::write(path, json)
    }

    /// Window title for `workspace`, tagged on a shadow window so two of them
    /// are distinguishable in the task bar.
    pub fn window_title(&self, workspace: &Path) -> String {
        match self.shadow_id() {
            Some(id) => {
                let label = id.rsplit('-').next().unwrap_or(id.as_str());
                format!("AI Chat — {} [shadow {label}]", workspace.display())
            }
            None => format!("AI Chat — {}", workspace.display()),
        }
    }

    /// Workspace a window opens when `workspace_dir` is cleared.
    ///
    /// The primary falls back to the app-data `workspace/`; a shadow must stay
    /// in its own fork — otherwise "reset to default" would silently put two
    /// windows on one SQLite database, which is exactly what the fork prevents.
    pub fn default_workspace(&self, app_data_workspace: PathBuf) -> PathBuf {
        match &self.shadow_dir {
            Some(dir) => dir.join("workspace"),
            None => app_data_workspace,
        }
    }

    /// What the Settings panel shows about this window.
    pub fn info(&self) -> InstanceInfo {
        InstanceInfo {
            role: self.role,
            shadow_id: self.shadow_id().unwrap_or_default(),
            config_path: self.config_path().to_string_lossy().into_owned(),
            main_config_path: self.main_config_path().to_string_lossy().into_owned(),
        }
    }

    /// Whether `path` lies inside the app's shadow root. Those workspaces belong
    /// to one temporary window and must never enter the shared history.
    fn is_shadow_path(&self, path: &str) -> bool {
        let normalize = |value: &str| value.replace('\\', "/").to_lowercase();
        let root = normalize(&shadow_root(&self.data_dir).to_string_lossy());
        let path = normalize(path.trim());
        !root.is_empty() && (path == root || path.starts_with(&format!("{root}/")))
    }

    /// Undo what this process owns. Idempotent — both exit hooks call it.
    ///
    /// A shadow merges its `workspace_history` into the main config (only that
    /// field, only while holding `main-config.lock`), then deletes its
    /// directory. The primary removes its lock file. Returns a warning for the
    /// app log when a step failed — nothing here is fatal.
    pub fn shutdown(&self) -> Option<String> {
        if self.finished.swap(true, Ordering::SeqCst) {
            return None;
        }
        match (&self.shadow_dir, self.role) {
            (Some(dir), _) => {
                let mut warning = self.merge_history_into_main(dir);
                if let Err(err) = fs::remove_dir_all(dir) {
                    let message = format!("failed to remove '{}': {err}", dir.display());
                    warning = Some(match warning {
                        Some(previous) => format!("{previous}; {message}"),
                        None => message,
                    });
                }
                warning
            }
            (None, Role::Primary) => {
                let lock = shadow_root(&self.data_dir).join(PRIMARY_LOCK_FILE);
                if let Err(err) = fs::remove_file(&lock) {
                    if err.kind() != io::ErrorKind::NotFound {
                        return Some(format!("failed to remove '{}': {err}", lock.display()));
                    }
                }
                None
            }
            (None, Role::Shadow) => None,
        }
    }

    /// Merge this shadow's `workspace_history` into the shared config.
    ///
    /// Read-modify-write under the write lock: only the history changes, so a
    /// setting the primary saved meanwhile survives. Every failure is a lost
    /// convenience entry, never a crash on exit.
    fn merge_history_into_main(&self, shadow_dir: &Path) -> Option<String> {
        let Ok(raw) = fs::read_to_string(shadow_dir.join("config.json")) else {
            return None; // already gone: nothing to merge
        };
        let Ok(shadow) = serde_json::from_str::<AppConfig>(&raw) else {
            return Some("shadow config is unreadable, workspace history not merged".to_string());
        };
        if shadow.workspace_history.is_empty() {
            return None;
        }

        let main = self.main_config_path();
        let Some(_guard) = MainConfigLock::acquire(&self.data_dir) else {
            return Some("shared config is busy, workspace history not merged".to_string());
        };
        let mut config: AppConfig = match fs::read_to_string(&main) {
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(config) => config,
                Err(err) => return Some(format!("shared config is unreadable ({err})")),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => AppConfig::default(),
            Err(err) => return Some(format!("shared config cannot be read ({err})")),
        };

        // Oldest first: `remember_workspace` inserts at the head, so the
        // shadow's newest workspace ends up first.
        let mut changed = false;
        for entry in shadow.workspace_history.iter().rev() {
            // The fork directory is temporary — sharing it would fill every
            // window's dropdown with paths that stop existing on exit.
            if self.is_shadow_path(entry) {
                continue;
            }
            changed |= config.remember_workspace(entry);
        }
        if !changed {
            return None;
        }
        match serde_json::to_string_pretty(&config) {
            Ok(json) => {
                if let Err(err) = fs::write(&main, json) {
                    return Some(format!("failed to merge the workspace history: {err}"));
                }
                None
            }
            Err(err) => Some(format!("failed to serialize the shared config: {err}")),
        }
    }
}

/// `<app_data>/shadow`.
pub fn shadow_root(data_dir: &Path) -> PathBuf {
    data_dir.join(SHADOW_DIR)
}

/// Remove shadow directories whose process is gone.
///
/// A shadow deletes its own directory on exit, but a killed process never runs
/// that hook, so startup sweeps the leftovers. A directory is only removed when
/// its owner is a dead pid (or when it carries no owner yet and is older than
/// [`ORPHAN_GRACE`]), so a live sibling window is never touched. Returns how
/// many directories were dropped.
pub fn collect_garbage(data_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(shadow_root(data_dir)) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_dir || !name.starts_with(SHADOW_PREFIX) {
            continue;
        }
        let path = entry.path();
        match read_owner(&path.join(OWNER_FILE)) {
            Some(owner) if pid_is_alive(owner.pid) => continue,
            Some(_) => {}
            None if !is_stale(&path) => continue,
            None => {}
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// One shadow's directory name: `config-<timestamp>-<random>`.
fn new_shadow_name() -> String {
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S");
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{SHADOW_PREFIX}{stamp}-{}", &id[..8])
}

fn write_owner(dir: &Path, pid: u32) -> Result<(), String> {
    let owner = Owner {
        pid,
        started_at_ms: now_ms(),
    };
    let json = serde_json::to_string(&owner).map_err(|e| e.to_string())?;
    fs::write(dir.join(OWNER_FILE), json)
        .map_err(|e| format!("failed to write '{OWNER_FILE}': {e}"))
}

fn read_owner(path: &Path) -> Option<Owner> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Whether a directory was last touched long enough ago to be an interrupted
/// creation rather than a sibling that is writing its owner file right now.
fn is_stale(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > ORPHAN_GRACE)
        .unwrap_or(false)
}

/// Create `path` exclusively, reclaiming it when its recorded owner is dead.
///
/// `Ok(true)` — this process created (or reclaimed) the file. `Ok(false)` — a
/// live process holds it. `Err` — the file system refused the attempt.
fn claim_file(
    path: &Path,
    attempts: u32,
    delay: Duration,
    is_alive: &dyn Fn(u32) -> bool,
) -> io::Result<bool> {
    for attempt in 1..=attempts.max(1) {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                let owner = Owner {
                    pid: std::process::id(),
                    started_at_ms: now_ms(),
                };
                if let Ok(json) = serde_json::to_string(&owner) {
                    let _ = file.write_all(json.as_bytes());
                    let _ = file.flush();
                }
                return Ok(true);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                match read_owner(path) {
                    Some(owner) if is_alive(owner.pid) && owner.pid != std::process::id() => {
                        if attempt == attempts.max(1) {
                            return Ok(false);
                        }
                    }
                    // A dead owner — or our own pid recycled from a crashed run,
                    // which cannot name a *different* live process.
                    Some(_) => {
                        let _ = fs::remove_file(path);
                    }
                    // Between `create_new` and the owner write.
                    None => {
                        if attempt == attempts.max(1) {
                            return Ok(false);
                        }
                    }
                }
                std::thread::sleep(delay);
            }
            Err(err) => return Err(err),
        }
    }
    Ok(false)
}

/// Short-lived exclusive lock around writes to the shared config.
struct MainConfigLock {
    path: PathBuf,
}

impl MainConfigLock {
    fn acquire(data_dir: &Path) -> Option<Self> {
        let path = shadow_root(data_dir).join(MAIN_CONFIG_LOCK_FILE);
        let _ = fs::create_dir_all(shadow_root(data_dir));
        match claim_file(
            &path,
            WRITE_LOCK_ATTEMPTS,
            WRITE_LOCK_RETRY_DELAY,
            &pid_is_alive,
        ) {
            Ok(true) => Some(Self { path }),
            _ => None,
        }
    }
}

impl Drop for MainConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Whether a process with `pid` is still running.
///
/// Values that cannot name a live process report `false`; on a platform without
/// a probe the answer is `true` (assume alive), because a wrongly reclaimed lock
/// would give the main config two owners.
fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        // `PROCESS_SYNCHRONIZE` is the cheapest access right that lets us wait
        // on the process, and it is enough to tell "running" from "exited".
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        unsafe { CloseHandle(handle) };
        return wait == WAIT_TIMEOUT;
    }

    #[cfg(unix)]
    {
        // `kill(pid, 0)` probes for existence; EPERM still means "alive, owned
        // by somebody else".
        match unsafe { libc::kill(pid as libc::pid_t, 0) } {
            0 => true,
            _ => io::Error::last_os_error().raw_os_error() == Some(libc::EPERM),
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        true
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-chat-instance-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_config(path: &Path, config: &AppConfig) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(path, serde_json::to_string_pretty(config).unwrap()).expect("config write");
    }

    /// A live sibling keeps the primary role away: this process ends up as a
    /// shadow without touching the main config.
    #[test]
    fn a_live_primary_makes_the_next_process_a_shadow() {
        let root = temp_root("claim-live");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        fs::write(
            root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
            r#"{"pid":4242,"started_at_ms":0}"#,
        )
        .unwrap();

        let instance = Instance::claim_with(root.clone(), &|pid| pid == 4242);
        assert_eq!(instance.role(), Role::Shadow);
        assert_eq!(instance.config_path(), root.join("config.json"));
        fs::remove_dir_all(root).ok();
    }

    /// A crashed run leaves its lock behind; the next process must take over
    /// instead of nesting every later window one level deeper.
    #[test]
    fn a_dead_owner_is_reclaimed() {
        let root = temp_root("claim-dead");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        let lock = root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE);
        fs::write(&lock, r#"{"pid":4243,"started_at_ms":0}"#).unwrap();

        let instance = Instance::claim_with(root.clone(), &|_| false);
        assert_eq!(instance.role(), Role::Primary);
        let owner = read_owner(&lock).expect("the lock names its new owner");
        assert_eq!(owner.pid, std::process::id());
        fs::remove_dir_all(root).ok();
    }

    /// The shadow copy is a fork: its `workspace_dir` points into its own
    /// directory, the shared config is left alone, and `prepare` is idempotent.
    #[test]
    fn a_shadow_forks_its_workspace_into_its_own_directory() {
        let root = temp_root("prepare");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        fs::write(
            root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
            r#"{"pid":4244,"started_at_ms":0}"#,
        )
        .unwrap();
        let mut main = AppConfig::default();
        main.workspace_dir = Some("D:/shared".to_string());
        main.workspace_history = vec!["D:/shared".to_string()];
        write_config(&root.join("config.json"), &main);

        let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4244);
        let config_path = instance.prepare().expect("shadow config");
        assert_ne!(config_path, root.join("config.json"));

        let copied: AppConfig =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let workspace = config_path.parent().unwrap().join("workspace");
        assert_eq!(
            copied.workspace_dir.as_deref(),
            Some(workspace.to_string_lossy().as_ref())
        );
        assert!(workspace.is_dir(), "the forked workspace exists");
        // The history travels with the copy (the dropdown stays useful)…
        assert_eq!(copied.workspace_history, vec!["D:/shared".to_string()]);
        // …while the shared config keeps its own workspace.
        let shared: AppConfig =
            serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
        assert_eq!(shared.workspace_dir.as_deref(), Some("D:/shared"));

        assert_eq!(instance.prepare().unwrap(), config_path, "idempotent");
        assert!(instance.shadow_id().is_some());
        fs::remove_dir_all(root).ok();
    }

    /// A shadow's exit merges only the workspace history, never the settings it
    /// was holding, and removes its directory.
    #[test]
    fn a_shadow_merges_only_the_workspace_history() {
        let root = temp_root("merge");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        fs::write(
            root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
            r#"{"pid":4245,"started_at_ms":0}"#,
        )
        .unwrap();
        let mut main = AppConfig::default();
        main.api_key = "keep-me".to_string();
        main.api_base_url = "https://shared.example/v1".to_string();
        main.workspace_history = vec!["C:/shared".to_string()];
        write_config(&root.join("config.json"), &main);

        let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4245);
        let config_path = instance.prepare().unwrap();
        // The shadow's own config: a different key and a newer workspace.
        let mut shadow: AppConfig =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        shadow.api_key = "shadow-only".to_string();
        shadow.workspace_history = vec!["D:/shadow".to_string(), "C:/shared".to_string()];
        fs::write(&config_path, serde_json::to_string_pretty(&shadow).unwrap()).unwrap();
        let shadow_dir = config_path.parent().unwrap().to_path_buf();

        instance.shutdown();

        let merged: AppConfig =
            serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
        assert_eq!(merged.api_key, "keep-me", "settings stay with the primary");
        assert_eq!(merged.api_base_url, "https://shared.example/v1");
        assert_eq!(
            merged.workspace_history,
            vec!["D:/shadow".to_string(), "C:/shared".to_string()],
            "the shadow's newest workspace heads the shared history"
        );
        assert!(!shadow_dir.exists(), "the shadow copy is removed on exit");
        fs::remove_dir_all(root).ok();
    }

    /// The fork directory is temporary: it must not show up in the shared
    /// history, while a workspace the user picked in that window still does.
    #[test]
    fn a_shadow_does_not_share_its_own_temporary_workspace() {
        let root = temp_root("merge-fork");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        fs::write(
            root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
            r#"{"pid":4246,"started_at_ms":0}"#,
        )
        .unwrap();
        write_config(&root.join("config.json"), &AppConfig::default());

        let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4246);
        let config_path = instance.prepare().unwrap();
        let mut shadow: AppConfig =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        let fork = shadow.workspace_dir.clone().expect("the fork is recorded");
        shadow.workspace_history = vec!["D:/real".to_string(), fork];
        fs::write(&config_path, serde_json::to_string_pretty(&shadow).unwrap()).unwrap();

        instance.shutdown();

        let merged: AppConfig =
            serde_json::from_str(&fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            merged.workspace_history,
            vec!["D:/real".to_string()],
            "only a workspace the user really picked is shared"
        );
        fs::remove_dir_all(root).ok();
    }

    /// Running the exit hook twice (window close, then app exit) must not merge
    /// or delete twice.
    #[test]
    fn shutdown_is_idempotent() {
        let root = temp_root("shutdown-twice");
        let instance = Instance::claim_with(root.clone(), &|_| false);
        instance.shutdown();
        let lock = root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE);
        assert!(!lock.exists(), "the primary drops its lock");
        // A second call has nothing left to do and must not fail.
        instance.shutdown();
        // A later launch can still become the primary.
        let again = Instance::claim_with(root.clone(), &|_| false);
        assert_eq!(again.role(), Role::Primary);
        fs::remove_dir_all(root).ok();
    }

    /// Startup GC drops leftovers of killed windows and keeps live siblings.
    #[test]
    fn garbage_collection_spares_live_shadows() {
        let root = temp_root("gc");
        let live = root.join(SHADOW_DIR).join("config-20260101T101530-aaaaaaaa");
        let dead = root.join(SHADOW_DIR).join("config-20260101T101531-bbbbbbbb");
        for dir in [&live, &dead] {
            fs::create_dir_all(dir.join("workspace")).unwrap();
            fs::write(dir.join("config.json"), "{}").unwrap();
        }
        write_owner(&live, std::process::id()).unwrap();
        write_owner(&dead, u32::MAX - 1).unwrap();

        assert_eq!(collect_garbage(&root), 1);
        assert!(live.is_dir(), "a running window's copy is never touched");
        assert!(!dead.exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn shadow_names_are_sorted_by_time_and_unique() {
        let name = new_shadow_name();
        assert!(name.starts_with(SHADOW_PREFIX));
        let id = name.strip_prefix(SHADOW_PREFIX).unwrap();
        let (stamp, random) = id.split_once('-').expect("stamp-random");
        assert_eq!(stamp.len(), "20260101T101530".len(), "sortable timestamp");
        assert_eq!(random.len(), 8, "short random suffix");
        assert_ne!(name, new_shadow_name());
    }

    #[test]
    fn a_shadow_window_title_is_tagged() {
        let root = temp_root("title");
        let mut instance = Instance::claim_with(root.join("app"), &|_| false);
        // Primary: the historical title, unchanged.
        assert_eq!(
            instance.window_title(Path::new("/ws")),
            "AI Chat — /ws"
        );
        instance.role = Role::Shadow;
        instance.shadow_dir = Some(root.join("app").join(SHADOW_DIR).join("config-1-a1b2c3d4"));
        let title = instance.window_title(Path::new("/ws"));
        assert!(title.contains("[shadow a1b2c3d4]"), "{title}");
        fs::remove_dir_all(root).ok();
    }

    /// Clearing `workspace_dir` must not send a shadow to the primary's
    /// default workspace: that would put two windows on one database.
    #[test]
    fn a_shadow_defaults_to_its_own_fork() {
        // Primary: the shared app-data workspace.
        let primary_root = temp_root("default-workspace-primary");
        let primary = Instance::claim_with(primary_root.clone(), &|_| false);
        let shared = primary_root.join("workspace");
        assert_eq!(primary.default_workspace(shared.clone()), shared);
        primary.shutdown();
        fs::remove_dir_all(primary_root).ok();

        // Shadow: its own fork once prepared.
        let root = temp_root("default-workspace-shadow");
        fs::create_dir_all(root.join(SHADOW_DIR)).unwrap();
        fs::write(
            root.join(SHADOW_DIR).join(PRIMARY_LOCK_FILE),
            r#"{"pid":4247,"started_at_ms":0}"#,
        )
        .unwrap();
        write_config(&root.join("config.json"), &AppConfig::default());

        let mut instance = Instance::claim_with(root.clone(), &|pid| pid == 4247);
        let config_path = instance.prepare().unwrap();
        let fork = config_path.parent().unwrap().join("workspace");
        assert_eq!(instance.default_workspace(shared), fork);
        fs::remove_dir_all(root).ok();
    }
}
