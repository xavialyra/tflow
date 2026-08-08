use crate::terminal::Terminal;
use crate::vt::VirtualTerminal;
use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(40);

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
    terminal: &Terminal,
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
    let outcome = relay(master, pid, terminal, (columns, rows), chrome);
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
    terminal: &Terminal,
    mut last_size: (u16, u16),
    chrome: &crate::chrome::ChromeFrame,
) -> Result<EmbeddedOutcome> {
    let mut input = InputRelay::default();
    let mut screen = VirtualTerminal::new(last_size.0, last_size.1);
    render_embedded(terminal, chrome, &screen)?;

    loop {
        if input.escape_expired() {
            terminate_child(pid);
            return Ok(EmbeddedOutcome::ReturnedToLauncher);
        }
        if let Some(status) = wait_status(pid, true)? {
            drain_output(master, &mut screen, terminal, chrome)?;
            return Ok(decode_status(status));
        }

        let outer_size = terminal.size();
        let current_size = content_size(outer_size.0, outer_size.1);
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
            && drain_output(master, &mut screen, terminal, chrome)?
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
    screen: &mut VirtualTerminal,
    terminal: &Terminal,
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
        screen.feed(&buffer[..count as usize]);
        render_embedded(terminal, chrome, screen)?;
    }
    Ok(reached_eof)
}

fn content_size(outer_columns: u16, outer_rows: u16) -> (u16, u16) {
    (outer_columns.max(1), outer_rows.saturating_sub(2).max(1))
}

fn render_embedded(
    terminal: &Terminal,
    chrome: &crate::chrome::ChromeFrame,
    screen: &VirtualTerminal,
) -> Result<()> {
    let (outer_columns, outer_rows) = terminal.size();
    let outer_columns = outer_columns as usize;
    let outer_rows = outer_rows as usize;
    let inner_width = outer_columns.max(1);
    let inner_rows = outer_rows.saturating_sub(2).max(1);
    let mut stdout = io::stdout().lock();

    stdout.write_all(b"\x1b[?25l")?;
    write_line(&mut stdout, 1, &chrome.header, outer_columns, true)?;
    for row in 0..inner_rows {
        write_line(
            &mut stdout,
            2 + row,
            &screen.row_text(row),
            outer_columns,
            false,
        )?;
    }
    write_line(
        &mut stdout,
        outer_rows.max(1),
        &chrome.footer,
        outer_columns,
        false,
    )?;

    let (cursor_x, cursor_y, visible) = screen.cursor();
    if visible {
        let row = (2 + cursor_y.min(inner_rows.saturating_sub(1))).min(outer_rows.max(1));
        let column = cursor_x.min(inner_width.saturating_sub(1)) + 1;
        write!(stdout, "\x1b[{};{}H\x1b[?25h", row, column)?;
    } else {
        stdout.write_all(b"\x1b[?25l")?;
    }
    stdout.flush().context("could not draw embedded screen")
}

fn write_line(
    stdout: &mut impl Write,
    row: usize,
    text: &str,
    width: usize,
    heading: bool,
) -> Result<()> {
    let text = clip_line(text, width);
    write!(stdout, "\x1b[{};1H\x1b[K", row)?;
    if heading {
        write!(stdout, "\x1b[1;36m{}\x1b[0m", text)?;
    } else {
        stdout.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn clip_line(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output
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
