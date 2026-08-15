use crate::embedded_terminal::EmbeddedTerminal;
use crate::engine::process::MANAGED_ENVIRONMENT;
use crate::engine::{EmbeddedResultConfig, EmbeddedResultFormat, PreparedProcess, ViewOutput};
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::thread;
use std::time::{Duration, Instant};

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);
const MAX_QUERY_SEQUENCE_LEN: usize = 4096;
const PTY_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
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

pub fn run(
    prepared: &PreparedProcess,
    result: Option<EmbeddedResultConfig>,
    escape_cancels: bool,
    initial_input: Vec<u8>,
    terminal: &mut Terminal,
    content_size: &dyn Fn(u16, u16) -> (u16, u16),
    render: &mut dyn FnMut(&mut Terminal, &EmbeddedTerminal) -> Result<()>,
) -> Result<EmbeddedRunResult> {
    if prepared.argv.is_empty() {
        bail!("embedded action has an empty command");
    }

    let command_cstrings = prepared
        .argv
        .iter()
        .map(|argument| CString::new(argument.as_str()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("embedded command contains a NUL byte")?;
    let working_dir_cstring = prepared
        .current_dir
        .as_deref()
        .map(|path| {
            CString::new(path.as_os_str().as_bytes())
                .context("embedded working directory contains a NUL byte")
        })
        .transpose()?;
    let environment_cstrings = prepared
        .environment
        .iter()
        .map(|(key, value)| {
            Ok((
                CString::new(key.as_str())
                    .context("embedded environment key contains a NUL byte")?,
                CString::new(value.as_str())
                    .context("embedded environment value contains a NUL byte")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let managed_environment_cstrings = MANAGED_ENVIRONMENT
        .iter()
        .map(|key| CString::new(*key).expect("managed environment key contains a NUL byte"))
        .collect::<Vec<_>>();

    let (result_read, result_write) = if result.is_some() {
        let (read, write) = create_pipe()?;
        (Some(read), Some(write))
    } else {
        (None, None)
    };
    let (outer_columns, outer_rows) = terminal.size();
    let (columns, rows) = content_size(outer_columns, outer_rows);
    let window = libc::winsize {
        ws_row: rows,
        ws_col: columns,
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
        exec_child(
            &command_cstrings,
            &managed_environment_cstrings,
            &environment_cstrings,
            working_dir_cstring.as_ref(),
        );
    }

    close_optional(result_write);
    let mut child_reaped = false;
    let outcome = (|| {
        set_nonblocking(master)?;
        if let Some(fd) = result_read {
            set_nonblocking(fd)?;
        }
        relay(
            master,
            result_read,
            pid,
            &mut child_reaped,
            result,
            escape_cancels,
            initial_input,
            terminal,
            (columns, rows),
            content_size,
            render,
        )
    })();
    if outcome.is_err() && !child_reaped {
        terminate_child(pid);
    }
    unsafe { libc::close(master) };
    close_optional(result_read);
    outcome
}

fn exec_child(
    command: &[CString],
    managed_environment: &[CString],
    environment: &[(CString, CString)],
    working_dir: Option<&CString>,
) -> ! {
    if let Some(working_dir) = working_dir
        && unsafe { libc::chdir(working_dir.as_ptr()) } != 0
    {
        unsafe { libc::_exit(127) };
    }
    for key in managed_environment {
        if unsafe { libc::unsetenv(key.as_ptr()) } != 0 {
            unsafe { libc::_exit(127) };
        }
    }
    for (key, value) in environment {
        if unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) } != 0 {
            unsafe { libc::_exit(127) };
        }
    }

    let mut arguments: Vec<*const libc::c_char> =
        command.iter().map(|argument| argument.as_ptr()).collect();
    arguments.push(std::ptr::null());
    unsafe {
        libc::execvp(command[0].as_ptr(), arguments.as_ptr());
        libc::_exit(127);
    }
}

#[allow(clippy::too_many_arguments)]
fn relay(
    master: RawFd,
    result_fd: Option<RawFd>,
    pid: libc::pid_t,
    child_reaped: &mut bool,
    result_config: Option<EmbeddedResultConfig>,
    escape_cancels: bool,
    initial_input: Vec<u8>,
    terminal: &mut Terminal,
    mut last_size: (u16, u16),
    content_size: &dyn Fn(u16, u16) -> (u16, u16),
    render: &mut dyn FnMut(&mut Terminal, &EmbeddedTerminal) -> Result<()>,
) -> Result<EmbeddedRunResult> {
    let mut input = InputRelay::new(escape_cancels);
    let mut result_bytes = Vec::new();
    let mut result_open = result_fd.is_some();
    let mut pty_open = true;
    let mut responder = TerminalResponder::default();
    let mut screen = EmbeddedTerminal::new(last_size.0, last_size.1);
    render(terminal, &screen)?;
    input.push(&initial_input, master)?;

    loop {
        if input.bare_escape_expired() {
            terminate_child(pid);
            input.take_pending();
            return Ok(EmbeddedRunResult {
                outcome: EmbeddedOutcome::Cancelled,
                remaining_input: Vec::new(),
            });
        }
        if let Some(status) = wait_status(pid, true)? {
            *child_reaped = true;
            terminate_process_group(pid);
            if pty_open {
                drain_output(master, &mut responder, &mut screen, terminal, render)?;
            }
            if let (Some(fd), Some(config)) = (result_fd, result_config) {
                drain_result_to_eof(
                    fd,
                    &mut result_bytes,
                    config.max_bytes,
                    Duration::from_secs(1),
                )?;
            }
            let outcome = if libc::WIFEXITED(status)
                && libc::WEXITSTATUS(status) == 0
                && let Some(config) = result_config
            {
                EmbeddedOutcome::Returned(parse_result(&result_bytes, config)?)
            } else {
                decode_status(status)
            };
            return Ok(EmbeddedRunResult {
                outcome,
                remaining_input: input.take_pending(),
            });
        }

        let outer_size = terminal.size();
        let current_size = content_size(outer_size.0, outer_size.1);
        if pty_open && current_size != last_size {
            screen.resize(current_size.0, current_size.1);
            resize_pty(master, pid, current_size)?;
            last_size = current_size;
            render(terminal, &screen)?;
        }

        let mut descriptors = [
            libc::pollfd {
                fd: terminal.input_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if pty_open { master } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: result_open.then_some(result_fd).flatten().unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, 40) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("could not poll embedded PTY");
        }

        if descriptors[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let Some(bytes) = read_input(terminal.input_fd())? else {
                terminate_child(pid);
                return Ok(EmbeddedRunResult {
                    outcome: EmbeddedOutcome::Cancelled,
                    remaining_input: input.take_pending(),
                });
            };
            input.push(&bytes, master)?;
        }
        if descriptors[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
            && drain_output(master, &mut responder, &mut screen, terminal, render)?
        {
            pty_open = false;
        }
        if descriptors[2].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
            && let (Some(fd), Some(config)) = (result_fd, result_config)
            && drain_result(fd, &mut result_bytes, config.max_bytes)?
        {
            result_open = false;
        }
    }
}

fn read_input(input_fd: RawFd) -> Result<Option<Vec<u8>>> {
    let mut buffer = [0_u8; 4096];
    let count = unsafe { libc::read(input_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if count == 0 {
        return Ok(None);
    }
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(Some(Vec::new()));
        }
        return Err(error).context("could not read embedded input");
    }
    Ok(Some(buffer[..count as usize].to_vec()))
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
    loop {
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
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

fn drain_output(
    master: RawFd,
    responder: &mut TerminalResponder,
    screen: &mut EmbeddedTerminal,
    terminal: &mut Terminal,
    render: &mut dyn FnMut(&mut Terminal, &EmbeddedTerminal) -> Result<()>,
) -> Result<bool> {
    let mut buffer = [0_u8; 8192];
    let mut reached_eof = false;
    loop {
        let count = unsafe { libc::read(master, buffer.as_mut_ptr().cast(), buffer.len()) };
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
            reached_eof = true;
            break;
        }
        let output = &buffer[..count as usize];
        for _ in 0..responder.primary_device_attribute_queries(output) {
            write_fd(master, PRIMARY_DEVICE_ATTRIBUTES)?;
        }
        screen.feed(output);
        render(terminal, screen)?;
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
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error()).context("could not create embedded result pipe");
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

fn wait_status(pid: libc::pid_t, nohang: bool) -> Result<Option<libc::c_int>> {
    let mut status = 0;
    let options = if nohang { libc::WNOHANG } else { 0 };
    let result = unsafe { libc::waitpid(pid, &mut status, options) };
    if result == pid {
        Ok(Some(status))
    } else if result == 0 {
        Ok(None)
    } else if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(error).context("could not wait for embedded process")
        }
    } else {
        Ok(None)
    }
}

fn terminate_process_group(pid: libc::pid_t) {
    if unsafe { libc::kill(-pid, libc::SIGTERM) } == 0 {
        thread::sleep(Duration::from_millis(10));
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

fn terminate_child(pid: libc::pid_t) {
    unsafe { libc::kill(-pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if matches!(wait_status(pid, true), Ok(Some(_))) {
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(-pid, libc::SIGKILL) };
    let _ = wait_status(pid, false);
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
