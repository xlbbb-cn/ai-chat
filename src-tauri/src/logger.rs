use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
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
    file_path: PathBuf,
}

impl AppLogger {
    pub fn new(enabled: bool, output: LoggerOutput, file_path: PathBuf) -> Self {
        Self {
            enabled,
            output,
            file_path,
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
        let line = format!("[{}][{}] {}", ts, level, message);

        match self.output {
            LoggerOutput::Println => {
                println!("{}", line);
            }
            LoggerOutput::File => {
                if let Ok(mut file) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.file_path)
                {
                    let _ = writeln!(file, "{}", line);
                }
            }
        }
    }
}
