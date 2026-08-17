use crate::cancellation::CancellationToken;
use crate::engine::{ProcessGroupGuard, clear_managed_environment};
use anyhow::{Context, Result, anyhow};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) fn run_bounded_command_with_stdin(
    mut process: Command,
    stdin: Option<&[u8]>,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
    cancellation: &CancellationToken,
) -> Result<std::process::Output> {
    clear_managed_environment(&mut process);
    process
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process_group =
        ProcessGroupGuard::spawn(process).context("could not spawn bounded command")?;
    let mut stdout_reader = process_group
        .child_mut()
        .stdout
        .take()
        .context("bounded command has no stdout pipe")?;
    let mut stderr_reader = process_group
        .child_mut()
        .stderr
        .take()
        .context("bounded command has no stderr pipe")?;
    let mut stdin_writer = process_group.child_mut().stdin.take();
    for fd in [
        stdout_reader.as_raw_fd(),
        stderr_reader.as_raw_fd(),
        stdin_writer.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1),
    ] {
        if fd >= 0
            && let Err(error) = set_nonblocking(fd)
        {
            process_group.force_kill();
            return Err(error);
        }
    }

    let input = stdin.unwrap_or_default();
    let mut input_offset = 0;
    let mut stdout = Vec::with_capacity(stdout_limit.min(8192));
    let mut stderr = Vec::with_capacity(stderr_limit.min(8192));
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut status = None;
    let deadline = Instant::now() + timeout;

    loop {
        if cancellation.is_cancelled() {
            process_group.force_kill();
            return Err(anyhow!("bounded command cancelled"));
        }
        if Instant::now() >= deadline {
            process_group.force_kill();
            return Err(anyhow!("bounded command timed out after {:?}", timeout));
        }

        stdout_eof |= match drain_pipe(&mut stdout_reader, &mut stdout, stdout_limit, "stdout") {
            Ok(eof) => eof,
            Err(error) => {
                process_group.force_kill();
                return Err(error);
            }
        };
        stderr_eof |= match drain_pipe(&mut stderr_reader, &mut stderr, stderr_limit, "stderr") {
            Ok(eof) => eof,
            Err(error) => {
                process_group.force_kill();
                return Err(error);
            }
        };

        if let Some(writer) = stdin_writer.as_mut() {
            match writer.write(&input[input_offset..]) {
                Ok(0) => {}
                Ok(count) => input_offset += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    process_group.force_kill();
                    return Err(error).context("could not write bounded command input");
                }
            }
            if input_offset == input.len() {
                stdin_writer = None;
            }
        }

        if status.is_none() {
            match process_group.try_wait() {
                Ok(Some(exit_status)) => {
                    status = Some(exit_status);
                    process_group.cleanup_group();
                }
                Ok(None) => {}
                Err(error) => {
                    process_group.force_kill();
                    return Err(anyhow!(error).context("could not inspect bounded command"));
                }
            }
        }
        if let Some(status) = status
            && stdout_eof
            && stderr_eof
            && stdin_writer.is_none()
        {
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

fn set_nonblocking(fd: libc::c_int) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect bounded command pipe");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error()).context("could not configure bounded command pipe");
    }
    Ok(())
}

fn drain_pipe(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    limit: usize,
    stream: &str,
) -> Result<bool> {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                let remaining = limit.saturating_sub(output.len());
                output.extend_from_slice(&buffer[..count.min(remaining)]);
                if count > remaining {
                    return Err(anyhow!(
                        "bounded command output exceeded configured limits ({} limit: {} bytes)",
                        stream,
                        limit
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read bounded command {}", stream));
            }
        }
    }
}

#[cfg(test)]
fn read_limited(mut reader: impl Read, limit: usize, exceeded: &AtomicBool) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_keeps_only_the_configured_prefix() {
        let exceeded = AtomicBool::new(false);
        let output = read_limited(&b"abcdef"[..], 3, &exceeded).unwrap();
        assert_eq!(output, b"abc");
        assert!(exceeded.load(Ordering::Relaxed));
    }

    #[test]
    fn reaped_leader_does_not_leave_pipe_holding_descendants() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & exit 0"]);
        let started = Instant::now();
        let output = run_bounded_command_with_stdin(
            command,
            None,
            Duration::from_secs(1),
            1024,
            1024,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detached_pipe_holding_descendant_is_still_bounded_by_the_deadline() {
        let mut command = Command::new("sh");
        command.args(["-c", "setsid sh -c 'sleep 1' & sleep 0.05; exit 0"]);
        let started = Instant::now();
        let error = run_bounded_command_with_stdin(
            command,
            None,
            Duration::from_millis(100),
            1024,
            1024,
            &CancellationToken::new(),
        )
        .expect_err("detached pipe holder should keep the command incomplete");
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn bounded_command_does_not_inherit_launcher_log_environment() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "test -z \"${LAUNCHER_LOG_FILE+x}\" && test -z \"${TUI_LAUNCHER_LOG_FILE+x}\"",
        ]);
        command.env("LAUNCHER_LOG_FILE", "/tmp/should-not-be-visible");
        command.env("TUI_LAUNCHER_LOG_FILE", "/tmp/should-not-be-visible-either");
        let output = run_bounded_command_with_stdin(
            command,
            None,
            Duration::from_secs(1),
            1024,
            1024,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn command_timeout_terminates_the_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 1"]);
        let error = run_bounded_command_with_stdin(
            command,
            None,
            Duration::from_millis(50),
            1024,
            1024,
            &CancellationToken::new(),
        )
        .expect_err("the command should time out");
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn timeout_applies_while_a_child_is_not_reading_stdin() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10"]);
        let input = vec![b'x'; 1024 * 1024];
        let started = Instant::now();

        let error = run_bounded_command_with_stdin(
            command,
            Some(&input),
            Duration::from_millis(50),
            1024,
            1024,
            &CancellationToken::new(),
        )
        .expect_err("the command should time out while stdin is blocked");

        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_terminates_the_process_group() {
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 10"]);
            run_bounded_command_with_stdin(
                command,
                None,
                Duration::from_secs(10),
                1024,
                1024,
                &worker_token,
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        token.cancel();
        let error = handle
            .join()
            .expect("bounded command thread should not panic")
            .expect_err("the command should be cancelled");
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn command_output_limit_returns_an_error() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 123456"]);
        let error = run_bounded_command_with_stdin(
            command,
            None,
            Duration::from_secs(1),
            3,
            1024,
            &CancellationToken::new(),
        )
        .expect_err("the command should exceed its output limit");
        assert!(error.to_string().contains("output exceeded"));
    }
}
