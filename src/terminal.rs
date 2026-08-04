use anyhow::{Context, Result, bail};
use std::io::{self, Write};
use std::os::fd::AsRawFd;

pub struct Terminal {
    input_fd: libc::c_int,
    original: libc::termios,
    raw: libc::termios,
    active: bool,
    screen_active: bool,
}

impl Terminal {
    pub fn enter() -> Result<Self> {
        let input_fd = io::stdin().as_raw_fd();
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

        let mut terminal = Self {
            input_fd,
            original,
            raw,
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

        let mut stdout = io::stdout().lock();
        stdout
            .write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m\x1b[2J\x1b[H")
            .context("could not restore terminal screen")?;
        stdout.flush().context("could not flush terminal screen")?;
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
        self.resume_screen()
    }

    pub fn resume_screen(&mut self) -> Result<()> {
        if !self.active || self.screen_active {
            return Ok(());
        }
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m\x1b[?1049h\x1b[2J\x1b[H\x1b[?25l")
            .context("could not resume launcher screen")?;
        stdout.flush().context("could not flush launcher screen")?;
        self.screen_active = true;
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
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = io::stdout().write_all(b"\x1b[?25h\x1b[?1049l\x1b[0m");
        let _ = io::stdout().flush();
        unsafe {
            libc::tcsetattr(self.input_fd, libc::TCSAFLUSH, &self.original);
        }
    }
}
