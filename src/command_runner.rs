use crate::cancellation::CancellationToken;
use anyhow::{Context, Result, anyhow};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    unsafe {
        process.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    process
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process.spawn().context("could not spawn bounded command")?;
    let stdout_reader = child
        .stdout
        .take()
        .context("bounded command has no stdout pipe")?;
    let stderr_reader = child
        .stderr
        .take()
        .context("bounded command has no stderr pipe")?;
    if let Some(input) = stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .context("bounded command has no stdin pipe")?;
        child_stdin
            .write_all(input)
            .context("could not write bounded command input")?;
    }
    let stdout_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_thread = spawn_limited_reader(stdout_reader, stdout_limit, &stdout_exceeded);
    let stderr_thread = spawn_limited_reader(stderr_reader, stderr_limit, &stderr_exceeded);
    let deadline = Instant::now() + timeout;

    let process_result: Result<std::process::ExitStatus> = loop {
        if cancellation.is_cancelled() {
            terminate_child(&mut child);
            break Err(anyhow!("bounded command cancelled"));
        }
        if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
            terminate_child(&mut child);
            break Err(anyhow!("bounded command output exceeded configured limits"));
        }

        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child);
                break Err(anyhow!(error).context("could not inspect bounded command"));
            }
        }

        if Instant::now() >= deadline {
            terminate_child(&mut child);
            break Err(anyhow!("bounded command timed out after {:?}", timeout));
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    };

    let stdout = join_reader(stdout_thread, "stdout")?;
    let stderr = join_reader(stderr_thread, "stderr")?;
    if stdout_exceeded.load(Ordering::Relaxed) || stderr_exceeded.load(Ordering::Relaxed) {
        return Err(anyhow!("bounded command output exceeded configured limits"));
    }
    let status = process_result?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn spawn_limited_reader<R>(
    reader: R,
    limit: usize,
    exceeded: &Arc<AtomicBool>,
) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    let exceeded = Arc::clone(exceeded);
    thread::spawn(move || read_limited(reader, limit, &exceeded))
}

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

fn join_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>, stream: &str) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow!("bounded command {} reader panicked", stream))?
        .with_context(|| format!("could not read bounded command {}", stream))
}

fn terminate_child(child: &mut Child) {
    let process_group = -(child.id() as libc::pid_t);
    if unsafe { libc::kill(process_group, libc::SIGKILL) } != 0 {
        let _ = child.kill();
    }
    let _ = child.wait();
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
