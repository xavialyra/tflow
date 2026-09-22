use super::SessionOutcome;
use crate::identity::PRODUCT;
use crate::lifecycle::CancellationToken;
use crate::workflow::config::CompiledConfig;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static INPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct InputArtifact {
    path: Option<PathBuf>,
    length: u64,
    is_tty: bool,
}

impl InputArtifact {
    pub(crate) fn empty() -> Self {
        Self {
            path: None,
            length: 0,
            is_tty: false,
        }
    }

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
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    outcome: SessionOutcome,
    _cancellation: &CancellationToken,
) -> Result<InvocationResult> {
    let SessionOutcome::Completed(returned) = outcome else {
        let exit_code = config
            .view(invocation.root_view())
            .with_context(|| {
                format!(
                    "invocation root view {:?} is not configured",
                    invocation.root_view()
                )
            })?
            .cancel_exit_code
            .unwrap_or(0)
            .into();
        return Ok(InvocationResult {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code,
        });
    };
    Ok(default_result(returned))
}

impl Drop for InputArtifact {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn default_result(value: Value) -> InvocationResult {
    let mut stdout = match value {
        Value::String(value) => value.into_bytes(),
        value => serde_json::to_vec(&value).expect("serde_json::Value serialization cannot fail"),
    };
    stdout.push(b'\n');
    InvocationResult {
        stdout,
        stderr: Vec::new(),
        exit_code: 0,
    }
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
            "{PRODUCT}-input-{}-{random:016x}",
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

#[cfg(test)]
mod tests {
    use super::default_result;
    use serde_json::json;

    #[test]
    fn root_result_serializes_strings_as_text_and_other_values_as_json() {
        assert_eq!(default_result(json!("text")).stdout, b"text\n");
        assert_eq!(
            default_result(json!({"value": 7})).stdout,
            b"{\"value\":7}\n"
        );
        assert_eq!(default_result(json!(null)).stdout, b"null\n");
    }
}
