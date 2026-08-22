use crate::cancellation::CancellationToken;
use crate::embedded_terminal::EmbeddedTerminal;
use crate::engine::process::MANAGED_ENVIRONMENT;
use crate::engine::{
    EmbeddedResultConfig, EmbeddedResultFormat, PreparedProcess, ProcessGroupGuard, ViewOutput,
};
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

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);
const MAX_QUERY_SEQUENCE_LEN: usize = 4096;
const PTY_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const PTY_CLOSE_EXIT_GRACE: Duration = Duration::from_millis(100);
const MAX_PTY_READ_BYTES: usize = 64 * 1024;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?6c";

pub enum EmbeddedOutcome {
    Cancelled,
    Exited(i32),
    Signaled(i32),
    Returned(ViewOutput),
}

pub struct EmbeddedRunResult {
    pub outcome: EmbeddedOutcome,
    pub remaining_input: Vec<u8>,
}

fn build_child_environment(overrides: &[(String, String)]) -> Result<Vec<CString>> {
    let mut environment: BTreeMap<OsString, OsString> = env::vars_os().collect();
    for key in MANAGED_ENVIRONMENT {
        environment.remove(OsStr::new(key));
    }
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
    input: InputRelay,
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
        escape_cancels: bool,
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
            input: InputRelay::new(escape_cancels),
            responder: TerminalResponder::default(),
            screen: EmbeddedTerminal::new(initial_size.0.max(1), initial_size.1.max(1)),
            last_size: (initial_size.0.max(1), initial_size.1.max(1)),
            requested_size: None,
            finished: false,
        };
        if let Err(error) = runtime.input.push(initial_input, runtime.master) {
            runtime.finish_cancelled();
            return Err(error);
        }
        Ok(runtime)
    }

    pub(crate) fn poll(
        &mut self,
        size: (u16, u16),
        cancellation: &CancellationToken,
    ) -> Result<EmbeddedPoll> {
        if self.finished {
            bail!("embedded runtime was polled after completion");
        }
        if cancellation.is_cancelled() || self.input.bare_escape_expired() {
            return Ok(EmbeddedPoll::Finished(self.finish_cancelled()));
        }
        let size = self
            .requested_size
            .take()
            .unwrap_or((size.0.max(1), size.1.max(1)));
        if size != self.last_size {
            self.screen.resize(size.0, size.1);
            resize_pty(self.master, self.process.pid(), size)?;
            self.last_size = size;
        }

        if let Some(status) = self.process.try_wait_raw()? {
            self.process.cleanup_group();
            self.drain_output()?;
            return self.finish_status(status).map(EmbeddedPoll::Finished);
        }

        let output_closed = self.drain_output()?;
        if self.result_open
            && let (Some(fd), Some(config)) = (self.result_fd, self.result_config)
            && drain_result(fd, &mut self.result_bytes, config.max_bytes)?
        {
            self.result_open = false;
        }
        if output_closed {
            if let Some(status) = wait_for_pty_exit(&mut self.process)? {
                self.process.cleanup_group();
                return self.finish_status(status).map(EmbeddedPoll::Finished);
            }
            return Ok(EmbeddedPoll::Finished(self.finish_cancelled()));
        }
        Ok(EmbeddedPoll::Running)
    }

    pub(crate) fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.input.push(bytes, self.master)
    }

    pub(crate) fn request_resize(&mut self, size: (u16, u16)) {
        let size = (size.0.max(1), size.1.max(1));
        self.screen.resize(size.0, size.1);
        self.requested_size = Some(size);
    }

    pub(crate) fn screen(&self) -> &EmbeddedTerminal {
        &self.screen
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
        )?;
        let remaining_input = self.input.take_pending();
        self.close_fds();
        self.finished = true;
        Ok(EmbeddedRunResult {
            outcome,
            remaining_input,
        })
    }

    fn finish_cancelled(&mut self) -> EmbeddedRunResult {
        self.process.force_kill();
        self.input.take_pending();
        self.close_fds();
        self.finished = true;
        EmbeddedRunResult {
            outcome: EmbeddedOutcome::Cancelled,
            remaining_input: Vec::new(),
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

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

struct InputRelay {
    escape_cancels: bool,
    pending: Vec<u8>,
    escape_started: Option<Instant>,
    in_bracketed_paste: bool,
}

impl Default for InputRelay {
    fn default() -> Self {
        Self::new(true)
    }
}

impl InputRelay {
    fn new(escape_cancels: bool) -> Self {
        Self {
            escape_cancels,
            pending: Vec::new(),
            escape_started: None,
            in_bracketed_paste: false,
        }
    }

    fn push(&mut self, bytes: &[u8], master: RawFd) -> Result<()> {
        if !self.escape_cancels {
            return write_fd(master, bytes);
        }
        self.pending.extend_from_slice(bytes);
        self.process(master)
    }

    fn bare_escape_expired(&self) -> bool {
        self.escape_cancels
            && !self.in_bracketed_paste
            && self.pending.as_slice() == b"\x1b"
            && self
                .escape_started
                .is_some_and(|started| started.elapsed() >= ESCAPE_TIMEOUT)
    }

    fn take_pending(&mut self) -> Vec<u8> {
        self.escape_started = None;
        self.in_bracketed_paste = false;
        std::mem::take(&mut self.pending)
    }

    fn process(&mut self, master: RawFd) -> Result<()> {
        loop {
            if self.pending.is_empty() {
                self.escape_started = None;
                return Ok(());
            }

            if self.in_bracketed_paste {
                if let Some(end) = self
                    .pending
                    .windows(BRACKETED_PASTE_END.len())
                    .position(|window| window == BRACKETED_PASTE_END)
                {
                    let count = end + BRACKETED_PASTE_END.len();
                    write_fd(master, &self.pending[..count])?;
                    self.pending.drain(..count);
                    self.in_bracketed_paste = false;
                    continue;
                }
                let keep = marker_prefix_suffix_len(&self.pending, BRACKETED_PASTE_END);
                let count = self.pending.len() - keep;
                if count > 0 {
                    write_fd(master, &self.pending[..count])?;
                    self.pending.drain(..count);
                }
                return Ok(());
            }

            if self.pending[0] != 0x1b {
                let count = self
                    .pending
                    .iter()
                    .position(|byte| *byte == 0x1b)
                    .unwrap_or(self.pending.len());
                write_fd(master, &self.pending[..count])?;
                self.pending.drain(..count);
                continue;
            }

            if self.pending.len() == 1 {
                self.escape_started.get_or_insert_with(Instant::now);
                return Ok(());
            }
            self.escape_started = None;

            if self.pending.starts_with(BRACKETED_PASTE_START) {
                write_fd(master, BRACKETED_PASTE_START)?;
                self.pending.drain(..BRACKETED_PASTE_START.len());
                self.in_bracketed_paste = true;
                continue;
            }

            if matches!(self.pending[1], b'[' | b'O') {
                if self.pending.len().saturating_sub(2) > MAX_QUERY_SEQUENCE_LEN {
                    let bytes = std::mem::take(&mut self.pending);
                    self.escape_started = None;
                    write_fd(master, &bytes)?;
                    continue;
                }
                let Some(end) = escape_sequence_end(&self.pending[2..]) else {
                    return Ok(());
                };
                let count = end + 3;
                write_fd(master, &self.pending[..count])?;
                self.pending.drain(..count);
                continue;
            }

            write_fd(master, &self.pending[..2])?;
            self.pending.drain(..2);
        }
    }
}

fn escape_sequence_end(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|byte| (0x40..=0x7e).contains(byte))
}

fn marker_prefix_suffix_len(bytes: &[u8], marker: &[u8]) -> usize {
    (1..marker.len())
        .rev()
        .find(|length| bytes.ends_with(&marker[..*length]))
        .unwrap_or(0)
}

fn parse_result(bytes: &[u8], config: EmbeddedResultConfig) -> Result<ViewOutput> {
    if bytes.is_empty() {
        if config.required {
            bail!("embedded process produced no result on stdout");
        }
        return Ok(ViewOutput::Value { value: Value::Null });
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
    Ok(ViewOutput::Value { value })
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
        EmbeddedResultConfig, EmbeddedResultFormat, InputRelay, TerminalResponder, create_pipe,
        parse_result,
    };
    use std::io::Read;
    use std::os::fd::FromRawFd;

    #[test]
    fn path_lookup_uses_the_embedded_working_directory() {
        use std::ffi::CString;
        use std::fs;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("tui-launcher-pty-path-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        let working_dir = root.join("plugin");
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
    fn input_relay_forwards_non_escape_bytes_without_waiting_for_utf8() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::default();
        relay.push(&[0xc3], write).unwrap();
        relay.push(&[0xa9, b'\r'], write).unwrap();
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [0xc3, 0xa9, b'\r']);
    }

    #[test]
    fn input_relay_reserves_only_a_timed_out_bare_escape() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::default();
        relay.push(b"\x1b", write).unwrap();
        assert!(!relay.bare_escape_expired());
        relay.escape_started = Some(std::time::Instant::now() - super::ESCAPE_TIMEOUT);
        assert!(relay.bare_escape_expired());
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn input_relay_forwards_an_incomplete_csi_after_the_sequence_limit() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::default();
        relay.push(b"\x1b[", write).unwrap();
        let payload = vec![b' '; super::MAX_QUERY_SEQUENCE_LEN + 1];
        relay.push(&payload, write).unwrap();
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [b"\x1b[".as_slice(), payload.as_slice()].concat());
    }

    #[test]
    fn input_relay_forwards_bare_escape_immediately_when_cancellation_is_disabled() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::new(false);
        relay.push(b"\x1b", write).unwrap();
        assert!(!relay.bare_escape_expired());
        assert!(relay.take_pending().is_empty());
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"\x1b");
    }

    #[test]
    fn input_relay_exposes_unwritten_bytes_for_the_restored_caller() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::default();
        relay.push(b"\x1b[", write).unwrap();
        assert_eq!(relay.take_pending(), b"\x1b[");
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn input_relay_preserves_escape_sequences_and_bracketed_paste() {
        let (read, write) = create_pipe().unwrap();
        let mut relay = InputRelay::default();
        relay.push(b"\x1b", write).unwrap();
        relay.push(b"[A\x1ba\x1b[200~paste\x1b", write).unwrap();
        assert!(!relay.bare_escape_expired());
        relay.push(b"[201~", write).unwrap();
        unsafe { libc::close(write) };
        let mut file = unsafe { std::fs::File::from_raw_fd(read) };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"\x1b[A\x1ba\x1b[200~paste\x1b[201~");
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
        assert_eq!(
            serde_json::to_value(text).unwrap()["value"],
            serde_json::json!("hello")
        );
        let json = parse_result(
            br#"{"name":"Ada"}"#,
            EmbeddedResultConfig {
                format: EmbeddedResultFormat::Json,
                required: true,
                max_bytes: 1024,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(json).unwrap()["value"]["name"], "Ada");
    }
}
