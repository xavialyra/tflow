use crate::cancellation::CancellationToken;
use crate::command_runner::run_bounded_command_with_stdin;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command as ProcessCommand, Output};
use std::time::Duration;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_SCRIPT_STDOUT: usize = 1024 * 1024;
pub(crate) const MAX_CONFIGURABLE_SCRIPT_STDOUT: usize = 64 * 1024 * 1024;
const MAX_SCRIPT_STDERR: usize = 64 * 1024;
const MAX_SCRIPT_ARGS: usize = 64 * 1024;

/// Run a plugin-relative script with argv arguments and return its raw output.
///
/// The launcher owns transport concerns here—path confinement, cancellation,
/// timeouts, and I/O limits—but deliberately does not interpret stdout or
/// exit status. The consuming engine decides what the script's result means.
pub(crate) fn run_script(
    root: &Path,
    target: &str,
    args: &[String],
    max_output_bytes: Option<usize>,
    cancellation: &CancellationToken,
) -> Result<Output> {
    if target.is_empty() {
        bail!("script source requires a non-empty file")
    }
    validate_max_output_bytes(max_output_bytes)?;
    let max_output_bytes = max_output_bytes.unwrap_or(DEFAULT_MAX_SCRIPT_STDOUT);
    let path = resolve_script_path(root, target)?;
    let mut process = ProcessCommand::new("sh");
    process.arg(&path).args(args).current_dir(root);
    run_bounded_command_with_stdin(
        process,
        None,
        SCRIPT_TIMEOUT,
        max_output_bytes,
        MAX_SCRIPT_STDERR,
        cancellation,
    )
    .with_context(|| format!("could not run script {}", path.display()))
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

pub(crate) fn resolve_argv(value: Option<&Value>, label: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .with_context(|| format!("{label} must evaluate to an array"))?;
    let mut total_bytes: usize = 0;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let argument = match value {
                Value::String(value) => value.clone(),
                value => serde_json::to_string(value)
                    .with_context(|| format!("{label}[{index}] could not be JSON-encoded"))?,
            };
            anyhow::ensure!(
                !argument.contains('\0'),
                "{label}[{index}] cannot contain a NUL byte"
            );
            total_bytes = total_bytes
                .checked_add(argument.len().saturating_add(1))
                .context("script argument byte budget overflow")?;
            anyhow::ensure!(
                total_bytes <= MAX_SCRIPT_ARGS,
                "{label} exceeded {} bytes",
                MAX_SCRIPT_ARGS
            );
            Ok(argument)
        })
        .collect()
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
    resolve_script_path(root, target).map(|_| ())
}

pub(crate) fn read_script(root: &Path, target: &str) -> Result<String> {
    let path = resolve_script_path(root, target)?;
    fs::read_to_string(&path).with_context(|| format!("could not read script {}", path.display()))
}

pub(crate) fn resolve_script_path(root: &Path, target: &str) -> Result<PathBuf> {
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
    if !fs::metadata(&canonical_path)
        .with_context(|| format!("could not inspect script {}", canonical_path.display()))?
        .is_file()
    {
        bail!("script path {:?} is not a regular file", target);
    }
    Ok(canonical_path)
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
            "tui-launcher-script-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_script(root: &Path, name: &str, body: &str) {
        fs::write(root.join(name), body).unwrap();
    }

    fn run(root: &Path, name: &str) -> Result<Output> {
        run_script(root, name, &[], None, &CancellationToken::new())
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
    fn argv_scripts_receive_exact_values_and_json_encode_structures() {
        let root = test_root();
        write_script(
            &root,
            "args.sh",
            "[ \"$#\" -eq 2 ] || exit 10\n[ \"$1\" = \"two words\" ] || exit 11\n[ \"$2\" = '{\"x\":1}' ] || exit 12\nprintf '%s' '[{\"label\":\"ok\"}]'\n",
        );
        let args = resolve_argv(
            Some(&serde_json::json!(["two words", {"x": 1}])),
            "script args",
        )
        .unwrap();
        let output = run_script(&root, "args.sh", &args, None, &CancellationToken::new()).unwrap();
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
    fn argv_and_stdout_limits_are_enforced() {
        let root = test_root();
        let error = resolve_argv(
            Some(&serde_json::json!(["x".repeat(MAX_SCRIPT_ARGS)])),
            "script args",
        )
        .unwrap_err();
        assert!(error.to_string().contains("script args exceeded"));

        write_script(&root, "large.sh", "printf '1234'\n");
        let error =
            run_script(&root, "large.sh", &[], Some(3), &CancellationToken::new()).unwrap_err();
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

    #[cfg(unix)]
    #[test]
    fn symlinks_must_resolve_inside_the_root() {
        use std::os::unix::fs::symlink;

        let root = test_root();
        let outside = root.parent().unwrap().join(format!(
            "tui-launcher-script-outside-{}",
            std::process::id()
        ));
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
