use std::ffi::CString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct DmenuProcess {
    pid: libc::pid_t,
    master: File,
    input: Option<File>,
    output: File,
}

struct LauncherProcess {
    pid: libc::pid_t,
    master: File,
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

struct RunResult {
    status: i32,
    stdout: Vec<u8>,
}

const TEST_VIEWTYPES: &str = r#"
[viewtypes.launcher.engine]
type = "launcher"

[viewtypes.launcher.engine.config]
items = "{{ runtime:view.current.items }}"
commands = "{{ runtime:view.current.command }}"

[viewtypes.capture.engine]
type = "capture"

[viewtypes.embedded.engine]
type = "embedded"

[test_items]
items = [{label = "Item", value = "value"}]
"#;

fn with_viewtypes(source: &str) -> String {
    format!("{TEST_VIEWTYPES}\n{source}")
}

#[test]
fn check_loads_the_project_configuration() {
    let output = Command::new(binary_path())
        .args(["--check", "--config"])
        .arg(project_config())
        .output()
        .expect("could not run tui-launcher --check");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("configuration is valid: {}\n", project_config().display())
    );
}

#[test]
fn dmenu_accepts_a_selected_line_without_terminal_bytes_on_stdout() {
    let result = run_dmenu(&[], b"first\nsecond\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"first\n");
}

#[test]
fn dmenu_supports_nul_records_and_index_output() {
    let result = run_dmenu(&["--dmenu0", "--index"], b"first\0second\0", b"\x1b[B\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"1\0");
}

#[test]
fn dmenu_accept_nth_returns_the_selected_projection() {
    let result = run_dmenu(&["--accept-nth", "2"], b"id\tlabel\n", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"label\n");
}

#[test]
fn dmenu_cancel_returns_nonzero_without_stdout() {
    let result = run_dmenu(&[], b"first\n", b"\x1b");

    assert_ne!(result.status, 0);
    assert!(result.stdout.is_empty());
}

#[test]
fn launcher_loads_items_from_an_expression() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:catalog.items }}"

        [catalog]
        items = [{label = "Item", value = "value"}]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        exit = true
        run = '''printf 'expression-marker:%s\\n' "$LAUNCHER_VALUE"'''
        "#,
    )
    .expect("could not write expression items config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\r")
        .expect("could not write launcher Enter key");
    process
        .master
        .flush()
        .expect("could not flush launcher Enter key");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(
        status,
        0,
        "launcher exited with output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        String::from_utf8_lossy(&output).contains("expression-marker:value"),
        "launcher output did not contain expression marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove expression items config");
}

#[test]
fn launcher_waits_for_items_before_running_enter_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        exit = true
        run = '''printf 'launcher-marker:%s\n' "$LAUNCHER_VALUE"'''
        "#,
    )
    .expect("could not write launcher integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\r")
        .expect("could not write launcher Enter key");
    process
        .master
        .flush()
        .expect("could not flush launcher Enter key");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("launcher-marker:value"),
        "launcher output did not contain command marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove launcher integration config");
}

#[test]
fn replacing_items_request_cancels_the_previous_script() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin_root = root.join("plugins/core");
    let script_root = plugin_root.join("scripts");
    fs::create_dir_all(&script_root).expect("could not create cancellation script directory");
    fs::write(
        plugin_root.join("plugin.toml"),
        "[views.placeholder]\ntype = \"launcher\"\n",
    )
    .expect("could not write cancellation plugin manifest");
    let old_pid_path = root.join("old.pid");
    fs::write(
        script_root.join("items.sh"),
        format!(
            r#"query=$(cat | jq -r '.query // empty')
if [ -z "$query" ]; then
    printf '%s\n' "$$" > "{}"
    sleep 10
    printf '[{{"label":"old-result"}}]\n'
else
    printf '[{{"label":"new-result"}}]\n'
fi
"#,
            old_pid_path.display()
        ),
    )
    .expect("could not write cancellation items script");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = '{{ script("scripts/items.sh", runtime:view.current) }}'
        "#,
    )
    .expect("could not write cancellation integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let old_pid = wait_for_nonempty_file(&old_pid_path)
        .trim()
        .parse::<libc::pid_t>()
        .expect("items script wrote an invalid PID");
    process
        .master
        .write_all(b"new")
        .expect("could not write replacement query");
    process
        .master
        .flush()
        .expect("could not flush replacement query");

    let output = wait_for_text(&process.master, "new-result");
    wait_for_process_exit(old_pid);
    assert!(
        !String::from_utf8_lossy(&output).contains("old-result"),
        "cancelled request produced an old result: {:?}",
        output
    );
    let log = fs::read_to_string(root.join("runtime.jsonl")).unwrap_or_default();
    assert!(
        !log.contains("cancelled"),
        "cancelled request leaked into runtime log: {log}"
    );

    process
        .master
        .write_all(b"\x03")
        .expect("could not close cancellation launcher");
    process
        .master
        .flush()
        .expect("could not flush cancellation launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove cancellation integration config");
}

#[test]
fn ctrl_k_opens_the_command_launcher_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        run = ":"

        [plugins.core.views.default.commands.apps]
        key = "alt+a"
        label = "Apps"
        exit = true
        run = '''printf 'command-marker:%s\n' "$LAUNCHER_VALUE"'''

        [plugins.core.views.default.commands.shell]
        key = "alt+s"
        label = "Shell"
        run = ":"

        [plugins.core.views.command]
        type = "launcher"
        "#,
    )
    .expect("could not write command view integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"\x0b")
        .expect("could not write Ctrl-K key");
    process.master.flush().expect("could not flush Ctrl-K key");
    let output = wait_for_text(&process.master, "[core:command]");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("Enter Run"), "output: {output}");
    assert!(output.contains("Alt-A Apps"), "output: {output}");
    assert!(output.contains("Alt-S Shell"), "output: {output}");

    process
        .master
        .write_all(b"\x1b[B\r")
        .expect("could not execute the selected command");
    process
        .master
        .flush()
        .expect("could not flush selected command");
    let (status, remaining) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    let remaining = String::from_utf8_lossy(&remaining);
    assert!(
        remaining.contains("command-marker:value"),
        "output: {remaining}"
    );
    fs::remove_dir_all(root).expect("could not remove command view config");
}

#[test]
fn items_errors_are_logged_and_do_not_block_exit() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:test_items }}"
        "#,
    )
    .expect("could not write error logging config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let output = wait_for_text(&process.master, "ERROR");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("core:default"), "output: {output}");
    assert!(output.contains(":"), "output: {output}");

    let log = fs::read_to_string(root.join("runtime.jsonl")).expect("could not read runtime log");
    let record: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(record["metadata"]["level"], "error");
    assert!(
        record["metadata"]["message"]
            .as_str()
            .unwrap()
            .contains("items expression must return a JSON array")
    );

    process
        .master
        .write_all(b"\x03")
        .expect("could not close error launcher");
    process
        .master
        .flush()
        .expect("could not flush error launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove error logging config");
}

#[test]
fn log_prefix_routes_to_the_configured_messages_launcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    fs::write(
        root.join("runtime.jsonl"),
        "{\"label\":\"preexisting log\",\"value\":\"1\",\"metadata\":{}}\n",
    )
    .expect("could not seed runtime log");
    let plugin_root = root.join("plugins/core");
    fs::create_dir_all(plugin_root.join("scripts")).expect("could not create test plugin");
    fs::write(
        plugin_root.join("plugin.toml"),
        "[views.placeholder]\ntype = \"launcher\"\n",
    )
    .expect("could not write test plugin manifest");
    fs::write(
        plugin_root.join("scripts/items.sh"),
        "input=$(cat)\nlog_file=$(printf '%s\\n' \"$input\" | jq -r '.log_file // empty')\nif [ -n \"$log_file\" ] && [ -f \"$log_file\" ]; then\n    jq -s '.' \"$log_file\"\nelse\n    printf '[]\\n'\nfi\n",
    )
    .expect("could not write test items script");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"

        [plugins.core.views.messages]
        type = "launcher"
        display_prefix = "log"
        items = '{{ script("scripts/items.sh", runtime:view.current) }}'
        "#,
    )
    .expect("could not write messages integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"log ")
        .expect("could not write log prefix");
    process.master.flush().expect("could not flush log prefix");
    let _ = wait_for_text(&process.master, "[core:messages]");
    let output = wait_for_text(&process.master, "preexisting log");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("preexisting log"), "output: {output}");

    process
        .master
        .write_all(b"\x03")
        .expect("could not close messages launcher");
    process
        .master
        .flush()
        .expect("could not flush messages launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove messages integration config");
}

#[test]
fn capture_command_returns_to_launcher_and_restores_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        view = "core:capture"
        run = '''printf 'capture-marker:%s\n' "$LAUNCHER_VALUE"'''

        [plugins.core.views.capture]
        type = "capture"
        "#,
    )
    .expect("could not write capture integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\r")
        .expect("could not write capture Enter key");
    process
        .master
        .flush()
        .expect("could not flush capture Enter key");

    let output = wait_for_text(&process.master, "capture-marker:value");
    process
        .master
        .write_all(b"\r")
        .expect("could not write capture return key");
    process
        .master
        .flush()
        .expect("could not flush capture return key");
    let launcher = wait_for_text(&process.master, "[core:default]");

    process
        .master
        .write_all(b"\x03")
        .expect("could not write launcher Ctrl-C key");
    process
        .master
        .flush()
        .expect("could not flush launcher Ctrl-C key");
    let (status, remaining) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    let mut output = output;
    output.extend(launcher);
    output.extend(remaining);
    assert!(
        String::from_utf8_lossy(&output).contains("capture-marker:value"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove capture integration config");
}

#[test]
fn embedded_command_returns_to_launcher_and_restores_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "launcher"
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        view = "core:embedded"
        run = '''printf 'embedded-marker:%s\n' "$LAUNCHER_VALUE"; exit 0'''

        [plugins.core.views.embedded]
        type = "embedded"
        "#,
    )
    .expect("could not write embedded integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\r")
        .expect("could not write embedded Enter key");
    process
        .master
        .flush()
        .expect("could not flush embedded Enter key");

    let mut output = wait_for_text(&process.master, "embedded-marker:value");
    process
        .master
        .write_all(b"\x03")
        .expect("could not write launcher Ctrl-C key");
    process
        .master
        .flush()
        .expect("could not flush launcher Ctrl-C key");
    let (status, remaining) = wait_for_launcher_exit(&mut process);
    output.extend(remaining);

    assert_eq!(status, 0);
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("embedded-marker:value"), "output: {output}");
    assert!(
        !output.contains("finished successfully"),
        "output: {output}"
    );
    fs::remove_dir_all(root).expect("could not remove embedded integration config");
}

fn write_test_config(path: &Path, source: &str) -> std::io::Result<()> {
    fs::write(path, with_viewtypes(source))
}

fn run_dmenu(extra_args: &[&str], input: &[u8], keys: &[u8]) -> RunResult {
    let config = project_config();
    let config = config.to_str().expect("project config path is not UTF-8");
    let mut args = vec!["--dmenu", "--config", config];
    args.extend_from_slice(extra_args);

    let mut process = spawn(&args);
    process
        .input
        .take()
        .expect("dmenu input pipe is missing")
        .write_all(input)
        .expect("could not write dmenu input");
    wait_for_ready(&process.master);
    process
        .master
        .write_all(keys)
        .expect("could not write dmenu key input");
    process
        .master
        .flush()
        .expect("could not flush dmenu key input");

    let status = wait_for_exit(&mut process);
    let mut stdout = Vec::new();
    process
        .output
        .read_to_end(&mut stdout)
        .expect("could not read dmenu stdout");
    RunResult { status, stdout }
}

fn spawn_launcher(config: &Path) -> LauncherProcess {
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
        let config =
            CString::new(config.as_os_str().as_bytes()).expect("config path contains a NUL byte");
        let command = [binary, CString::new("--config").unwrap(), config];
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

fn wait_for_ready(master: &File) {
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

fn wait_for_nonempty_file(path: &Path) -> String {
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

fn wait_for_process_exit(pid: libc::pid_t) {
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

fn wait_for_text(master: &File, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    loop {
        drain_master_into(master, &mut output);
        if String::from_utf8_lossy(&output).contains(needle) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "process did not emit {needle:?}; output: {:?}",
            output
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_launcher_exit(process: &mut LauncherProcess) -> (i32, Vec<u8>) {
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

fn binary_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_tui-launcher")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_tui-launcher is not set")
}

fn project_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config/config.toml")
}

fn temporary_root() -> PathBuf {
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
