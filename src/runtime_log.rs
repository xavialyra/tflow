use crate::discovery::sanitize_text;
use serde_json::json;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Error => "ERROR",
        }
    }

    fn metadata_value(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub timestamp: String,
    pub level: LogLevel,
    pub source_view: Option<String>,
    pub command: Option<String>,
    pub message: String,
    pub label: String,
    pub value: String,
}

impl LogRecord {
    fn new(
        sequence: u64,
        level: LogLevel,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
    ) -> Self {
        let (timestamp, clock) = timestamp();
        let message = sanitize_text(message);
        let source = source_view
            .map(|view| format!(" [{view}]"))
            .unwrap_or_default();
        let label = format!("{clock} {}{source}: {message}", level.as_str());
        Self {
            timestamp,
            level,
            source_view: source_view.map(str::to_string),
            command: command.map(str::to_string),
            message,
            label,
            value: format!("{}-{sequence}", std::process::id()),
        }
    }
}

pub struct RuntimeLog {
    file: Option<File>,
    path: Option<PathBuf>,
    sequence: u64,
}

impl RuntimeLog {
    pub fn open() -> std::io::Result<Self> {
        let path = log_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: Some(file),
            path: Some(path),
            sequence: 0,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn record(
        &mut self,
        level: LogLevel,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
    ) -> LogRecord {
        self.sequence += 1;
        let record = LogRecord::new(self.sequence, level, source_view, command, message);
        if let Some(file) = &mut self.file {
            let line = json!({
                "label": record.label,
                "value": record.value,
                "metadata": {
                    "timestamp": record.timestamp,
                    "level": record.level.metadata_value(),
                    "view": record.source_view,
                    "command": record.command,
                    "message": record.message,
                }
            });
            if let Ok(line) = serde_json::to_string(&line) {
                if writeln!(file, "{line}").is_err() || file.flush().is_err() {
                    self.file = None;
                }
            } else {
                self.file = None;
            }
        }
        record
    }
}

fn log_path() -> std::io::Result<PathBuf> {
    let path = env::var_os("TUI_LAUNCHER_LOG_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .map(|path| path.join("tui-launcher/runtime.jsonl"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local/state/tui-launcher/runtime.jsonl"))
        })
        .unwrap_or_else(|| PathBuf::from("tui-launcher-runtime.jsonl"));
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn timestamp() -> (String, String) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    let mut current = unsafe { std::mem::zeroed::<libc::tm>() };
    unsafe {
        libc::gmtime_r(&seconds, &mut current);
    }
    let year = current.tm_year + 1900;
    let month = current.tm_mon + 1;
    let day = current.tm_mday;
    let hour = current.tm_hour;
    let minute = current.tm_min;
    let second = current.tm_sec;
    (
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
        format!("{hour:02}:{minute:02}:{second:02}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn records_discovery_items_as_jsonl() {
        let root = env::temp_dir().join(format!("tui-launcher-log-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("runtime.jsonl");
        let mut log = RuntimeLog {
            file: Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .unwrap(),
            ),
            path: Some(path.clone()),
            sequence: 0,
        };

        let record = log.record(
            LogLevel::Error,
            Some("apps:default"),
            None,
            "discovery failed",
        );
        let contents = fs::read_to_string(path).unwrap();
        let item: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(item["label"], record.label);
        assert_eq!(item["metadata"]["level"], "error");
        assert_eq!(item["metadata"]["view"], "apps:default");
        fs::remove_dir_all(root).unwrap();
    }
}
