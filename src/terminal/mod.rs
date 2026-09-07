mod sanitize;

pub(crate) use sanitize::{sanitize_terminal_text, sanitize_text};

use crate::lifecycle::CancellationToken;
use anyhow::{Context, Result, bail};
use base64::{Engine as _, encoded_len, engine::general_purpose::STANDARD};
use image::DynamicImage;
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal as RatatuiTerminal};
use ratatui_image::FontSize;
use ratatui_image::picker::ProtocolType;
use ratatui_image::protocol::{
    StatefulProtocol, StatefulProtocolType, halfblocks::Halfblocks, iterm2::Iterm2,
    kitty::StatefulKitty, sixel::Sixel,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ImageProtocol {
    #[default]
    Halfblocks,
    Kitty,
    Sixel,
    Iterm2,
}

const RESTORE_SCREEN: &[u8] = b"\x1b[?25h\x1b[?1049l\x1b[0m\x1b[2J\x1b[H";
const OUTPUT_POLL_INTERVAL_MS: i32 = 50;
const RESTORE_OUTPUT_DEADLINE: Duration = Duration::from_millis(100);
static KITTY_IMAGE_ID: OnceLock<AtomicU32> = OnceLock::new();

struct TerminalInitGuard {
    input_fd: libc::c_int,
    output_fd: libc::c_int,
    original: libc::termios,
    armed: bool,
}

impl TerminalInitGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalInitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = write_fd(
            self.output_fd,
            RESTORE_SCREEN,
            None,
            Some(Instant::now() + RESTORE_OUTPUT_DEADLINE),
        );
        unsafe {
            libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original);
        }
    }
}

pub(crate) enum InputRead {
    Timeout,
    Data(Vec<u8>),
    Eof,
}

struct TerminalWriter {
    file: File,
    cancellation: CancellationToken,
    discard: Arc<AtomicBool>,
}

impl TerminalWriter {
    fn new(file: File, cancellation: CancellationToken, discard: Arc<AtomicBool>) -> Self {
        Self {
            file,
            cancellation,
            discard,
        }
    }
}

impl Write for TerminalWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.discard.load(Ordering::Acquire) {
            return Ok(bytes.len());
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            wait_for_output(self.file.as_raw_fd(), Some(&self.cancellation), None)?;
            match self.file.write(bytes) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.discard.load(Ordering::Acquire) {
            return Ok(());
        }
        if self.cancellation.is_cancelled() {
            return Err(shutdown_error());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImagePicker {
    font_size: FontSize,
    protocol: ProtocolType,
    is_tmux: bool,
}

impl PartialEq for ImagePicker {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint() == other.fingerprint()
    }
}

impl Eq for ImagePicker {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImagePickerFingerprint {
    font_width: u16,
    font_height: u16,
    protocol: u8,
    is_tmux: bool,
}

impl ImagePicker {
    pub(crate) fn new_resize_protocol(self, image: DynamicImage) -> StatefulProtocol {
        let protocol = match self.protocol {
            ProtocolType::Halfblocks => StatefulProtocolType::Halfblocks(Halfblocks::default()),
            ProtocolType::Sixel => StatefulProtocolType::Sixel(Sixel {
                is_tmux: self.is_tmux,
                ..Sixel::default()
            }),
            ProtocolType::Kitty => {
                StatefulProtocolType::Kitty(StatefulKitty::new(next_kitty_image_id(), self.is_tmux))
            }
            ProtocolType::Iterm2 => StatefulProtocolType::ITerm2(Iterm2 {
                is_tmux: self.is_tmux,
                ..Iterm2::default()
            }),
        };
        StatefulProtocol::new(image, self.font_size, None, protocol)
    }

    pub(crate) fn fingerprint(self) -> ImagePickerFingerprint {
        ImagePickerFingerprint {
            font_width: self.font_size.width,
            font_height: self.font_size.height,
            protocol: match self.protocol {
                ProtocolType::Halfblocks => 0,
                ProtocolType::Kitty => 1,
                ProtocolType::Sixel => 2,
                ProtocolType::Iterm2 => 3,
            },
            is_tmux: self.is_tmux,
        }
    }

    #[cfg(test)]
    pub(crate) fn test_halfblocks() -> Self {
        Self {
            font_size: FontSize::new(10, 20),
            protocol: ProtocolType::Halfblocks,
            is_tmux: false,
        }
    }
}

pub struct Terminal {
    input_fd: libc::c_int,
    output_fd: libc::c_int,
    _output: File,
    original: libc::termios,
    launcher_process_group: Option<libc::pid_t>,
    renderer: RatatuiTerminal<CrosstermBackend<TerminalWriter>>,
    renderer_discard: Arc<AtomicBool>,
    image_picker: ImagePicker,
    cancellation: CancellationToken,
    active: bool,
    screen_active: bool,
    #[cfg(test)]
    fail_next_foreground_resume: bool,
}

impl Terminal {
    pub fn enter_with_fds_and_cancellation(
        input_fd: libc::c_int,
        output_fd: libc::c_int,
        image_protocol: ImageProtocol,
        cancellation: CancellationToken,
    ) -> Result<Self> {
        if unsafe { libc::isatty(input_fd) } != 1 {
            bail!("tui-launcher needs to run inside a terminal");
        }

        set_fd_cloexec(input_fd)?;
        if output_fd != input_fd {
            set_fd_cloexec(output_fd)?;
        }
        let launcher_process_group = launcher_foreground_process_group(input_fd);
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(input_fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error()).context("could not read terminal settings");
        }

        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(input_fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("could not enable raw terminal mode");
        }
        let mut rollback = TerminalInitGuard {
            input_fd,
            output_fd,
            original,
            armed: true,
        };

        let output = duplicate_fd(output_fd)?;
        set_nonblocking_fd(output.as_raw_fd())?;
        let renderer_output = duplicate_fd(output.as_raw_fd())?;
        let renderer_discard = Arc::new(AtomicBool::new(false));
        let renderer = RatatuiTerminal::new(CrosstermBackend::new(TerminalWriter::new(
            renderer_output,
            cancellation.clone(),
            Arc::clone(&renderer_discard),
        )))
        .context("could not initialize Ratatui terminal backend")?;
        let mut terminal = Self {
            input_fd,
            output_fd: output.as_raw_fd(),
            _output: output,
            original,
            launcher_process_group,
            renderer,
            renderer_discard,
            image_picker: picker_from_protocol(output_fd, image_protocol),
            cancellation,
            active: true,
            screen_active: false,
            #[cfg(test)]
            fail_next_foreground_resume: false,
        };
        terminal.resume_screen()?;
        rollback.disarm();
        Ok(terminal)
    }

    pub fn leave(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        let (screen_result, settings_result) = self.restore_terminal_state();
        self.active = false;
        self.prepare_renderer_for_drop();
        screen_result.and(settings_result)
    }

    /// Yield the terminal to a foreground child while retaining the ability
    /// to resume the launcher afterward.
    pub(crate) fn suspend_for_foreground(&mut self) -> Result<()> {
        if !self.active {
            bail!("launcher terminal is not active");
        }
        let (screen_result, settings_result) = self.restore_terminal_state();
        screen_result.and(settings_result)
    }

    /// Configure a foreground child to use the controlling terminal rather
    /// than the launcher's possibly redirected standard streams.
    pub(crate) fn configure_foreground_command(&self, command: &mut Command) -> io::Result<bool> {
        if self.launcher_process_group.is_none() {
            return Ok(false);
        }
        command.stdin(Stdio::from(open_controlling_terminal()?));
        command.stdout(Stdio::from(open_controlling_terminal()?));
        command.stderr(Stdio::from(open_controlling_terminal()?));
        Ok(true)
    }

    pub(crate) fn foreground_terminal_fd(&self) -> Option<RawFd> {
        self.launcher_process_group.map(|_| self.input_fd)
    }

    /// Restore foreground ownership to the launcher's process group.
    pub(crate) fn reclaim_foreground_process(&self) -> io::Result<()> {
        if let Some(process_group) = self.launcher_process_group {
            set_terminal_foreground_process_group(self.input_fd, process_group)?;
        }
        Ok(())
    }

    /// Resume raw mode and the launcher screen after a foreground child.
    pub(crate) fn resume_after_foreground(&mut self) -> Result<()> {
        if !self.active {
            bail!("launcher terminal is not active");
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_foreground_resume) {
            bail!("injected launcher terminal resume failure");
        }
        let mut raw = self.original;
        unsafe { libc::cfmakeraw(&mut raw) };
        let settings_result =
            if unsafe { libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &raw) } != 0 {
                Err(io::Error::last_os_error()).context("could not resume terminal settings")
            } else {
                Ok(())
            };
        let screen_result = write_fd(
            self.output_fd,
            b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l",
            None,
            Some(Instant::now() + RESTORE_OUTPUT_DEADLINE),
        )
        .context("could not resume launcher screen");
        if screen_result.is_ok() {
            self.screen_active = true;
        }
        self.invalidate_renderer();
        settings_result.and(screen_result)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_foreground_resume(&mut self) {
        self.fail_next_foreground_resume = true;
    }

    pub fn resume_screen(&mut self) -> Result<()> {
        if !self.active || self.screen_active {
            return Ok(());
        }
        self.write_output(b"\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l")
            .context("could not resume launcher screen")?;
        self.screen_active = true;
        Ok(())
    }

    fn restore_terminal_state(&mut self) -> (Result<()>, Result<()>) {
        let screen_result = write_fd(
            self.output_fd,
            RESTORE_SCREEN,
            None,
            Some(Instant::now() + RESTORE_OUTPUT_DEADLINE),
        )
        .context("could not restore terminal screen");
        self.screen_active = false;
        let settings_result =
            if unsafe { libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original) } != 0 {
                Err(io::Error::last_os_error()).context("could not restore terminal settings")
            } else {
                Ok(())
            };
        (screen_result, settings_result)
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(shutdown_error()).context("could not draw Ratatui frame");
        }
        self.renderer
            .draw(render)
            .context("could not draw Ratatui frame")?;
        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(shutdown_error()).context("could not clear Ratatui terminal");
        }
        self.write_output(b"\x1b[2J\x1b[H")
            .context("could not clear terminal screen")?;
        self.invalidate_renderer();
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        let mut window: libc::winsize = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::ioctl(self.input_fd, libc::TIOCGWINSZ, &mut window) };
        if result == 0 && window.ws_col > 0 && window.ws_row > 0 {
            (window.ws_col, window.ws_row)
        } else {
            (80, 24)
        }
    }

    pub(crate) fn image_picker(&self) -> Option<ImagePicker> {
        let mut picker = self.image_picker;
        if let Some(font_size) = font_size_from_fd(self.output_fd) {
            picker.font_size = font_size;
        }
        Some(picker)
    }

    pub(crate) fn copy_to_clipboard(&self, value: &str) -> Result<()> {
        let sequence = osc52_sequence(value, self.image_picker.is_tmux)
            .context("could not encode capture output for the terminal clipboard")?;
        self.write_output(&sequence)
            .context("could not copy capture output to the terminal clipboard")
    }

    pub(crate) fn read_input(&mut self, timeout_ms: i32) -> Result<InputRead> {
        let mut descriptor = libc::pollfd {
            fd: self.input_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(InputRead::Timeout);
            }
            return Err(error).context("could not poll terminal input");
        }
        if result == 0 {
            return Ok(InputRead::Timeout);
        }
        self.read_input_ready(descriptor.revents)
    }

    pub(crate) fn read_input_ready(&mut self, revents: libc::c_short) -> Result<InputRead> {
        if revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
            && revents & libc::POLLIN == 0
        {
            return Ok(InputRead::Eof);
        }
        let mut buffer = [0_u8; 4096];
        let count = unsafe { libc::read(self.input_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count == 0 {
            return Ok(InputRead::Eof);
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted
                || error.kind() == io::ErrorKind::WouldBlock
            {
                return Ok(InputRead::Timeout);
            }
            if error.raw_os_error() == Some(libc::EIO) {
                return Ok(InputRead::Eof);
            }
            return Err(error).context("could not read terminal input");
        }
        Ok(InputRead::Data(buffer[..count as usize].to_vec()))
    }

    fn write_output(&self, bytes: &[u8]) -> Result<()> {
        write_fd(self.output_fd, bytes, Some(&self.cancellation), None)
    }

    fn invalidate_renderer(&mut self) {
        self.renderer.swap_buffers();
        self.renderer.swap_buffers();
    }

    fn prepare_renderer_for_drop(&mut self) {
        self.renderer_discard.store(true, Ordering::Release);
        let _ = self.renderer.show_cursor();
    }
}

fn osc52_sequence(value: &str, is_tmux: bool) -> Result<Vec<u8>> {
    let payload_len = encoded_len(value.len(), true).context("capture output is too large")?;
    let wrapper_len = if is_tmux { 10 } else { 0 };
    let capacity = payload_len
        .checked_add(8 + wrapper_len)
        .context("clipboard sequence is too large")?;
    let mut sequence = Vec::with_capacity(capacity);
    if is_tmux {
        sequence.extend_from_slice(b"\x1bPtmux;\x1b");
    }
    sequence.extend_from_slice(b"\x1b]52;c;");
    let payload_start = sequence.len();
    sequence.resize(payload_start + payload_len, 0);
    let written = STANDARD
        .encode_slice(value.as_bytes(), &mut sequence[payload_start..])
        .context("could not base64 encode capture output")?;
    debug_assert_eq!(written, payload_len);
    sequence.push(0x07);
    if is_tmux {
        sequence.extend_from_slice(b"\x1b\\");
    }
    Ok(sequence)
}

fn duplicate_fd(fd: libc::c_int) -> Result<File> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error()).context("could not duplicate terminal output");
    }
    Ok(unsafe { File::from_raw_fd(duplicate) })
}

fn open_controlling_terminal() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("/dev/tty")
}

fn launcher_foreground_process_group(fd: RawFd) -> Option<libc::pid_t> {
    let process_group = unsafe { libc::getpgrp() };
    let terminal_foreground_group = unsafe { libc::tcgetpgrp(fd) };
    (process_group > 0 && terminal_foreground_group == process_group).then_some(process_group)
}

pub(crate) fn set_terminal_foreground_process_group(
    fd: RawFd,
    process_group: libc::pid_t,
) -> io::Result<()> {
    let mut signal = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    unsafe {
        libc::sigemptyset(&mut signal);
        libc::sigaddset(&mut signal, libc::SIGTTOU);
    }
    let mut previous = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    let blocked = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signal, &mut previous) };
    if blocked != 0 {
        return Err(io::Error::from_raw_os_error(blocked));
    }

    let handoff = loop {
        if unsafe { libc::tcsetpgrp(fd, process_group) } == 0 {
            break Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            break Err(error);
        }
    };
    let restored =
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
    if restored != 0 {
        return Err(io::Error::from_raw_os_error(restored));
    }
    handoff
}

fn set_fd_cloexec(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect terminal fd flags");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error()).context("could not set terminal fd close-on-exec");
    }
    Ok(())
}

fn set_nonblocking_fd(fd: RawFd) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error()).context("could not inspect terminal output flags");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error())
            .context("could not set terminal output nonblocking");
    }
    Ok(())
}

fn shutdown_error() -> io::Error {
    io::Error::other("launcher shutdown requested")
}

fn wait_for_output(
    fd: RawFd,
    cancellation: Option<&CancellationToken>,
    deadline: Option<Instant>,
) -> io::Result<()> {
    loop {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(shutdown_error());
        }
        let timeout = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "terminal output deadline expired",
                    ));
                }
                remaining.as_millis().min(OUTPUT_POLL_INTERVAL_MS as u128) as i32
            }
            None => OUTPUT_POLL_INTERVAL_MS,
        };
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            continue;
        }
        if descriptor.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal output fd is invalid",
            ));
        }
        if descriptor.revents & (libc::POLLOUT | libc::POLLERR | libc::POLLHUP) != 0 {
            return Ok(());
        }
    }
}

fn write_fd(
    fd: RawFd,
    bytes: &[u8],
    cancellation: Option<&CancellationToken>,
    deadline: Option<Instant>,
) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        wait_for_output(fd, cancellation, deadline)?;
        let count =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if count > 0 {
            offset += count as usize;
        } else if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted
                || error.kind() == io::ErrorKind::WouldBlock
            {
                continue;
            }
            return Err(error).context("could not write terminal output");
        } else {
            bail!("could not write terminal output: write returned zero");
        }
    }
    Ok(())
}

fn next_kitty_image_id() -> u32 {
    KITTY_IMAGE_ID
        .get_or_init(|| AtomicU32::new(random_kitty_image_seed()))
        .fetch_add(1, Ordering::Relaxed)
}

fn random_kitty_image_seed() -> u32 {
    let mut bytes = [0_u8; 4];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok()
    {
        return u32::from_ne_bytes(bytes);
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mixed = nanos ^ (std::process::id() as u64).rotate_left(17);
    (mixed as u32) ^ ((mixed >> 32) as u32)
}

fn picker_from_protocol(output_fd: libc::c_int, image_protocol: ImageProtocol) -> ImagePicker {
    ImagePicker {
        font_size: font_size_from_fd(output_fd).unwrap_or(FontSize::new(10, 20)),
        protocol: match image_protocol {
            ImageProtocol::Halfblocks => ProtocolType::Halfblocks,
            ImageProtocol::Kitty => ProtocolType::Kitty,
            ImageProtocol::Sixel => ProtocolType::Sixel,
            ImageProtocol::Iterm2 => ProtocolType::Iterm2,
        },
        is_tmux: tmux_environment(),
    }
}

fn tmux_environment() -> bool {
    tmux_environment_for(
        std::env::var_os("TMUX").is_some_and(|value| !value.is_empty()),
        &std::env::var("TERM").unwrap_or_default(),
        &std::env::var("TERM_PROGRAM").unwrap_or_default(),
    )
}

fn tmux_environment_for(has_tmux: bool, term: &str, term_program: &str) -> bool {
    has_tmux
        || term.to_ascii_lowercase().starts_with("tmux")
        || term_program.eq_ignore_ascii_case("tmux")
}

fn font_size_from_fd(fd: libc::c_int) -> Option<FontSize> {
    let mut window: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut window) } != 0
        || window.ws_xpixel == 0
        || window.ws_ypixel == 0
        || window.ws_col == 0
        || window.ws_row == 0
    {
        return None;
    }
    Some(FontSize::new(
        window.ws_xpixel / window.ws_col,
        window.ws_ypixel / window.ws_row,
    ))
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.active {
            let _ = write_fd(
                self.output_fd,
                RESTORE_SCREEN,
                None,
                Some(Instant::now() + RESTORE_OUTPUT_DEADLINE),
            );
            unsafe {
                libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original);
            }
        }
        self.prepare_renderer_for_drop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_termios_eq(left: &libc::termios, right: &libc::termios) {
        assert_eq!(left.c_iflag, right.c_iflag);
        assert_eq!(left.c_oflag, right.c_oflag);
        assert_eq!(left.c_cflag, right.c_cflag);
        assert_eq!(left.c_lflag, right.c_lflag);
        assert_eq!(left.c_cc, right.c_cc);
        assert_eq!(unsafe { libc::cfgetispeed(left) }, unsafe {
            libc::cfgetispeed(right)
        });
        assert_eq!(unsafe { libc::cfgetospeed(left) }, unsafe {
            libc::cfgetospeed(right)
        });
    }

    #[test]
    fn osc52_clipboard_sequence_encodes_utf8_and_wraps_for_tmux() {
        assert_eq!(
            osc52_sequence("hello", false).unwrap(),
            b"\x1b]52;c;aGVsbG8=\x07"
        );
        assert_eq!(
            osc52_sequence("hello", true).unwrap(),
            b"\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\"
        );
        assert_eq!(
            osc52_sequence("\u{4f60}", false).unwrap(),
            b"\x1b]52;c;5L2g\x07"
        );
    }

    #[test]
    fn tmux_wrapper_follows_caller_environment_without_mutating_it() {
        assert!(tmux_environment_for(true, "screen-256color", ""));
        assert!(tmux_environment_for(false, "tmux-256color", ""));
        assert!(tmux_environment_for(false, "screen-256color", "tmux"));
        assert!(!tmux_environment_for(false, "screen-256color", "kitty"));
    }

    #[test]
    fn terminal_writer_stops_when_output_is_full_and_cancelled() {
        let mut pipe = [-1; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        set_nonblocking_fd(pipe[1]).unwrap();
        let mut byte = [0_u8; 8192];
        loop {
            let count = unsafe { libc::write(pipe[1], byte.as_mut_ptr().cast(), byte.len()) };
            if count >= 0 {
                continue;
            }
            assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
            break;
        }
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let mut writer = TerminalWriter::new(
            unsafe { File::from_raw_fd(pipe[1]) },
            worker_cancellation,
            Arc::new(AtomicBool::new(false)),
        );
        let worker = std::thread::spawn(move || writer.write_all(b"blocked"));
        std::thread::sleep(Duration::from_millis(20));
        cancellation.cancel();
        let error = worker
            .join()
            .unwrap()
            .expect_err("full terminal output should observe cancellation");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        unsafe { libc::close(pipe[0]) };
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_input_reports_eof_after_pty_peer_closes() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let terminal = Terminal::enter_with_fds_and_cancellation(
            slave,
            slave,
            ImageProtocol::default(),
            CancellationToken::new(),
        )
        .unwrap();
        unsafe { libc::close(master) };
        let mut terminal = terminal;
        assert!(matches!(terminal.read_input(100).unwrap(), InputRead::Eof));
        drop(terminal);
        unsafe { libc::close(slave) };
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn screen_initialization_failure_after_raw_mode_restores_termios() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let output = unsafe { libc::open(c"/dev/full".as_ptr(), libc::O_WRONLY) };
        assert!(output >= 0);
        let mut before = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(slave, &mut before) }, 0);

        assert!(
            Terminal::enter_with_fds_and_cancellation(
                slave,
                output,
                ImageProtocol::default(),
                CancellationToken::new(),
            )
            .is_err()
        );
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(slave, &mut after) }, 0);
        assert_termios_eq(&before, &after);

        unsafe {
            libc::close(output);
            libc::close(master);
            libc::close(slave);
        }
    }

    #[test]
    fn initialization_failure_after_raw_mode_restores_termios() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut before = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(slave, &mut before) }, 0);

        let error = Terminal::enter_with_fds_and_cancellation(
            slave,
            -1,
            ImageProtocol::default(),
            CancellationToken::new(),
        )
        .err()
        .expect("invalid output fd should fail initialization");
        assert!(error.to_string().contains("terminal fd"));
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(slave, &mut after) }, 0);
        assert_termios_eq(&before, &after);

        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_exposes_image_picker_with_configured_protocol() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let terminal = Terminal::enter_with_fds_and_cancellation(
            slave,
            slave,
            ImageProtocol::Halfblocks,
            CancellationToken::new(),
        )
        .unwrap();

        let picker = terminal
            .image_picker()
            .expect("image picker should be available");
        assert_eq!(picker.protocol, ProtocolType::Halfblocks);

        drop(terminal);
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_clear_succeeds_and_resets_buffers() {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut terminal = Terminal::enter_with_fds_and_cancellation(
            slave,
            slave,
            ImageProtocol::Halfblocks,
            CancellationToken::new(),
        )
        .unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("initial text"),
                    frame.area(),
                );
            })
            .unwrap();

        assert!(terminal.clear().is_ok());

        terminal
            .draw(|frame| {
                frame.render_widget(
                    ratatui::widgets::Paragraph::new("after clear"),
                    frame.area(),
                );
            })
            .unwrap();

        drop(terminal);
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
    }
}
