use crate::lifecycle::CancellationStatus;
#[cfg(test)]
use crate::lifecycle::CancellationToken;
use crate::terminal::{Terminal, set_terminal_foreground_process_group};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::Duration;

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct ProcessGroupGuard {
    pid: libc::pid_t,
    child: Option<Child>,
    reaped: bool,
    group_cleaned: bool,
    leader_killed: bool,
}

pub(crate) enum ProcessWait {
    Exited(ExitStatus),
    Stopped(libc::c_int),
}

#[derive(Debug)]
pub(crate) struct ForegroundTerminalReclaimError {
    source: io::Error,
}

impl ForegroundTerminalReclaimError {
    fn new(source: io::Error) -> Self {
        Self { source }
    }
}

impl std::fmt::Display for ForegroundTerminalReclaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "could not reclaim launcher terminal ownership: {}",
            self.source
        )
    }
}

impl std::error::Error for ForegroundTerminalReclaimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl ProcessGroupGuard {
    pub(crate) fn spawn(mut command: Command, terminal_fd: Option<RawFd>) -> io::Result<Self> {
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if let Some(fd) = terminal_fd {
                    set_terminal_foreground_process_group(fd, libc::getpgrp())?;
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok(Self {
            pid: child.id() as libc::pid_t,
            child: Some(child),
            reaped: false,
            group_cleaned: false,
            leader_killed: false,
        })
    }

    pub(crate) fn from_pid(pid: libc::pid_t) -> Self {
        Self {
            pid,
            child: None,
            reaped: false,
            group_cleaned: false,
            leader_killed: false,
        }
    }

    pub(crate) fn pid(&self) -> libc::pid_t {
        self.pid
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("process group guard has no Child")
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(child) = self.child.as_mut() else {
            return Err(io::Error::other("process group guard does not own a Child"));
        };
        let status = child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }

    /// Observe completion and job-control stops for the managed leader.
    ///
    /// `Child::try_wait` deliberately does not request stopped statuses. A
    /// foreground group must observe them so the host can retake the terminal
    /// instead of leaving a stopped group as terminal foreground indefinitely.
    pub(crate) fn try_wait_with_stops(&mut self) -> io::Result<Option<ProcessWait>> {
        if self.child.is_none() {
            return Err(io::Error::other("process group guard does not own a Child"));
        }
        let mut status = 0;
        let result =
            unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if result == 0 {
            return Ok(None);
        }
        if result < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(error)
            };
        }

        if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
            self.reaped = true;
            self.child.take();
            return Ok(Some(ProcessWait::Exited(ExitStatus::from_raw(status))));
        }
        if libc::WIFSTOPPED(status) {
            return Ok(Some(ProcessWait::Stopped(libc::WSTOPSIG(status))));
        }
        Err(io::Error::other(
            "waitpid returned an unsupported process status",
        ))
    }

    pub(crate) fn try_wait_raw(&mut self) -> io::Result<Option<libc::c_int>> {
        if self.child.is_some() {
            return Err(io::Error::other("process group guard owns a Child"));
        }
        let mut status = 0;
        let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if result == self.pid {
            self.reaped = true;
            Ok(Some(status))
        } else if result == 0 {
            Ok(None)
        } else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(error)
            }
        }
    }

    pub(crate) fn cleanup_group(&mut self) {
        if self.group_cleaned {
            return;
        }
        let group_result = unsafe { libc::kill(-self.pid, libc::SIGKILL) };
        if group_result == 0 {
            self.group_cleaned = true;
            return;
        }

        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) && !self.reaped && !self.leader_killed {
            let leader_result = unsafe { libc::kill(self.pid, libc::SIGKILL) };
            if leader_result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                self.leader_killed = true;
            }
        }
    }

    pub(crate) fn force_kill(&mut self) {
        self.cleanup_group();
        if let Some(child) = self.child.as_mut() {
            if !self.reaped && child.wait().is_ok() {
                self.reaped = true;
            }
        } else if !self.reaped {
            loop {
                let result = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
                if result == self.pid {
                    self.reaped = true;
                    break;
                }
                if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break;
                }
            }
        }
        if !self.group_cleaned {
            self.cleanup_group();
            if self.reaped {
                self.group_cleaned = true;
            }
        }
    }

    /// Whether this guard has positively observed its managed leader exit.
    ///
    /// Callers use this as an execution fact, so failed wait operations must
    /// remain distinguishable from a child that was actually reaped.
    pub(crate) fn reaped(&self) -> bool {
        self.reaped
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if !self.group_cleaned || !self.reaped {
            self.force_kill();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedProcess {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

impl PreparedProcess {
    fn command(&self) -> io::Result<Command> {
        let Some(program) = self.argv.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "foreground command has an empty argv",
            ));
        };
        let mut command = Command::new(program);
        command.args(&self.argv[1..]);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        Ok(command)
    }
}

/// Run a prepared foreground command and reap its managed process group.
///
/// When a usable terminal is supplied, this function hands it to the child
/// group and reclaims it before returning. It also owns command construction,
/// cancellation-aware waiting, and cleanup of the leader and its descendants.
pub(crate) fn run_foreground_process(
    prepared: &PreparedProcess,
    cancellation: &dyn CancellationStatus,
    terminal: Option<&Terminal>,
) -> io::Result<ExitStatus> {
    if cancellation.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "launcher shutdown requested",
        ));
    }

    let mut command = prepared.command()?;
    let terminal_handoff = terminal
        .map(|terminal| terminal.configure_foreground_command(&mut command))
        .transpose()?
        .unwrap_or(false);
    let terminal_fd = terminal.and_then(Terminal::foreground_terminal_fd);
    let mut process = match ProcessGroupGuard::spawn(command, terminal_fd) {
        Ok(process) => process,
        Err(error) => {
            if terminal_handoff
                && let Some(terminal) = terminal
                && let Err(reclaim_error) = terminal.reclaim_foreground_process()
            {
                return Err(io::Error::other(ForegroundTerminalReclaimError::new(
                    reclaim_error,
                )));
            }
            return Err(error);
        }
    };

    let result = loop {
        if cancellation.is_cancelled() {
            process.force_kill();
            break Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "launcher shutdown requested",
            ));
        }
        match process.try_wait_with_stops() {
            Ok(Some(ProcessWait::Exited(status))) => {
                process.cleanup_group();
                break Ok(status);
            }
            Ok(Some(ProcessWait::Stopped(signal))) => {
                break Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!(
                        "foreground command stopped by signal {signal}; launcher terminated the managed process group"
                    ),
                ));
            }
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => break Err(error),
        }
    };
    if result.is_err() {
        process.force_kill();
    }

    if terminal_handoff
        && let Some(terminal) = terminal
        && let Err(error) = terminal.reclaim_foreground_process()
    {
        return Err(io::Error::other(ForegroundTerminalReclaimError::new(error)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cancellation_kills_the_process_group() {
        let prepared = PreparedProcess {
            argv: vec!["sh".to_string(), "-c".to_string(), "sleep 30".to_string()],
            environment: Vec::new(),
            current_dir: None,
        };
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let started = std::time::Instant::now();
        let worker =
            thread::spawn(move || run_foreground_process(&prepared, &worker_cancellation, None));
        thread::sleep(Duration::from_millis(50));
        cancellation.cancel();

        let error = worker
            .join()
            .expect("process worker should not panic")
            .expect_err("cancellation should interrupt the process");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stopped_foreground_command_is_interrupted_and_reaped() {
        let prepared = PreparedProcess {
            argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "kill -STOP $$; sleep 30".to_string(),
            ],
            environment: Vec::new(),
            current_dir: None,
        };
        let started = std::time::Instant::now();

        let error = run_foreground_process(&prepared, &CancellationToken::new(), None)
            .expect_err("a stopped foreground process should interrupt execution");

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(error.to_string().contains("stopped by signal"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn normal_completion_kills_background_processes_in_the_group() {
        let pid_file = std::env::temp_dir().join(format!(
            "tui-launcher-normal-descendant-{}",
            std::process::id()
        ));
        fs::remove_file(&pid_file).ok();
        let script = format!("sleep 30 & echo $! > {}; exit 0", pid_file.display());
        let prepared = PreparedProcess {
            argv: vec!["sh".to_string(), "-c".to_string(), script],
            environment: Vec::new(),
            current_dir: None,
        };
        let status = run_foreground_process(&prepared, &CancellationToken::new(), None).unwrap();
        assert!(status.success());
        let descendant = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(descendant, 0) } == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "background process {descendant} survived normal completion"
            );
            thread::sleep(Duration::from_millis(10));
        }
        fs::remove_file(pid_file).unwrap();
    }

    #[test]
    fn cancellation_kills_a_signal_resistant_descendant() {
        let pid_file = std::env::temp_dir().join(format!(
            "tui-launcher-foreground-descendant-{}",
            std::process::id()
        ));
        fs::remove_file(&pid_file).ok();
        let script = format!(
            "trap 'exit 0' TERM; sh -c 'trap \"\" TERM; echo $$ > {}; while :; do sleep 1; done' & while :; do sleep 1; done",
            pid_file.display()
        );
        let prepared = PreparedProcess {
            argv: vec!["sh".to_string(), "-c".to_string(), script],
            environment: Vec::new(),
            current_dir: None,
        };
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker =
            thread::spawn(move || run_foreground_process(&prepared, &worker_cancellation, None));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !pid_file.is_file() {
            assert!(
                std::time::Instant::now() < deadline,
                "descendant did not start"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let descendant = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<libc::pid_t>()
            .unwrap();
        cancellation.cancel();

        let error = worker
            .join()
            .expect("process worker should not panic")
            .expect_err("cancellation should interrupt the process");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(descendant, 0) } == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "descendant {descendant} survived"
            );
            thread::sleep(Duration::from_millis(10));
        }
        fs::remove_file(pid_file).unwrap();
    }

    #[test]
    fn guard_drop_kills_a_live_process() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let mut guard = ProcessGroupGuard::spawn(command, None).unwrap();
        assert!(guard.try_wait().unwrap().is_none());
        let pid = guard.pid();
        guard.force_kill();
        assert!(unsafe { libc::kill(pid, 0) } != 0);
    }
}
