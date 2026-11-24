use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoggerOutput {
    File,
    Println,
}

impl Default for LoggerOutput {
    fn default() -> Self {
        Self::File
    }
}

pub struct AppLogger {
    enabled: bool,
    output: LoggerOutput,
    /// Directory the dated log files live in. One file per local day,
    /// `app-<YYYY-MM-DD>.log` — the date is resolved per log call, so a
    /// process that runs across midnight simply starts the next file.
    dir: PathBuf,
    /// Process tag rendered into every line (`primary-<pid>` / `shadow-<pid>`),
    /// so two multi-instance windows writing the same file stay attributable.
    prefix: String,
}

/// True when this binary carries `debug_assertions` (`tauri dev`, `cargo build`).
/// The logger is gated on exactly this flag (`AppLogger::new(cfg!(debug_assertions), …)`),
/// so a release build writes no log lines at all — Settings → Debug hides the
/// log-output switch there instead of offering one that would do nothing.
pub fn is_debug_build() -> bool {
    cfg!(debug_assertions)
}

impl AppLogger {
    pub fn new(enabled: bool, output: LoggerOutput, dir: PathBuf, prefix: String) -> Self {
        Self {
            enabled,
            output,
            dir,
            prefix,
        }
    }

    pub fn set_output(&mut self, output: LoggerOutput) {
        self.output = output;
    }

    pub fn log(&self, level: &str, message: &str) {
        if !self.enabled {
            return;
        }

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let line = format!("[{}][{}][{}] {}", ts, self.prefix, level, message);

        match self.output {
            LoggerOutput::Println => {
                println!("{}", line);
            }
            LoggerOutput::File => {
                let date = chrono::Local::now().format("%Y-%m-%d");
                let path = self.dir.join(format!("app-{date}.log"));
                if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
                    let _ = writeln!(file, "{}", line);
                }
            }
        }
    }
}

/// Delete `app-<YYYY-MM-DD>.log` files older than `retention_days` local days —
/// the same `log_retention_days` setting that prunes the database log rows;
/// `0` disables the sweep, matching `db::prune_old_logs`. Files whose name does
/// not parse as a dated log (the legacy `app.log`, foreign files) are left
/// untouched. Returns how many files were removed.
pub fn sweep_dated_files(dir: &Path, retention_days: u32) -> usize {
    if retention_days == 0 || !dir.is_dir() {
        return 0;
    }
    let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(retention_days as i64);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(date) = name
            .strip_prefix("app-")
            .and_then(|rest| rest.strip_suffix(".log"))
        else {
            continue;
        };
        if date.len() != 10 {
            continue;
        }
        let Ok(file_date) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
            continue;
        };
        if file_date < cutoff && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}
