use crate::engine::EmbeddedTerminal;
use crate::engine::{EmbeddedResultConfig, EmbeddedResultFormat};
use crate::execution::{PreparedProcess, ProcessGroupGuard};
use crate::lifecycle::{CancellationObserver, CancellationStatus};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const MAX_QUERY_SEQUENCE_LEN: usize = 4096;
const PTY_CLOSE_EXIT_GRACE: Duration = Duration::from_millis(100);
const PTY_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PTY_READ_BYTES: usize = 64 * 1024;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?6c";

#[derive(Clone)]
pub enum EmbeddedOutcome {
    Cancelled,
    Exited(i32),
    Signaled(i32),
    Failed(String),
    Returned(Value),
}

#[derive(Clone)]
pub struct EmbeddedRunResult {
    pub outcome: EmbeddedOutcome,
}

fn build_child_environment(overrides: &[(String, String)]) -> Result<Vec<CString>> {
    let mut environment: BTreeMap<OsString, OsString> = env::vars_os().collect();
    for (key, value) in overrides {
        environment.insert(OsString::from(key), OsString::from(value));
    }
    environment
        .into_iter()
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            CString::new(key.as_os_str().as_bytes())
                .context("embedded environment contains a NUL byte")
        })
        .collect()
}

fn resolve_executable(
    command: &CString,
    working_dir: Option<&Path>,
    overrides: &[(String, String)],
) -> Result<CString> {
    if command.as_bytes().contains(&b'/') {
        return Ok(command.clone());
    }
    let command_os = OsStr::from_bytes(command.as_bytes());
    let path = overrides
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| OsString::from(value))
        .or_else(|| env::var_os("PATH"))
        .unwrap_or_else(|| OsString::from("/usr/local/bin:/usr/bin:/bin"));
    let base = match working_dir {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => env::current_dir()?.join(path),
        None => env::current_dir()?,
    };
    for directory in env::split_paths(&path) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            base.join(directory)
        };
        let candidate = directory.join(command_os);
        let executable = fs::metadata(&candidate)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
        if executable && let Ok(candidate) = CString::new(candidate.as_os_str().as_bytes()) {
            return Ok(candidate);
        }
    }
    bail!(
        "embedded executable {:?} was not found in PATH",
        command.to_string_lossy()
    )
}

fn exec_child(
    command: &CString,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
    working_dir: Option<&CString>,
) -> ! {
    if let Some(working_dir) = working_dir
        && unsafe { libc::chdir(working_dir.as_ptr()) } != 0
    {
        unsafe { libc::_exit(127) };
    }
    unsafe {
        libc::execve(command.as_ptr(), argv, envp);
        libc::_exit(127);
    }
}

pub(crate) enum EmbeddedPoll {
    Running,
    Finished(EmbeddedRunResult),
}

/// Owns one live embedded child, its PTY, terminal screen, and input state.
/// The owner View can poll it repeatedly without recreating the child.
pub(crate) struct EmbeddedRuntime {
    process: ProcessGroupGuard,
    master: RawFd,
    result_fd: Option<RawFd>,
    result_config: Option<EmbeddedResultConfig>,
    result_bytes: Vec<u8>,
    result_open: bool,
    output_open: bool,
    responder: TerminalResponder,
    screen: EmbeddedTerminal,
    last_size: (u16, u16),
    requested_size: Option<(u16, u16)>,
    finished: bool,
}

impl EmbeddedRuntime {
    pub(crate) fn start(
        prepared: &PreparedProcess,
        result_config: Option<EmbeddedResultConfig>,
        initial_size: (u16, u16),
        initial_input: &[u8],
    ) -> Result<Self> {
        if prepared.argv.is_empty() {
            bail!("embedded action has an empty command");
        }

        let command_cstrings = prepared
            .argv
            .iter()
            .map(|argument| CString::new(argument.as_str()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("embedded command contains a NUL byte")?;
        let command_path = resolve_executable(
            &command_cstrings[0],
            prepared.current_dir.as_deref(),
            &prepared.environment,
        )?;
        let mut argv = command_cstrings
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        argv.push(std::ptr::null());
        let environment_cstrings = build_child_environment(&prepared.environment)?;
        let mut envp = environment_cstrings
            .iter()
            .map(|entry| entry.as_ptr())
            .collect::<Vec<_>>();
        envp.push(std::ptr::null());
        let working_dir_cstring = prepared
            .current_dir
            .as_deref()
            .map(|path| {
                CString::new(path.as_os_str().as_bytes())
                    .context("embedded working directory contains a NUL byte")
            })
            .transpose()?;

        let (result_read, result_write) = if result_config.is_some() {
            let (read, write) = create_pipe()?;
            (Some(read), Some(write))
        } else {
            (None, None)
        };
        let window = libc::winsize {
            ws_row: initial_size.1.max(1),
            ws_col: initial_size.0.max(1),
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master: libc::c_int = -1;
        let pid =
            unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &window) };
        if pid < 0 {
            close_optional(result_read);
            close_optional(result_write);
            return Err(io::Error::last_os_error()).context("could not create embedded PTY");
        }

        if pid == 0 {
            if let Some(read) = result_read {
                unsafe { libc::close(read) };
            }
            if let Some(write) = result_write {
                if unsafe { libc::dup2(write, libc::STDOUT_FILENO) } < 0 {
                    unsafe { libc::_exit(127) };
                }
                if write != libc::STDOUT_FILENO {
                    unsafe { libc::close(write) };
                }
            }
            unsafe { libc::close(master) };
            exec_child(
                &command_path,
                argv.as_ptr(),
                envp.as_ptr(),
                working_dir_cstring.as_ref(),
            );
        }

        close_optional(result_write);
        let mut process = ProcessGroupGuard::from_pid(pid);
        let setup = (|| {
            set_cloexec(master)?;
            set_nonblocking(master)?;
            if let Some(fd) = result_read {
                set_nonblocking(fd)?;
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = setup {
            process.force_kill();
            unsafe { libc::close(master) };
            close_optional(result_read);
            return Err(error);
        }
        let mut runtime = Self {
            process,
            master,
            result_fd: result_read,
            result_config,
            result_bytes: Vec::new(),
            result_open: result_read.is_some(),
            output_open: true,
            responder: TerminalResponder::default(),
            screen: EmbeddedTerminal::new(initial_size.0.max(1), initial_size.1.max(1)),
            last_size: (initial_size.0.max(1), initial_size.1.max(1)),
            requested_size: None,
            finished: false,
        };
        if !initial_input.is_empty()
            && let Err(error) = write_fd(runtime.master, initial_input)
        {
            runtime.finish_cancelled();
            return Err(error);
        }
        Ok(runtime)
    }

    pub(crate) fn poll(
        &mut self,
        size: (u16, u16),
        cancellation: &CancellationObserver,
    ) -> Result<EmbeddedPoll> {
        self.poll_with_resize(Some(size), cancellation)
    }

    pub(crate) fn poll_background(
        &mut self,
        cancellation: &CancellationObserver,
    ) -> Result<EmbeddedPoll> {
        self.poll_with_resize(None, cancellation)
    }

    fn poll_with_resize(
        &mut self,
        size: Option<(u16, u16)>,
        cancellation: &CancellationObserver,
    ) -> Result<EmbeddedPoll> {
        if self.finished {
            bail!("embedded runtime was polled after completion");
        }
        if cancellation.is_cancelled() {
            return Ok(EmbeddedPoll::Finished(self.finish_cancelled()));
        }
        if let Some(size) = size {
            let size = self
                .requested_size
                .take()
                .unwrap_or((size.0.max(1), size.1.max(1)));
            if size != self.last_size {
                self.screen.resize(size.0, size.1);
                resize_pty(self.master, self.process.pid(), size)?;
                self.last_size = size;
            }
        }

        if let Some(status) = self.process.try_wait_raw()? {
            self.process.cleanup_group();
            if self.output_open
                && let Err(error) = self.drain_output()
            {
                return Err(self.finish_error(error));
            }
            return self.finish_status(status).map(EmbeddedPoll::Finished);
        }

        if self.output_open {
            match self.drain_output() {
                Ok(true) => self.output_open = false,
                Ok(false) => {}
                Err(error) => return Err(self.finish_error(error)),
            }
        }
        if self.result_open
            && let (Some(fd), Some(config)) = (self.result_fd, self.result_config)
        {
            let closed = match drain_result(fd, &mut self.result_bytes, config.max_bytes) {
                Ok(closed) => closed,
                Err(error) => return Err(self.finish_error(error)),
            };
            if closed {
                self.result_open = false;
            }
        }
        if !self.output_open && self.result_fd.is_none() {
            if let Some(status) = match wait_for_pty_exit(&mut self.process) {
                Ok(status) => status,
                Err(error) => return Err(self.finish_error(error)),
            } {
                self.process.cleanup_group();
                return self.finish_status(status).map(EmbeddedPoll::Finished);
            }
            return Ok(EmbeddedPoll::Finished(self.finish_cancelled()));
        }
        Ok(EmbeddedPoll::Running)
    }

    pub(crate) fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        if self.finished || bytes.is_empty() {
            return Ok(());
        }
        write_fd(self.master, bytes)
    }

    pub(crate) fn screen(&self) -> &EmbeddedTerminal {
        &self.screen
    }

    #[cfg(test)]
    fn last_size_for_test(&self) -> (u16, u16) {
        self.last_size
    }

    fn drain_output(&mut self) -> Result<bool> {
        drain_output_to_screen(self.master, &mut self.responder, &mut self.screen)
    }

    fn finish_status(&mut self, status: libc::c_int) -> Result<EmbeddedRunResult> {
        let outcome = process_outcome(
            status,
            self.result_fd,
            self.result_config,
            &mut self.result_bytes,
        );
        self.close_fds();
        self.finished = true;
        Ok(EmbeddedRunResult { outcome: outcome? })
    }

    fn finish_error(&mut self, error: anyhow::Error) -> anyhow::Error {
        self.process.force_kill();
        self.close_fds();
        self.finished = true;
        error
    }

    fn finish_cancelled(&mut self) -> EmbeddedRunResult {
        self.process.force_kill();
        self.close_fds();
        self.finished = true;
        EmbeddedRunResult {
            outcome: EmbeddedOutcome::Cancelled,
        }
    }

    fn close_fds(&mut self) {
        if self.master >= 0 {
            unsafe { libc::close(self.master) };
            self.master = -1;
        }
        close_optional(self.result_fd.take());
    }
}

impl Drop for EmbeddedRuntime {
    fn drop(&mut self) {
        if !self.finished {
            self.process.force_kill();
        }
        self.close_fds();
    }
}

fn parse_result(bytes: &[u8], config: EmbeddedResultConfig) -> Result<Value> {
    if bytes.is_empty() {
        if config.required {
            bail!("embedded process produced no result on stdout");
        }
        return Ok(Value::Null);
    }
    let value = match config.format {
        EmbeddedResultFormat::Text => {
            let mut text = std::str::from_utf8(bytes)
                .context("embedded text result is not valid UTF-8")?
                .to_string();
            if text.ends_with('\n') {
                text.pop();
                if text.ends_with('\r') {
                    text.pop();
                }
            }
            Value::String(text)
        }
        EmbeddedResultFormat::Json => {
            serde_json::from_slice(bytes).context("embedded result is not valid JSON")?
        }
    };
    Ok(value)
}

fn drain_result(fd: RawFd, bytes: &mut Vec<u8>, limit: usize) -> Result<bool> {
    let mut buffer = [0_u8; 8192];
    let mut drained = 0;
    while drained < MAX_PTY_READ_BYTES {
        let read_size = (MAX_PTY_READ_BYTES - drained).min(buffer.len());
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), read_size) };
        if count == 0 {
            return Ok(true);
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                break;
            }
            return Err(error).context("could not read embedded result stdout");
        }
        let count = count as usize;
        if bytes.len().saturating_add(count) > limit {
            bail!("embedded result stdout exceeded {limit} bytes");
        }
        bytes.extend_from_slice(&buffer[..count]);
        drained += count;
    }
    Ok(false)
}

fn drain_result_to_eof(
    fd: RawFd,
    bytes: &mut Vec<u8>,
    limit: usize,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if drain_result(fd, bytes, limit)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("embedded result stdout remained open after the child exited");
        }
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        let polled = unsafe { libc::poll(&mut descriptor, 1, 20) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error).context("could not poll embedded result stdout");
            }
        }
    }
}

fn drain_output_to_screen(
    master: RawFd,
    responder: &mut TerminalResponder,
    screen: &mut EmbeddedTerminal,
) -> Result<bool> {
    let mut buffer = [0_u8; 8192];
    let mut drained = 0;
    let mut reached_eof = false;
    while drained < MAX_PTY_READ_BYTES {
        let read_size = (MAX_PTY_READ_BYTES - drained).min(buffer.len());
        let count = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), read_size) };
        if count == 0 {
            reached_eof = true;
            break;
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                break;
            }
            if error.raw_os_error() == Some(libc::EIO) {
                reached_eof = true;
                break;
            }
            return Err(error).context("could not read embedded PTY");
        }
        let count = count as usize;
        drained += count;
        let output = &buffer[..count];
        for _ in 0..responder.primary_device_attribute_queries(output) {
            write_fd(master, PRIMARY_DEVICE_ATTRIBUTES)?;
        }
        screen.feed(output);
    }
    Ok(reached_eof)
}

#[derive(Default)]
struct TerminalResponder {
    state: TerminalQueryState,
}

#[derive(Default)]
enum TerminalQueryState {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
}

impl TerminalResponder {
    fn primary_device_attribute_queries(&mut self, input: &[u8]) -> usize {
        let mut queries = 0;
        for &byte in input {
            match &mut self.state {
                TerminalQueryState::Ground if byte == 0x1b => {
                    self.state = TerminalQueryState::Escape;
                }
                TerminalQueryState::Ground => {}
                TerminalQueryState::Escape if byte == b'[' => {
                    self.state = TerminalQueryState::Csi(Vec::new());
                }
                TerminalQueryState::Escape if byte == 0x1b => {}
                TerminalQueryState::Escape => self.state = TerminalQueryState::Ground,
                TerminalQueryState::Csi(_) if byte == 0x1b => {
                    self.state = TerminalQueryState::Escape;
                }
                TerminalQueryState::Csi(_) if matches!(byte, 0x18 | 0x1a) => {
                    self.state = TerminalQueryState::Ground;
                }
                TerminalQueryState::Csi(sequence) => {
                    sequence.push(byte);
                    if (0x40..=0x7e).contains(&byte) {
                        if byte == b'c' && matches!(sequence.as_slice(), [b'c'] | [b'0', b'c']) {
                            queries += 1;
                        }
                        self.state = TerminalQueryState::Ground;
                    } else if sequence.len() >= MAX_QUERY_SEQUENCE_LEN {
                        self.state = TerminalQueryState::Ground;
                    }
                }
            }
        }
        queries
    }
}

fn create_pipe() -> Result<(RawFd, RawFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).context("could not create embedded result pipe");
    }
    if let Err(error) = set_cloexec(fds[0]).and_then(|()| set_cloexec(fds[1])) {
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        return Err(error);
    }
    Ok((fds[0], fds[1]))
}

fn close_optional(fd: Option<RawFd>) {
    if let Some(fd) = fd {
        unsafe { libc::close(fd) };
    }
}

fn write_fd(fd: RawFd, bytes: &[u8]) -> Result<()> {
    let mut offset = 0;
    let deadline = Instant::now() + PTY_WRITE_TIMEOUT;
    while offset < bytes.len() {
        let count =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if count > 0 {
            offset += count as usize;
            continue;
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                if Instant::now() >= deadline {
                    bail!("embedded PTY did not accept input within 2 seconds");
                }
                let mut descriptor = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let polled = unsafe { libc::poll(&mut descriptor, 1, 20) };
                if polled < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error).context("could not poll embedded input descriptor");
                    }
                }
                continue;
            }
            return Err(error).context("could not write embedded input");
        }
    }
    Ok(())
}

fn resize_pty(master: RawFd, pid: libc::pid_t, size: (u16, u16)) -> Result<()> {
    let window = libc::winsize {
        ws_row: size.1,
        ws_col: size.0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &window) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("could not resize embedded PTY");
    }
    unsafe { libc::kill(-pid, libc::SIGWINCH) };
    Ok(())
}

fn set_cloexec(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect embedded descriptor");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error())
            .context("could not set embedded descriptor close-on-exec");
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect embedded descriptor");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error()).context("could not configure embedded descriptor");
    }
    Ok(())
}

fn wait_for_pty_exit(process: &mut ProcessGroupGuard) -> Result<Option<libc::c_int>> {
    let deadline = Instant::now() + PTY_CLOSE_EXIT_GRACE;
    loop {
        if let Some(status) = process.try_wait_raw()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn process_outcome(
    status: libc::c_int,
    result_fd: Option<RawFd>,
    result_config: Option<EmbeddedResultConfig>,
    result_bytes: &mut Vec<u8>,
) -> Result<EmbeddedOutcome> {
    if let (Some(fd), Some(config)) = (result_fd, result_config) {
        drain_result_to_eof(fd, result_bytes, config.max_bytes, Duration::from_secs(1))?;
    }
    if libc::WIFEXITED(status)
        && libc::WEXITSTATUS(status) == 0
        && let Some(config) = result_config
    {
        Ok(EmbeddedOutcome::Returned(parse_result(
            result_bytes,
            config,
        )?))
    } else {
        Ok(decode_status(status))
    }
}

fn decode_status(status: libc::c_int) -> EmbeddedOutcome {
    if libc::WIFEXITED(status) {
        EmbeddedOutcome::Exited(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        EmbeddedOutcome::Signaled(libc::WTERMSIG(status))
    } else {
        EmbeddedOutcome::Signaled(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmbeddedOutcome, EmbeddedPoll, EmbeddedResultConfig, EmbeddedResultFormat, EmbeddedRuntime,
        PreparedProcess, TerminalResponder, Value, parse_result,
    };
    use crate::lifecycle::CancellationToken;
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn pty_eof_does_not_cancel_a_process_waiting_to_write_its_result() {
        let cancellation = CancellationToken::new();
        let prepared = PreparedProcess {
            argv: vec![
                "sh".to_string(),
                "-c".to_string(),
                "exec 0<&- 2>&-; sleep 0.5; printf result".to_string(),
            ],
            environment: Vec::new(),
            current_dir: None,
        };
        let mut runtime = EmbeddedRuntime::start(
            &prepared,
            Some(EmbeddedResultConfig {
                format: EmbeddedResultFormat::Text,
                required: true,
                max_bytes: 1024,
            }),
            (20, 10),
            &[],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while runtime.output_open {
            assert!(matches!(
                runtime.poll_background(&cancellation.observer()).unwrap(),
                EmbeddedPoll::Running
            ));
            assert!(Instant::now() < deadline, "PTY output did not close");
            thread::sleep(Duration::from_millis(5));
        }

        thread::sleep(Duration::from_millis(150));
        assert!(matches!(
            runtime.poll_background(&cancellation.observer()).unwrap(),
            EmbeddedPoll::Running
        ));

        loop {
            match runtime.poll_background(&cancellation.observer()).unwrap() {
                EmbeddedPoll::Running => {
                    assert!(Instant::now() < deadline, "embedded process did not finish");
                    thread::sleep(Duration::from_millis(5));
                }
                EmbeddedPoll::Finished(result) => {
                    let EmbeddedOutcome::Returned(output) = result.outcome else {
                        panic!("PTY EOF changed the embedded process outcome")
                    };
                    assert_eq!(output, Value::String("result".to_string()));
                    break;
                }
            }
        }
    }

    #[test]
    fn background_poll_does_not_resize_the_pty() {
        let cancellation = CancellationToken::new();
        let prepared = PreparedProcess {
            argv: vec!["sh".to_string(), "-c".to_string(), "sleep 1".to_string()],
            environment: Vec::new(),
            current_dir: None,
        };
        let mut runtime = EmbeddedRuntime::start(&prepared, None, (20, 10), &[]).unwrap();
        assert_eq!(runtime.last_size_for_test(), (20, 10));

        runtime.poll_background(&cancellation.observer()).unwrap();

        assert_eq!(runtime.last_size_for_test(), (20, 10));
        cancellation.cancel();
        runtime.poll_background(&cancellation.observer()).unwrap();
    }

    #[test]
    fn path_lookup_uses_the_embedded_working_directory() {
        use std::ffi::CString;
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("tflow-pty-path-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        let working_dir = root.join("workflow");
        let bin_dir = working_dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let executable = bin_dir.join("probe");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let command = CString::new("probe").unwrap();
        let resolved = super::resolve_executable(
            &command,
            Some(&working_dir),
            &[("PATH".to_string(), "bin".to_string())],
        )
        .unwrap();
        assert_eq!(resolved.to_bytes(), executable.as_os_str().as_bytes());
        assert!(
            super::resolve_executable(
                &CString::new("missing-probe").unwrap(),
                Some(&working_dir),
                &[("PATH".to_string(), "/usr/bin:/bin".to_string())],
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn responds_to_primary_device_attributes_across_pty_read_boundaries() {
        let mut responder = TerminalResponder::default();
        assert_eq!(responder.primary_device_attribute_queries(b"\x1b["), 0);
        assert_eq!(responder.primary_device_attribute_queries(b"0c"), 1);
    }

    #[test]
    fn ignores_non_primary_device_attribute_sequences() {
        let mut responder = TerminalResponder::default();
        assert_eq!(responder.primary_device_attribute_queries(b"\x1b[?6c"), 0);
        assert_eq!(responder.primary_device_attribute_queries(b"\x1b[>0c"), 0);
    }

    #[test]
    fn restarts_after_an_incomplete_or_cancelled_csi_sequence() {
        let mut responder = TerminalResponder::default();
        assert_eq!(
            responder.primary_device_attribute_queries(b"\x1b[31\x1b[0c"),
            1
        );
        assert_eq!(
            responder.primary_device_attribute_queries(b"\x1b[31\x18\x1b[0c"),
            1
        );
    }

    #[test]
    fn parses_text_and_json_results() {
        let text = parse_result(
            b"hello\r\n",
            EmbeddedResultConfig {
                format: EmbeddedResultFormat::Text,
                required: true,
                max_bytes: 1024,
            },
        )
        .unwrap();
        assert_eq!(text, Value::String("hello".to_string()));
        let json = parse_result(
            br#"{"name":"Ada"}"#,
            EmbeddedResultConfig {
                format: EmbeddedResultFormat::Json,
                required: true,
                max_bytes: 1024,
            },
        )
        .unwrap();
        assert_eq!(json["name"], "Ada");
    }
}
