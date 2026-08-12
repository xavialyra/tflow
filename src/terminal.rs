use anyhow::{Context, Result, bail};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal as RatatuiTerminal};
use ratatui_image::FontSize;
use ratatui_image::picker::ProtocolType;
use ratatui_image::picker::cap_parser::{Parser, QueryStdioOptions, Response};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

pub struct Terminal {
    input_fd: libc::c_int,
    output_fd: libc::c_int,
    original: libc::termios,
    raw: libc::termios,
    renderer: RatatuiTerminal<CrosstermBackend<File>>,
    image_picker: Option<ratatui_image::picker::Picker>,
    active: bool,
    screen_active: bool,
}

impl Terminal {
    pub fn enter() -> Result<Self> {
        Self::enter_with_fds(io::stdin().as_raw_fd(), io::stdout().as_raw_fd())
    }

    pub fn enter_with_fds(input_fd: libc::c_int, output_fd: libc::c_int) -> Result<Self> {
        if unsafe { libc::isatty(input_fd) } != 1 {
            bail!("tui-launcher needs to run inside a terminal");
        }

        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(input_fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error()).context("could not read terminal settings");
        }

        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(input_fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error()).context("could not enable raw terminal mode");
        }

        let output = duplicate_fd(output_fd)?;
        let renderer = RatatuiTerminal::new(CrosstermBackend::new(output))
            .context("could not initialize Ratatui terminal backend")?;
        let mut terminal = Self {
            input_fd,
            output_fd,
            original,
            raw,
            renderer,
            image_picker: None,
            active: true,
            screen_active: false,
        };
        terminal.resume_screen()?;
        Ok(terminal)
    }

    pub fn leave(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }

        self.write_output(b"\x1b[?25h\x1b[?1049l\x1b[0m\x1b[2J\x1b[H")
            .context("could not restore terminal screen")?;
        self.screen_active = false;

        if unsafe { libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original) } != 0 {
            return Err(io::Error::last_os_error()).context("could not restore terminal settings");
        }
        self.active = false;
        Ok(())
    }

    pub fn reenter(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }

        if unsafe { libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.raw) } != 0 {
            return Err(io::Error::last_os_error())
                .context("could not re-enable terminal settings");
        }
        self.active = true;
        self.screen_active = false;
        self.reset_renderer()?;
        self.resume_screen()
    }

    pub fn resume_screen(&mut self) -> Result<()> {
        if !self.active || self.screen_active {
            return Ok(());
        }
        self.write_output(b"\x1b[?25h\x1b[?1049l\x1b[0m\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l")
            .context("could not resume launcher screen")?;
        self.screen_active = true;
        Ok(())
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> Result<()> {
        self.renderer
            .draw(render)
            .context("could not draw Ratatui frame")?;
        Ok(())
    }

    pub fn input_fd(&self) -> libc::c_int {
        self.input_fd
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

    pub(crate) fn image_picker(&mut self) -> ratatui_image::picker::Picker {
        if let Some(picker) = &self.image_picker {
            return picker.clone();
        }

        let options = ratatui_image::picker::cap_parser::QueryStdioOptions {
            timeout: Duration::from_millis(250),
            ..Default::default()
        };
        let picker = query_image_picker(self.input_fd, self.output_fd, options)
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
        self.image_picker = Some(picker.clone());
        picker
    }

    pub fn read_input(&self, timeout_ms: i32) -> Result<Vec<u8>> {
        let mut descriptor = libc::pollfd {
            fd: self.input_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(Vec::new());
            }
            return Err(error).context("could not poll terminal input");
        }
        if result == 0 || descriptor.revents & libc::POLLIN == 0 {
            return Ok(Vec::new());
        }

        let mut buffer = [0_u8; 4096];
        let count = unsafe { libc::read(self.input_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error()).context("could not read terminal input");
        }
        Ok(buffer[..count as usize].to_vec())
    }

    fn reset_renderer(&mut self) -> Result<()> {
        let output = duplicate_fd(self.output_fd)?;
        self.renderer = RatatuiTerminal::new(CrosstermBackend::new(output))
            .context("could not reset Ratatui terminal backend")?;
        Ok(())
    }

    fn write_output(&self, bytes: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            let count = unsafe {
                libc::write(
                    self.output_fd,
                    bytes[offset..].as_ptr().cast(),
                    bytes.len() - offset,
                )
            };
            if count > 0 {
                offset += count as usize;
                continue;
            }
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error).context("could not write terminal output");
            }
            bail!("could not write terminal output: write returned zero");
        }
        Ok(())
    }
}

fn duplicate_fd(fd: libc::c_int) -> Result<File> {
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error()).context("could not duplicate terminal output");
    }
    Ok(unsafe { File::from_raw_fd(duplicate) })
}

fn query_image_picker(
    input_fd: libc::c_int,
    output_fd: libc::c_int,
    options: QueryStdioOptions,
) -> Result<ratatui_image::picker::Picker> {
    let is_tmux = std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|term| term == "tmux");
    let timeout = options.timeout;
    let query = Parser::query(is_tmux, options);
    write_fd(output_fd, query.as_bytes())?;

    let deadline = Instant::now() + timeout;
    let mut parser = Parser::new();
    let mut responses = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("terminal image capability query timed out");
        }
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: input_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let polled = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if polled < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("could not read terminal image capabilities");
        }
        if polled == 0 {
            bail!("terminal image capability query timed out");
        }
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            continue;
        }

        let mut buffer = [0_u8; 256];
        let count = unsafe { libc::read(input_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("could not read terminal image capabilities");
        }
        if count == 0 {
            bail!("terminal image capability query reached end of input");
        }
        for byte in &buffer[..count as usize] {
            responses.extend(parser.push(char::from(*byte)));
        }
        if responses
            .iter()
            .any(|response| matches!(response, Response::Status))
        {
            break;
        }
    }

    let font_size = responses
        .iter()
        .find_map(|response| match response {
            Response::CellSize(Some((width, height))) => Some(FontSize::new(*width, *height)),
            _ => None,
        })
        .or_else(|| font_size_from_fd(output_fd));
    let Some(font_size) = font_size else {
        return Ok(ratatui_image::picker::Picker::halfblocks());
    };

    #[allow(deprecated)]
    let mut picker = ratatui_image::picker::Picker::from_fontsize(font_size);
    let protocol = responses
        .iter()
        .find_map(|response| match response {
            Response::Kitty => Some(ProtocolType::Kitty),
            Response::Sixel => Some(ProtocolType::Sixel),
            _ => None,
        })
        .or_else(environment_image_protocol)
        .unwrap_or(ProtocolType::Halfblocks);
    picker.set_protocol_type(protocol);
    Ok(picker)
}

fn write_fd(fd: libc::c_int, bytes: &[u8]) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let count =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if count > 0 {
            offset += count as usize;
        } else if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("could not query terminal image capabilities");
        } else {
            bail!("could not query terminal image capabilities: write returned zero");
        }
    }
    Ok(())
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

fn environment_image_protocol() -> Option<ProtocolType> {
    if std::env::var("ITERM_SESSION_ID").is_ok_and(|value| !value.is_empty())
        || std::env::var("WEZTERM_EXECUTABLE").is_ok_and(|value| !value.is_empty())
    {
        return Some(ProtocolType::Iterm2);
    }
    let term_program = std::env::var("TERM_PROGRAM").ok()?;
    if term_program.contains("iTerm")
        || term_program.contains("WezTerm")
        || term_program.contains("mintty")
        || term_program.contains("vscode")
        || term_program.contains("Tabby")
        || term_program.contains("Hyper")
        || term_program.contains("rio")
        || term_program.contains("Bobcat")
        || term_program.contains("WarpTerminal")
        || std::env::var("LC_TERMINAL").is_ok_and(|term| term.contains("iTerm"))
    {
        Some(ProtocolType::Iterm2)
    } else {
        None
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = self.write_output(b"\x1b[?25h\x1b[?1049l\x1b[0m");
        unsafe {
            libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original);
        }
    }
}
