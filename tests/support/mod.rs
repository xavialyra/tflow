#![allow(dead_code)]

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct DmenuProcess {
    pid: libc::pid_t,
    master: File,
    input: Option<File>,
    output: File,
    finished: bool,
}

impl Drop for DmenuProcess {
    fn drop(&mut self) {
        if !self.finished {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            }
        }
    }
}

pub struct LauncherProcess {
    pid: libc::pid_t,
    pub master: File,
    finished: bool,
}

impl Drop for LauncherProcess {
    fn drop(&mut self) {
        if !self.finished {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            }
        }
    }
}

pub struct RunResult {
    pub status: i32,
    pub stdout: Vec<u8>,
}

const TEST_CONFIG: &str = r#"
[test_items]
items = [{label = "Item", value = "value", metadata = {target = "core:capture"}}]
"#;

fn with_test_config(source: &str) -> String {
    format!("{TEST_CONFIG}\n{source}")
}

pub fn write_test_config(path: &Path, source: &str) -> std::io::Result<()> {
    fs::write(path, with_test_config(source))
}

pub fn run_dmenu(extra_args: &[&str], input: &[u8], keys: &[u8]) -> RunResult {
    run_dmenu_steps(extra_args, input, &[keys])
}

pub fn run_dmenu_steps(extra_args: &[&str], input: &[u8], key_steps: &[&[u8]]) -> RunResult {
    let config = fixture_config();
    let config = config.to_str().expect("fixture config path is not UTF-8");
    let mut args = vec!["--config", config, "dmenu:main"];
    args.extend_from_slice(extra_args);
    run_invocation_steps(&args, input, key_steps)
}

pub fn run_invocation(args: &[&str], input: &[u8], keys: &[u8]) -> RunResult {
    run_invocation_steps(args, input, &[keys])
}

pub fn run_invocation_steps(args: &[&str], input: &[u8], key_steps: &[&[u8]]) -> RunResult {
    let mut process = spawn(args);
    process
        .input
        .take()
        .expect("invocation input pipe is missing")
        .write_all(input)
        .expect("could not write invocation input");
    wait_for_ready(&process.master);
    for (index, keys) in key_steps.iter().enumerate() {
        process
            .master
            .write_all(keys)
            .expect("could not write invocation key input");
        process
            .master
            .flush()
            .expect("could not flush invocation key input");
        if index + 1 < key_steps.len() {
            wait_for_ready(&process.master);
        }
    }

    let status = wait_for_exit(&mut process);
    let mut stdout = Vec::new();
    process
        .output
        .read_to_end(&mut stdout)
        .expect("could not read invocation stdout");
    RunResult { status, stdout }
}

pub fn run_tty_invocation_with_redirected_stdout(args: &[&str], keys: &[u8]) -> RunResult {
    let mut process = spawn_tty_with_redirected_stdout(args);
    wait_for_ready(&process.master);
    process.master.write_all(keys).unwrap();
    process.master.flush().unwrap();
    let status = wait_for_exit(&mut process);
    let mut stdout = Vec::new();
    process.output.read_to_end(&mut stdout).unwrap();
    RunResult { status, stdout }
}

pub fn spawn_launcher(config: &Path) -> LauncherProcess {
    spawn_launcher_with_args(config, &[])
}

pub fn spawn_launcher_with_args(config: &Path, extra_args: &[&str]) -> LauncherProcess {
    spawn_launcher_with_args_and_env(config, extra_args, &[])
}

pub fn spawn_launcher_with_args_and_env(
    config: &Path,
    extra_args: &[&str],
    environment: &[(&str, &str)],
) -> LauncherProcess {
    let window = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master = -1;
    let pid =
        unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &window) };
    assert!(pid >= 0, "could not create launcher test PTY");

    if pid == 0 {
        let binary = CString::new(binary_path().as_os_str().as_bytes())
            .expect("binary path contains a NUL byte");
        let log_path = config
            .parent()
            .expect("test config has no parent")
            .join("runtime.jsonl");
        let log_path = CString::new(log_path.as_os_str().as_bytes())
            .expect("test log path contains a NUL byte");
        let log_name = CString::new("TUI_LAUNCHER_LOG_FILE").unwrap();
        unsafe {
            libc::setenv(log_name.as_ptr(), log_path.as_ptr(), 1);
        }
        for (key, value) in environment {
            let key = CString::new(*key).expect("environment key contains a NUL byte");
            let value = CString::new(*value).expect("environment value contains a NUL byte");
            if unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) } != 0 {
                unsafe { libc::_exit(127) };
            }
        }
        let config =
            CString::new(config.as_os_str().as_bytes()).expect("config path contains a NUL byte");
        let mut command = vec![binary, CString::new("--config").unwrap(), config];
        command.extend(
            extra_args
                .iter()
                .map(|argument| CString::new(*argument).expect("argument contains a NUL byte")),
        );
        let mut argv = command
            .iter()
            .map(|argument| argument.as_ptr())
            .collect::<Vec<_>>();
        argv.push(std::ptr::null());
        unsafe {
            libc::execv(command[0].as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }

    set_nonblocking(master);
    LauncherProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        finished: false,
    }
}

fn spawn(args: &[&str]) -> DmenuProcess {
    let mut input_pipe = [0; 2];
    let mut output_pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(input_pipe.as_mut_ptr()) }, 0);
    assert_eq!(unsafe { libc::pipe(output_pipe.as_mut_ptr()) }, 0);

    let window = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master = -1;
    let pid =
        unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &window) };
    assert!(pid >= 0, "could not create test PTY");

    if pid == 0 {
        child_exec(input_pipe, output_pipe, master, args);
    }

    close_fd(input_pipe[0]);
    close_fd(output_pipe[1]);
    set_nonblocking(master);

    DmenuProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        input: Some(unsafe { File::from_raw_fd(input_pipe[1]) }),
        output: unsafe { File::from_raw_fd(output_pipe[0]) },
        finished: false,
    }
}

fn child_exec(input_pipe: [RawFd; 2], output_pipe: [RawFd; 2], master: RawFd, args: &[&str]) -> ! {
    if unsafe { libc::dup2(input_pipe[0], libc::STDIN_FILENO) } < 0 {
        unsafe { libc::_exit(127) };
    }
    if unsafe { libc::dup2(output_pipe[1], libc::STDOUT_FILENO) } < 0 {
        unsafe { libc::_exit(127) };
    }

    for fd in [
        input_pipe[0],
        input_pipe[1],
        output_pipe[0],
        output_pipe[1],
        master,
    ] {
        close_fd(fd);
    }

    exec_binary(args)
}

fn spawn_tty_with_redirected_stdout(args: &[&str]) -> DmenuProcess {
    let mut output_pipe = [0; 2];
    assert_eq!(unsafe { libc::pipe(output_pipe.as_mut_ptr()) }, 0);
    let window = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master = -1;
    let pid =
        unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &window) };
    assert!(pid >= 0, "could not create redirected-output PTY");

    if pid == 0 {
        if unsafe { libc::dup2(output_pipe[1], libc::STDOUT_FILENO) } < 0 {
            unsafe { libc::_exit(127) };
        }
        close_fd(output_pipe[0]);
        close_fd(output_pipe[1]);
        close_fd(master);
        exec_binary(args);
    }

    close_fd(output_pipe[1]);
    set_nonblocking(master);
    DmenuProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        input: None,
        output: unsafe { File::from_raw_fd(output_pipe[0]) },
        finished: false,
    }
}

fn exec_binary(args: &[&str]) -> ! {
    let binary = binary_path();
    let mut command = Vec::with_capacity(args.len() + 1);
    command.push(
        CString::new(binary.as_os_str().as_bytes()).expect("binary path contains a NUL byte"),
    );
    command.extend(
        args.iter()
            .map(|argument| CString::new(*argument).expect("test argument contains a NUL byte")),
    );
    let mut argv = command
        .iter()
        .map(|argument| argument.as_ptr())
        .collect::<Vec<_>>();
    argv.push(std::ptr::null());

    unsafe {
        libc::execv(command[0].as_ptr(), argv.as_ptr());
        libc::_exit(127);
    }
}

pub fn wait_for_ready(master: &File) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buffer = [0_u8; 4096];
    loop {
        let count =
            unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if count > 0 {
            return;
        }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::WouldBlock,
                "could not read test PTY"
            );
        }
        assert!(
            Instant::now() < deadline,
            "process did not render a ready screen"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_exit(process: &mut DmenuProcess) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        drain_master(&process.master);
        let mut status = 0;
        let result = unsafe { libc::waitpid(process.pid, &mut status, libc::WNOHANG) };
        if result == process.pid {
            process.finished = true;
            if libc::WIFEXITED(status) {
                return libc::WEXITSTATUS(status);
            }
            if libc::WIFSIGNALED(status) {
                return 128 + libc::WTERMSIG(status);
            }
            return 255;
        }
        assert!(result >= 0, "could not wait for test process");
        assert!(Instant::now() < deadline, "dmenu process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_nonempty_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = fs::read_to_string(path)
            && !contents.trim().is_empty()
        {
            return contents;
        }
        assert!(
            Instant::now() < deadline,
            "file did not become available: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_process_exit(pid: libc::pid_t) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let running = unsafe { libc::kill(pid, 0) == 0 };
        if !running {
            return;
        }
        assert!(Instant::now() < deadline, "process {pid} did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_text(master: &File, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut screen = avt::Vt::new(80, 24);
    let mut pending_utf8 = Vec::new();
    let mut parsed = 0;
    loop {
        drain_master_into(master, &mut output);
        feed_terminal_output(&mut screen, &mut pending_utf8, &output[parsed..]);
        parsed = output.len();
        let visible = visible_screen(&screen);
        if String::from_utf8_lossy(&output).contains(needle) || visible.contains(needle) {
            let mut observed = output;
            observed.extend_from_slice(b"\n--- visible screen ---\n");
            observed.extend_from_slice(visible.as_bytes());
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "process did not render {needle:?}; visible screen: {visible:?}; output: {:?}",
            output
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_launcher_exit(process: &mut LauncherProcess) -> (i32, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        drain_master_into(&process.master, &mut output);
        let mut status = 0;
        let result = unsafe { libc::waitpid(process.pid, &mut status, libc::WNOHANG) };
        if result == process.pid {
            drain_master_into(&process.master, &mut output);
            process.finished = true;
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                255
            };
            return (code, output);
        }
        assert!(result >= 0, "could not wait for launcher test process");
        assert!(Instant::now() < deadline, "launcher process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn visible_screen(screen: &avt::Vt) -> String {
    screen
        .view()
        .map(avt::Line::text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn feed_terminal_output(screen: &mut avt::Vt, pending_utf8: &mut Vec<u8>, bytes: &[u8]) {
    pending_utf8.extend_from_slice(bytes);
    loop {
        match std::str::from_utf8(pending_utf8) {
            Ok(text) => {
                screen.feed_str(text);
                pending_utf8.clear();
                return;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let text = std::str::from_utf8(&pending_utf8[..valid_up_to])
                        .expect("valid UTF-8 prefix");
                    screen.feed_str(text);
                    pending_utf8.drain(..valid_up_to);
                }
                if let Some(invalid_length) = error.error_len() {
                    screen.feed_str("\u{fffd}");
                    pending_utf8.drain(..invalid_length);
                } else {
                    return;
                }
            }
        }
    }
}

fn drain_master(master: &File) {
    let mut discarded = Vec::new();
    drain_master_into(master, &mut discarded);
}

fn drain_master_into(master: &File, output: &mut Vec<u8>) {
    let mut buffer = [0_u8; 4096];
    loop {
        let count =
            unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if count > 0 {
            output.extend_from_slice(&buffer[..count as usize]);
            continue;
        }
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::Interrupted
            {
                return;
            }
        }
        return;
    }
}

fn set_nonblocking(fd: RawFd) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags >= 0, "could not inspect test PTY flags");
    assert!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "could not set test PTY nonblocking"
    );
}

fn close_fd(fd: RawFd) {
    if fd > libc::STDERR_FILENO {
        unsafe {
            libc::close(fd);
        }
    }
}

pub fn binary_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_tui-launcher")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_tui-launcher is not set")
}

pub fn fixture_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/config.toml")
}

pub fn temporary_root() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "tui-launcher-cli-test-{}-{}",
        std::process::id(),
        timestamp
    ));
    fs::create_dir_all(&root).expect("could not create launcher integration directory");
    root
}
