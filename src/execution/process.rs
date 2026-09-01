use crate::lifecycle::CancellationStatus;
#[cfg(test)]
use crate::lifecycle::CancellationToken;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::Duration;

pub(crate) const MANAGED_ENVIRONMENT: &[&str] = &[
    "LAUNCHER_COMMAND",
    "LAUNCHER_INPUT",
    "LAUNCHER_ITEM",
    "LAUNCHER_ITEM_PLUGIN",
    "LAUNCHER_ITEM_VIEW_REF",
    "LAUNCHER_LOG_FILE",
    "LAUNCHER_METADATA",
    "LAUNCHER_PLUGIN",
    "LAUNCHER_PLUGIN_DIR",
    "LAUNCHER_QUERY",
    "LAUNCHER_STDIN_FILE",
    "LAUNCHER_VALUE",
    "LAUNCHER_VIEW",
    "LAUNCHER_VIEW_REF",
];

pub(crate) struct ProcessGroupGuard {
    pid: libc::pid_t,
    child: Option<Child>,
    reaped: bool,
    group_cleaned: bool,
    leader_killed: bool,
}

impl ProcessGroupGuard {
    pub(crate) fn spawn(mut command: Command) -> io::Result<Self> {
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
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
            if !self.reaped {
                let _ = child.wait();
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
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if !self.group_cleaned || !self.reaped {
            self.force_kill();
        }
    }
}

#[derive(Clone)]
pub(crate) struct PreparedProcess {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

pub(crate) fn clear_managed_environment(command: &mut Command) {
    for key in MANAGED_ENVIRONMENT {
        command.env_remove(key);
    }
}

impl PreparedProcess {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.argv[0]);
        command.args(&self.argv[1..]);
        clear_managed_environment(&mut command);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        command
    }

    pub(crate) fn status(
        &self,
        cancellation: &dyn CancellationStatus,
    ) -> io::Result<std::process::ExitStatus> {
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "launcher shutdown requested",
            ));
        }

        let mut process = ProcessGroupGuard::spawn(self.command())?;
        loop {
            if cancellation.is_cancelled() {
                process.force_kill();
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "launcher shutdown requested",
                ));
            }
            if let Some(status) = process.try_wait()? {
                process.cleanup_group();
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    #[test]
    fn command_removes_unset_launcher_environment() {
        let prepared = PreparedProcess {
            argv: vec!["true".to_string()],
            environment: vec![("LAUNCHER_VIEW".to_string(), "core:default".to_string())],
            current_dir: None,
        };

        let command = prepared.command();
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment["LAUNCHER_VIEW"].as_deref(),
            Some("core:default")
        );
        assert_eq!(environment["LAUNCHER_PLUGIN_DIR"], None);
        assert_eq!(environment["LAUNCHER_LOG_FILE"], None);
        assert_eq!(environment["LAUNCHER_STDIN_FILE"], None);
    }

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
        let worker = thread::spawn(move || prepared.status(&worker_cancellation));
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
        let status = prepared.status(&CancellationToken::new()).unwrap();
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
        let worker = thread::spawn(move || prepared.status(&worker_cancellation));
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
        let mut guard = ProcessGroupGuard::spawn(command).unwrap();
        assert!(guard.try_wait().unwrap().is_none());
        let pid = guard.pid();
        guard.force_kill();
        assert!(unsafe { libc::kill(pid, 0) } != 0);
    }
}
