mod support;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use support::{
    current_screen, discard_pending_master_output, fixture_config,
    run_tty_invocation_with_blocked_stdout_signal, spawn_launcher, spawn_launcher_with_args,
    spawn_launcher_with_args_and_env, spawn_launcher_with_redirected_stdout, temporary_root,
    wait_for_fresh_screen, wait_for_fresh_text, wait_for_launcher_exit,
    wait_for_launcher_exit_without_reading, wait_for_nonempty_file, wait_for_output,
    wait_for_process_exit, wait_for_ready, wait_for_stable_text, wait_for_text, write_test_config,
};

fn write_workflow_script(root: &Path, workflow: &str, file: &str, source: &str) {
    let path = root.join("workflows").join(workflow).join(file);
    fs::create_dir_all(path.parent().expect("script path has no parent")).unwrap();
    fs::write(path, source).unwrap();
}

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

fn assert_terminal_restored(process: &support::LauncherProcess, output: &[u8]) {
    assert_termios_eq(process.original_termios(), &process.current_termios());
    assert!(
        output
            .windows(b"\x1b[?25h".len())
            .any(|bytes| bytes == b"\x1b[?25h"),
        "launcher output did not restore the cursor: {:?}",
        output
    );
    assert!(
        output
            .windows(b"\x1b[?1049l".len())
            .any(|bytes| bytes == b"\x1b[?1049l"),
        "launcher output did not leave the alternate screen: {:?}",
        output
    );
}

#[test]
fn normal_exit_restores_terminal_state() {
    let mut process = spawn_launcher(&fixture_config());
    wait_for_ready(&process.master);
    assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    assert_terminal_restored(&process, &output);
}

#[test]
fn first_signal_aborts_a_blocked_final_output_write() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow_root = root.join("workflows/custom");
    fs::create_dir_all(workflow_root.join("scripts")).unwrap();
    fs::write(
        workflow_root.join("workflow.toml"),
        "[workflow]\napi = 1\nname = \"custom\"\n\n[views.main.engine]\ntype = \"picker\"\n[views.main.engine.config]\nitems = []\n",
    )
    .unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [workflows.custom.views.main]
        [workflows.custom.views.main.engine]
        type = "picker"
        [workflows.custom.views.main.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.custom.views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        producer = "script"
        [workflows.custom.views.main.commands.accept.handler]
        file = "scripts/large.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "custom",
        "scripts/large.sh",
        r#"#!/usr/bin/env python3
import json

value = "x" * 900000
json.dump(
    {"version": 1, "operation": {"type": "return", "value": value}},
    __import__("sys").stdout,
    separators=(",", ":"),
)
__import__("sys").stdout.write("\n")
"#,
    );
    let args = ["--suite", config.to_str().unwrap()];
    let result = run_tty_invocation_with_blocked_stdout_signal(&args, b"\r", libc::SIGTERM);
    assert_eq!(
        result.status,
        128 + libc::SIGTERM,
        "output: {:?}",
        result.stdout
    );
    assert!(result.stdout.len() < 16 * 1024 * 1024);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_signal_exits_when_outer_terminal_stops_reading() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [workflows.custom.views.main]
        [workflows.custom.views.main.keymap]
        escape = false
        [workflows.custom.views.main.engine]
        type = "embedded"
        [workflows.custom.views.main.engine.config]
        command = ["sh", "-c", "while :; do printf x; done"]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &[]);
    wait_for_ready(&process.master);
    std::thread::sleep(Duration::from_millis(300));
    process.send_signal(libc::SIGTERM);
    let status = wait_for_launcher_exit_without_reading(&mut process, Duration::from_secs(3));
    assert_eq!(status, 128 + libc::SIGTERM);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_signals_restore_terminal_state() {
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT, libc::SIGINT] {
        let mut process = spawn_launcher(&fixture_config());
        wait_for_ready(&process.master);
        assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);
        process.send_signal(signal);

        let (status, output) = wait_for_launcher_exit(&mut process);
        assert_eq!(status, 128 + signal, "launcher output: {:?}", output);
        assert_terminal_restored(&process, &output);
    }
}

#[test]
fn terminal_disconnect_exits_with_sighup_without_panicking() {
    let mut process = spawn_launcher(&fixture_config());
    wait_for_ready(&process.master);
    let master = std::mem::replace(&mut process.master, File::open("/dev/null").unwrap());
    drop(master);

    let (status, _output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 128 + libc::SIGHUP);
}

#[test]
fn signal_exit_terminates_capture_script_source() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow = root.join("workflows/custom");
    let pid_file = root.join("capture.pid");
    fs::create_dir_all(workflow.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        [suite]
        api = 1
        name = "custom"
        entrypoint = "custom:main"

        [workflows]
        custom = { dir = "./workflows/custom" }
        "#,
    )
    .unwrap();
    fs::write(
        workflow.join("workflow.toml"),
        r#"
        [workflow]
        api = 1
        name = "custom"
        entrypoint = "main"
        [views.main.engine]
        type = "capture"
        [views.main.engine.config.output]
        producer = "script"
        [views.main.engine.config.output.handler]
        file = "scripts/output.sh"
        "#,
    )
    .unwrap();
    fs::write(
        workflow.join("scripts/output.sh"),
        "#!/bin/sh\nprintf '%s' $$ > \"$PID_FILE\"\nsleep 30\nprintf '\"done\"\\n'\n",
    )
    .unwrap();
    let pid_path = pid_file.to_string_lossy().to_string();
    let mut process =
        spawn_launcher_with_args_and_env(&config, &[], &[("PID_FILE", pid_path.as_str())]);
    let child_pid = wait_for_nonempty_file(&pid_file)
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();

    process.send_signal(libc::SIGTERM);
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 128 + libc::SIGTERM, "launcher output: {:?}", output);
    wait_for_process_exit(child_pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signal_exit_waits_for_items_worker_cleanup() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow = root.join("workflows/custom");
    let pid_file = root.join("items.pid");
    fs::create_dir_all(workflow.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        [suite]
        api = 1
        name = "custom"
        entrypoint = "custom:main"

        [workflows]
        custom = { dir = "./workflows/custom" }
        "#,
    )
    .unwrap();
    fs::write(
        workflow.join("workflow.toml"),
        r#"
        [workflow]
        api = 1
        name = "custom"
        entrypoint = "main"
        [views.main.engine]
        type = "picker"
        [views.main.engine.config.items]
        producer = "script"
        [views.main.engine.config.items.handler]
        file = "scripts/items.sh"
        "#,
    )
    .unwrap();
    fs::write(
        workflow.join("scripts/items.sh"),
        "#!/bin/sh\nprintf '%s' $$ > \"$PID_FILE\"\nsleep 30\nprintf '[{\"display\":\"Item\"}]'\n",
    )
    .unwrap();
    let pid_path = pid_file.to_string_lossy().to_string();
    let mut process =
        spawn_launcher_with_args_and_env(&config, &[], &[("PID_FILE", pid_path.as_str())]);
    wait_for_ready(&process.master);
    let child_pid = wait_for_nonempty_file(&pid_file)
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();

    process.send_signal(libc::SIGTERM);
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 128 + libc::SIGTERM, "launcher output: {:?}", output);
    wait_for_process_exit(child_pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreground_command_failure_resumes_the_launcher_terminal() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/fail.sh\"" ] }
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/fail.sh",
        "printf failure-marker\n; exit 7\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "external effect failed");
    assert!(
        String::from_utf8_lossy(&output).contains("external effect failed"),
        "launcher output: {:?}",
        output
    );
    assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    assert_terminal_restored(&process, &output);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreground_command_timeout_restores_terminal_and_resumes_the_launcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "sleep 30" ], timeout_ms = 200 }
        "#,
    )
    .unwrap();

    let started = std::time::Instant::now();
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "external effect failed");
    assert!(
        String::from_utf8_lossy(&output).contains("external effect failed"),
        "launcher output: {:?}",
        output
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "command took too long, timeout did not terminate the command"
    );
    assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    assert_terminal_restored(&process, &output);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stopped_foreground_command_reclaims_the_terminal_and_resumes_the_launcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/sleep.sh\"" ] }
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/sleep.sh",
        "printf 'foreground-ready\\n'\nsleep 30\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "foreground-ready");
    process.master.write_all(b"\x1a").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "external effect failed");
    assert!(
        String::from_utf8_lossy(&output).contains("external effect failed"),
        "launcher output: {output:?}"
    );
    assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    assert_terminal_restored(&process, &output);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreground_command_uses_the_controlling_terminal_and_restores_the_launcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/read-terminal.sh\"" ] }
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/read-terminal.sh",
        "printf 'child-ready\\n'\nIFS= read -r input\nprintf 'child-read:%s\\n' \"$input\"\nprintf 'child-stderr\\n' >&2\nIFS= read -r _\n",
    );

    let mut process = spawn_launcher_with_redirected_stdout(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "child-ready");
    process.master.write_all(b"terminal-input\n").unwrap();
    process.master.flush().unwrap();
    let child_output = wait_for_text(&process.master, "child-stderr");
    let child_output = String::from_utf8_lossy(&child_output);
    assert!(child_output.contains("child-read:terminal-input"));
    assert!(child_output.contains("child-stderr"));

    process.master.write_all(b"continue\n").unwrap();
    process.master.flush().unwrap();
    let resumed = wait_for_fresh_screen(&process.master, |visible| visible.contains("Item"));
    assert!(
        String::from_utf8_lossy(&resumed).contains("Item"),
        "launcher did not redraw after foreground command: {resumed:?}"
    );
    assert_eq!(process.current_termios().c_lflag & libc::ICANON, 0);

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    assert_terminal_restored(&process, &output);
    let mut redirected_stdout = process.take_stdout().unwrap();
    let mut redirected_output = Vec::new();
    redirected_stdout
        .read_to_end(&mut redirected_output)
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&redirected_output).contains("child-"),
        "foreground child used redirected launcher stdout: {redirected_output:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreground_command_cancellation_restores_terminal_and_reaps_descendant() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let descendant_pid = root.join("foreground-descendant.pid");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/foreground.sh\""], exit = true }
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/foreground.sh",
        "#!/bin/sh\nsh -c 'trap \"\" TERM; printf \"%s\" $$ > \"$DESCENDANT_PID\"; while :; do sleep 1; done' &\nwhile :; do sleep 1; done\n",
    );
    let descendant_pid_path = descendant_pid.to_string_lossy().into_owned();
    let mut process = spawn_launcher_with_args_and_env(
        &config,
        &[],
        &[("DESCENDANT_PID", descendant_pid_path.as_str())],
    );
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let child_pid = wait_for_nonempty_file(&descendant_pid)
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();

    process.send_signal(libc::SIGTERM);
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 128 + libc::SIGTERM, "launcher output: {:?}", output);
    assert_terminal_restored(&process, &output);
    wait_for_process_exit(child_pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signal_exit_terminates_embedded_process() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let pid_file = root.join("embedded.pid");
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [workflows.custom.views.main]
        [workflows.custom.views.main.keymap]
        escape = false
        [workflows.custom.views.main.engine]
        type = "embedded"
        [workflows.custom.views.main.engine.config]
        command = ["sh", "-c", "printf '%s' $$ > \"$PID_FILE\"; sleep 30"]
        "#,
    )
    .unwrap();
    let pid_path = pid_file.to_string_lossy().to_string();
    let mut process =
        spawn_launcher_with_args_and_env(&config, &[], &[("PID_FILE", pid_path.as_str())]);
    wait_for_ready(&process.master);
    let child_pid = wait_for_nonempty_file(&pid_file)
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();

    process.send_signal(libc::SIGTERM);
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 128 + libc::SIGTERM, "launcher output: {:?}", output);
    assert_terminal_restored(&process, &output);
    wait_for_process_exit(child_pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_pty_disconnect_cancels_a_running_child() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let pid_file = root.join("embedded-disconnect.pid");
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [workflows.custom.views.main]
        [workflows.custom.views.main.keymap]
        escape = false
        [workflows.custom.views.main.engine]
        type = "embedded"
        [workflows.custom.views.main.engine.config]
        command = ["sh", "-c", "printf '%s' $$ > \"$PID_FILE\"; exec 0<&- 1>&- 2>&-; sleep 30"]
        "#,
    )
    .unwrap();
    let pid_path = pid_file.to_string_lossy().to_string();
    let mut process =
        spawn_launcher_with_args_and_env(&config, &[], &[("PID_FILE", pid_path.as_str())]);
    wait_for_ready(&process.master);
    let child_pid = wait_for_nonempty_file(&pid_file)
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    wait_for_process_exit(child_pid);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_log_open_failure_is_reported_without_stopping_the_launcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        log_file = "/dev/full"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "runtime log disabled");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_log_warning_reaches_stderr_on_immediate_exit() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        log_file = "/dev/full"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]
        [workflows.core.views.default.commands.exit]
        key = "enter"
        label = "Exit"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/exit.sh\""], exit = true }
        "#,
    )
    .unwrap();
    write_workflow_script(&root, "core", "scripts/exit.sh", "printf 'done\\n'\n");
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output).contains("runtime log disabled"),
        "immediate exit dropped the runtime warning: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_copy_feedback_requires_success_and_is_logged_on_exit() {
    for (code, exit) in [(0, false), (0, true), (7, false)] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(&config, &format!(r#"
            default_view = "core:default"
            log_file = "runtime.jsonl"
            [workflows.core.views.default.engine]
            type = "picker"
            [workflows.core.views.default.engine.config]
            items = [{{display = "Item", value = "value"}}]
            [workflows.core.views.default.commands.copy]
            key = "enter"
            type = "run"
            producer = "declared"
            handler = {{mode = "foreground", argv = ["sh", "-c", "exit {code}"], exit = {exit}, success_message = "Copied to clipboard"}}
        "#)).unwrap();
        let mut process = spawn_launcher(&config);
        wait_for_ready(&process.master);
        process.master.write_all(b"\r").unwrap();
        process.master.flush().unwrap();
        if !exit {
            wait_for_text(
                &process.master,
                if code == 0 {
                    "Copied to clipboard"
                } else {
                    "ERROR"
                },
            );
            process.master.write_all(b"\x03").unwrap();
            process.master.flush().unwrap();
        }
        let (status, _) = wait_for_launcher_exit(&mut process);
        assert_eq!(status, 0);
        let log = fs::read_to_string(root.join("runtime.jsonl")).unwrap();
        let records: Vec<serde_json::Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0]["metadata"]["level"],
            if code == 0 { "info" } else { "error" }
        );
        assert_eq!(records[0]["metadata"]["view"], "core:default");
        assert_eq!(log.contains("Copied to clipboard"), code == 0);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn loads_items_and_runs_a_view_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]
        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'command-marker:value\\n'"], exit = true }
        "#,
    )
    .expect("could not write items command config");
    write_workflow_script(
        &root,
        "core",
        "scripts/command.sh",
        "printf 'command-marker:%s\\n' \"$1\"\n",
    );

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
    assert_terminal_restored(&process, &output);
    assert!(
        String::from_utf8_lossy(&output).contains("command-marker:value"),
        "launcher output did not contain command marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove items command config");
}

#[test]
fn application_launch_detaches_started_process_from_launcher_group() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let app_pid = root.join("app.pid");
    let gio = root.join("bin/gio");
    write_test_config(
        &config,
        r#"
        default_view = "apps:main"

        [workflows.apps.views.main]
        [workflows.apps.views.main.engine]
        type = "picker"
        [workflows.apps.views.main.engine.config]
        items = [{ display = "App", value = "fixture.desktop" }]

        [workflows.apps.views.main.commands.open]
        key = "enter"
        label = "Open"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "exec sh \"$TFLOW_WORKFLOW_DIR/scripts/open.sh\" fixture.desktop"], exit = true }
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "apps",
        "scripts/open.sh",
        "exec setsid --wait gio launch \"$@\" </dev/null >/dev/null 2>&1\n",
    );
    fs::create_dir_all(gio.parent().unwrap()).unwrap();
    fs::write(
        &gio,
        "#!/bin/sh\nprintf 'gio-log\\n'\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"$GIO_APP_PID_FILE\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&gio).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&gio, permissions).unwrap();
    let app_pid = app_pid.to_string_lossy().into_owned();
    let path = format!(
        "{}:{}",
        gio.parent().unwrap().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut process = spawn_launcher_with_args_and_env(
        &config,
        &[],
        &[
            ("GIO_APP_PID_FILE", app_pid.as_str()),
            ("PATH", path.as_str()),
        ],
    );
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "App");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        !String::from_utf8_lossy(&output).contains("gio-log"),
        "application logs leaked into launcher output: {:?}",
        output
    );
    let app_pid = wait_for_nonempty_file(&root.join("app.pid"))
        .trim()
        .parse::<libc::pid_t>()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(app_pid, 0) },
        0,
        "launched app was killed"
    );
    unsafe {
        libc::kill(app_pid, libc::SIGKILL);
    }
    wait_for_process_exit(app_pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn loads_items_from_a_native_toml_array() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{ display = "Static item", value = "static-value" }]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'static-marker:static-value\\n'"], exit = true }
        "#,
    )
    .expect("could not write static items config");
    write_workflow_script(
        &root,
        "core",
        "scripts/static.sh",
        "printf 'static-marker:%s\\n' \"$1\"\n",
    );

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
        String::from_utf8_lossy(&output).contains("static-marker:static-value"),
        "launcher output did not contain static marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove static items config");
}

#[test]
fn capture_does_not_render_its_private_query_as_a_host_input_row() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:capture"

        [workflows.core.views.capture.engine]
        type = "capture"
        [workflows.core.views.capture.engine.config]
        output = "fixed-capture"
        [workflows.core.views.capture.query]
        type = "object"
        secret = { type = "string" }
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:capture", "--secret=hidden-query"]);
    let output = wait_for_text(&process.master, "fixed-capture");
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(!visible.contains("hidden-query"), "screen: {visible}");
    assert_eq!(
        visible
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim),
        Some("fixed-capture")
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_embedded_view_receives_typed_query_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.direct]
        [workflows.core.views.direct.engine]
        type = "embedded"
        [workflows.core.views.direct.engine.config]
        command = ["sh", "-lc", "printf 'input=%s\\n' \"$TFLOW_INPUT\""]
        [workflows.core.views.direct.query]
        type = "object"
        input = "text"
        text = { type = "string", default = "" }
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:direct", "--text=from-option"]);
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("input=from-option"), "output: {output}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_does_not_render_its_private_query_as_a_host_input_row() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:embedded"

        [workflows.core.views.embedded.engine]
        type = "embedded"
        [workflows.core.views.embedded.engine.config]
        command = ["sh", "-c", "printf 'fixed-embedded'; trap 'exit 0' INT TERM; while :; do sleep 1; done"]
        [workflows.core.views.embedded.query]
        type = "object"
        secret = { type = "string" }
        "#,
    )
    .unwrap();

    let mut process =
        spawn_launcher_with_args(&config, &["core:embedded", "--secret=hidden-query"]);
    let output = wait_for_text(&process.master, "fixed-embedded");
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(!visible.contains("hidden-query"), "screen: {visible}");
    assert_eq!(
        visible
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim),
        Some("fixed-embedded")
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_embedded_view_runs_without_picker_intent() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.direct]
        [workflows.core.views.direct.engine]
        type = "embedded"
        [workflows.core.views.direct.engine.config]
        command = ["sh", "-lc", "printf 'direct-embedded\\n'; exit 0"]
"#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:direct"]);
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("direct-embedded"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn waits_for_items_before_running_enter_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'picker-marker:value\\n'"], exit = true }
        "#,
    )
    .expect("could not write launcher integration config");
    write_workflow_script(
        &root,
        "core",
        "scripts/picker.sh",
        "printf 'picker-marker:%s\\n' \"$1\"\n",
    );

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
        String::from_utf8_lossy(&output).contains("picker-marker:value"),
        "launcher output did not contain command marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove launcher integration config");
}

#[test]
fn view_commands_accept_unreserved_control_bindings() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.default.commands.run]
        key = "ctrl+r"
        label = "Run"
        type = "run"


        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'ctrl-command:value\\n'"], exit = true }
        "#,
    )
    .expect("could not write control command config");
    write_workflow_script(
        &root,
        "core",
        "scripts/ctrl.sh",
        "printf 'ctrl-command:%s\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"\x12")
        .expect("could not write Ctrl-R command key");
    process
        .master
        .flush()
        .expect("could not flush Ctrl-R command key");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("ctrl-command:value"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove control command config");
}

#[test]
fn view_command_overrides_printable_picker_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [
            { display = "First", value = "first" },
            { display = "Second", value = "second" },
        ]
        [workflows.core.views.default.keymap]
        "space" = "select_next"
        [workflows.core.views.default.commands.space]
        key = "space"
        label = "HiddenSpace"
        type = "run"

        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'hidden-space-command\\n'"], exit = true }

        [workflows.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "run"

        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'space-selection:first\\n'"], exit = true }
        "#,
    )
    .expect("could not write printable keymap config");
    write_workflow_script(
        &root,
        "core",
        "scripts/hidden-space.sh",
        "printf 'hidden-space-command\\n'\n",
    );
    write_workflow_script(
        &root,
        "core",
        "scripts/space-selection.sh",
        "printf 'space-selection:%s\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let initial = wait_for_text(&process.master, "First");
    let initial = String::from_utf8_lossy(&initial);
    assert!(initial.contains("Commands"), "output: {initial}");
    assert!(initial.contains("Accept"), "output: {initial}");
    process.master.write_all(b" ").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("hidden-space-command"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_keys_observe_dynamic_picker_bindings() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "First", value = "first"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"x\x1b\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn toggle_preview_without_configuration_consumes_its_bound_key() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "First", value = "first"}]
        [workflows.core.views.default.keymap]
        space = "toggle_preview"

        [workflows.core.views.default.commands.inspect]
        key = "enter"
        label = "Inspect"
        type = "run"

        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'toggle-query::end\\n'"], exit = true }
        "#,
    )
    .expect("could not write default preview config");
    write_workflow_script(
        &root,
        "core",
        "scripts/inspect.sh",
        "printf 'toggle-query:%s:end\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let initial = wait_for_text(&process.master, "First");
    let initial = String::from_utf8_lossy(&initial);
    assert!(initial.contains("Inspect"), "output: {initial}");

    process.master.write_all(b" \r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    let output = String::from_utf8_lossy(&output);
    assert_eq!(status, 0, "output: {output}");
    assert!(output.contains("toggle-query::end"), "output: {output}");
    assert!(!output.contains("space-command"), "output: {output}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn uppercase_printable_keymap_binding_matches_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [
            { display = "First", value = "first" },
            { display = "Second", value = "second" },
        ]
        [workflows.core.views.default.keymap]
        a = "select_next"
        [workflows.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "run"

        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'uppercase-selection:second\\n'"], exit = true }
        "#,
    )
    .expect("could not write uppercase keymap config");
    write_workflow_script(
        &root,
        "core",
        "scripts/uppercase-selection.sh",
        "printf 'uppercase-selection:%s\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "First");
    process.master.write_all(b"A\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("uppercase-selection:second"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unbound_uppercase_printable_input_reaches_the_editor() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = []
        [workflows.core.views.default.keymap]
        a = "select_next"
        [workflows.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "run"
        producer = "script"
        [workflows.core.views.default.commands.accept.handler]
        file = "scripts/uppercase-input.sh"
        "#,
    )
    .expect("could not write uppercase input config");
    write_workflow_script(
        &root,
        "core",
        "scripts/uppercase-input.sh",
        r#"#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
value = request["context"]["engine"]["state"]["input"]
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "uppercase-input:%s\\n", value],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"Z\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("uppercase-input:Z"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_default_view_command_overrides_builtin_tab_completion() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]

        [workflows.core.views.default.commands.run]
        key = "tab"
        label = "Run"
        type = "run"

        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'tab-command\\n'"], exit = true }
        "#,
    )
    .expect("could not write Tab command config");
    write_workflow_script(&root, "core", "scripts/tab.sh", "printf 'tab-command'\n");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\t").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(String::from_utf8_lossy(&output).contains("tab-command"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_input_sequence_exits_cleanly_after_escape() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]
        "#,
    )
    .expect("could not write route completion config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\t\x1b[999~\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replacing_items_request_cancels_the_previous_script() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow_root = root.join("workflows/core");
    let script_root = workflow_root.join("scripts");
    fs::create_dir_all(&script_root).expect("could not create cancellation script directory");
    fs::write(
        workflow_root.join("workflow.toml"),
        "[workflow]\napi = 1\nname = \"core\"\n\n[views.placeholder.engine]\ntype = \"picker\"\n[views.placeholder.engine.config]\n",
    )
    .expect("could not write cancellation workflow manifest");
    let old_pid_path = root.join("old.pid");
    fs::write(
        script_root.join("items.sh"),
        format!(
            r#"request=$(cat)
if printf '%s' "$request" | grep -q '"input":"new"'; then
    printf '{{"version":1,"items":[{{"display":"new-result"}}]}}\n'
else
    printf '%s\n' "$$" > "{}"
    sleep 10
    printf '{{"version":1,"items":[{{"display":"old-result"}}]}}\n'
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
        log_file = "runtime.jsonl"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        [workflows.core.views.default.engine.config.items]
        producer = "script"
        [workflows.core.views.default.engine.config.items.handler]
        file = "scripts/items.sh"
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
fn ctrl_g_opens_native_parameter_form_and_replaces_the_target_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "example:main"

        [workflows.example.views.main.engine]
        type = "picker"
        [workflows.example.views.main.engine.config]
        items = [{ display = "Ready", value = "ready" }]

        [workflows.example.views.main.query]
        type = "object"
        input = "name"
        name = { type = "string", default = "" }
    "#,
    )
    .unwrap();
    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "Ready");
    send_bytes(&mut process, b"\x07");
    wait_for_text(&process.master, "name");
    send_bytes(&mut process, b"changed\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("changed")
            && !screen.contains("__query:main")
            && !screen.contains("__form:main")
    });
    send_bytes(&mut process, b"\x03");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ctrl_g_builds_a_form_from_the_target_query_and_replaces_typed_parameters() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "dynamic:main"

        [workflows.dynamic.views.main]
        [workflows.dynamic.views.main.query]
        type = "object"
        input = "title"
        title = { type = "string", default = "initial title" }
        count = { type = "integer", default = 2 }
        enabled = { type = "boolean", default = false }
        tags = { type = "array<string>", default = ["one"] }
        metadata = { type = "object", default = { mode = "safe" } }

        [workflows.dynamic.views.main.engine]
        type = "capture"
        [workflows.dynamic.views.main.engine.config.output]
        producer = "script"
        [workflows.dynamic.views.main.engine.config.output.handler]
        script = '''#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
parameters = request.get("context", {}).get("parameters", {})
output = "|".join([
    str(parameters.get("title", "")),
    str(parameters.get("count", "")),
    str(parameters.get("enabled", "")),
    json.dumps(parameters.get("tags", []), separators=(",", ":")),
    json.dumps(parameters.get("metadata", {}), separators=(",", ":")),
])
json.dump({"version": 1, "output": output}, sys.stdout)
sys.stdout.write("\n")
'''
    "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "dynamic");
    send_bytes(&mut process, b"\x07");
    wait_for_fresh_screen(&process.master, |screen| {
        let field_position = |label: &str| {
            screen.lines().position(|line| {
                line.contains(&format!("> {label}")) || line.contains(&format!("  {label}"))
            })
        };
        let Some(title) = field_position("title") else {
            return false;
        };
        let Some(count) = field_position("count") else {
            return false;
        };
        let Some(enabled) = field_position("enabled") else {
            return false;
        };
        title < count
            && count < enabled
            && screen.contains("initial title")
            && screen.contains("> title")
            && screen.contains("2")
            && screen.contains("false")
    });

    send_bytes(
        &mut process,
        b"\x15changed\t\x157\t \t\x15{\"mode\":\"fast\"}\t\x15[\"x\",\"y\"]\r",
    );
    wait_for_fresh_text(
        &process.master,
        "changed|7|True|[\"x\",\"y\"]|{\"mode\":\"fast\"}",
    );
    send_bytes(&mut process, b"\x1b");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ctrl_g_repeated_does_not_open_nested_query_panel() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "dynamic:main"

        [workflows.dynamic.views.main]
        [workflows.dynamic.views.main.query]
        type = "string"

        [workflows.dynamic.views.main.engine]
        type = "picker"
        [workflows.dynamic.views.main.engine.config]
        items = [{ display = "Ready", value = "ready" }]
    "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "Ready");

    // First Ctrl+G opens the query form
    send_bytes(&mut process, b"\x07");
    wait_for_text(&process.master, "__query:main");

    // Second Ctrl+G should be a no-op (stay), NOT open a nested query panel
    send_bytes(&mut process, b"\x07");

    // Single Escape should exit the query form and return to the main view
    send_bytes(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Ready") && !screen.contains("__query:main")
    });

    send_bytes(&mut process, b"\x03");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ctrl_g_builds_a_single_query_field_for_a_string_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "dynamic:main"

        [workflows.dynamic.views.main]
        [workflows.dynamic.views.main.query]
        type = "string"

        [workflows.dynamic.views.main.engine]
        type = "picker"
        [workflows.dynamic.views.main.engine.config]
        items = [{ display = "Ready", value = "ready" }]
    "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "dynamic");
    send_bytes(&mut process, b"initial");
    send_bytes(&mut process, b"\x07");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("> Query") && screen.contains("initial")
    });
    send_bytes(&mut process, b"\x15changed\r");
    wait_for_fresh_text(&process.master, "changed");
    send_bytes(&mut process, b"\x03");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    fs::remove_dir_all(root).unwrap();
}

fn send_bytes(process: &mut support::LauncherProcess, bytes: &[u8]) {
    process.master.write_all(bytes).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn picker_left_prefix_marks_views_pushed_on_a_parent() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Open app", value = "app"}]
        [workflows.core.views.default.commands.open]
        key = "enter"
        label = "Open app"
        type = "navigate"
        producer = "declared"
        handler = { target = "apps:main" }

        [workflows.apps.views.main]
        [workflows.apps.views.main.engine]
        type = "picker"
        [workflows.apps.views.main.engine.config]
        items = [{display = "App item", value = "item"}]
"#,
    )
    .expect("could not write left prefix config");
    fs::write(
        root.join("settings.toml"),
        "[defaults.picker]\nleft_prefix = \"\u{3008}\"\n",
    )
    .expect("could not write left prefix settings");

    fn visible_screen(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes)
            .rsplit("--- visible screen ---")
            .next()
            .unwrap_or_default()
            .to_string()
    }

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);

    // The root View has no parent, so the input line stays bare.
    let root_screen = visible_screen(&wait_for_text(&process.master, "Open app"));
    assert!(
        !root_screen.contains('\u{3008}'),
        "root view showed a left prefix: {root_screen}"
    );

    // Opening the item pushes apps:main, which now advertises its parent.
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let child_screen = visible_screen(&wait_for_text(&process.master, "App item"));
    assert!(
        child_screen
            .lines()
            .any(|line| line.trim_start().starts_with('\u{3008}')),
        "pushed view did not show a left prefix: {child_screen}"
    );

    // Returning to the root drops the prefix again.
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let back_screen = visible_screen(&wait_for_text(&process.master, "Open app"));
    assert!(
        !back_screen.contains('\u{3008}'),
        "returning to the root kept the left prefix: {back_screen}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove left prefix config");
}

#[test]
fn picker_left_prefix_route_mode_uses_the_alias() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [aliases]
        app = "apps:main"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Open app", value = "app"}]
        [workflows.core.views.default.commands.open]
        key = "enter"
        label = "Open app"
        type = "navigate"
        producer = "declared"
        handler = { target = "apps:main" }

        [workflows.apps.views.main]
        [workflows.apps.views.main.engine]
        type = "picker"
        [workflows.apps.views.main.engine.config]
        items = [{display = "App item", value = "item"}]
"#,
    )
    .expect("could not write route prefix config");
    fs::write(
        root.join("settings.toml"),
        "[defaults.picker]\nleft_prefix = \"$route\"\n",
    )
    .expect("could not write route prefix settings");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Open app");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let screen = wait_for_text(&process.master, "App item");
    let screen = String::from_utf8_lossy(&screen);
    let visible = screen.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible
            .lines()
            .any(|line| line.trim_start().starts_with("app ")),
        "pushed view did not show the route alias prefix: {visible}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove route prefix config");
}

// Three Picker levels whose suite aliases double as the rendered left prefix,
// so the input line identifies the View that Backspace landed on.
const BACKSPACE_CHAIN_CONFIG: &str = r#"
    default_view = "core:default"

    [aliases]
    mid = "demo:mid"
    leaf = "demo:leaf"

    [workflows.core.views.default]
    [workflows.core.views.default.engine]
    type = "picker"
    [workflows.core.views.default.engine.config]
    items = [{display = "Root entry", value = "root"}]
    [workflows.core.views.default.commands.open]
    key = "enter"
    label = "Open mid"
    type = "navigate"
    producer = "declared"
    handler = { target = "demo:mid" }

    [workflows.demo.views.mid]
    [workflows.demo.views.mid.engine]
    type = "picker"
    [workflows.demo.views.mid.engine.config]
    items = [{display = "Mid entry", value = "mid"}]
    [workflows.demo.views.mid.commands.open]
    key = "enter"
    label = "Open leaf"
    type = "navigate"
    producer = "declared"
    handler = { target = "demo:leaf" }

    [workflows.demo.views.leaf]
    [workflows.demo.views.leaf.engine]
    type = "picker"
    [workflows.demo.views.leaf.engine.config]
    items = [{display = "Leaf entry", value = "leaf"}]
"#;

/// Push `demo:leaf`, press Backspace on its empty input line, then type a
/// character so the frame is unmistakably fresh even when Backspace did
/// nothing.
fn backspace_chain_screen(settings: &str, expected_item: &str, expected_input: &str) -> String {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(&config, BACKSPACE_CHAIN_CONFIG)
        .expect("could not write backspace chain config");
    fs::write(root.join("settings.toml"), settings).expect("could not write backspace settings");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Open mid");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Open leaf");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Leaf entry");

    process.master.write_all(b"\x7f").unwrap();
    process.master.flush().unwrap();
    process.master.write_all(b"e").unwrap();
    process.master.flush().unwrap();
    let screen = wait_for_fresh_screen(&process.master, |visible| {
        visible.contains(expected_item) && visible.lines().any(|line| line.trim() == expected_input)
    });

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove backspace chain config");

    String::from_utf8_lossy(&screen)
        .rsplit("--- visible screen ---")
        .next()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn left_prefix_backspace_parent_returns_one_level() {
    let screen = backspace_chain_screen(
        "[defaults.picker]\nleft_prefix = \"$route\"\nleft_prefix_backspace = \"parent\"\n",
        "Mid entry",
        "mid e",
    );
    assert!(screen.contains("Mid entry"), "{screen}");
    assert!(screen.contains("Open leaf"), "{screen}");
}

#[test]
fn left_prefix_backspace_root_returns_to_the_root_view() {
    let screen = backspace_chain_screen(
        "[defaults.picker]\nleft_prefix = \"$route\"\nleft_prefix_backspace = \"root\"\n",
        "Root entry",
        "e",
    );
    assert!(screen.contains("Root entry"), "{screen}");
    assert!(screen.contains("Open mid"), "{screen}");
}

#[test]
fn left_prefix_backspace_unset_leaves_backspace_inert() {
    let screen = backspace_chain_screen(
        "[defaults.picker]\nleft_prefix = \"$route\"\n",
        "Leaf entry",
        "leaf e",
    );
    assert!(screen.contains("Leaf entry"), "{screen}");
}

#[test]
fn fixture_input_placeholder_shows_until_the_user_types() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &[]);
    wait_for_ready(&process.master);

    // core:default declares `input_placeholder` in the shared fixture, so the
    // empty query row shows the hint instead of a lone cursor cell.
    let entry = wait_for_text(&process.master, "Enter Open");
    let entry = String::from_utf8_lossy(&entry);
    let visible = entry.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible
            .lines()
            .any(|line| line.trim() == "Type a route or search"),
        "entry placeholder was not rendered: {visible}"
    );

    // Typing replaces the hint: it was presentation, never query input.
    process.master.write_all(b"s").unwrap();
    process.master.flush().unwrap();
    let typed = wait_for_fresh_screen(&process.master, |visible| {
        !visible.contains("Type a route or search")
    });
    let typed = String::from_utf8_lossy(&typed);
    let visible = typed.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible.lines().any(|line| line.trim() == "s"),
        "typing did not replace the placeholder: {visible}"
    );

    // A pushed View keeps its own hint, rendered after the route prefix.
    process.master.write_all(b"\x15").unwrap();
    process.master.flush().unwrap();
    process.master.write_all(b"app ").unwrap();
    process.master.flush().unwrap();
    let child = wait_for_text(&process.master, "Search applications");
    let child = String::from_utf8_lossy(&child);
    let visible = child.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible
            .lines()
            .any(|line| line.trim_start().starts_with("app Search applications")),
        "pushed placeholder was not rendered after the prefix: {visible}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn ctrl_k_passes_page_commands_through_selector_query_and_invokes_an_opaque_ref() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["sys:main"]);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Run");

    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "sys:output");

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn command_palette_opens_for_an_empty_aggregate_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = []
        [workflows.core.views.default.commands.aggregate]
        label = "Aggregate command"
        type = "return"
        producer = "declared"
        handler = { value = "aggregate" }

        [workflows.selectors.views.commands]
        [workflows.selectors.views.commands.engine]
        type = "picker"
        [workflows.selectors.views.commands.engine.config.items]
        producer = "script"
        [workflows.selectors.views.commands.engine.config.items.handler]
        file = "scripts/items.sh"
        [workflows.selectors.views.commands.query]
        type = "object"
        input = "search"
        search = { type = "string", default = "" }
        commands = { type = "array<object>", default = [] }
        "#,
    )
    .expect("could not write empty aggregate command config");
    write_workflow_script(
        &root,
        "selectors",
        "scripts/items.sh",
        r#"#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
commands = request["context"]["parameters"].get("commands", [])
items = [
    {"display": command["label"], "value": command["label"]}
    for command in commands
]
json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "(no matches)");
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "Aggregate command");
    assert!(
        String::from_utf8_lossy(&output).contains("Aggregate command"),
        "aggregate command did not reach the command palette: {output:?}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn aggregate_view_commands_remain_in_footer_and_dispatch_directly() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        [[workflows.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [workflows.apps.views.default.engine]
        type = "picker"
        [workflows.apps.views.default.engine.config]
        items = []
        [workflows.core.views.default.commands.aggregate]
        key = "enter"
        label = "Aggregate command"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'aggregate-view-command\\n'"], exit = true }
        "#,
    )
    .expect("could not write aggregate View command config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let initial = wait_for_text(&process.master, "Aggregate command");
    assert!(
        String::from_utf8_lossy(&initial).contains("Aggregate command"),
        "aggregate View command did not reach the footer: {initial:?}"
    );

    process.master.write_all(b"query").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |visible| {
        visible.contains("query") && visible.contains("Aggregate command")
    });
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "launcher output: {output:?}");
    assert!(
        String::from_utf8_lossy(&output).contains("aggregate-view-command"),
        "aggregate View command did not dispatch: {output:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_selector_does_not_expose_an_owner_from_stale_items() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["sys:main"]);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"no-match\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "(no matches)");
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible.contains("Info"),
        "page command disappeared: {visible}"
    );
    assert!(
        visible.contains("Run"),
        "page command disappeared: {visible}"
    );
    assert!(
        !visible.contains("Open"),
        "stale owner command leaked: {visible}"
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    discard_pending_master_output(&process.master);
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn items_errors_are_logged_and_do_not_block_exit() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        log_file = "runtime.jsonl"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config.items]
        producer = "script"
        [workflows.core.views.default.engine.config.items.handler]
        script = "printf 'not-json\\n'"
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
            .contains("valid items response")
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
fn view_keymap_dispatches_workflow_command_with_selected_item() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.commands.page]
        label = "Page"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'page-command\\n'"], exit = true }

        [workflows.core.views.default]
        [workflows.core.views.default.keymap]
        "ctrl+r" = "core:page"

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Row", value = "row"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Row");
    process.master.write_all(b"\x12").unwrap(); // Ctrl-R
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("page-command"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn item_bindings_dispatch_and_display_in_footer() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.commands.open]
        label = "Selection"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'item-selection-command:row\\n'"], exit = true }

        [workflows.core.views.default]
        keymap_mode = "item_merge"
        [workflows.core.views.default.keymap]

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Row", value = "row", bindings = { enter = "core:open" }}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let footer = wait_for_text(&process.master, "Selection");
    let footer = String::from_utf8_lossy(&footer);
    assert!(
        footer.contains("Selection"),
        "item selection command did not reach the footer: {footer:?}"
    );

    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("item-selection-command:row"),
        "item selection command did not dispatch: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn navigation_without_query_uses_the_target_view_default() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "navigate"
        producer = "declared"
        handler = { target = "core:capture" }

        [workflows.core.views.capture]
        [workflows.core.views.capture.engine]
        type = "capture"
        [workflows.core.views.capture.engine.config]
        output = "target-default"
        [workflows.core.views.capture.query]
        type = "object"
        input = "text"
        text = { type = "string", default = "target-default" }
        "#,
    )
    .expect("could not write navigation default config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\r")
        .expect("could not navigate without a query");
    process
        .master
        .flush()
        .expect("could not flush navigation key");
    let output = wait_for_text(&process.master, "target-default");
    assert!(
        String::from_utf8_lossy(&output).contains("target-default"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x1b")
        .expect("could not return from default capture view");
    process
        .master
        .flush()
        .expect("could not flush capture return");
    let _ = wait_for_text(&process.master, "Item");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove navigation default config");
}

#[test]
fn capture_command_returns_to_launcher_and_restores_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        log_file = "runtime.jsonl"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"
        producer = "declared"
        handler = { target = "core:capture" }

        [workflows.core.views.capture]
        alias = "cap"
        [workflows.core.views.capture.engine]
        type = "capture"
        [workflows.core.views.capture.engine.config]
        output = "capture-marker:value\u001b[31m\n\u4e16\u754c\u001b[0m"

        [workflows.core.views.capture.keymap]
        enter = false
        "ctrl+y" = "copy"
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
        .write_all(b"\x19")
        .expect("could not write capture copy key");
    process
        .master
        .flush()
        .expect("could not flush capture copy key");
    let copied = wait_for_text(&process.master, "Copied to clipboard");
    let sequence = b"\x1b]52;c;Y2FwdHVyZS1tYXJrZXI6dmFsdWUbWzMxbQrkuJbnlYwbWzBt\x07";
    assert!(
        copied
            .windows(sequence.len())
            .any(|bytes| bytes == sequence)
    );
    let log = fs::read_to_string(root.join("runtime.jsonl")).unwrap();
    let record: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(record["metadata"]["level"], "info");
    assert_eq!(record["metadata"]["view"], "core:capture");
    assert_eq!(record["metadata"]["message"], "Copied to clipboard");
    process
        .master
        .write_all(b"\x1b")
        .expect("could not write capture return key");
    process
        .master
        .flush()
        .expect("could not flush capture return key");
    let launcher = wait_for_text(&process.master, "Item");

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
    output.extend(copied);
    output.extend(launcher);
    output.extend(remaining);
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("capture-marker:value"), "output: {output}");
    assert!(output.contains("cap"), "output: {output}");
    fs::remove_dir_all(root).expect("could not remove capture integration config");
}

#[test]
fn capture_keeps_workflow_commands_available() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [defaults.capture.bindings]
        copy = ["ctrl+y"]

        [workflows.core.commands.details]
        key = "ctrl+k"
        label = "Details"
        type = "call"
        producer = "declared"
        handler = { target = "core:details" }

        [workflows.core.views.default.engine]
        type = "capture"
        [workflows.core.views.default.engine.config]
        output = "xxxxxxxxxxxxxx"

        [workflows.core.views.details.engine]
        type = "capture"
        [workflows.core.views.details.engine.config]
        output = "footer-capture"
        "#,
    )
    .expect("could not write capture footer config");

    let mut process = spawn_launcher(&config);
    let root_output = wait_for_text(&process.master, "xxxxxxxxxxxxxx");
    assert!(String::from_utf8_lossy(&root_output).contains("xxxxxxxxxxxxxx"));

    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let called_output = wait_for_text(&process.master, "footer-capture");
    assert!(String::from_utf8_lossy(&called_output).contains("footer-capture"));
    let mut observed = root_output;
    observed.extend(called_output);
    assert!(
        !String::from_utf8_lossy(&observed).contains("\x1b]52;"),
        "footer key unexpectedly copied capture output: {observed:?}"
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let _ = wait_for_text(&process.master, "xxxxxxxxxxxxxx");
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove capture footer config");
}

#[test]
fn embedded_command_returns_to_launcher_and_restores_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"
        producer = "declared"
        handler = { target = "core:embedded" }

        [workflows.core.views.embedded]
        alias = "emb"
        [workflows.core.views.embedded.engine]
        type = "embedded"
        [workflows.core.views.embedded.engine.config]
        command = ["sh", "-lc", "printf 'embedded-marker:value\\n'; exit 0"]
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

    let mut output = wait_for_output(&process.master, b"embedded-marker:value");
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
    assert!(output.contains("emb"), "output: {output}");
    assert!(
        !output.contains("finished successfully"),
        "output: {output}"
    );
    fs::remove_dir_all(root).expect("could not remove embedded integration config");
}

#[test]
fn invalid_embedded_command_is_rejected_during_startup() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        [workflows.core.views.broken]
        [workflows.core.views.broken.engine]
        type = "embedded"
        [workflows.core.views.broken.engine.config]
        command = "sh"
"#,
    )
    .expect("could not write failed navigation integration config");

    let mut process = spawn_launcher(&config);
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 1, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output)
            .contains("Error: view \"core:broken\" embedded command must be an argv array"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove invalid navigation config");
}

#[test]
fn qualified_view_path_navigates_to_any_engine() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        [workflows.core.views.embedded]
        [workflows.core.views.embedded.engine]
        type = "embedded"
        [workflows.core.views.embedded.engine.config]
        command = ["sh", "-lc", "printf 'route-marker\\n'"]
"#,
    )
    .expect("could not write qualified route integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"core:embedded printf route-marker")
        .expect("could not write qualified embedded route");
    process
        .master
        .flush()
        .expect("could not flush qualified embedded route");

    let mut output = wait_for_output(&process.master, b"route-marker");
    let launcher = wait_for_stable_text(&process.master, "0 of 0");
    output.extend(launcher);
    process
        .master
        .write_all(b"\x03")
        .expect("could not close launcher after embedded route");
    process
        .master
        .flush()
        .expect("could not flush launcher close");
    let (status, remaining) = wait_for_launcher_exit(&mut process);
    output.extend(remaining);

    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("route-marker"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove qualified route config");
}

#[test]
fn script_command_producer_receives_stdin_and_runs_its_operation() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [{ display = "Item", value = "value" }]

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "script"
        [workflows.core.views.default.commands.run.handler]
        file = "scripts/command.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/command.sh",
        r#"#!/bin/sh
set -eu
exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
value = item.get("value") if isinstance(item, dict) else None
if request.get("entrypoint") != "command" or value != "value":
    raise SystemExit("unexpected command producer request")
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "producer-command:%s\\n", value],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output).contains("producer-command:value"),
        "producer output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn picker_items_producer_receives_the_current_request_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config.items]
        producer = "script"
        [workflows.core.views.default.engine.config.items.handler]
        file = "scripts/items.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/items.sh",
        r#"#!/bin/sh
set -eu
exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
if request.get("entrypoint") != "picker-items":
    raise SystemExit("unexpected items producer request")
query = request["context"]["engine"]["state"]["input"]
if query != "needle":
    raise SystemExit("items request input was not the current picker input")
json.dump({
    "version": 1,
    "items": [{"display": "query:" + query, "value": query}],
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"needle").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "query:needle");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_output_producer_runs_after_the_view_is_mounted() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:main"

        [workflows.core.views.main]
        [workflows.core.views.main.query]
        type = "object"
        message = { type = "string", default = "from-capture-producer" }
        [workflows.core.views.main.engine]
        type = "capture"
        [workflows.core.views.main.engine.config.output]
        producer = "script"
        [workflows.core.views.main.engine.config.output.handler]
        file = "scripts/output.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/output.sh",
        r#"#!/bin/sh
set -eu
exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
parameters = context.get("parameters", {}) if isinstance(context, dict) else {}
message = parameters.get("message") if isinstance(parameters, dict) else None
input_descriptor = context.get("input") if isinstance(context, dict) else None
engine = context.get("engine") if isinstance(context, dict) else None
stdin_descriptor = input_descriptor.get("stdin") if isinstance(input_descriptor, dict) else None
if (
    request.get("entrypoint") != "capture-output"
    or not isinstance(message, str)
    or not isinstance(stdin_descriptor, dict)
    or stdin_descriptor.get("is_tty") is not True
    or engine != {"type": "capture", "state": None}
):
    raise SystemExit("unexpected capture producer request")
json.dump({"version": 1, "output": "capture:" + message}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "capture:from-capture-producer");
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn return_processor_runs_after_restoring_the_caller() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:caller"

        [workflows.core.views.caller]
        [workflows.core.views.caller.engine]
        type = "picker"
        [workflows.core.views.caller.engine.config]
        items = [{ display = "Open child", value = "open" }]

        [workflows.core.views.caller.commands.open]
        key = "enter"
        label = "Open child"
        type = "call"
        producer = "declared"
        [workflows.core.views.caller.commands.open.handler]
        target = "core:child"
        [workflows.core.views.caller.commands.open.return_processor]
        type = "navigate"
        producer = "script"
        [workflows.core.views.caller.commands.open.return_processor.handler]
        file = "scripts/process.sh"

        [workflows.core.views.child]
        [workflows.core.views.child.engine]
        type = "picker"
        [workflows.core.views.child.engine.config]
        items = [{ display = "Return child value", value = "child" }]

        [workflows.core.views.child.commands.accept]
        key = "enter"
        label = "Return"
        type = "return"
        producer = "declared"
        [workflows.core.views.child.commands.accept.handler]
        value = { kind = "child", value = 7 }

        [workflows.core.views.processed.engine]
        type = "capture"
        [workflows.core.views.processed.engine.config]
        output = "processed"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/process.sh",
        r#"#!/bin/sh
set -eu
exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
result = context.get("result")
if (
    request.get("entrypoint") != "return"
    or result != {"kind": "child", "value": 7}
    or not isinstance(context.get("engine"), dict)
):
    raise SystemExit("unexpected return processor request")
json.dump({
    "version": 1,
    "operation": {"type": "navigate", "target": "core:processed", "replace": True},
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Return child value");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "processed");
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_selector_displays_keybindings_for_commands() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["sys:main"]);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "Run");
    let screen = String::from_utf8_lossy(&output);
    assert!(
        screen.contains("Run"),
        "screen should contain command label 'Run': {screen}"
    );
    assert!(
        screen.contains("enter"),
        "screen should contain command keybinding 'enter': {screen}"
    );
    assert!(
        screen.contains("Info"),
        "screen should contain command label 'Info': {screen}"
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn command_selector_repeated_ctrl_k_does_not_open_nested_menu() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["sys:main"]);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Show date");

    // First Ctrl+K opens the command palette
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Run");

    // Second Ctrl+K should be a no-op (stay), NOT open a nested commands menu
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();

    // A single Escape should immediately return to the main view, confirming no nested menu
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "Show date");
    let screen = String::from_utf8_lossy(&output);
    assert!(
        screen.contains("Show date"),
        "single escape should return to main view: {screen}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn command_selector_can_open_edit_query_form_and_apply_parameters() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["sys:main"]);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Show date");

    // Open command palette with Ctrl+K
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "Edit Query");
    let screen = String::from_utf8_lossy(&output);
    assert!(screen.contains("Edit Query"));
    assert!(screen.contains("[host]"));

    // Navigate down twice: Info -> Run -> Edit Query
    process.master.write_all(b"\x1b[B").unwrap();
    process.master.flush().unwrap();
    process.master.write_all(b"\x1b[B").unwrap();
    process.master.flush().unwrap();

    // Select Edit Query and press Enter to open parameter form
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();

    // Query form popup should open
    let form_output = wait_for_text(&process.master, "__query:main");
    let form_screen = String::from_utf8_lossy(&form_output);
    assert!(form_screen.contains("__query:main"));

    // Cancel form with Escape
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn script_command_producer_returns_structured_error_feedback() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
            default_view = "core:default"
            [workflows.core.views.default]
            [workflows.core.views.default.engine]
            type = "picker"
            [workflows.core.views.default.engine.config]
            items = [{display = "Item", value = "value"}]

            [workflows.core.views.default.commands.warn]
            key = "enter"
            type = "run"
            producer = "script"

            [workflows.core.views.default.commands.warn.handler]
            file = "scripts/warn.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "core",
        "scripts/warn.sh",
        r#"#!/bin/sh
printf '{"version":1,"error":{"message":"Custom branch warning","level":"warning"}}\n'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Custom branch warning");

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_sibling_route_reports_error_and_keeps_workflow_usable() {
    let root = temporary_root();
    let config = root.join("suite.toml");
    write_test_config(
        &config,
        r#"
default_view = "tool:main"
[workflows.tool.views.main.engine]
type = "picker"
[workflows.tool.views.main.engine.config]
items = [{display = "Still usable", value = "ok"}]
[workflows.tool.views.main.commands.sibling]
key = "ctrl+o"
type = "navigate"
producer = "declared"
handler = {target = "missing:main"}
"#,
    )
    .unwrap();
    let mut process = spawn_launcher(&config);
    wait_for_text(&process.master, "Still usable");
    send_bytes(&mut process, b"\x0f");
    wait_for_text(&process.master, "ERROR");
    send_bytes(&mut process, b"\x03");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn aggregate_view_footer_commands_survive_a_slow_refresh() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "slow:main"

        [workflows.slow.commands.open]
        label = "Open"
        type = "run"
        producer = "declared"
        handler = { mode = "foreground", argv = ["sh", "-c", "printf 'opened\\n'"], exit = true }

        [workflows.slow.views.main]
        keymap_mode = "item_merge"
        [workflows.slow.views.main.keymap]
        [workflows.slow.views.main.engine]
        type = "picker"
        [workflows.slow.views.main.engine.config.items]
        producer = "script"
        [workflows.slow.views.main.engine.config.items.handler]
        file = "scripts/items.sh"
        "#,
    )
    .unwrap();
    write_workflow_script(
        &root,
        "slow",
        "scripts/items.sh",
        r#"#!/bin/sh
sleep 0.35
printf '%s\n' '{"version":1,"items":[{"display":"Row","value":"row","bindings":{"enter":"slow:open"}}]}'
"#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Open");

    process.master.write_all(b"A").unwrap();
    process.master.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let screen = current_screen(&process.master);
    assert!(
        screen.contains("Open"),
        "item commands disappeared while a slow producer was refreshing: {screen}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tab_completion_without_candidates_does_not_open_popup() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &[]);
    wait_for_ready(&process.master);

    // Type a nonexistent prefix with zero candidate routes
    process.master.write_all(b"nonexistent").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_text(&process.master, "nonexistent");

    // Press Tab
    process.master.write_all(b"\t").unwrap();
    process.master.flush().unwrap();

    // Give a short period and verify screen remains on main view with input preserved and without popup
    std::thread::sleep(Duration::from_millis(200));
    let screen = current_screen(&process.master);
    assert!(
        screen.contains("nonexistent"),
        "input was cleared or lost: {screen}"
    );
    assert!(
        !screen.contains("completion"),
        "popup opened when no candidates existed: {screen}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}
