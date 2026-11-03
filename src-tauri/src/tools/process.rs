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

    // A bare program name is exactly one `Normal` component. Anything else
    // carries a separator, a drive prefix or a root — that is a path.
    let mut components = Path::new(trimmed).components();
    let is_bare_name = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none();

    if !is_bare_name {
        return Err(format!(
            "⛔ run_cmd only runs programs that PATH resolves (for example 'python' or \
             'python3'). '{}' is a path — pass a bare program name instead, or use \
             `run_shell` when an absolute path is really needed.",
            trimmed
        ));
    }

    Ok(())
}

/// Build a command for `cmd_type` rooted at `cwd`, without running it.
/// Used by `run_command` (synchronous) and by `serve` (long-lived children).
pub(crate) fn build_command(
    cmd_type: &str,
    code: &str,
    cwd: Option<PathBuf>,
) -> Result<tokio::process::Command, String> {
    let mut cmd = match cmd_type {
        "direct" => {
            // Run the executable directly — no shell wrapper needed.
            let parts = split_command_line(code);
            if parts.is_empty() {
                return Err("Empty command".to_string());
            }
            // `direct` is the `run_cmd` contract: a PATH-resolved program name,
            // never a path.
            validate_path_resolved_program(&parts[0])?;
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
            let mut c = tokio::process::Command::new("powershell");
            let wrapped = wrap_powershell_script_for_utf8(code);
            c.args(["-NoProfile", "-NonInteractive", "-Command", &wrapped]);
            c
        }
        _ => return Err(format!("Unsupported command type: {}", cmd_type)),
    };

    if let Some(dir) = cwd {
        cmd.current_dir(&dir);
        // `current_dir` does not update the inherited `PWD`, and programs that
        // resolve their project root from it would keep seeing the app's launch
        // directory. Keep the two in sync for run_cmd / run_shell (same rule as
        // `agents/cli.rs` and `mcp.rs`).
        cmd.env("PWD", &dir);
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

    Ok(cmd)
}

pub async fn run_command(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
) -> Result<String, String> {
    let decode_hint = decode_hint_for_shell(&cmd_type);
    let mut cmd = build_command(&cmd_type, &code, cwd)?;
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
pub(crate) async fn run_command_timed(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
    timeout_secs: u64,
    cancel: Option<Arc<AtomicBool>>,
) -> String {
    match cancel {
        Some(flag) => run_command_cancellable(cmd_type, code, cwd, timeout_secs, flag).await,
        None => tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            run_command(cmd_type, code, cwd),
        )
        .await
        .unwrap_or_else(|_| Ok(format!("Command timed out after {} seconds.", timeout_secs)))
        .unwrap_or_else(|e| format!("Error: {}", e)),
    }
}

/// Cancellable, tree-killing variant used by background tasks.
async fn run_command_cancellable(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
    timeout_secs: u64,
    cancel: Arc<AtomicBool>,
) -> String {
    let decode_hint = decode_hint_for_shell(&cmd_type);
    let mut cmd = match build_command(&cmd_type, &code, cwd) {
        Ok(cmd) => cmd,
        Err(err) => return format!("Error: {err}"),
    };
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
    let tracked = crate::process_guard::track_child(&child, format!("background {cmd_type} tool"));

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
            "⛔ Background task cancelled — the process tree was terminated.".to_string()
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
    use super::validate_path_resolved_program;

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
        )
        .await;

        assert!(output.contains("cancelled"), "unexpected output: {output}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "cancellation did not interrupt the command"
        );
    }

    /// An empty program name is not a program at all.
    #[test]
    fn an_empty_program_is_refused() {
        assert!(validate_path_resolved_program("").is_err());
        assert!(validate_path_resolved_program("   ").is_err());
    }
}
