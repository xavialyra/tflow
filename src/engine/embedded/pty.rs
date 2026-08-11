use crate::embedded_terminal::EmbeddedTerminal;
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);
const MAX_QUERY_SEQUENCE_LEN: usize = 4096;
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?6c";

#[derive(Debug, Clone, Copy)]
pub enum EmbeddedOutcome {
    ReturnedToLauncher,
    Exited(i32),
    Signaled(i32),
}

pub fn run(
    command: &[String],
    environment: &[(String, String)],
    working_dir: Option<&Path>,
    terminal: &mut Terminal,
    chrome: &crate::chrome::ChromeFrame,
) -> Result<EmbeddedOutcome> {
    if command.is_empty() {
        bail!("embedded action has an empty command");
    }

    let command_cstrings = command
        .iter()
        .map(|argument| CString::new(argument.as_str()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("embedded command contains a NUL byte")?;
    let working_dir_cstring = working_dir
        .map(|path| {
            CString::new(path.as_os_str().as_bytes())
                .context("embedded working directory contains a NUL byte")
        })
        .transpose()?;
    let environment_cstrings = environment
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

    let layout = chrome.layout();
    let (outer_columns, outer_rows) = terminal.size();
    let (columns, rows) = content_size(outer_columns, outer_rows, layout);
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
        return Err(io::Error::last_os_error()).context("could not create embedded PTY");
    }

    if pid == 0 {
        exec_child(
            &command_cstrings,
            &environment_cstrings,
            working_dir_cstring.as_ref(),
        );
    }

    set_nonblocking(master)?;
    let outcome = relay(master, pid, terminal, (columns, rows), chrome, layout);
    if outcome.is_err() {
        terminate_child(pid);
    }
    unsafe {
        libc::close(master);
    }
    outcome
}

fn exec_child(
    command: &[CString],
    environment: &[(CString, CString)],
    working_dir: Option<&CString>,
) -> ! {
    if let Some(working_dir) = working_dir
        && unsafe { libc::chdir(working_dir.as_ptr()) } != 0
    {
        unsafe { libc::_exit(127) };
    }
    for (key, value) in environment {
        unsafe {
            libc::setenv(key.as_ptr(), value.as_ptr(), 1);
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

fn relay(
    master: RawFd,
    pid: libc::pid_t,
    terminal: &mut Terminal,
    mut last_size: (u16, u16),
    chrome: &crate::chrome::ChromeFrame,
    layout: crate::chrome::ChromeLayout,
) -> Result<EmbeddedOutcome> {
    let mut input = InputRelay::default();
    let mut responder = TerminalResponder::default();
    let mut screen = EmbeddedTerminal::new(last_size.0, last_size.1);
    render_embedded(terminal, chrome, &screen)?;

    loop {
        if input.escape_expired() {
            terminate_child(pid);
            return Ok(EmbeddedOutcome::ReturnedToLauncher);
        }
        if let Some(status) = wait_status(pid, true)? {
            drain_output(master, &mut responder, &mut screen, terminal, chrome)?;
            return Ok(decode_status(status));
        }

        let outer_size = terminal.size();
        let current_size = content_size(outer_size.0, outer_size.1, layout);
        if current_size != last_size {
            screen.resize(current_size.0, current_size.1);
            resize_pty(master, pid, current_size)?;
            last_size = current_size;
            render_embedded(terminal, chrome, &screen)?;
        }

        let mut descriptors = [
            libc::pollfd {
                fd: terminal.input_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: master,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, 100) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            terminate_child(pid);
            return Err(error).context("could not poll embedded PTY");
        }

        if descriptors[0].revents & libc::POLLIN != 0
            && read_input_and_forward(terminal.input_fd(), &mut input, master)?
        {
            terminate_child(pid);
            return Ok(EmbeddedOutcome::ReturnedToLauncher);
        }

        if descriptors[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
            && drain_output(master, &mut responder, &mut screen, terminal, chrome)?
            && let Some(status) = wait_status(pid, true)?
        {
            return Ok(decode_status(status));
        }
    }
}

fn read_input_and_forward(input_fd: RawFd, relay: &mut InputRelay, master: RawFd) -> Result<bool> {
    let mut buffer = [0_u8; 4096];
    let count = unsafe { libc::read(input_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if count == 0 {
        return Ok(true);
    }
    if count < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error).context("could not read embedded input");
    }

    relay.push(&buffer[..count as usize], master)
}

#[derive(Default)]
struct InputRelay {
    pending: Vec<u8>,
    escape_started: Option<Instant>,
}

impl InputRelay {
    fn push(&mut self, bytes: &[u8], master: RawFd) -> Result<bool> {
        self.pending.extend_from_slice(bytes);
        self.process(master)
    }

    fn escape_expired(&self) -> bool {
        self.escape_started
            .is_some_and(|started| started.elapsed() >= ESCAPE_TIMEOUT)
    }

    fn process(&mut self, master: RawFd) -> Result<bool> {
        loop {
            if self.pending.is_empty() {
                self.escape_started = None;
                return Ok(false);
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
                return Ok(false);
            }

            if matches!(self.pending[1], b'[' | b'O') {
                if let Some(end) = escape_sequence_end(&self.pending[2..]) {
                    let count = 3 + end;
                    write_fd(master, &self.pending[..count])?;
                    self.pending.drain(..count);
                    self.escape_started = None;
                    continue;
                }
                self.escape_started.get_or_insert_with(Instant::now);
                return Ok(false);
            }

            // An ESC followed by a non-CSI byte is an Alt-style sequence.
            // It is not the bare launcher escape, so pass both bytes through.
            write_fd(master, &self.pending[..2])?;
            self.pending.drain(..2);
            self.escape_started = None;
        }
    }
}

fn escape_sequence_end(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|byte| (0x40..=0x7e).contains(byte))
}

fn drain_output(
    master: RawFd,
    responder: &mut TerminalResponder,
    screen: &mut EmbeddedTerminal,
    terminal: &mut Terminal,
    chrome: &crate::chrome::ChromeFrame,
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
            if error.kind() == io::ErrorKind::WouldBlock
                || error.kind() == io::ErrorKind::Interrupted
            {
                break;
            }
            // A closed PTY commonly reports EIO instead of a zero-byte EOF.
            // Treat other terminal-read failures as closed as well; the child
            // status is checked by the relay loop before returning.
            reached_eof = true;
            break;
        }
        let output = &buffer[..count as usize];
        for _ in 0..responder.primary_device_attribute_queries(output) {
            write_fd(master, PRIMARY_DEVICE_ATTRIBUTES)?;
        }
        screen.feed(output);
        render_embedded(terminal, chrome, screen)?;
    }
    Ok(reached_eof)
}

fn content_size(
    outer_columns: u16,
    outer_rows: u16,
    layout: crate::chrome::ChromeLayout,
) -> (u16, u16) {
    (
        layout.content_width(outer_columns as usize).max(1) as u16,
        layout.content_rows(outer_rows as usize).max(1) as u16,
    )
}

fn render_embedded(
    terminal: &mut Terminal,
    chrome: &crate::chrome::ChromeFrame,
    screen: &EmbeddedTerminal,
) -> Result<()> {
    terminal.draw(|frame| {
        let area = chrome.render_chrome(frame);
        frame.render_widget(screen.widget(), area);
        if let Some((column, row)) = screen.cursor()
            && column < area.width as usize
            && row < area.height as usize
        {
            frame.set_cursor_position((
                area.x.saturating_add(column as u16),
                area.y.saturating_add(row as u16),
            ));
        }
    })
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

fn write_fd(fd: RawFd, bytes: &[u8]) -> Result<()> {
    let mut offset = 0;
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
                thread::sleep(Duration::from_millis(1));
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
    unsafe {
        libc::kill(-pid, libc::SIGWINCH);
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect embedded PTY");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error()).context("could not configure embedded PTY");
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

fn terminate_child(pid: libc::pid_t) {
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if matches!(wait_status(pid, true), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
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
    use super::TerminalResponder;

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
}
