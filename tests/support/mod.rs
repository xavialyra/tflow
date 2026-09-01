#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct DmenuProcess {
    pid: libc::pid_t,
    master: File,
    input: Option<File>,
    output: File,
    state_home: PathBuf,
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
        fs::remove_dir_all(&self.state_home).ok();
    }
}

pub struct LauncherProcess {
    pid: libc::pid_t,
    pub master: File,
    original_termios: libc::termios,
    state_home: PathBuf,
    finished: bool,
}

impl LauncherProcess {
    pub fn send_signal(&self, signal: libc::c_int) {
        assert_eq!(unsafe { libc::kill(self.pid, signal) }, 0);
    }

    pub fn original_termios(&self) -> &libc::termios {
        &self.original_termios
    }

    pub fn current_termios(&self) -> libc::termios {
        let mut settings = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.master.as_raw_fd(), &mut settings) },
            0
        );
        settings
    }
}

impl Drop for LauncherProcess {
    fn drop(&mut self) {
        if !self.finished {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            }
        }
        fs::remove_dir_all(&self.state_home).ok();
    }
}

pub struct RunResult {
    pub status: i32,
    pub stdout: Vec<u8>,
}

static DMENU_TEST_LOCK: Mutex<()> = Mutex::new(());

struct PreparedExec {
    _command: Vec<CString>,
    argv: Vec<*const libc::c_char>,
    _environment: Vec<CString>,
    envp: Vec<*const libc::c_char>,
    state_home: PathBuf,
}

static TEST_STATE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn prepare_exec(arguments: Vec<String>, overrides: &[(&str, &str)]) -> PreparedExec {
    let command = arguments
        .into_iter()
        .map(|argument| CString::new(argument).expect("test argument contains a NUL byte"))
        .collect::<Vec<_>>();
    let mut argv = command
        .iter()
        .map(|argument| argument.as_ptr())
        .collect::<Vec<_>>();
    argv.push(std::ptr::null());

    let state_home = std::env::temp_dir().join(format!(
        "tui-launcher-test-state-{}-{}",
        std::process::id(),
        TEST_STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&state_home).expect("could not create test XDG state directory");
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    environment.insert(
        OsString::from("XDG_STATE_HOME"),
        OsString::from(state_home.as_os_str()),
    );
    for (key, value) in overrides {
        environment.insert(OsString::from(key), OsString::from(value));
    }
    let environment = environment
        .into_iter()
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            CString::new(key.as_os_str().as_bytes()).expect("test environment contains a NUL byte")
        })
        .collect::<Vec<_>>();
    let mut envp = environment
        .iter()
        .map(|entry| entry.as_ptr())
        .collect::<Vec<_>>();
    envp.push(std::ptr::null());

    PreparedExec {
        _command: command,
        argv,
        _environment: environment,
        envp,
        state_home,
    }
}

fn exec_prepared(prepared: &PreparedExec) -> ! {
    unsafe {
        libc::execve(
            prepared.argv[0],
            prepared.argv.as_ptr(),
            prepared.envp.as_ptr(),
        );
        libc::_exit(127);
    }
}

fn with_test_config(source: &str) -> String {
    source.to_string()
}

pub fn write_test_config(path: &Path, source: &str) -> io::Result<()> {
    let mut config: toml::Value = toml::from_str(&with_test_config(source))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let plugins = config
        .as_table_mut()
        .and_then(|table| table.remove("plugins"));
    let Some(plugins) = plugins else {
        return fs::write(path, toml::to_string(&config).map_err(io::Error::other)?);
    };
    let toml::Value::Table(plugins) = plugins else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "test plugins must be a TOML table",
        ));
    };
    let config_root = path.parent().unwrap_or_else(|| Path::new("."));
    for (plugin_id, plugin) in plugins {
        materialize_test_plugin(config_root, &plugin_id, plugin)?;
    }
    let config_source = toml::to_string(&config).map_err(io::Error::other)?;
    fs::write(path, config_source)
}

fn materialize_test_plugin(
    config_root: &Path,
    plugin_id: &str,
    plugin: toml::Value,
) -> io::Result<()> {
    let toml::Value::Table(plugin) = plugin else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("test plugin {plugin_id:?} must be a TOML table"),
        ));
    };
    let plugin_root = config_root.join("plugins").join(plugin_id);
    fs::create_dir_all(&plugin_root)?;
    let manifest_path = plugin_root.join("plugin.toml");
    let mut manifest = if manifest_path.is_file() {
        let source = fs::read_to_string(&manifest_path)?;
        toml::from_str(&source)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };
    let manifest_table = manifest.as_table_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("test plugin manifest {manifest_path:?} must be a TOML table"),
        )
    })?;
    let header = manifest_table
        .entry("plugin".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let header = header.as_table_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("test plugin manifest {manifest_path:?} has an invalid plugin header"),
        )
    })?;
    header
        .entry("api".to_string())
        .or_insert(toml::Value::Integer(1));
    if let Some(name) = plugin.get("name") {
        header.insert("name".to_string(), name.clone());
    } else {
        header
            .entry("name".to_string())
            .or_insert_with(|| toml::Value::String(plugin_id.to_string()));
    }

    let views = plugin
        .get("views")
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let manifest_views = manifest_table
        .entry("views".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    merge_test_values(manifest_views, views);

    let manifest_source = toml::to_string(&manifest).map_err(io::Error::other)?;
    fs::write(manifest_path, manifest_source)
}

fn merge_test_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_test_values(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn lock_dmenu_tests() -> MutexGuard<'static, ()> {
    DMENU_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn run_dmenu(extra_args: &[&str], input: &[u8], keys: &[u8]) -> RunResult {
    run_dmenu_steps(extra_args, input, &[keys])
}

pub fn run_dmenu_steps(extra_args: &[&str], input: &[u8], key_steps: &[&[u8]]) -> RunResult {
    let _guard = lock_dmenu_tests();
    let config = fixture_config();
    let config = config.to_str().expect("fixture config path is not UTF-8");
    let mut args = vec!["--config", config, "dmenu:main"];
    args.extend_from_slice(extra_args);
    run_invocation_steps(&args, input, key_steps)
}

pub fn run_dmenu_steps_waiting_for_text(
    extra_args: &[&str],
    input: &[u8],
    key_steps: &[&[u8]],
    screen_texts: &[&str],
) -> RunResult {
    assert_eq!(
        screen_texts.len(),
        key_steps.len(),
        "expected one screen marker before each dmenu key step"
    );
    let _guard = lock_dmenu_tests();
    let config = fixture_config();
    let config = config.to_str().expect("fixture config path is not UTF-8");
    let mut args = vec!["--config", config, "dmenu:main"];
    args.extend_from_slice(extra_args);

    let mut process = spawn(&args);
    process
        .input
        .take()
        .expect("invocation input pipe is missing")
        .write_all(input)
        .expect("could not write invocation input");
    wait_for_text(&process.master, screen_texts[0]);
    for (index, keys) in key_steps.iter().enumerate() {
        process
            .master
            .write_all(keys)
            .expect("could not write invocation key input");
        process
            .master
            .flush()
            .expect("could not flush invocation key input");
        if let Some(screen_text) = screen_texts.get(index + 1) {
            wait_for_text(&process.master, screen_text);
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

pub fn run_tty_dmenu(keys: &[u8]) -> RunResult {
    let _guard = lock_dmenu_tests();
    let config = fixture_config();
    let config = config.to_str().expect("fixture config path is not UTF-8");
    run_tty_invocation_with_redirected_stdout(&["--config", config, "dmenu:main"], keys)
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
    finish_tty_invocation(&mut process, keys)
}

pub fn run_tty_invocation_with_redirected_stdout_after_marker(
    args: &[&str],
    marker: &str,
    keys: &[u8],
) -> RunResult {
    let mut process = spawn_tty_with_redirected_stdout(args);
    wait_for_text(&process.master, marker);
    finish_tty_invocation(&mut process, keys)
}

pub fn run_tty_invocation_with_blocked_stdout_signal(
    args: &[&str],
    keys: &[u8],
    signal: libc::c_int,
) -> RunResult {
    let mut process = spawn_tty_with_redirected_stdout(args);
    wait_for_ready(&process.master);
    process.master.write_all(keys).unwrap();
    process.master.flush().unwrap();
    wait_for_output_start(&process);
    assert_eq!(unsafe { libc::kill(process.pid, signal) }, 0);
    let status = wait_for_exit(&mut process);
    let mut stdout = Vec::new();
    process.output.read_to_end(&mut stdout).unwrap();
    RunResult { status, stdout }
}

fn finish_tty_invocation(process: &mut DmenuProcess, keys: &[u8]) -> RunResult {
    process.master.write_all(keys).unwrap();
    process.master.flush().unwrap();
    let status = wait_for_exit(process);
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
    let binary = binary_path();
    let mut arguments = vec![
        binary.to_string_lossy().into_owned(),
        "--config".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    arguments.extend(extra_args.iter().map(|argument| (*argument).to_string()));
    let prepared = prepare_exec(arguments, environment);
    let state_home = prepared.state_home.clone();
    let window = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let gate = create_cloexec_pipe();
    let mut master = -1;
    let pid =
        unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null(), &window) };
    assert!(pid >= 0, "could not create launcher test PTY");

    if pid == 0 {
        close_fd(gate[1]);
        close_fd(master);
        let mut byte = 0_u8;
        if unsafe { libc::read(gate[0], (&mut byte as *mut u8).cast(), 1) } != 1 {
            unsafe { libc::_exit(127) };
        }
        close_fd(gate[0]);
        exec_prepared(&prepared);
    }

    close_fd(gate[0]);
    set_cloexec(master);
    let mut original_termios = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(unsafe { libc::tcgetattr(master, &mut original_termios) }, 0);
    let byte = 1_u8;
    assert_eq!(
        unsafe { libc::write(gate[1], (&byte as *const u8).cast(), 1) },
        1
    );
    close_fd(gate[1]);
    set_nonblocking(master);
    LauncherProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        original_termios,
        state_home,
        finished: false,
    }
}

fn spawn(args: &[&str]) -> DmenuProcess {
    let mut arguments = vec![binary_path().to_string_lossy().into_owned()];
    arguments.extend(args.iter().map(|argument| (*argument).to_string()));
    let prepared = prepare_exec(arguments, &[]);
    let state_home = prepared.state_home.clone();
    let input_pipe = create_cloexec_pipe();
    let output_pipe = create_cloexec_pipe();

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
        child_exec(input_pipe, output_pipe, master, &prepared);
    }

    close_fd(input_pipe[0]);
    close_fd(output_pipe[1]);
    set_cloexec(master);
    set_nonblocking(master);

    DmenuProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        input: Some(unsafe { File::from_raw_fd(input_pipe[1]) }),
        output: unsafe { File::from_raw_fd(output_pipe[0]) },
        state_home,
        finished: false,
    }
}

fn child_exec(
    input_pipe: [RawFd; 2],
    output_pipe: [RawFd; 2],
    master: RawFd,
    prepared: &PreparedExec,
) -> ! {
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

    exec_prepared(prepared)
}

fn spawn_tty_with_redirected_stdout(args: &[&str]) -> DmenuProcess {
    let mut arguments = vec![binary_path().to_string_lossy().into_owned()];
    arguments.extend(args.iter().map(|argument| (*argument).to_string()));
    let prepared = prepare_exec(arguments, &[]);
    let state_home = prepared.state_home.clone();
    let output_pipe = create_cloexec_pipe();
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
        exec_prepared(&prepared);
    }

    close_fd(output_pipe[1]);
    set_cloexec(master);
    set_nonblocking(master);
    DmenuProcess {
        pid,
        master: unsafe { File::from_raw_fd(master) },
        input: None,
        output: unsafe { File::from_raw_fd(output_pipe[0]) },
        state_home,
        finished: false,
    }
}

fn wait_for_output_start(process: &DmenuProcess) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut descriptor = libc::pollfd {
            fd: process.output.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, 20) };
        assert!(result >= 0, "could not poll invocation stdout");
        let mut status = 0;
        let wait = unsafe { libc::waitpid(process.pid, &mut status, libc::WNOHANG) };
        assert!(wait >= 0, "could not check invocation process");
        assert_eq!(wait, 0, "invocation exited before final output started");
        if result > 0 && descriptor.revents & libc::POLLIN != 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "invocation did not start final output"
        );
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
    wait_for_exit_with_output(process).0
}

fn wait_for_exit_with_output(process: &mut DmenuProcess) -> (i32, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        drain_master_into(&process.master, &mut output);
        let mut status = 0;
        let result = unsafe { libc::waitpid(process.pid, &mut status, libc::WNOHANG) };
        if result == process.pid {
            drain_master_into(&process.master, &mut output);
            process.finished = true;
            let status = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                255
            };
            return (status, output);
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

pub fn discard_pending_master_output(master: &File) {
    let quiet_period = Duration::from_millis(50);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    let mut screen = avt::Vt::new(80, 24);
    let mut pending_utf8 = Vec::new();
    let mut parsed = 0;
    let mut last_visible = visible_screen(&screen);
    let mut stable_since = Instant::now();
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        if output.len() != before {
            feed_terminal_output(&mut screen, &mut pending_utf8, &output[parsed..]);
            parsed = output.len();
            let visible = visible_screen(&screen);
            if visible != last_visible {
                last_visible = visible;
                stable_since = Instant::now();
            }
        }
        if Instant::now().duration_since(stable_since) >= quiet_period {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "PTY visible screen did not reach a stable frame before the fresh-screen action"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

pub fn wait_for_fresh_text(master: &File, needle: &str) -> Vec<u8> {
    wait_for_fresh_screen(master, |visible| visible.contains(needle))
}

pub fn wait_for_fresh_screen<F>(master: &File, ready: F) -> Vec<u8>
where
    F: Fn(&str) -> bool,
{
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
        if ready(&visible) {
            let mut observed = output;
            observed.extend_from_slice(b"\n--- visible screen ---\n");
            observed.extend_from_slice(visible.as_bytes());
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "process did not render the expected fresh screen; visible screen: {visible:?}; output: {:?}",
            output
        );
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
            let code = launcher_status(status);
            return (code, output);
        }
        assert!(result >= 0, "could not wait for launcher test process");
        assert!(Instant::now() < deadline, "launcher process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_launcher_exit_without_reading(
    process: &mut LauncherProcess,
    timeout: Duration,
) -> i32 {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(process.pid, &mut status, libc::WNOHANG) };
        if result == process.pid {
            process.finished = true;
            return launcher_status(status);
        }
        assert!(result >= 0, "could not wait for launcher test process");
        assert!(Instant::now() < deadline, "launcher process did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn launcher_status(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        128 + libc::WTERMSIG(status)
    } else {
        255
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

fn create_cloexec_pipe() -> [RawFd; 2] {
    let mut fds = [-1; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    set_cloexec(fds[0]);
    set_cloexec(fds[1]);
    fds
}

fn set_cloexec(fd: RawFd) {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert!(flags >= 0, "could not inspect test fd flags");
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) },
        0,
        "could not set test fd close-on-exec"
    );
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
    PathBuf::from(env!("CARGO_BIN_EXE_tui-launcher"))
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
