#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct DmenuProcess {
    pid: libc::pid_t,
    master: File,
    observer: PtyObserverKey,
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
        remove_pty_observer(self.observer);
        fs::remove_dir_all(&self.state_home).ok();
    }
}

pub struct LauncherProcess {
    pid: libc::pid_t,
    pub master: File,
    observer: PtyObserverKey,
    original_termios: libc::termios,
    stdout: Option<File>,
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

    pub fn take_stdout(&mut self) -> Option<File> {
        self.stdout.take()
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
        remove_pty_observer(self.observer);
        fs::remove_dir_all(&self.state_home).ok();
    }
}

pub struct RunResult {
    pub status: i32,
    pub stdout: Vec<u8>,
}

static DMENU_TEST_LOCK: Mutex<()> = Mutex::new(());
const PTY_SCREEN_QUIET_PERIOD: Duration = Duration::from_millis(100);
static PTY_OBSERVER_TOKEN: AtomicU64 = AtomicU64::new(1);
static PTY_OBSERVERS: LazyLock<Mutex<BTreeMap<RawFd, RegisteredPtyObserver>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Clone, Copy)]
struct PtyObserverKey {
    fd: RawFd,
    token: u64,
}

struct RegisteredPtyObserver {
    token: u64,
    state: PtyObserverState,
}

struct PtyObserverState {
    screen: avt::Vt,
    pending_utf8: Vec<u8>,
    visible: String,
    revision: u64,
    completed_waits: u64,
    allow_cached_wait: bool,
}

#[derive(Clone)]
struct PtyObservation {
    visible: String,
    revision: u64,
    allow_cached_wait: bool,
}

fn register_pty_observer(master: &File) -> PtyObserverKey {
    let fd = master.as_raw_fd();
    let token = PTY_OBSERVER_TOKEN.fetch_add(1, Ordering::Relaxed);
    let screen = avt::Vt::new(80, 24);
    let visible = visible_screen(&screen);
    let observer = RegisteredPtyObserver {
        token,
        state: PtyObserverState {
            screen,
            pending_utf8: Vec::new(),
            visible,
            revision: 0,
            completed_waits: 0,
            allow_cached_wait: false,
        },
    };
    PTY_OBSERVERS
        .lock()
        .expect("PTY observer registry lock poisoned")
        .insert(fd, observer);
    PtyObserverKey { fd, token }
}

fn remove_pty_observer(key: PtyObserverKey) {
    let mut observers = PTY_OBSERVERS
        .lock()
        .expect("PTY observer registry lock poisoned");
    if observers
        .get(&key.fd)
        .is_some_and(|observer| observer.token == key.token)
    {
        observers.remove(&key.fd);
    }
}

fn observe_pty_output(master: &File, bytes: &[u8]) -> Option<PtyObservation> {
    let mut observers = PTY_OBSERVERS
        .lock()
        .expect("PTY observer registry lock poisoned");
    let observer = observers.get_mut(&master.as_raw_fd())?;
    let state = &mut observer.state;
    if !bytes.is_empty() {
        let PtyObserverState {
            screen,
            pending_utf8,
            ..
        } = state;
        feed_terminal_output(screen, pending_utf8, bytes);
        let visible = visible_screen(screen);
        if visible != state.visible {
            state.visible = visible;
            state.revision = state.revision.wrapping_add(1);
        }
    }
    Some(PtyObservation {
        visible: state.visible.clone(),
        revision: state.revision,
        allow_cached_wait: state.allow_cached_wait,
    })
}

fn current_pty_observation(master: &File) -> Option<PtyObservation> {
    observe_pty_output(master, &[])
}

fn allow_cached_pty_wait(master: &File) {
    if let Some(observer) = PTY_OBSERVERS
        .lock()
        .expect("PTY observer registry lock poisoned")
        .get_mut(&master.as_raw_fd())
        && observer.state.completed_waits == 0
    {
        observer.state.allow_cached_wait = true;
    }
}

fn complete_pty_wait(master: &File) {
    if let Some(observer) = PTY_OBSERVERS
        .lock()
        .expect("PTY observer registry lock poisoned")
        .get_mut(&master.as_raw_fd())
    {
        observer.state.allow_cached_wait = false;
        observer.state.completed_waits = observer.state.completed_waits.wrapping_add(1);
    }
}

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
        "tflow-test-state-{}-{}",
        std::process::id(),
        TEST_STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&state_home).expect("could not create test XDG state directory");
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    if let Some(bin_dir) = binary_path().parent() {
        let current_path = environment
            .get(std::ffi::OsStr::new("PATH"))
            .cloned()
            .unwrap_or_default();
        let mut new_path = bin_dir.as_os_str().to_os_string();
        if !current_path.is_empty() {
            new_path.push(":");
            new_path.push(current_path);
        }
        environment.insert(OsString::from("PATH"), new_path);
    }
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
    let workflows = config
        .as_table_mut()
        .and_then(|table| table.remove("workflows"));

    let config_root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut suite_aliases = BTreeMap::new();
    let mut mounted_workflows = BTreeMap::new();

    if let Some(table) = config.as_table()
        && let Some(aliases_val) = table.get("aliases").and_then(toml::Value::as_table)
    {
        for (k, v) in aliases_val {
            if let Some(target) = v.as_str() {
                suite_aliases.insert(k.clone(), target.to_string());
            }
        }
    }

    let default_view_str = config
        .as_table()
        .and_then(|t| t.get("default_view").and_then(toml::Value::as_str))
        .map(str::to_string);

    if let Some(toml::Value::Table(workflows)) = workflows {
        for (workflow_id, workflow) in workflows {
            let aliases = materialize_test_workflow(
                config_root,
                &workflow_id,
                workflow,
                default_view_str.as_deref(),
            )?;
            for (alias, target) in aliases {
                suite_aliases.insert(alias, target);
            }
            let mut m = toml::map::Map::new();
            m.insert(
                "dir".to_string(),
                toml::Value::String(format!("./workflows/{workflow_id}")),
            );
            mounted_workflows.insert(workflow_id.clone(), toml::Value::Table(m));
        }
    }

    // Now turn config into a valid suite manifest if it has default_view or workflows
    if let Some(table) = config.as_table_mut() {
        if let Some(dv) = table.remove("default_view") {
            let dv_str = dv.as_str().unwrap_or("core:default").to_string();
            let mut suite_header = toml::map::Map::new();
            suite_header.insert("api".to_string(), toml::Value::Integer(1));
            suite_header.insert(
                "name".to_string(),
                toml::Value::String("Test Suite".to_string()),
            );
            suite_header.insert("entrypoint".to_string(), toml::Value::String(dv_str));
            table.insert("suite".to_string(), toml::Value::Table(suite_header));
        } else if !mounted_workflows.is_empty() && !table.contains_key("suite") {
            let mut suite_header = toml::map::Map::new();
            suite_header.insert("api".to_string(), toml::Value::Integer(1));
            suite_header.insert(
                "name".to_string(),
                toml::Value::String("Test Suite".to_string()),
            );
            let first_entry = mounted_workflows
                .keys()
                .next()
                .map(|k| format!("{k}:main"))
                .unwrap_or_else(|| "core:default".to_string());
            suite_header.insert("entrypoint".to_string(), toml::Value::String(first_entry));
            table.insert("suite".to_string(), toml::Value::Table(suite_header));
        }

        if !mounted_workflows.is_empty() {
            table.insert(
                "workflows".to_string(),
                toml::Value::Table(mounted_workflows.into_iter().collect()),
            );
        }

        if !suite_aliases.is_empty() {
            let mut aliases_table = table
                .remove("aliases")
                .and_then(|v| v.as_table().cloned())
                .unwrap_or_default();
            for (k, v) in suite_aliases {
                aliases_table.insert(k, toml::Value::String(v));
            }
            table.insert("aliases".to_string(), toml::Value::Table(aliases_table));
        }
    }

    let mut settings = toml::Table::new();
    for key in ["image_protocol", "log_file", "defaults"] {
        if let Some(value) = config.as_table_mut().unwrap().remove(key) {
            settings.insert(key.to_string(), value);
        }
    }
    if !settings.is_empty() {
        fs::write(
            config_root.join("settings.toml"),
            toml::to_string(&settings).map_err(io::Error::other)?,
        )?;
    }
    let config_source = toml::to_string(&config).map_err(io::Error::other)?;
    fs::write(path, config_source)
}

fn materialize_test_workflow(
    config_root: &Path,
    workflow_id: &str,
    workflow: toml::Value,
    default_view_hint: Option<&str>,
) -> io::Result<Vec<(String, String)>> {
    let toml::Value::Table(workflow) = workflow else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("test workflow {workflow_id:?} must be a TOML table"),
        ));
    };
    let workflow_root = config_root.join("workflows").join(workflow_id);
    fs::create_dir_all(&workflow_root)?;
    let manifest_path = workflow_root.join("workflow.toml");
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
            format!("test workflow manifest {manifest_path:?} must be a TOML table"),
        )
    })?;
    if let Some(commands) = workflow.get("commands") {
        manifest_table.insert("commands".into(), commands.clone());
    }
    let views = workflow
        .get("views")
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
    let manifest_views = manifest_table
        .entry("views".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    merge_test_values(manifest_views, views);

    let mut collected_aliases = Vec::new();
    let mut first_view_name = None;
    let mut matched_entrypoint = None;
    let mut promoted_commands = Vec::new();

    if let Some(views_table) = manifest_views.as_table_mut() {
        for (view_name, view_val) in views_table.iter_mut() {
            if let Some(view_tbl) = view_val.as_table_mut() {
                if let Some(view_cmds) = view_tbl.remove("commands") {
                    if let Some(view_cmds_table) = view_cmds.as_table() {
                        let keymap = view_tbl
                            .entry("keymap".to_string())
                            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                            .as_table_mut()
                            .unwrap();
                        for (cmd_id, cmd_val) in view_cmds_table {
                            if let Some(cmd_table) = cmd_val.as_table() {
                                if let Some(key) = cmd_table.get("key").and_then(toml::Value::as_str) {
                                    keymap.insert(key.to_string(), toml::Value::String(cmd_id.clone()));
                                }
                            }
                            promoted_commands.push((cmd_id.clone(), cmd_val.clone()));
                        }
                    }
                }
                if let Some(engine) = view_tbl.get_mut("engine").and_then(toml::Value::as_table_mut) {
                    if let Some(config) = engine.get_mut("config").and_then(toml::Value::as_table_mut) {
                        config.remove("feeds");
                        config.remove("source_badge");
                    }
                }
                if let Some(alias_val) = view_tbl.remove("alias")
                    && let Some(alias_str) = alias_val.as_str()
                {
                    collected_aliases
                        .push((alias_str.to_string(), format!("{workflow_id}:{view_name}")));
                }
            }
            if first_view_name.is_none() {
                first_view_name = Some(view_name.clone());
            }
            if let Some(hint) = default_view_hint
                && (hint == format!("{workflow_id}:{view_name}") || hint == view_name.as_str())
            {
                matched_entrypoint = Some(view_name.clone());
            }
        }
    }

    if !promoted_commands.is_empty() {
        let wf_cmds = manifest_table
            .entry("commands".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .unwrap();
        for (cmd_id, cmd_val) in promoted_commands {
            wf_cmds.insert(cmd_id, cmd_val);
        }
    }

    let entrypoint = matched_entrypoint
        .or(first_view_name)
        .unwrap_or_else(|| "main".to_string());

    let header = manifest_table
        .entry("workflow".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let header = header.as_table_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("test workflow manifest {manifest_path:?} has an invalid workflow header"),
        )
    })?;
    header
        .entry("api".to_string())
        .or_insert(toml::Value::Integer(1));
    if let Some(name) = workflow.get("name") {
        header.insert("name".to_string(), name.clone());
    } else {
        header
            .entry("name".to_string())
            .or_insert_with(|| toml::Value::String(workflow_id.to_string()));
    }
    header.insert("entrypoint".to_string(), toml::Value::String(entrypoint));

    let manifest_source = toml::to_string(&manifest).map_err(io::Error::other)?;
    fs::write(manifest_path, manifest_source)?;
    Ok(collected_aliases)
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
    let mut args = vec!["--suite", config, "dmenu:main"];
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
    let mut args = vec!["--suite", config, "dmenu:main"];
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
    run_tty_invocation_with_redirected_stdout(&["--suite", config, "dmenu:main"], keys)
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
            thread::sleep(Duration::from_millis(20));
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
    let (status, _terminal_output) = wait_for_exit_with_output(process);
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
        "--suite".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    let settings = config.with_file_name("settings.toml");
    if settings.is_file() {
        arguments.push("--settings".to_string());
        arguments.push(settings.to_string_lossy().into_owned());
    }
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
    let master = unsafe { File::from_raw_fd(master) };
    let observer = register_pty_observer(&master);
    LauncherProcess {
        pid,
        master,
        observer,
        original_termios,
        stdout: None,
        state_home,
        finished: false,
    }
}

pub fn spawn_launcher_with_redirected_stdout(config: &Path) -> LauncherProcess {
    spawn_launcher_with_args_and_redirected_stdout(config, &[])
}

pub fn spawn_launcher_with_args_and_redirected_stdout(
    config: &Path,
    extra_args: &[&str],
) -> LauncherProcess {
    let binary = binary_path();
    let mut arguments = vec![
        binary.to_string_lossy().into_owned(),
        "--suite".to_string(),
        config.to_string_lossy().into_owned(),
    ];
    let settings = config.with_file_name("settings.toml");
    if settings.is_file() {
        arguments.push("--settings".to_string());
        arguments.push(settings.to_string_lossy().into_owned());
    }
    arguments.extend(extra_args.iter().map(|argument| (*argument).to_string()));
    let prepared = prepare_exec(arguments, &[]);
    let state_home = prepared.state_home.clone();
    let output_pipe = create_cloexec_pipe();
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
    assert!(pid >= 0, "could not create redirected-output launcher PTY");

    if pid == 0 {
        close_fd(gate[1]);
        close_fd(output_pipe[0]);
        close_fd(master);
        let mut byte = 0_u8;
        if unsafe { libc::read(gate[0], (&mut byte as *mut u8).cast(), 1) } != 1
            || unsafe { libc::dup2(output_pipe[1], libc::STDOUT_FILENO) } < 0
        {
            unsafe { libc::_exit(127) };
        }
        close_fd(gate[0]);
        close_fd(output_pipe[1]);
        exec_prepared(&prepared);
    }

    close_fd(gate[0]);
    close_fd(output_pipe[1]);
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
    let master = unsafe { File::from_raw_fd(master) };
    let observer = register_pty_observer(&master);
    LauncherProcess {
        pid,
        master,
        observer,
        original_termios,
        stdout: Some(unsafe { File::from_raw_fd(output_pipe[0]) }),
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
    let master = unsafe { File::from_raw_fd(master) };
    let observer = register_pty_observer(&master);

    DmenuProcess {
        pid,
        master,
        observer,
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
    let master = unsafe { File::from_raw_fd(master) };
    let observer = register_pty_observer(&master);
    DmenuProcess {
        pid,
        master,
        observer,
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
            observe_pty_output(master, &buffer[..count as usize]);
            allow_cached_pty_wait(master);
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

pub fn current_screen(master: &File) -> String {
    drain_master(master);
    current_pty_observation(master)
        .expect("test PTY is not registered")
        .visible
}

pub fn discard_pending_master_output(master: &File) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    let mut last_revision = current_pty_observation(master)
        .expect("test PTY is not registered")
        .revision;
    let mut stable_since = Instant::now();
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        let observation =
            observe_pty_output(master, &output[before..]).expect("test PTY is not registered");
        if observation.revision != last_revision {
            last_revision = observation.revision;
            stable_since = Instant::now();
        }
        if Instant::now().duration_since(stable_since) >= PTY_SCREEN_QUIET_PERIOD {
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
    let start_revision = current_pty_observation(master)
        .expect("test PTY is not registered")
        .revision;
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        let observation =
            observe_pty_output(master, &output[before..]).expect("test PTY is not registered");
        if observation.revision > start_revision && ready(&observation.visible) {
            complete_pty_wait(master);
            let mut observed = output;
            observed.extend_from_slice(b"\n--- visible screen ---\n");
            observed.extend_from_slice(observation.visible.as_bytes());
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "process did not render the expected fresh screen; visible screen: {:?}; output: {:?}",
            observation.visible,
            output
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_text(master: &File, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let start = current_pty_observation(master).expect("test PTY is not registered");
    let mut matched_frame = None;
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        let observation =
            observe_pty_output(master, &output[before..]).expect("test PTY is not registered");
        let fresh = observation.revision > start.revision;
        let cached = start.allow_cached_wait && start.visible.contains(needle);
        let ready = !observation.visible.contains("(searching...)");
        let matched = ready && observation.visible.contains(needle) && (fresh || cached);
        if matched {
            let (revision, stable_since) =
                matched_frame.get_or_insert_with(|| (observation.revision, Instant::now()));
            if *revision != observation.revision {
                *revision = observation.revision;
                *stable_since = Instant::now();
            }
            if Instant::now().duration_since(*stable_since) >= PTY_SCREEN_QUIET_PERIOD {
                complete_pty_wait(master);
                let mut observed = output;
                observed.extend_from_slice(b"\n--- visible screen ---\n");
                observed.extend_from_slice(observation.visible.as_bytes());
                return observed;
            }
        } else {
            matched_frame = None;
        }
        assert!(
            Instant::now() < deadline,
            "process did not render {needle:?}; visible screen: {:?}; output: {:?}",
            observation.visible,
            output
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_stable_text(master: &File, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut matched_since = None;
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        let observation =
            observe_pty_output(master, &output[before..]).expect("test PTY is not registered");
        if !observation.visible.contains("(searching...)") && observation.visible.contains(needle) {
            let started = *matched_since.get_or_insert_with(Instant::now);
            if started.elapsed() >= PTY_SCREEN_QUIET_PERIOD {
                complete_pty_wait(master);
                let mut observed = output;
                observed.extend_from_slice(b"\n--- visible screen ---\n");
                observed.extend_from_slice(observation.visible.as_bytes());
                return observed;
            }
        } else {
            matched_since = None;
        }
        assert!(
            Instant::now() < deadline,
            "process did not settle on {needle:?}; visible screen: {:?}; output: {:?}",
            observation.visible,
            output
        );
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn wait_for_output(master: &File, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        let before = output.len();
        drain_master_into(master, &mut output);
        let observation =
            observe_pty_output(master, &output[before..]).expect("test PTY is not registered");
        if output.windows(needle.len()).any(|window| window == needle) {
            complete_pty_wait(master);
            let mut observed = output;
            observed.extend_from_slice(b"\n--- visible screen ---\n");
            observed.extend_from_slice(observation.visible.as_bytes());
            return observed;
        }
        assert!(
            Instant::now() < deadline,
            "process did not emit raw output {:?}; visible screen: {:?}; output: {:?}",
            needle,
            observation.visible,
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
    PathBuf::from(env!("CARGO_BIN_EXE_tflow"))
}

pub fn fixture_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config/default.toml")
}

pub fn quick_picker_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/quick-picker.toml")
}

pub fn temporary_root() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "tflow-cli-test-{}-{}",
        std::process::id(),
        timestamp
    ));
    fs::create_dir_all(&root).expect("could not create launcher integration directory");
    root
}
