// Child-process execution and output decoding: `run_command`, and the
// Windows code-page aware decoding of child stdout/stderr.

use std::path::PathBuf;

use super::split_command_line;
use super::truncate_tool_output;

#[derive(Clone, Copy)]
pub(super) enum OutputDecodeHint {
    Default,
    Direct,
    CmdShell,
    PowerShell,
}

pub(super) fn decode_process_bytes(bytes: &[u8], _hint: OutputDecodeHint) -> String {
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

pub async fn run_command(
    cmd_type: String,
    code: String,
    cwd: Option<PathBuf>,
) -> Result<String, String> {
    let decode_hint = match cmd_type.as_str() {
        "direct" => OutputDecodeHint::Direct,
        "bash" | "sh" if cfg!(windows) => OutputDecodeHint::CmdShell,
        "powershell" | "pwsh" => OutputDecodeHint::PowerShell,
        _ => OutputDecodeHint::Default,
    };

    let mut cmd = match cmd_type.as_str() {
        "direct" => {
            // Run the executable directly — no shell wrapper needed.
            let parts = split_command_line(&code);
            if parts.is_empty() {
                return Err("Empty command".to_string());
            }
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
                let wrapped = wrap_cmd_script_for_utf8(&code);
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
            let wrapped = wrap_powershell_script_for_utf8(&code);
            c.args(["-NoProfile", "-NonInteractive", "-Command", &wrapped]);
            c
        }
        _ => return Err(format!("Unsupported command type: {}", cmd_type)),
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output().await.map_err(|e| e.to_string())?;
    Ok(format_process_output(output, decode_hint))
}
