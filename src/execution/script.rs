use crate::execution::{BoundedCommandOutcome, run_bounded_command_with_stdin_outcome};
use crate::lifecycle::CancellationStatus;
#[cfg(test)]
use crate::lifecycle::CancellationToken;
use anyhow::{Context, Result, bail};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command as ProcessCommand, Output};
use std::time::Duration;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_SCRIPT_STDOUT: usize = 1024 * 1024;
pub(crate) const MAX_CONFIGURABLE_SCRIPT_STDOUT: usize = 64 * 1024 * 1024;
const MAX_SCRIPT_STDERR: usize = 64 * 1024;
const MAX_SCRIPT_SOURCE_BYTES: usize = 1024 * 1024;

/// Execute a resolved script with an optional protocol request on stdin.
#[cfg(test)]
pub(crate) fn run_resolved_script_with_stdin_outcome(
    workflow_id: &str,
    source_label: &str,
    root: Option<&Path>,
    source: &crate::workflow::config::ResolvedScriptSource,
    args: &[String],
    stdin: Option<&[u8]>,
    cancellation: &dyn CancellationStatus,
) -> BoundedCommandOutcome {
    run_resolved_script_with_stdin_outcome_with_limit(
        workflow_id,
        source_label,
        root,
        source,
        args,
        stdin,
        None,
        cancellation,
    )
}

/// Execute a resolved script with an optional protocol request on stdin and
/// an entry-point-specific stdout limit override.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_resolved_script_with_stdin_outcome_with_limit(
    workflow_id: &str,
    source_label: &str,
    root: Option<&Path>,
    source: &crate::workflow::config::ResolvedScriptSource,
    args: &[String],
    stdin: Option<&[u8]>,
    max_output_override: Option<usize>,
    cancellation: &dyn CancellationStatus,
) -> BoundedCommandOutcome {
    let mut managed_child_reaped = false;
    let result = (|| -> Result<Output> {
        let max_output_bytes = max_output_override.unwrap_or(DEFAULT_MAX_SCRIPT_STDOUT);
        validate_max_output_bytes(Some(max_output_bytes))?;
        match &source.target {
            crate::workflow::config::ResolvedScriptTarget::Inline(script_body) => {
                let argv = crate::execution::prepare_inline_script_command(
                    workflow_id,
                    source_label,
                    script_body,
                    args,
                )?;
                let mut process = ProcessCommand::new(&argv[0]);
                process.args(&argv[1..]);
                if let Some(root) = root {
                    process.env("WORKFLOW_DIR", root);
                }
                let outcome = run_bounded_command_with_stdin_outcome(
                    process,
                    stdin,
                    SCRIPT_TIMEOUT,
                    max_output_bytes,
                    MAX_SCRIPT_STDERR,
                    cancellation,
                );
                managed_child_reaped = outcome.managed_child_reaped();
                outcome
                    .into_result()
                    .with_context(|| format!("could not run inline script for {}", source_label))
            }
            crate::workflow::config::ResolvedScriptTarget::File(target) => {
                if target.is_empty() {
                    bail!("script source requires a non-empty file");
                }
                let target_path = Path::new(target);
                let (display_path, script_path, _file, shebang, interpreter) = if target_path
                    .is_absolute()
                {
                    let content = fs::read_to_string(target_path).with_context(|| {
                        format!("could not read script {}", target_path.display())
                    })?;
                    let shebang = crate::execution::parse_shebang(&content);
                    let interpreter = crate::execution::verify_interpreter(&shebang.interpreter)?;
                    (target.clone(), target.clone(), None, shebang, interpreter)
                } else {
                    let root = root.with_context(|| {
                        format!(
                            "single-file workflow cannot reference relative script file {:?}",
                            target
                        )
                    })?;
                    let content = read_script(root, target)?;
                    let shebang = crate::execution::parse_shebang(&content);
                    let interpreter = crate::execution::verify_interpreter(&shebang.interpreter)?;
                    let (path, file) = open_confined_script(root, target)?;
                    #[cfg(target_os = "linux")]
                    {
                        clear_close_on_exec(file.as_raw_fd())?;
                        let fd_path = format!("/proc/self/fd/{}", file.as_raw_fd());
                        (
                            path.display().to_string(),
                            fd_path,
                            Some(file),
                            shebang,
                            interpreter,
                        )
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        (
                            path.display().to_string(),
                            path.to_string_lossy().into_owned(),
                            Some(file),
                            shebang,
                            interpreter,
                        )
                    }
                };
                let mut process = ProcessCommand::new(interpreter);
                process.args(&shebang.args);
                process.arg(script_path).args(args);
                if let Some(root) = root {
                    process.env("WORKFLOW_DIR", root);
                }
                let outcome = run_bounded_command_with_stdin_outcome(
                    process,
                    stdin,
                    SCRIPT_TIMEOUT,
                    max_output_bytes,
                    MAX_SCRIPT_STDERR,
                    cancellation,
                );
                managed_child_reaped = outcome.managed_child_reaped();
                outcome
                    .into_result()
                    .with_context(|| format!("could not run script {}", display_path))
            }
        }
    })();
    BoundedCommandOutcome::from_result_and_reap(result, managed_child_reaped)
}

/// Preserve the common diagnostic for a script that did not complete
/// successfully while leaving successful stdout available to the caller.
pub(crate) fn ensure_script_success(output: &Output) -> Result<()> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("script exited with status {}", output.status);
        }
        bail!("script failed: {}", stderr);
    }
    Ok(())
}

pub(crate) fn validate_max_output_bytes(max_output_bytes: Option<usize>) -> Result<()> {
    if max_output_bytes.is_some_and(|value| value == 0 || value > MAX_CONFIGURABLE_SCRIPT_STDOUT) {
        bail!(
            "script max_output_bytes must be between 1 and {}",
            MAX_CONFIGURABLE_SCRIPT_STDOUT
        );
    }
    Ok(())
}

pub(crate) fn validate_script_target(root: &Path, target: &str) -> Result<()> {
    open_confined_script(root, target).map(|_| ())
}

pub(crate) fn read_script(root: &Path, target: &str) -> Result<String> {
    let (path, bytes) = read_script_bytes(root, target)?;
    String::from_utf8(bytes)
        .with_context(|| format!("script {} is not valid UTF-8", path.display()))
}

fn read_script_bytes(root: &Path, target: &str) -> Result<(PathBuf, Vec<u8>)> {
    let (path, file) = open_confined_script(root, target)?;
    let read_limit = u64::try_from(MAX_SCRIPT_SOURCE_BYTES)
        .context("script source size limit does not fit in u64")?
        .checked_add(1)
        .context("script source size limit overflow")?;
    let mut bytes = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read script {}", path.display()))?;
    if bytes.len() > MAX_SCRIPT_SOURCE_BYTES {
        bail!(
            "script source exceeded maximum size of {MAX_SCRIPT_SOURCE_BYTES} bytes: {}",
            path.display()
        );
    }
    Ok((path, bytes))
}

fn open_confined_script(root: &Path, target: &str) -> Result<(PathBuf, File)> {
    let relative = Path::new(target);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "script path {:?} must stay below {}",
            target,
            root.display()
        );
    }
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("could not resolve script root {}", root.display()))?;
    let path = root.join(relative);
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("could not read script {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!("script path {:?} escapes {}", target, root.display());
    }
    let file = open_script_file(&canonical_path)
        .with_context(|| format!("could not read script {}", canonical_path.display()))?;
    if !file
        .metadata()
        .with_context(|| format!("could not inspect script {}", canonical_path.display()))?
        .is_file()
    {
        bail!("script path {:?} is not a regular file", target);
    }
    #[cfg(target_os = "linux")]
    {
        let opened_path = fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .with_context(|| {
                format!(
                    "could not inspect opened script {}",
                    canonical_path.display()
                )
            })?;
        if !opened_path.starts_with(&canonical_root) {
            bail!("script path {:?} escapes {}", target, root.display());
        }
    }
    Ok((canonical_path, file))
}

#[cfg(target_os = "linux")]
fn clear_close_on_exec(fd: std::os::fd::RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("could not inspect script file descriptor");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("could not prepare script file descriptor");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_script_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(target_os = "linux"))]
fn open_script_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_root() -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tlaunch-script-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_script(root: &Path, name: &str, body: &str) {
        fs::write(root.join(name), body).unwrap();
    }

    fn run(root: &Path, name: &str) -> Result<Output> {
        let source = crate::workflow::config::ResolvedScriptSource {
            target: crate::workflow::config::ResolvedScriptTarget::File(name.to_string()),
        };
        run_resolved_script_with_stdin_outcome(
            "test",
            name,
            Some(root),
            &source,
            &[],
            None,
            &CancellationToken::new(),
        )
        .into_result()
    }

    #[test]
    fn successful_script_returns_raw_output() {
        let root = test_root();
        write_script(&root, "ok.sh", "printf '%s' '{\"ok\":true}'\n");
        let output = run(&root, "ok.sh").unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, br#"{"ok":true}"#);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolved_scripts_receive_literal_arguments() {
        let root = test_root();
        write_script(
            &root,
            "args.sh",
            "[ \"$#\" -eq 2 ] || exit 10\n[ \"$1\" = \"two words\" ] || exit 11\n[ \"$2\" = '{\"x\":1}' ] || exit 12\nprintf '%s' '[{\"label\":\"ok\"}]'\n",
        );
        let args = vec!["two words".to_string(), r#"{"x":1}"#.to_string()];
        let source = crate::workflow::config::ResolvedScriptSource {
            target: crate::workflow::config::ResolvedScriptTarget::File("args.sh".to_string()),
        };
        let output = run_resolved_script_with_stdin_outcome(
            "test",
            "args.sh",
            Some(&root),
            &source,
            &args,
            None,
            &CancellationToken::new(),
        )
        .into_result()
        .unwrap();
        ensure_script_success(&output).unwrap();
        assert_eq!(output.stdout, br#"[{"label":"ok"}]"#);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nonzero_exit_includes_stderr_and_handles_empty_stderr() {
        let root = test_root();
        write_script(&root, "stderr.sh", "printf 'bad news\\n' >&2; exit 7\n");
        let error = ensure_script_success(&run(&root, "stderr.sh").unwrap()).unwrap_err();
        assert!(error.to_string().contains("script failed: bad news"));

        write_script(&root, "silent.sh", "exit 9\n");
        let error = ensure_script_success(&run(&root, "silent.sh").unwrap()).unwrap_err();
        assert!(error.to_string().contains("script exited with status"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn raw_stdout_is_returned_without_protocol_parsing() {
        let root = test_root();
        write_script(&root, "empty.sh", ":\n");
        assert!(run(&root, "empty.sh").unwrap().stdout.is_empty());

        write_script(&root, "invalid.sh", "printf '%s' 'not-json'\n");
        assert_eq!(run(&root, "invalid.sh").unwrap().stdout, b"not-json");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stdout_limits_are_enforced_for_resolved_scripts() {
        let root = test_root();
        write_script(&root, "large.sh", "printf '1234'\n");
        let source = crate::workflow::config::ResolvedScriptSource {
            target: crate::workflow::config::ResolvedScriptTarget::File("large.sh".to_string()),
        };
        let error = run_resolved_script_with_stdin_outcome_with_limit(
            "test",
            "large.sh",
            Some(&root),
            &source,
            &[],
            None,
            Some(3),
            &CancellationToken::new(),
        )
        .into_result()
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("output exceeded") || message.contains("stdout limit"),
            "unexpected output limit error: {message}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configurable_output_limit_has_a_valid_range() {
        assert!(validate_max_output_bytes(Some(0)).is_err());
        assert!(validate_max_output_bytes(Some(MAX_CONFIGURABLE_SCRIPT_STDOUT + 1)).is_err());
        assert!(validate_max_output_bytes(Some(1)).is_ok());
    }

    #[test]
    fn script_source_size_is_bounded_before_returning_contents() {
        let root = test_root();
        let exact = "x".repeat(MAX_SCRIPT_SOURCE_BYTES);
        write_script(&root, "exact-handler.sh", &exact);
        assert_eq!(read_script(&root, "exact-handler.sh").unwrap(), exact);

        write_script(
            &root,
            "large-handler.sh",
            &"x".repeat(MAX_SCRIPT_SOURCE_BYTES + 1),
        );
        let error = read_script(&root, "large-handler.sh").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("script source exceeded maximum size")
        );

        fs::write(root.join("invalid-handler.sh"), [0xff]).unwrap();
        let error = read_script(&root, "invalid-handler.sh").unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_invalid_script_reports_the_size_limit_first() {
        let root = test_root();
        let mut bytes = vec![b'x'; MAX_SCRIPT_SOURCE_BYTES + 1];
        bytes[MAX_SCRIPT_SOURCE_BYTES] = 0xff;
        fs::write(root.join("oversized-invalid.sh"), bytes).unwrap();
        let error = read_script(&root, "oversized-invalid.sh").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("script source exceeded maximum size")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn paths_stay_below_the_root_and_must_be_files() {
        let root = test_root();
        write_script(&root, "ok.sh", "printf 'null'\n");
        assert!(validate_script_target(&root, "ok.sh").is_ok());
        assert!(validate_script_target(&root, "../outside.sh").is_err());
        assert!(validate_script_target(&root, "/tmp/outside.sh").is_err());
        fs::create_dir(root.join("directory")).unwrap();
        assert!(validate_script_target(&root, "directory").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn special_files_are_rejected_without_blocking_open() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let root = test_root();
        let fifo = root.join("pipe.sh");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        let result = unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o644) };
        assert_eq!(result, 0);
        assert!(validate_script_target(&root, "pipe.sh").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_must_resolve_inside_the_root() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let outside = root
            .parent()
            .unwrap()
            .join(format!("tlaunch-script-outside-{}", std::process::id()));
        fs::write(&outside, "printf 'null'\n").unwrap();
        symlink(&outside, root.join("outside.sh")).unwrap();
        assert!(validate_script_target(&root, "outside.sh").is_err());

        write_script(&root, "inside.sh", "printf 'null'\n");
        symlink(root.join("inside.sh"), root.join("alias.sh")).unwrap();
        let output = run(&root, "alias.sh").unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"null");

        fs::remove_file(&outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
