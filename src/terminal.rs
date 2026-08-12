use anyhow::{Context, Result, bail};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal as RatatuiTerminal};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::Duration;

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
    options: ratatui_image::picker::cap_parser::QueryStdioOptions,
) -> Result<ratatui_image::picker::Picker> {
    let saved_stdin = duplicate_fd(libc::STDIN_FILENO)?;
    let saved_stdout = duplicate_fd(libc::STDOUT_FILENO)?;

    let result = (|| {
        redirect_fd(input_fd, libc::STDIN_FILENO)?;
        redirect_fd(output_fd, libc::STDOUT_FILENO)?;
        ratatui_image::picker::Picker::from_query_stdio_with_options(options)
            .context("could not query terminal image capabilities")
    })();

    let stdin_restore = restore_fd(&saved_stdin, libc::STDIN_FILENO);
    let stdout_restore = restore_fd(&saved_stdout, libc::STDOUT_FILENO);
    match (result, stdin_restore, stdout_restore) {
        (Ok(picker), Ok(()), Ok(())) => Ok(picker),
        (Err(error), _, _) => Err(error),
        (Ok(_), Err(error), _) | (Ok(_), Ok(()), Err(error)) => Err(error),
    }
}

fn redirect_fd(source: libc::c_int, target: libc::c_int) -> Result<()> {
    if unsafe { libc::dup2(source, target) } < 0 {
        return Err(io::Error::last_os_error()).context("could not redirect terminal fd");
    }
    Ok(())
}

fn restore_fd(saved: &File, target: libc::c_int) -> Result<()> {
    if unsafe { libc::dup2(saved.as_raw_fd(), target) } < 0 {
        return Err(io::Error::last_os_error()).context("could not restore standard fd");
    }
    Ok(())
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
