// Execution backend for `cli`-type sub-agents: launches a headless coding CLI
// installed on this machine (opencode, Claude Code, Codex, …), streams its
// output into the chat and reports the final text back to the caller.
//
// The agent definition (command/args/env/timeout) lives in
// `SubAgent::cli`; this module owns everything about *running* it: executable
// resolution (PATH + PATHEXT), Windows `.cmd`/`.bat`/`.ps1` wrapping, output
// streaming, timeouts and process-tree termination.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

use crate::tools::{decode_process_bytes, truncate_tool_output, OutputDecodeHint};
use crate::AppState;

use super::{CliAgentSettings, KeyValue, SubAgent};

/// Head/tail character budget kept for the tool result. The middle of a long
/// transcript is dropped first: the prompt echo at the top and the final
/// summary at the bottom matter most for the model.
const CAPTURE_HEAD_CHARS: usize = 8_000;
const CAPTURE_TAIL_CHARS: usize = 6_000;
/// A single streamed chunk is split at this size so one event never carries a
/// multi-megabyte payload.
const EMIT_CHUNK_CHARS: usize = 4_000;
/// Minimum gap between two streamed events of the same stream.
const EMIT_INTERVAL_MS: u64 = 300;
/// Timeout for the "probe" command (resolve + optional --version).
const PROBE_TIMEOUT_SECS: u64 = 15;
/// Upper bound for a run's timeout.
pub const MAX_TIMEOUT_SECS: u64 = 7_200;
/// Cancellation/timeout poll interval while waiting for the child process.
const WAIT_POLL_MS: u64 = 250;
/// How long to wait for the output pumps after the child exited. A grandchild
/// holding the pipe open must not block the result forever.
const PUMP_DRAIN_MS: u64 = 1_500;
/// `cmd.exe /C` gives up well before the OS command-line limit.
const WINDOWS_CMD_LINE_LIMIT: usize = 8_000;
/// CreateProcess accepts at most 32767 UTF-16 code units.
const WINDOWS_CREATE_PROCESS_LIMIT: usize = 32_000;

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CliProbe {
    pub command: String,
    pub resolved_path: String,
    pub exit_code: Option<i32>,
    pub output: String,
}

/// Outcome of one CLI agent run.
pub struct CliRunOutcome {
    /// "success" | "failed" | "timeout" | "cancelled"
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// Captured stdout/stderr (head+tail, truncated for the caller).
    pub output: String,
    /// Short one-line summary for the mission record / chat entry.
    pub summary: String,
}

// ─── Program resolution ───────────────────────────────────────────────────────

/// Resolve a configured `command` to a real file. Bare names are searched on
/// `PATH` (with Windows `PATHEXT` extensions), paths are used as given. Windows
/// package-manager CLIs (npm, bun, pnpm) are `.cmd` shims, which `CreateProcess`
/// cannot start directly — the caller wraps those in `cmd.exe`.
pub fn resolve_executable(command: &str) -> Result<PathBuf, String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("The command is empty.".to_string());
    }

    let candidate = PathBuf::from(trimmed);
    let looks_like_path =
        candidate.is_absolute() || trimmed.contains('/') || trimmed.contains('\\');
    if looks_like_path {
        return find_with_extensions(&candidate)
            .ok_or_else(|| format!("'{}' was not found.", trimmed));
    }

    let path_var = std::env::var_os("PATH")
        .ok_or_else(|| "PATH is not set in this environment.".to_string())?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = find_with_extensions(&dir.join(trimmed)) {
            return Ok(found);
        }
    }

    Err(format!(
        "'{}' was not found on PATH. Put the full path to the executable in the agent's command field if it is installed elsewhere.",
        trimmed
    ))
}

fn find_with_extensions(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }

    #[cfg(windows)]
    {
        let ext = candidate
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        let already_has_extension =
            matches!(ext.as_deref(), Some("exe" | "cmd" | "bat" | "com" | "ps1"));
        if !already_has_extension {
            for extension in windows_path_extensions() {
                let with_ext = PathBuf::from(format!("{}{}", candidate.display(), extension));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }

    None
}

#[cfg(windows)]
fn windows_path_extensions() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut extensions: Vec<String> = raw
        .split(';')
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase())
        .collect();
    if !extensions.iter().any(|value| value == ".ps1") {
        extensions.push(".ps1".to_string());
    }
    if extensions.is_empty() {
        extensions.push(".exe".to_string());
    }
    extensions
}

// ─── Spawn targets & argument quoting ────────────────────────────────────────

enum SpawnTarget {
    Direct(PathBuf),
    /// Windows `.cmd` / `.bat` shim — must go through `cmd.exe`.
    CmdShim(PathBuf),
    /// PowerShell script — must go through `powershell -File`.
    PsFile(PathBuf),
}

fn spawn_target(program: &Path) -> SpawnTarget {
    #[cfg(windows)]
    {
        let ext = program
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "cmd" | "bat" => return SpawnTarget::CmdShim(program.to_path_buf()),
            "ps1" => return SpawnTarget::PsFile(program.to_path_buf()),
            _ => {}
        }
    }
    let _ = program;
    SpawnTarget::Direct(program.to_path_buf())
}

/// Output decode hint matching how the process was launched, so Windows code
/// pages are handled the same way `run_command` handles them.
fn decode_hint_for(target: &SpawnTarget) -> OutputDecodeHint {
    match target {
        SpawnTarget::CmdShim(_) => OutputDecodeHint::CmdShell,
        SpawnTarget::PsFile(_) => OutputDecodeHint::PowerShell,
        SpawnTarget::Direct(_) => OutputDecodeHint::Direct,
    }
}

/// Quote one argument for the Windows command line as re-parsed by
/// `CommandLineToArgvW` (the CRT/Go/Rust convention): wrap in double quotes and
/// escape embedded quotes, doubling any run of backslashes directly before a
/// quote. Inside the quotes cmd.exe treats `& | < > ^ ( )` literally, so only
/// `%VAR%` expansion (rare in prompts) can still alter the text — that is what
/// the `stdin` prompt mode is for.
fn quote_windows_argument(argument: &str) -> String {
    if argument.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quotes = argument
        .chars()
        .any(|ch| ch.is_whitespace() || "\"&|<>^()".contains(ch));
    if !needs_quotes {
        return argument.to_string();
    }

    let mut out = String::with_capacity(argument.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in argument.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push_str("\\\"");
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// PowerShell single-quoted literal (`''` escapes a quote). Used for `-File`
/// script arguments, which PowerShell parses with its own rules.
fn quote_powershell_argument(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "''"))
}

fn build_command(
    target: &SpawnTarget,
    args: &[String],
    env: &[KeyValue],
    cwd: &Path,
) -> tokio::process::Command {
    let mut command = match target {
        SpawnTarget::Direct(program) => {
            let mut command = tokio::process::Command::new(program);
            command.args(args);
            command
        }
        SpawnTarget::CmdShim(program) => {
            // `cmd.exe /D /S /C "<quoted program> <quoted args>"` — the outer
            // quote pair is required for `/S` to strip exactly one level.
            let inner = std::iter::once(quote_windows_argument(&program.to_string_lossy()))
                .chain(args.iter().map(|arg| quote_windows_argument(arg)))
                .collect::<Vec<_>>()
                .join(" ");
            let wrapped = format!("\"{}\"", inner);
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
            let mut command = tokio::process::Command::new(comspec);
            command.args(["/D", "/S", "/C", &wrapped]);
            command
        }
        SpawnTarget::PsFile(script) => {
            let mut command = tokio::process::Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]);
            command.arg(script);
            // Everything after `-File <script>` is passed through to the script
            // as literal strings: PowerShell parses none of it, so the arguments
            // must NOT be quoted for PowerShell (adding `'…'` would hand the
            // script a value that literally contains the quotes).
            command.args(args);
            command
        }
    };

    for KeyValue { key, value } in env {
        if !key.trim().is_empty() {
            command.env(key, value);
        }
    }

    command.current_dir(cwd);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Never leave an orphaned agent behind if the future is dropped.
    command.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        // Own process group so timeout/cancel can kill the whole tree.
        command.process_group(0);
    }
    command
}

/// Terminate the child and everything it spawned. CLI agents start their own
/// subprocesses (node, git, build tools), so killing only the shim is not
/// enough.
fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut command = std::process::Command::new("taskkill");
        command
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        let _ = command.status();
    }

    #[cfg(unix)]
    {
        // Negative pid = whole process group (see `process_group(0)`).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

// ─── Capture buffers ─────────────────────────────────────────────────────────

#[derive(Default)]
struct Capture {
    head: String,
    head_chars: usize,
    tail: String,
    dropped_chars: usize,
}

impl Capture {
    fn push_line(&mut self, line: &str) {
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');

        if self.head_chars < CAPTURE_HEAD_CHARS {
            let room = CAPTURE_HEAD_CHARS - self.head_chars;
            let take = text.chars().count().min(room);
            let cut = byte_index_for_chars(&text, take);
            self.head.push_str(&text[..cut]);
            self.head_chars += take;
            if cut < text.len() {
                self.dropped_chars += text[cut..].chars().count();
                self.tail.push_str(&text[cut..]);
            }
        } else {
            self.dropped_chars += text.chars().count();
            self.tail.push_str(&text);
        }

        let tail_chars = self.tail.chars().count();
        if tail_chars > CAPTURE_TAIL_CHARS {
            let drop = tail_chars - CAPTURE_TAIL_CHARS;
            let cut = byte_index_for_chars(&self.tail, drop);
            self.tail.drain(..cut);
        }
    }

    fn finish(self) -> String {
        if self.tail.is_empty() {
            return self.head;
        }
        format!(
            "{}\n...[truncated {} chars of output]...\n{}",
            self.head, self.dropped_chars, self.tail
        )
    }
}

/// Byte index of the `chars`-th character (clamped to the string length).
fn byte_index_for_chars(text: &str, chars: usize) -> usize {
    if chars == 0 {
        return 0;
    }
    match text.char_indices().nth(chars) {
        Some((index, _)) => index,
        None => text.len(),
    }
}

// ─── Streaming ───────────────────────────────────────────────────────────────

struct PumpOutput {
    text: String,
}

fn emit_chunk(
    app: &AppHandle,
    task_id: &str,
    pending: &mut String,
    last_emit: &mut tokio::time::Instant,
    force: bool,
) {
    if pending.is_empty() {
        return;
    }
    let due = force
        || pending.chars().count() >= EMIT_CHUNK_CHARS
        || last_emit.elapsed() >= Duration::from_millis(EMIT_INTERVAL_MS);
    if !due {
        return;
    }

    while !pending.is_empty() {
        let take = pending.chars().count().min(EMIT_CHUNK_CHARS);
        let cut = byte_index_for_chars(pending, take);
        let chunk: String = pending.drain(..cut).collect();
        // Same shape as the LLM runner: { task_id, token } on `agent-task-token`.
        let _ = app.emit("agent-task-token", json!({ "task_id": task_id, "token": chunk }));
    }
    *last_emit = tokio::time::Instant::now();
}

async fn pump<R: AsyncRead + Unpin>(
    app: AppHandle,
    task_id: String,
    reader: R,
    decode_hint: OutputDecodeHint,
) -> PumpOutput {
    let mut reader = BufReader::new(reader);
    let mut buffer: Vec<u8> = Vec::new();
    let mut capture = Capture::default();
    let mut pending = String::new();
    let mut last_emit = tokio::time::Instant::now();

    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {
                // Decode per line with the code-page-aware helper: Windows CLIs
                // (and cmd.exe shims) write in the OEM/ANSI code page, not UTF-8.
                let line = decode_process_bytes(&buffer, decode_hint)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                capture.push_line(&line);
                pending.push_str(&line);
                pending.push('\n');
                emit_chunk(&app, &task_id, &mut pending, &mut last_emit, false);
            }
            Err(_) => break,
        }
    }

    emit_chunk(&app, &task_id, &mut pending, &mut last_emit, true);
    PumpOutput {
        text: capture.finish(),
    }
}

// ─── Argument templating ─────────────────────────────────────────────────────

fn expand_template(
    template: &str,
    prompt: Option<&str>,
    model: Option<&str>,
    cwd: &Path,
    agent_name: &str,
) -> String {
    let mut out = template
        .replace("{cwd}", &cwd.to_string_lossy())
        .replace("{agent_name}", agent_name);
    if let Some(model) = model {
        out = out.replace("{model}", model);
    }
    if let Some(prompt) = prompt {
        out = out.replace("{prompt}", prompt);
    }
    out
}

/// Renders the argv actually passed to the CLI. In `arg` mode the prompt fills
/// `{prompt}`; if no template position uses it, it is appended as the last
/// argument so a bare `opencode run` + empty args still works.
fn render_arguments(
    settings: &CliAgentSettings,
    agent_name: &str,
    prompt: &str,
    model: Option<&str>,
    cwd: &Path,
    session_continue: bool,
) -> (Vec<String>, bool) {
    let use_stdin = settings.prompt_via.eq_ignore_ascii_case("stdin");
    let prompt_for_args = if use_stdin { None } else { Some(prompt) };

    let mut placeholder_used = false;
    let mut args: Vec<String> = Vec::new();
    {
        let push = |template: &str, args: &mut Vec<String>, placeholder_used: &mut bool| {
            if template.contains("{prompt}") {
                *placeholder_used = true;
            }
            args.push(expand_template(template, prompt_for_args, model, cwd, agent_name));
        };

        for template in &settings.args {
            push(template, &mut args, &mut placeholder_used);
        }
        if session_continue {
            for template in &settings.continue_args {
                push(template, &mut args, &mut placeholder_used);
            }
        }
        if model.is_some() {
            for template in &settings.model_args {
                push(template, &mut args, &mut placeholder_used);
            }
        }
    }
    if !use_stdin && !placeholder_used {
        args.push(prompt.to_string());
    }

    (args, use_stdin)
}

/// Short human-readable preview of a command line (panel preview, confirm
/// dialog, chat entry).
pub fn render_preview(program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote_cli_display(&program.to_string_lossy())];
    for arg in args {
        parts.push(quote_cli_display(arg));
    }
    let mut rendered = parts.join(" ");
    if rendered.chars().count() > 600 {
        rendered = rendered.chars().take(600).collect::<String>() + " …";
    }
    rendered
}

fn quote_cli_display(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    if value.chars().any(|ch| ch.is_whitespace()) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// The command line an agent would run for `prompt` (used for the approval
/// dialog text and panel previews). Returns an error when the executable
/// cannot be resolved.
pub fn describe_run(agent: &SubAgent, prompt: &str, cwd: &Path) -> Result<String, String> {
    let resolved = resolve_executable(&agent.cli.command)?;
    let model = agent.model.as_deref();
    let (args, _) = render_arguments(&agent.cli, &agent.name, prompt, model, cwd, false);
    Ok(render_preview(&resolved, &args))
}

// ─── Probe ───────────────────────────────────────────────────────────────────

/// Resolve a command and (optionally) run it, so the panel can show the real
/// executable path and the CLI's version output before an agent uses it.
pub async fn probe(command: &str, args: &[String]) -> Result<CliProbe, String> {
    let resolved = resolve_executable(command)?;
    let target = spawn_target(&resolved);
    let decode_hint = decode_hint_for(&target);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut child = build_command(&target, args, &[], &cwd)
        .spawn()
        .map_err(|e| format!("Failed to start '{}': {e}", resolved.display()))?;

    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    drop(child.stdin.take());

    let stdout_task = stdout.map(|reader| tokio::spawn(read_all(reader, decode_hint)));
    let stderr_task = stderr.map(|reader| tokio::spawn(read_all(reader, decode_hint)));

    let wait = tokio::time::timeout(Duration::from_secs(PROBE_TIMEOUT_SECS), child.wait()).await;
    let (exit_code, timed_out) = match wait {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(err)) => return Err(err.to_string()),
        Err(_) => {
            kill_process_tree(pid);
            let _ = child.wait().await;
            (None, true)
        }
    };

    let mut output = String::new();
    if let Some(task) = stdout_task {
        if let Ok(Ok(text)) = tokio::time::timeout(Duration::from_millis(PUMP_DRAIN_MS), task).await
        {
            output.push_str(&text);
        }
    }
    if let Some(task) = stderr_task {
        if let Ok(Ok(text)) = tokio::time::timeout(Duration::from_millis(PUMP_DRAIN_MS), task).await {
            if !text.trim().is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("STDERR:\n");
                output.push_str(&text);
            }
        }
    }
    if timed_out {
        output.push_str(&format!(
            "\n[probe timed out after {PROBE_TIMEOUT_SECS}s and was terminated]"
        ));
    }

    Ok(CliProbe {
        command: command.to_string(),
        resolved_path: resolved.display().to_string(),
        exit_code,
        output: truncate_tool_output(output),
    })
}

async fn read_all<R: AsyncRead + Unpin>(reader: R, decode_hint: OutputDecodeHint) -> String {
    let mut reader = BufReader::new(reader);
    let mut buffer: Vec<u8> = Vec::new();
    let mut out = String::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {
                out.push_str(
                    decode_process_bytes(&buffer, decode_hint).trim_end_matches(['\r', '\n']),
                );
                out.push('\n');
            }
            Err(_) => break,
        }
    }
    out
}

// ─── Execution ───────────────────────────────────────────────────────────────

/// Runs a `cli`-type agent for `prompt` inside `cwd` (already sandbox-checked by
/// the caller) and returns its outcome. Output is streamed as `agent-task-token`
/// events keyed by `task_id`.
pub async fn run(
    app: &AppHandle,
    agent: &SubAgent,
    task_id: &str,
    prompt: &str,
    cwd: PathBuf,
) -> Result<CliRunOutcome, String> {
    let settings = &agent.cli;
    let resolved = resolve_executable(&settings.command)?;

    let timeout_seconds = (settings.default_timeout_seconds as u64).clamp(1, MAX_TIMEOUT_SECS);
    let model = agent.model.as_deref();
    let (rendered_args, use_stdin) =
        render_arguments(settings, &agent.name, prompt, model, &cwd, false);
    let preview = render_preview(&resolved, &rendered_args);

    // Windows CreateProcess rejects command lines over 32767 chars, and cmd.exe
    // shims choke far earlier — surface the real cause instead of a bare spawn
    // error and point at the stdin mode.
    let raw_length = resolved.to_string_lossy().chars().count()
        + rendered_args
            .iter()
            .map(|arg| arg.chars().count() + 1)
            .sum::<usize>();
    let shim_limit = matches!(spawn_target(&resolved), SpawnTarget::CmdShim(_))
        .then_some(WINDOWS_CMD_LINE_LIMIT)
        .unwrap_or(WINDOWS_CREATE_PROCESS_LIMIT);
    if cfg!(windows) && !use_stdin && raw_length > shim_limit {
        return Err(format!(
            "the rendered command line is {raw_length} characters, above the Windows limit ({shim_limit}). Set this agent's prompt delivery to `stdin`, or shorten the prompt."
        ));
    }

    let target = spawn_target(&resolved);
    let decode_hint = decode_hint_for(&target);
    let mut child = build_command(&target, &rendered_args, &settings.env, &cwd)
        .spawn()
        .map_err(|e| format!("failed to start '{}': {e}", resolved.display()))?;

    let stdout_task = child.stdout.take().map(|reader| {
        tokio::spawn(pump(app.clone(), task_id.to_string(), reader, decode_hint))
    });
    let stderr_task = child.stderr.take().map(|reader| {
        tokio::spawn(pump(app.clone(), task_id.to_string(), reader, decode_hint))
    });

    if use_stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let payload = prompt.to_string();
            tokio::spawn(async move {
                let _ = stdin.write_all(payload.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    } else {
        drop(child.stdin.take());
    }

    let pid = child.id();
    let started = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let mut poll = tokio::time::interval(Duration::from_millis(WAIT_POLL_MS));
    poll.tick().await; // the first tick is immediate

    let mut outcome: &'static str = "success";
    let mut exit_code: Option<i32> = None;
    let mut stop_reason: Option<&'static str> = None;

    loop {
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(status) => {
                        exit_code = status.code();
                        outcome = if status.success() { "success" } else { "failed" };
                    }
                    Err(_) => {
                        outcome = "failed";
                        stop_reason = Some("wait-error");
                    }
                }
                break;
            }
            _ = tokio::time::sleep_until(deadline) => {
                stop_reason = Some("timeout");
                break;
            }
            _ = poll.tick() => {
                if app.state::<AppState>().chat_cancelled.load(Ordering::SeqCst) {
                    stop_reason = Some("cancelled");
                    break;
                }
            }
        }
    }

    if let Some(reason) = stop_reason {
        kill_process_tree(pid);
        let _ = child.wait().await;
        outcome = reason;
    }

    let mut stdout_text = String::new();
    if let Some(task) = stdout_task {
        if let Ok(Ok(pumped)) = tokio::time::timeout(Duration::from_millis(PUMP_DRAIN_MS), task).await
        {
            stdout_text = pumped.text;
        }
    }
    let mut stderr_text = String::new();
    if let Some(task) = stderr_task {
        if let Ok(Ok(pumped)) = tokio::time::timeout(Duration::from_millis(PUMP_DRAIN_MS), task).await
        {
            stderr_text = pumped.text;
        }
    }

    let duration = started.elapsed();
    let duration_text = format!("{:.1}s", duration.as_secs_f32());

    let header = match outcome {
        "success" => format!(
            "CLI agent \"{}\" finished in {} (exit code 0).",
            agent.name, duration_text
        ),
        "failed" => format!(
            "CLI agent \"{}\" exited with code {} after {}.",
            agent.name,
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            duration_text
        ),
        "timeout" => format!(
            "CLI agent \"{}\" hit its {}s timeout and its process tree was terminated.",
            agent.name, timeout_seconds
        ),
        "cancelled" => format!(
            "CLI agent \"{}\" was cancelled by the user after {} and its process tree was terminated.",
            agent.name, duration_text
        ),
        _ => format!(
            "CLI agent \"{}\" could not be waited on; it may have been terminated.",
            agent.name
        ),
    };

    let mut body = format!(
        "{}\nCommand: {}\nWorking directory: {}\n",
        header,
        preview,
        cwd.display()
    );
    if !stdout_text.trim().is_empty() {
        body.push_str("\n--- output ---\n");
        body.push_str(&stdout_text);
    }
    if !stderr_text.trim().is_empty() {
        body.push_str("\n--- stderr ---\n");
        body.push_str(&stderr_text);
    }
    if outcome != "success" {
        body.push_str(
            "\nThe CLI agent did not report success. Inspect the output above and decide whether to retry with adjusted arguments or continue with other tools.",
        );
    }

    Ok(CliRunOutcome {
        status: outcome,
        exit_code,
        duration_ms: duration.as_millis() as u64,
        output: truncate_tool_output(body),
        summary: header,
    })
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{CliAgentSettings, SubAgent};

    fn settings(args: Vec<&str>, prompt_via: &str) -> CliAgentSettings {
        CliAgentSettings {
            command: "opencode".to_string(),
            args: args.into_iter().map(|value| value.to_string()).collect(),
            prompt_via: prompt_via.to_string(),
            continue_args: vec!["--continue".to_string()],
            model_args: vec!["--model".to_string(), "{model}".to_string()],
            env: Vec::new(),
            default_timeout_seconds: 600,
            cwd: String::new(),
        }
    }

    fn agent(settings: CliAgentSettings) -> SubAgent {
        SubAgent {
            id: "agent-1".to_string(),
            name: "opencode".to_string(),
            description: String::new(),
            agent_type: crate::agents::AGENT_TYPE_CLI.to_string(),
            system_prompt: String::new(),
            model: None,
            max_complete_tokens: None,
            max_tokens: None,
            temperature: None,
            allowed_tools: Vec::new(),
            allowed_skills: Vec::new(),
            max_iterations: 10,
            cli: settings,
            cloud: Default::default(),
            require_approval: true,
            enabled: true,
        }
    }

    #[test]
    fn template_placeholders_are_expanded() {
        let cwd = PathBuf::from("work dir");
        let expanded = expand_template(
            "run --dir {cwd} {prompt}",
            Some("fix the bug"),
            Some("gpt-5"),
            &cwd,
            "opencode",
        );
        assert!(expanded.contains("--dir work dir"));
        assert!(expanded.ends_with("fix the bug"));
    }

    #[test]
    fn arguments_use_prompt_placeholder() {
        let settings = settings(vec!["run", "{prompt}"], "arg");
        let (args, use_stdin) =
            render_arguments(&settings, "opencode", "hello world", None, Path::new("."), false);
        assert_eq!(args, vec!["run".to_string(), "hello world".to_string()]);
        assert!(!use_stdin);
    }

    #[test]
    fn arguments_append_prompt_when_placeholder_missing() {
        let settings = settings(vec!["-p"], "arg");
        let (args, _) =
            render_arguments(&settings, "opencode", "hello", None, Path::new("."), false);
        assert_eq!(args, vec!["-p".to_string(), "hello".to_string()]);
    }

    #[test]
    fn stdin_mode_never_inlines_the_prompt() {
        let settings = settings(vec!["-p"], "stdin");
        let (args, use_stdin) =
            render_arguments(&settings, "opencode", "hello", None, Path::new("."), false);
        assert_eq!(args, vec!["-p".to_string()]);
        assert!(use_stdin);
    }

    #[test]
    fn continue_and_model_arguments_are_appended() {
        let settings = settings(vec!["run", "{prompt}"], "arg");
        let (args, _) = render_arguments(
            &settings,
            "opencode",
            "hi",
            Some("anthropic/claude"),
            Path::new("."),
            true,
        );
        assert_eq!(
            args,
            vec![
                "run".to_string(),
                "hi".to_string(),
                "--continue".to_string(),
                "--model".to_string(),
                "anthropic/claude".to_string(),
            ]
        );
    }

    #[test]
    fn describe_run_renders_the_command_line() {
        let mut agent = agent(settings(vec!["run", "{prompt}"], "arg"));
        agent.cli.command = std::env::current_exe()
            .expect("test exe")
            .to_string_lossy()
            .to_string();
        let preview = describe_run(&agent, "hello", Path::new(".")).expect("preview");
        assert!(preview.contains("run"));
        assert!(preview.contains("hello"));
    }

    #[test]
    fn windows_quoting_handles_quotes_and_backslashes() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument("two words"), "\"two words\"");
        assert_eq!(quote_windows_argument("say \"hi\""), "\"say \\\"hi\\\"\"");
        // Trailing backslash must be doubled so it does not escape the quote.
        assert_eq!(
            quote_windows_argument("C:\\dir with space\\"),
            "\"C:\\dir with space\\\\\""
        );
    }

    #[test]
    fn powershell_quoting_escapes_single_quotes() {
        assert_eq!(quote_powershell_argument("it's"), "'it''s'");
    }

    #[test]
    fn absolute_paths_resolve_when_the_file_exists() {
        let exe = std::env::current_exe().expect("test executable path");
        let resolved = resolve_executable(&exe.to_string_lossy()).expect("resolve current exe");
        assert_eq!(resolved, exe);
    }

    #[test]
    fn capture_keeps_head_and_tail() {
        let mut capture = Capture::default();
        for index in 0..2000 {
            capture.push_line(&format!("line {index:04} {}", "x".repeat(20)));
        }
        let text = capture.finish();
        assert!(text.contains("line 0000"));
        assert!(text.contains("line 1999"));
        assert!(text.contains("truncated"));
    }

    /// End-to-end smoke test of resolution + spawn + streaming + decoding
    /// against a real Windows shim-free executable.
    #[cfg(windows)]
    #[tokio::test]
    async fn probe_runs_a_real_process_and_captures_output() {
        let result = probe(
            "cmd",
            &["/c".to_string(), "echo agent-cli-probe".to_string()],
        )
        .await
        .expect("probe cmd.exe");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.output.contains("agent-cli-probe"));
        assert!(result.resolved_path.to_lowercase().ends_with("cmd.exe"));
    }

    /// A hanging process must be killed by the probe timeout instead of
    /// blocking the app — the same kill path used for cancelled runs.
    #[cfg(windows)]
    #[tokio::test]
    async fn probe_kills_a_hanging_process() {
        let started = std::time::Instant::now();
        let result = probe(
            "cmd",
            &[
                "/c".to_string(),
                format!("ping -n {} 127.0.0.1 > nul", PROBE_TIMEOUT_SECS + 30),
            ],
        )
        .await
        .expect("probe cmd.exe");
        assert!(result.output.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(PROBE_TIMEOUT_SECS + 15));
    }
}
