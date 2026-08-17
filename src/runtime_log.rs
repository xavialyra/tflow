use crate::text::sanitize_text;
use serde_json::json;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_MESSAGE_CHARS: usize = 16 * 1024;

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
        level: LogLevel,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
        sequence: u64,
    ) -> Self {
        let (timestamp, clock) = timestamp();
        let sanitized = sanitize_text(message);
        let mut characters = sanitized.chars();
        let mut message = characters
            .by_ref()
            .take(MAX_LOG_MESSAGE_CHARS)
            .collect::<String>();
        if characters.next().is_some() {
            message.push_str("...");
        }
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
    path: Option<PathBuf>,
    file: Option<File>,
    pending_warning: Option<String>,
    next_sequence: u64,
}

impl RuntimeLog {
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            path: None,
            file: None,
            pending_warning: None,
            next_sequence: 0,
        }
    }

    pub fn open(configured_path: Option<&Path>) -> Self {
        match log_path(configured_path) {
            Ok(path) => Self::open_configured(path),
            Err(error) => Self::with_warning(None, error),
        }
    }

    #[cfg(test)]
    fn open_path(path: PathBuf) -> Self {
        Self::open_configured(path)
    }

    fn open_configured(path: PathBuf) -> Self {
        match open_append_file(&path) {
            Ok(file) => Self {
                path: Some(path),
                file: Some(file),
                pending_warning: None,
                next_sequence: 0,
            },
            Err(error) => Self::with_warning(Some(path), error),
        }
    }

    fn with_warning(path: Option<PathBuf>, error: io::Error) -> Self {
        let target = path
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "the configured path".to_string());
        Self {
            path,
            file: None,
            pending_warning: Some(format!("runtime log disabled for {target}: {error}")),
            next_sequence: 0,
        }
    }

    fn new_record(
        &mut self,
        level: LogLevel,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
    ) -> LogRecord {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        LogRecord::new(level, source_view, command, message, sequence)
    }

    pub(crate) fn take_warning_record(&mut self) -> Option<LogRecord> {
        let warning = self.pending_warning.take()?;
        Some(self.new_record(LogLevel::Error, None, None, &warning))
    }

    pub fn record(
        &mut self,
        level: LogLevel,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
    ) -> LogRecord {
        let record = self.new_record(level, source_view, command, message);
        let Some(line) = serialize_record(&record) else {
            return record;
        };
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if let Some(file) = self.file.as_mut()
            && let Err(error) = file.write_all(&bytes).and_then(|_| file.flush())
        {
            self.disable(error);
        }
        record
    }

    fn disable(&mut self, error: io::Error) {
        self.file = None;
        if self.pending_warning.is_none() {
            let target = self
                .path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "the configured path".to_string());
            self.pending_warning = Some(format!("runtime log disabled for {target}: {error}"));
        }
    }
}

fn serialize_record(record: &LogRecord) -> Option<String> {
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
    serde_json::to_string(&line).ok()
}

fn open_append_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure_regular_file(&file, path)?;
    Ok(file)
}

fn ensure_regular_file(file: &File, path: &Path) -> io::Result<()> {
    if file.metadata()?.file_type().is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{} is not a regular file", path.display()),
    ))
}

fn log_path(configured_path: Option<&Path>) -> io::Result<PathBuf> {
    if let Some(path) = configured_path {
        return Ok(path.to_path_buf());
    }

    let xdg_state_home = env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    let home = env::var_os("HOME").map(PathBuf::from);
    default_log_path(xdg_state_home.as_deref(), home.as_deref())
}

fn default_log_path(xdg_state_home: Option<&Path>, home: Option<&Path>) -> io::Result<PathBuf> {
    let state_home = xdg_state_home
        .filter(|path| path.is_absolute())
        .map(Path::to_path_buf)
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|path| path.join(".local/state"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither XDG_STATE_HOME nor HOME is set to an absolute path",
            )
        })?;
    Ok(state_home.join("tui-launcher/runtime.jsonl"))
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

    fn temporary_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("tui-launcher-log-{name}-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn default_log_path_prefers_xdg_state_home() {
        assert_eq!(
            default_log_path(Some(Path::new("/tmp/state")), Some(Path::new("/tmp/home"))).unwrap(),
            PathBuf::from("/tmp/state/tui-launcher/runtime.jsonl")
        );
    }

    #[test]
    fn default_log_path_falls_back_to_home_state_directory() {
        assert_eq!(
            default_log_path(None, Some(Path::new("/tmp/home"))).unwrap(),
            PathBuf::from("/tmp/home/.local/state/tui-launcher/runtime.jsonl")
        );
    }

    #[test]
    fn records_log_items_as_jsonl() {
        let root = temporary_root("record");
        let path = root.join("runtime.jsonl");
        let mut log = RuntimeLog::open_path(path.clone());

        let record = log.record(LogLevel::Error, Some("apps:default"), None, "items failed");
        let contents = fs::read_to_string(path).unwrap();
        let item: serde_json::Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(item["label"], record.label);
        assert_eq!(item["metadata"]["level"], "error");
        assert_eq!(item["metadata"]["view"], "apps:default");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retains_one_append_file_for_multiple_records() {
        let root = temporary_root("multiple");
        let path = root.join("runtime.jsonl");
        let mut log = RuntimeLog::open_path(path.clone());

        let first = log.record(LogLevel::Info, None, None, "first");
        let second = log.record(LogLevel::Info, None, None, "second");
        let contents = fs::read_to_string(path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(first.value.ends_with("-0"));
        assert!(second.value.ends_with("-1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disabled_log_does_not_create_a_file() {
        let root = temporary_root("disabled");
        let path = root.join("runtime.jsonl");
        let mut log = RuntimeLog::disabled();
        log.record(LogLevel::Info, None, None, "not written");
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_failure_degrades_and_reports_once() {
        let root = temporary_root("startup-failure");
        let path = root.join("runtime.jsonl");
        fs::create_dir(&path).unwrap();
        let mut log = RuntimeLog::open_path(path);

        assert!(
            log.take_warning_record()
                .unwrap()
                .message
                .contains("runtime log disabled")
        );
        assert!(log.take_warning_record().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn special_file_failure_degrades_and_reports_once() {
        let mut log = RuntimeLog::open_path(PathBuf::from("/dev/full"));
        assert!(
            log.take_warning_record()
                .unwrap()
                .message
                .contains("runtime log disabled")
        );
        log.record(LogLevel::Info, None, None, "not written");
        assert!(log.take_warning_record().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fifo_log_path_is_rejected_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let root = temporary_root("fifo");
        let path = root.join("runtime.fifo");
        let path_c = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);

        let mut log = RuntimeLog::open_path(path);
        assert!(
            log.take_warning_record()
                .unwrap()
                .message
                .contains("runtime log disabled")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn message_is_sanitized_and_bounded() {
        let record = LogRecord::new(LogLevel::Info, None, None, &"x".repeat(20_000), 0);
        assert!(record.message.chars().count() <= MAX_LOG_MESSAGE_CHARS + 3);
        assert!(record.message.ends_with("..."));
    }
}
