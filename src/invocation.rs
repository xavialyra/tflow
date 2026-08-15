use crate::cancellation::CancellationToken;
use crate::command_runner::run_bounded_command_with_stdin;
use crate::config::{Config, ConfigReadContext, ConfigScope};
use crate::engine::{SessionOutcome, ViewOutput};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static INPUT_COUNTER: AtomicU64 = AtomicU64::new(0);
const RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const RESULT_STDOUT_LIMIT: usize = 16 * 1024 * 1024;
const RESULT_STDERR_LIMIT: usize = 64 * 1024;

pub(crate) struct InputArtifact {
    path: Option<PathBuf>,
    length: u64,
    is_tty: bool,
}

impl InputArtifact {
    pub(crate) fn capture() -> Result<Self> {
        let is_tty = unsafe { libc::isatty(io::stdin().as_raw_fd()) } == 1;
        if is_tty {
            return Ok(Self {
                path: None,
                length: 0,
                is_tty: true,
            });
        }

        let (path, mut file) = create_input_file()?;
        let mut artifact = Self {
            path: Some(path),
            length: 0,
            is_tty: false,
        };
        artifact.length = io::copy(&mut io::stdin().lock(), &mut file)
            .context("could not capture invocation stdin")?;
        file.flush()
            .context("could not flush captured invocation stdin")?;
        if artifact
            .path
            .as_ref()
            .is_some_and(|path| path.to_str().is_none())
        {
            bail!("captured invocation stdin path is not valid UTF-8");
        }
        Ok(artifact)
    }

    pub(crate) fn is_tty(&self) -> bool {
        self.is_tty
    }

    pub(crate) fn value(&self) -> Value {
        serde_json::json!({
            "stdin": {
                "path": self.path.as_ref().map(|path| {
                    path.to_str()
                        .expect("captured input path was validated as UTF-8")
                }),
                "length": self.length,
                "is_tty": self.is_tty,
            }
        })
    }
}

pub(crate) struct InvocationResult {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: i32,
}

pub(crate) fn finish(
    config: &Config,
    root_view: &str,
    outcome: SessionOutcome,
) -> Result<InvocationResult> {
    let SessionOutcome::Completed(returned) = outcome else {
        let exit_code = config
            .view(root_view)
            .with_context(|| format!("invocation root view {:?} is not configured", root_view))?
            .cancel_exit_code
            .unwrap_or(0)
            .into();
        return Ok(InvocationResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code,
        });
    };
    let returned = *returned;
    let Some(adapter) = returned.adapter.as_ref() else {
        return Ok(default_result(returned.output));
    };
    let command = config
        .view(&adapter.command.view)
        .and_then(|view| view.commands.get(&adapter.command.id))
        .with_context(|| {
            format!(
                "return command {:?} is not configured for view {:?}",
                adapter.command.id, adapter.command.view
            )
        })?;
    let crate::config::CommandAction::Return { payload } = &command.action else {
        anyhow::bail!(
            "command {:?} for view {:?} is no longer a return command",
            adapter.command.id,
            adapter.command.view
        );
    };
    let Some(handler_config) = payload.handler.as_deref() else {
        return Ok(default_result(returned.output));
    };
    let plugin_root = config
        .plugin_root(&adapter.command.view)
        .unwrap_or_else(|| Path::new("."));
    let handler = resolve_handler(plugin_root, handler_config)?;
    let owner = if adapter.context.page.view_ref == adapter.command.view {
        &adapter.context.page
    } else {
        &adapter
            .context
            .selection
            .as_ref()
            .filter(|selection| selection.owner.view_ref == adapter.command.view)
            .context("return command owner is not available in its adapter context")?
            .owner
    };
    let returned_value = crate::engine::command::return_value(&returned);
    let params = config.evaluate_value(
        ConfigReadContext {
            scope: ConfigScope::View(&owner.state),
            runtime: &adapter.context.runtime,
            input: &config.input_value,
            cancellation: None,
            binding_raw: Some(&owner.binding_raw),
        },
        &toml::Value::Table(payload.params.clone()),
        adapter.context.request.as_ref(),
        Some(&returned_value),
    )?;
    let input = serde_json::to_vec(&params).context("could not serialize return params")?;
    let mut command = Command::new("sh");
    command.arg(&handler).current_dir(plugin_root);
    let output = run_bounded_command_with_stdin(
        command,
        Some(&input),
        RESULT_TIMEOUT,
        RESULT_STDOUT_LIMIT,
        RESULT_STDERR_LIMIT,
        &CancellationToken::new(),
    )
    .with_context(|| format!("could not run result handler {}", handler.display()))?;
    Ok(InvocationResult {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: output.status.code().unwrap_or(1),
    })
}

impl Drop for InputArtifact {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn default_result(output: ViewOutput) -> InvocationResult {
    let mut stdout = match output {
        ViewOutput::Selected { item, input } => item
            .and_then(|item| item.value)
            .unwrap_or(input)
            .into_bytes(),
        ViewOutput::Value {
            value: Value::String(value),
        } => value.into_bytes(),
        ViewOutput::Value { value } => {
            serde_json::to_vec(&value).expect("serde_json::Value serialization cannot fail")
        }
    };
    stdout.push(b'\n');
    InvocationResult {
        stdout,
        stderr: Vec::new(),
        exit_code: 0,
    }
}

fn resolve_handler(root: &Path, target: &str) -> Result<PathBuf> {
    let relative = Path::new(target);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "result handler path {:?} must stay below {}",
            target,
            root.display()
        );
    }
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("could not resolve plugin root {}", root.display()))?;
    let path = root.join(relative);
    let canonical_path = std::fs::canonicalize(&path)
        .with_context(|| format!("could not read result handler {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!(
            "result handler path {:?} escapes {}",
            target,
            root.display()
        );
    }
    Ok(canonical_path)
}

fn create_input_file() -> Result<(PathBuf, File)> {
    let directory = std::env::temp_dir();
    for _ in 0..100 {
        let sequence = INPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let random = random_u64().unwrap_or_else(|_| {
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            elapsed.as_nanos() as u64 ^ sequence.rotate_left(17)
        });
        let path = directory.join(format!(
            "tui-launcher-input-{}-{random:016x}",
            std::process::id(),
        ));
        match open_private_file(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not create invocation input {}", path.display())
                });
            }
        }
    }
    bail!("could not allocate a unique invocation input file")
}

fn random_u64() -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(u64::from_ne_bytes(bytes))
}

fn open_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}
