// Child-process execution and output decoding: `run_command`, and the
// Windows code-page aware decoding of child stdout/stderr.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::split_command_line;
use super::truncate_tool_output;

#[derive(Clone, Copy)]
pub(crate) enum OutputDecodeHint {
    Default,
    Direct,
    CmdShell,
    PowerShell,
}

pub(crate) fn decode_process_bytes(bytes: &[u8], _hint: OutputDecodeHint) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Some(decoded) = decode_with_bom(bytes) {
        return decoded;
    }

    if let Ok(decoded) = std::str::from_utf8(bytes) {
        return decoded.to_string();
    }

    #[cfg(windows)]
    if let Some(decoded) = decode_windows_process_bytes(bytes, _hint) {
        return decoded;
    }

    String::from_utf8_lossy(bytes).to_string()
}

fn decode_with_bom(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(&bytes[3..])
            .ok()
            .map(|text| text.to_string());
    }

    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Some(String::from_utf16_lossy(&utf16_units_le(&bytes[2..])));
    }

    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Some(String::from_utf16_lossy(&utf16_units_be(&bytes[2..])));
    }

    None
}

fn utf16_units_le(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn utf16_units_be(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect()
}

#[cfg(windows)]
fn decode_windows_process_bytes(bytes: &[u8], hint: OutputDecodeHint) -> Option<String> {
    use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};

    let mut code_pages = vec![65001];
    match hint {
        OutputDecodeHint::CmdShell => {
            code_pages.push(unsafe { GetOEMCP() });
            code_pages.push(unsafe { GetACP() });
        }
        _ => {
            code_pages.push(unsafe { GetACP() });
            code_pages.push(unsafe { GetOEMCP() });
        }
    }

    decode_windows_process_bytes_with_code_pages(bytes, &code_pages)
}

#[cfg(windows)]
pub(super) fn decode_windows_process_bytes_with_code_pages(
    bytes: &[u8],
    code_pages: &[u32],
) -> Option<String> {
    let mut attempted = Vec::new();
    for &code_page in code_pages {
        if code_page == 0 || attempted.contains(&code_page) {
            continue;
        }
        attempted.push(code_page);
        if let Some(decoded) = decode_windows_code_page(bytes, code_page) {
            return Some(decoded);
        }
    }
    None
}

#[cfg(windows)]
fn decode_windows_code_page(bytes: &[u8], code_page: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, MB_ERR_INVALID_CHARS};

    let flags = if code_page == 65001 {
        MB_ERR_INVALID_CHARS
    } else {
        0
    };

    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return None;
    }

    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            flags,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return None;
    }

    Some(String::from_utf16_lossy(&wide[..written as usize]))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn wrap_cmd_script_for_utf8(code: &str) -> String {
    #[cfg(windows)]
    {
        format!("chcp 65001>nul & {code}")
    }
    #[cfg(not(windows))]
    {
        code.to_string()
    }
}

fn wrap_powershell_script_for_utf8(code: &str) -> String {
    #[cfg(windows)]
    {
        return format!(
            "$utf8NoBom = New-Object System.Text.UTF8Encoding($false); \
[Console]::InputEncoding = $utf8NoBom; \
[Console]::OutputEncoding = $utf8NoBom; \
$OutputEncoding = $utf8NoBom; \
chcp 65001 > $null; \
{code}"
        );
    }

    #[cfg(not(windows))]
    {
        code.to_string()
    }
}

/// PowerShell 7+ (`pwsh`) when it is installed, else the built-in Windows
/// PowerShell — which is what a Windows build's `run_shell` uses, so the
/// modern shell is preferred rather than required.
fn pwsh_program() -> &'static str {
    static PROGRAM: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    PROGRAM.get_or_init(|| {
        if crate::executable::resolve_executable("pwsh").is_ok() {
            "pwsh"
        } else {
            "powershell"
        }
    })
}

fn format_process_output(output: std::process::Output, hint: OutputDecodeHint) -> String {
    let stdout = decode_process_bytes(&output.stdout, hint);
    let stderr = decode_process_bytes(&output.stderr, hint);

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("STDERR:\n");
        result.push_str(&stderr);
    }
    if !output.status.success() {
        result.push_str(&format!(
            "\nExit code: {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    if result.is_empty() {
        result = "(no output)".to_string();
    }
    truncate_tool_output(result)
}

pub(crate) fn decode_hint_for_shell(cmd_type: &str) -> OutputDecodeHint {
    match cmd_type {
        "direct" => OutputDecodeHint::Direct,
        "bash" | "sh" if cfg!(windows) => OutputDecodeHint::CmdShell,
        "powershell" | "pwsh" => OutputDecodeHint::PowerShell,
        _ => OutputDecodeHint::Default,
    }
}

/// Reject a `run_cmd` program that is a path instead of a bare name.
///
/// `run_cmd` runs a *program* that the operating system resolves through the
/// `PATH` environment variable — `python`, `python3`, `git`, `node`, ….
/// Absolute paths (`C:\Python\python.exe`, `/usr/bin/python3`) and relative
/// paths (`.\build\tool.exe`, `sub/tool`) are refused: they would let the
/// assistant run an arbitrary file instead of a PATH-resolved command.
///
/// `run_shell` is the tool with that freedom — it may `cd` anywhere and run
/// any script — and every script it runs is audited by `tools/risk.rs`.
pub(crate) fn validate_path_resolved_program(program: &str) -> Result<(), String> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return Err("run_cmd requires a program name.".to_string());
    }

    // A Windows path stays a path on every platform: on Linux the backslashes
    // are ordinary characters, so `C:\Python\python.exe` would otherwise look
    // like one bare name and only fail later with a confusing ENOENT.
    if looks_like_windows_path(trimmed) {
        return Err(format!(
            "Error: run_cmd only runs programs that PATH resolves (for example 'python' or \
             'python3'). '{}' is a path — pass a bare program name instead, or use \
             `run_shell` when an absolute path is really needed.",
            trimmed
        ));
    }

    // A bare program name is exactly one `Normal` component. Anything else
    // carries a separator, a drive prefix or a root — that is a path.
    let mut components = Path::new(trimmed).components();
    let is_bare_name = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none();

    if !is_bare_name {
        return Err(format!(
            "Error: run_cmd only runs programs that PATH resolves (for example 'python' or \
             'python3'). '{}' is a path — pass a bare program name instead, or use \
             `run_shell` when an absolute path is really needed.",
            trimmed
        ));
    }

    Ok(())
}

/// Shell interpreters `run_cmd` refuses to launch, compared against the
/// lowercased file stem (`bash.exe`, `CMD`, `Powershell` all match).
///
/// Running `run_cmd {command: "bash script.sh"}` would hand arbitrary script
/// text to an interpreter and bypass the audit `run_shell` applies to every
/// script it runs (syntax gate, dangerous-pattern scan, per-hash approval).
/// Workspace library scripts are the exception: they are content-scanned and
/// approved, and the resolved-script path never reaches this check.
const SHELL_INTERPRETER_NAMES: &[&str] = &[
    // POSIX / Unix shells.
    "sh", "bash", "dash", "ash", "csh", "tcsh", "zsh", "ksh", "ksh93", "fish",
    // Windows shells and interpreters of the same kind.
    "cmd", "powershell", "pwsh",
    // Bridges that would start a shell of another platform.
    "wsl", "busybox",
];

/// Reject a `run_cmd` program that is itself a shell interpreter.
///
/// `run_cmd` runs a single program with fixed arguments; the interpreter
/// tools (`bash`, `sh`, `csh`, `cmd`, `powershell`, …) are refused because
/// their arguments are free-form code that the "direct" risk assessment of
/// `run_cmd` cannot audit. `run_shell` is the tool that runs scripts, under
/// the full risk audit.
pub(crate) fn reject_shell_interpreter(program: &str) -> Result<(), String> {
    let trimmed = program.trim();
    let stem = Path::new(trimmed)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(trimmed)
        .to_lowercase();
    if SHELL_INTERPRETER_NAMES.contains(&stem.as_str()) {
        return Err(format!(
            "Error: run_cmd cannot run shell interpreters ('{trimmed}'). Its arguments would \
             bypass the script audit. Write the script with `file_actions` and run it through \
             `run_shell`, or save it in `<workspace>/.data/scripts` and run the library \
             script by name."
        ));
    }
    Ok(())
}

/// `C:\dir\x.exe` or `dir\x.exe`: a Windows path, whatever the host platform.
pub(crate) fn looks_like_windows_path(value: &str) -> bool {
    if value.contains('\\') {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Extra environment mounted into a child this module launches.
///
/// Both entries are optional and independent:
///
/// - `scripts` — the workspace script library; prepended to the child's `PATH`
///   so `run_cmd` resolves library scripts (and programs they spawn) there.
/// - `venv` — the host Python venv (`<app data>/env/py313`); its `bin/`
///   (`Scripts/` on Windows) joins the child's `PATH` right behind the script
///   library, and `VIRTUAL_ENV` names it, so `python` / `python3` inside a
///   `run_cmd` child is the 3.13 interpreter and `sys.prefix` points at the
///   venv. `run_shell` deliberately passes `Mounts::default()`: an arbitrary
///   shell script keeps the host interpreter.
#[derive(Clone, Copy, Default)]
pub(crate) struct Mounts<'a> {
    pub scripts: Option<&'a Path>,
    /// Host directories from app data `env/` (e.g. `env/bin` for tools the
    /// assistant installed), mounted behind the script library — `run_cmd`
    /// passes `env::tool_path_dirs`, `run_shell` passes nothing.
    pub env_dirs: &'a [PathBuf],
    /// The 3.13 venv `VIRTUAL_ENV` names and whose `bin/` goes on `PATH`.
    pub venv: Option<&'a Path>,
}

/// Build a command for `cmd_type` rooted at `cwd`, without running it.
/// Used by `run_command` (synchronous) and by `serve` (long-lived children).
pub(crate) fn build_command(
    cmd_type: &str,
    code: &str,
    cwd: Option<PathBuf>,
) -> Result<tokio::process::Command, String> {
    build_command_with_path(cmd_type, code, cwd, Mounts::default())
}

/// Like [`build_command`], but with the [`Mounts`] the caller wants in the
/// child's environment. `run_cmd` mounts the script library and the host venv;
/// every other caller passes `Mounts::default()`.
pub(crate) fn build_command_with_path(
    cmd_type: &str,
    code: &str,
    cwd: Option<PathBuf>,
    mounts: Mounts<'_>,
) -> Result<tokio::process::Command, String> {
    let mut cmd = match cmd_type {
        "direct" => {
            // Run the executable directly — no shell wrapper needed.
            let parts = split_command_line(code);
            if parts.is_empty() {
                return Err("Empty command".to_string());
            }
            // `direct` is the `run_cmd` contract: a PATH-resolved program name,
            // never a path, and never a shell interpreter.
            validate_path_resolved_program(&parts[0])?;
            reject_shell_interpreter(&parts[0])?;
            let mut c = tokio::process::Command::new(&parts[0]);
            if parts.len() > 1 {
                c.args(&parts[1..]);
            }
            c
        }
        "bash" | "sh" => {
            #[cfg(windows)]
            let c = {
                let mut cmd = tokio::process::Command::new("cmd.exe");
                let wrapped = wrap_cmd_script_for_utf8(code);
                cmd.args(["/D", "/S", "/C", &wrapped]);
                cmd
            };

            #[cfg(not(windows))]
            let c = {
                // Use the user's login shell (`$SHELL`) with `-l -c` so
                // that shell init files are sourced, picking up PATH
                // modifications from nvm, Homebrew, rustup, etc.
                // `.zshrc` / `.bashrc` are only loaded for interactive
                // shells, so we source them explicitly here.
                let shell = std::env::var("SHELL").unwrap_or_else(|_| {
                    #[cfg(target_os = "macos")]
                    {
                        "/bin/zsh".to_string()
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        "/bin/sh".to_string()
                    }
                });
                let shell_name = std::path::Path::new(&shell)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sh");
                let source_rc = match shell_name {
                    "zsh" => "[ -r ~/.zshrc ] && source ~/.zshrc; ",
                    "bash" => "[ -r ~/.bashrc ] && source ~/.bashrc; ",
                    _ => "",
                };
                let full_cmd = format!("{}{}", source_rc, code);
                let mut cmd = tokio::process::Command::new(&shell);
                cmd.args(["-l", "-c", &full_cmd]);
                cmd
            };

            c
        }
        "powershell" | "pwsh" => {
            // `pwsh` (the Windows build's shell) runs PowerShell 7+ when it is
            // installed and falls back to Windows PowerShell; the explicit
            // `powershell` cmd_type keeps the built-in shell, so existing
            // 5.1-only scripts are not silently moved to a different engine.
            let program = if cmd_type == "pwsh" {
                pwsh_program()
            } else {
                "powershell"
            };
            let mut c = tokio::process::Command::new(program);
            let wrapped = wrap_powershell_script_for_utf8(code);
            c.args(["-NoProfile", "-NonInteractive", "-Command", &wrapped]);
            c
        }
        _ => return Err(format!("Unsupported command type: {}", cmd_type)),
    };

    finish_command(&mut cmd, cwd, mounts);
    Ok(cmd)
}

/// Apply the process-wide launch options shared by every child this module
/// starts: working directory, `PWD`, the [`Mounts`] `PATH` entries, and the
/// platform-specific process-group/console settings.
fn finish_command(
    cmd: &mut tokio::process::Command,
    cwd: Option<PathBuf>,
    mounts: Mounts<'_>,
) {
    if let Some(dir) = cwd {
        cmd.current_dir(&dir);
        // `current_dir` does not update the inherited `PWD`, and programs that
        // resolve their project root from it would keep seeing the app's launch
        // directory. Keep the two in sync for run_cmd / run_shell (same rule as
        // `agents/cli.rs` and `mcp.rs`).
        cmd.env("PWD", &dir);
    }

    // Mounted in order: the script library first (a library script beats a
    // same-named program), then the app-data environment directories
    // (`<env root>/bin`, the venv's `bin/`), then the inherited `PATH`.
    // `prepend_to_path` de-duplicates, so a directory named twice is harmless.
    let mut mounted: Vec<PathBuf> = Vec::new();
    if let Some(dir) = mounts.scripts {
        mounted.push(dir.to_path_buf());
    }
    mounted.extend(mounts.env_dirs.iter().cloned());
    if let Some(venv) = mounts.venv {
        mounted.push(crate::env::venv_bin_dir(venv));
        // A venv on `PATH` without `VIRTUAL_ENV` is half an activation: tools
        // that resolve their environment from it would still see the host.
        cmd.env("VIRTUAL_ENV", venv);
        // `PYTHONHOME` overrides the venv's own prefix, so the "mounted" python
        // would silently load the host stdlib — a real activation clears it.
        cmd.env_remove("PYTHONHOME");
    }
    if !mounted.is_empty() {
        let dirs: Vec<&Path> = mounted.iter().map(PathBuf::as_path).collect();
        cmd.env("PATH", prepend_to_path(&dirs));
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(unix)]
    {
        // Own process group, so a timeout/cancel can kill the whole tree (see
        // `crate::process_guard::kill_process_tree`).
        cmd.process_group(0);
    }

    // A dropped run (e.g. the 30s timeout future in `execute_tool`) must not
    // leave the child running.
    cmd.kill_on_drop(true);
}

/// Prepend `dirs` to the current `PATH` value, in order, dropping duplicates.
///
/// `split_paths` / `join_paths` handle the platform separator and quoting, so
/// a directory containing `;` or spaces is passed through correctly.
fn prepend_to_path(dirs: &[&Path]) -> std::ffi::OsString {
    use std::ffi::OsString;

    // Callers assemble the mounts from several sources (script library, the
    // environment root, the venv) and may name the same directory twice.
    let mut unique: Vec<&Path> = Vec::new();
    for dir in dirs {
        if !unique.iter().any(|seen| seen == dir) {
            unique.push(dir);
        }
    }
    let dirs = unique.as_slice();

    let mut paths: Vec<PathBuf> = std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_else(OsString::new),
    )
    .collect();
    paths.retain(|existing| !dirs.contains(&existing.as_path()));
    for dir in dirs.iter().rev() {
        paths.insert(0, dir.to_path_buf());
    }
    std::env::join_paths(paths).unwrap_or_else(|_| {
        // A mounted directory holding the platform separator cannot be joined;
        // fall back to the mounts alone instead of the inherited PATH.
        let separator = if cfg!(windows) { ";" } else { ":" };
        dirs.iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(separator)
            .into()
    })
}

pub async fn run_command(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
    mounts: Mounts<'_>,
) -> Result<String, String> {
    let decode_hint = decode_hint_for_shell(&cmd_type);
    let mut cmd = build_command_with_path(&cmd_type, &code, cwd, mounts)?;
    let output = cmd.output().await.map_err(|e| e.to_string())?;
    Ok(format_process_output(output, decode_hint))
}

/// Run a command with a timeout, optionally in a cancellable background-task
/// mode.
///
/// The foreground path keeps the historical `tokio::time::timeout` wrapper
/// (dropping the future + `kill_on_drop`). The background path must actually
/// kill the whole process tree: a `powershell` script routinely spawns
/// grandchildren, and dropping the future only terminates the direct child on
/// Windows. It therefore polls the task's cancel flag and calls
/// `process_guard::kill_process_tree`, which is also what the Stop button and
/// app exit use.
///
/// `mounts` is what `run_cmd` passes (the script library and the host venv);
/// `run_shell` passes `Mounts::default()` and keeps the host interpreter.
pub(crate) async fn run_command_timed(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
    timeout_secs: u64,
    cancel: Option<Arc<AtomicBool>>,
    mounts: Mounts<'_>,
) -> String {
    match cancel {
        Some(flag) => {
            run_command_cancellable(cmd_type, code, cwd, timeout_secs, flag, mounts).await
        }
        None => tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            run_command(cmd_type, code, cwd, mounts),
        )
        .await
        .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
        .unwrap_or_else(|e| format!("Error: {}", e)),
    }
}

/// Run a program with pre-split arguments and a timeout.
///
/// This is the library-script entry point: the script path stays one `Command`
/// argument, so spaces / quotes / Windows backslashes cannot be re-parsed as
/// shell syntax. Cancellation, tree killing and output decoding are identical
/// to `run_command_timed`.
pub(crate) async fn run_program_timed(
    program: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    timeout_secs: u64,
    cancel: Option<Arc<AtomicBool>>,
    mounts: Mounts<'_>,
) -> String {
    let decode_hint = decode_hint_for_program(&program);
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args);
    finish_command(&mut cmd, cwd, mounts);

    match cancel {
        Some(flag) => {
            run_prepared_cancellable(
                cmd,
                timeout_secs,
                flag,
                decode_hint,
                "background script tool".to_string(),
            )
            .await
        }
        None => match tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await {
            Ok(Ok(output)) => format_process_output(output, decode_hint),
            Ok(Err(err)) => format!("Error: failed to start '{program}': {err}"),
            Err(_) => format!("Command timed out after {timeout_secs} seconds."),
        },
    }
}

fn decode_hint_for_program(program: &str) -> OutputDecodeHint {
    let name = Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    match name.as_str() {
        "powershell" | "pwsh" => OutputDecodeHint::PowerShell,
        "cmd" => OutputDecodeHint::CmdShell,
        _ => OutputDecodeHint::Direct,
    }
}
/// Cancellable, tree-killing variant used by background tasks.
async fn run_command_cancellable(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
    timeout_secs: u64,
    cancel: Arc<AtomicBool>,
    mounts: Mounts<'_>,
) -> String {
    let decode_hint = decode_hint_for_shell(&cmd_type);
    let cmd = match build_command_with_path(&cmd_type, &code, cwd, mounts) {
        Ok(cmd) => cmd,
        Err(err) => return format!("Error: {err}"),
    };
    let label = format!("background {cmd_type} tool");
    run_prepared_cancellable(cmd, timeout_secs, cancel, decode_hint, label).await
}

/// Shared cancellable runner: spawn the prepared command, then race its output
/// against the timeout and the task's cancel flag.
async fn run_prepared_cancellable(
    mut cmd: tokio::process::Command,
    timeout_secs: u64,
    cancel: Arc<AtomicBool>,
    decode_hint: OutputDecodeHint,
    label: String,
) -> String {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return format!("Error: failed to start command: {err}"),
    };
    let pid = child.id();
    // Registered for app-exit cleanup even though this task is turn-scoped:
    // a crash must not leave the child running.
    let tracked = crate::process_guard::track_child(&child, label);

    // `wait_with_output` drains both pipes concurrently and reaps the child.
    // The select below only decides whether to kill it first; the inner task
    // stays responsible for the actual wait, so a kill cannot deadlock on a
    // full pipe buffer.
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let outcome = child.wait_with_output().await;
        let _ = tx.send(outcome);
    });

    let outcome = tokio::select! {
        received = rx => Some(received.ok().and_then(|res| res.ok())),
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            crate::process_guard::kill_process_tree(pid);
            None
        }
        _ = wait_cancelled(&cancel) => {
            crate::process_guard::kill_process_tree(pid);
            None
        }
    };
    drop(tracked);

    match outcome {
        Some(Some(output)) => format_process_output(output, decode_hint),
        Some(None) => "Error: the command could not be awaited (process handle lost).".to_string(),
        None if cancel.load(Ordering::SeqCst) => {
            "Error: Background task cancelled — the process tree was terminated.".to_string()
        }
        None => format!("Command timed out after {timeout_secs} seconds."),
    }
}

/// Resolve once `flag` is set. Never completes otherwise, so `select!` uses it
/// purely as an interrupt.
async fn wait_cancelled(flag: &Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{reject_shell_interpreter, validate_path_resolved_program};

    /// `run_cmd` runs a program the OS resolves through `PATH`.
    #[test]
    fn bare_program_names_are_accepted() {
        for program in ["python", "python3", "git", "node", "cargo", "npm"] {
            assert!(
                validate_path_resolved_program(program).is_ok(),
                "'{program}' must be accepted"
            );
        }
    }

    /// Absolute and relative paths are refused: they are files, not
    /// PATH-resolved programs.
    #[test]
    fn paths_are_refused() {
        let paths = [
            "/usr/bin/python3",
            "/bin/sh",
            "./script.sh",
            "../script.sh",
            "sub/dir/tool.exe",
            r"C:\Python\python.exe",
            r"C:\Windows\System32\cmd.exe",
            r"..\..\evil.exe",
        ];
        for path in paths {
            let err = validate_path_resolved_program(path)
                .expect_err(&format!("'{path}' must be refused"));
            assert!(
                err.contains("PATH"),
                "unexpected error for '{path}': {err}"
            );
        }
    }

    /// A drive-relative path (`C:foo`) is a path too.
    #[cfg(windows)]
    #[test]
    fn drive_relative_paths_are_refused() {
        assert!(validate_path_resolved_program("C:tool.exe").is_err());
    }

    /// The background runner must react to the task's cancel flag: the whole
    /// process tree is killed and a cancellation marker is returned instead of
    /// the command's normal output.
    #[cfg(windows)]
    #[tokio::test]
    async fn cancelled_background_command_reports_cancellation() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let flag = Arc::new(AtomicBool::new(false));
        let trigger = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            trigger.store(true, Ordering::SeqCst);
        });

        let started = std::time::Instant::now();
        let output = super::run_command_cancellable(
            "powershell".to_string(),
            "Start-Sleep -Seconds 30".to_string(),
            None,
            60,
            flag,
            super::Mounts::default(),
        )
        .await;

        assert!(output.contains("cancelled"), "unexpected output: {output}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "cancellation did not interrupt the command"
        );
    }

    /// A library script is executed as interpreter + path + arguments. This
    /// covers the whole chain (`resolve` → `command_parts` →
    /// `run_program_timed`), including a path containing a space.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_library_script_runs_through_its_interpreter() {
        use std::fs;

        let workspace = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-run-script-{}",
            uuid::Uuid::new_v4()
        ));
        let scripts_dir = crate::workspace::scripts_root(&workspace);
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(
            scripts_dir.join("hello script.ps1"),
            "param([string]$Name)\nWrite-Output \"hello $Name\"\n",
        )
        .unwrap();

        let script = super::super::scripts::resolve(&workspace, "hello script")
            .unwrap()
            .expect("library script resolves");
        let (program, args) = script.command_parts(&["world".to_string()]);
        let output = super::run_program_timed(
            program,
            args,
            Some(workspace.clone()),
            60,
            None,
            super::Mounts {
                scripts: Some(&scripts_dir),
                env_dirs: &[],
                venv: None,
            },
        )
        .await;

        assert!(output.contains("hello world"), "unexpected output: {output}");
        let _ = fs::remove_dir_all(&workspace);
    }

    /// Mounted directories come first, in the order given, and an entry that
    /// is already on `PATH` is not duplicated.
    #[test]
    fn mounted_dirs_go_first_in_order() {
        let one = std::path::Path::new("/tmp/mount-one");
        let two = std::path::Path::new("/tmp/mount-two");
        let joined = super::prepend_to_path(&[one, two]);
        let parts: Vec<String> = std::env::split_paths(&joined)
            .map(|part| part.display().to_string())
            .collect();

        assert_eq!(parts[0], one.display().to_string());
        assert_eq!(parts[1], two.display().to_string());
        assert_eq!(
            parts
                .iter()
                .filter(|part| **part == one.display().to_string())
                .count(),
            1,
            "a mounted dir must appear once: {parts:?}"
        );
    }

    /// The `run_cmd` contract: a mounted venv becomes the child's `python`,
    /// with `VIRTUAL_ENV` pointing at it — so a library `.py` script (whose
    /// interpreter is the bare name `python` / `python3`) runs under 3.13.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_mounted_venv_is_the_childs_python() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let venv = std::env::temp_dir().join(format!(
            "rabbit-chat-studio-venv-{}",
            uuid::Uuid::new_v4()
        ));
        let bin = venv.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("python");
        fs::write(&fake, "#!/bin/sh\nprintf 'fake-venv %s\\n' \"$VIRTUAL_ENV\"\n").unwrap();
        let mut perms = fs::metadata(&fake).unwrap().permissions();
        PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&fake, perms).unwrap();

        let output = super::run_command(
            "direct".to_string(),
            "python -V".to_string(),
            None,
            super::Mounts {
                scripts: None,
                env_dirs: &[],
                venv: Some(&venv),
            },
        )
        .await
        .expect("the mounted interpreter runs");

        assert!(output.contains("fake-venv"), "unexpected output: {output}");
        assert!(
            output.contains(&venv.display().to_string()),
            "VIRTUAL_ENV must name the mounted venv: {output}"
        );
        let _ = fs::remove_dir_all(&venv);
    }
    /// An empty program name is not a program at all.
    #[test]
    fn an_empty_program_is_refused() {
        assert!(validate_path_resolved_program("").is_err());
        assert!(validate_path_resolved_program("   ").is_err());
    }

    /// Shell interpreters are refused: `run_cmd "bash script.sh"` would hand
    /// free-form code to an interpreter and bypass the script audit of
    /// `run_shell`.
    #[test]
    fn shell_interpreters_are_refused() {
        for program in [
            "bash",
            "sh",
            "csh",
            "tcsh",
            "zsh",
            "ksh",
            "dash",
            "fish",
            "cmd",
            "powershell",
            "pwsh",
            "wsl",
            "busybox",
            // Case and Windows spelling variations hit the same check.
            "Bash",
            "SH",
            "CMD",
            "Powershell",
            "bash.exe",
            "sh.exe",
            "cmd.exe",
            "powershell.exe",
        ] {
            let err = reject_shell_interpreter(program)
                .expect_err(&format!("'{program}' must be refused"));
            assert!(
                err.contains("run_shell"),
                "unexpected error for '{program}': {err}"
            );
        }
    }

    /// Ordinary PATH programs stay accepted — including a name that merely
    /// contains a shell word.
    #[test]
    fn ordinary_programs_pass_the_interpreter_check() {
        for program in ["python", "git", "node", "cargo", "bashike", "git-bash", "resh"] {
            assert!(
                reject_shell_interpreter(program).is_ok(),
                "'{program}' must be accepted"
            );
        }
    }
}
