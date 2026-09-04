mod support;

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use support::{
    discard_pending_master_output, fixture_config, run_tty_invocation_with_blocked_stdout_signal,
    spawn_launcher, spawn_launcher_with_args, spawn_launcher_with_args_and_env, temporary_root,
    wait_for_fresh_screen, wait_for_launcher_exit, wait_for_launcher_exit_without_reading,
    wait_for_nonempty_file, wait_for_output, wait_for_process_exit, wait_for_ready, wait_for_text,
    write_test_config,
};

fn write_plugin_script(root: &Path, plugin: &str, file: &str, source: &str) {
    let path = root.join("plugins").join(plugin).join(file);
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
    let plugin_root = root.join("plugins/custom");
    fs::create_dir_all(plugin_root.join("scripts")).unwrap();
    fs::write(
        plugin_root.join("plugin.toml"),
        "[plugin]\napi = 1\nname = \"custom\"\n\n[views.main.engine]\ntype = \"picker\"\n[views.main.engine.config]\nitems = []\n",
    )
    .unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [plugins.custom.views.main]
        [plugins.custom.views.main.engine]
        type = "picker"
        [plugins.custom.views.main.engine.config]
        items = [{display = "Item", value = "value"}]

        [plugins.custom.views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        [plugins.custom.views.main.commands.accept.payload]
        handler = "scripts/large.sh"
        "#,
    )
    .unwrap();
    fs::write(
        plugin_root.join("scripts/large.sh"),
        "head -c 16777216 /dev/zero\n",
    )
    .unwrap();

    let args = ["--config", config.to_str().unwrap()];
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

        [plugins.custom.views.main]
        [plugins.custom.views.main.engine]
        type = "embedded"
        [plugins.custom.views.main.engine.config]
        command = ["sh", "-c", "while :; do printf x; done"]
        escape-cancels = false
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
fn escaped_dynamic_opener_remains_literal_at_runtime() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:main"
        [plugins.core.views.main.engine]
        type = "capture"
        [plugins.core.views.main.engine.config]
        output = '''literal \{{ page.input }}'''
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &[]);
    let output = wait_for_text(&process.master, r"literal \{{ page.input }}");
    assert!(String::from_utf8_lossy(&output).contains(r"literal \{{ page.input }}"));
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_script_source_renders_its_json_string() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(&config, "default_view = \"custom:main\"\n").unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"
        [views.main.engine]
        type = "capture"
        [views.main.engine.config.output]
        source = "script"
        file = "scripts/output.sh"
        args = ["{{ view.query }}"]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/output.sh"),
        "printf '\"capture-source-output\"\\n'\n",
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &[]);
    let output = wait_for_text(&process.master, "capture-source-output");
    assert!(String::from_utf8_lossy(&output).contains("capture-source-output"));
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dynamic_capture_output_can_resolve_to_a_script_source() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(&config, "default_view = \"custom:main\"\n").unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"
        [views.main.engine]
        type = "capture"
        [views.main.engine.config]
        output = "{{ view.query }}"
        [views.main.query]
        type = "object"
        source = { type = "string", default = "script" }
        file = { type = "string", default = "scripts/output.sh" }
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/output.sh"),
        "printf '%s\\n' '\"dynamic-capture-output\"'\n",
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &[]);
    let output = wait_for_text(&process.master, "dynamic-capture-output");
    assert!(String::from_utf8_lossy(&output).contains("dynamic-capture-output"));
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signal_exit_terminates_capture_script_source() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    let pid_file = root.join("capture.pid");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(&config, "default_view = \"custom:main\"\n").unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"
        [views.main.engine]
        type = "capture"
        [views.main.engine.config.output]
        source = "script"
        file = "scripts/output.sh"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/output.sh"),
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
    let plugin = root.join("plugins/custom");
    let pid_file = root.join("items.pid");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        default_view = "custom:main"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"
        [views.main.engine]
        type = "picker"
        [views.main.engine.config.items]
        source = "script"
        file = "scripts/items.sh"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/items.sh"),
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
fn signal_exit_terminates_embedded_process() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let pid_file = root.join("embedded.pid");
    write_test_config(
        &config,
        r#"
        default_view = "custom:main"

        [plugins.custom.views.main]
        [plugins.custom.views.main.engine]
        type = "embedded"
        [plugins.custom.views.main.engine.config]
        command = ["sh", "-c", "printf '%s' $$ > \"$PID_FILE\"; sleep 30"]
        escape-cancels = false
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

        [plugins.custom.views.main]
        [plugins.custom.views.main.engine]
        type = "embedded"
        [plugins.custom.views.main.engine.config]
        command = ["sh", "-c", "printf '%s' $$ > \"$PID_FILE\"; exec 0<&- 1>&- 2>&-; sleep 30"]
        escape-cancels = false
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]
        [plugins.core.views.default.commands.exit]
        key = "enter"
        label = "Exit"
        type = "run"
        [plugins.core.views.default.commands.exit.payload]
        handler = { source = "script", file = "scripts/exit.sh" }
        exit = true
        "#,
    )
    .unwrap();
    write_plugin_script(&root, "core", "scripts/exit.sh", "printf 'done\\n'\n");
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
fn loads_items_and_runs_a_view_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]
        [catalog]
        items = [{display = "Item", value = "value"}]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/command.sh" }
        exit = true
        "#,
    )
    .expect("could not write items command config");
    write_plugin_script(
        &root,
        "core",
        "scripts/command.sh",
        "printf 'command-marker:%s\\n' \"$LAUNCHER_VALUE\"\n",
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
        String::from_utf8_lossy(&output).contains("command-marker:value"),
        "launcher output did not contain command marker: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove items command config");
}

#[test]
fn selected_picker_item_is_passed_to_command_when_navigating_down() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [
            { display = "FirstApp", value = "app-one" },
            { display = "SecondApp", value = "app-two" },
        ]

        [plugins.core.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "run"

        [plugins.core.views.default.commands.open.payload]
        handler = { source = "script", file = "scripts/open.sh" }
        args = ["{{ selection.value }}"]
        exit = true
        "#,
    )
    .expect("could not write selection navigation config");
    write_plugin_script(
        &root,
        "core",
        "scripts/open.sh",
        "printf 'opened:%s\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "FirstApp");
    process
        .master
        .write_all(b"\x1b[B\r")
        .expect("could not write launcher keys");
    process
        .master
        .flush()
        .expect("could not flush launcher keys");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(
        status,
        0,
        "launcher exited with output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        String::from_utf8_lossy(&output).contains("opened:app-two"),
        "launcher output did not contain second item: {:?}",
        String::from_utf8_lossy(&output)
    );
    fs::remove_dir_all(root).expect("could not remove test root");
}

#[test]
fn selected_picker_item_is_passed_to_command_when_navigating_down_and_up() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [
            { display = "FirstApp", value = "app-one" },
            { display = "SecondApp", value = "app-two" },
        ]

        [plugins.core.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "run"

        [plugins.core.views.default.commands.open.payload]
        handler = { source = "script", file = "scripts/open.sh" }
        args = ["{{ selection.value }}"]
        exit = true
        "#,
    )
    .expect("could not write selection navigation config");
    write_plugin_script(
        &root,
        "core",
        "scripts/open.sh",
        "printf 'opened:%s\\n' \"$1\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "FirstApp");
    // Down then Up then Enter
    process
        .master
        .write_all(b"\x1b[B\x1b[A\r")
        .expect("could not write launcher keys");
    process
        .master
        .flush()
        .expect("could not flush launcher keys");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(
        status,
        0,
        "launcher exited with output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        String::from_utf8_lossy(&output).contains("opened:app-one"),
        "launcher output did not contain first item: {:?}",
        String::from_utf8_lossy(&output)
    );
    fs::remove_dir_all(root).expect("could not remove test root");
}

#[test]
fn typing_space_without_route_completion_does_not_error_and_preserves_query() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [
            { display = "Google Chrome", value = "chrome" },
        ]

        [plugins.core.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "run"

        [plugins.core.views.default.commands.open.payload]
        handler = { source = "script", file = "scripts/open.sh" }
        args = ["{{ selection.value }}", "{{ page.raw_input }}"]
        exit = true
        "#,
    )
    .expect("could not write test config");
    write_plugin_script(
        &root,
        "core",
        "scripts/open.sh",
        "printf 'opened:%s:query=%s\\n' \"$1\" \"$2\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Google Chrome");
    // Type "google " (word with space that has no route selector) then Enter
    process
        .master
        .write_all(b"google \r")
        .expect("could not write launcher keys");
    process
        .master
        .flush()
        .expect("could not flush launcher keys");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(
        status,
        0,
        "launcher exited with output: {:?}",
        String::from_utf8_lossy(&output)
    );
    let out = String::from_utf8_lossy(&output);
    assert!(!out.contains("unknown route selector"), "reported error: {out}");
    assert!(out.contains("opened:chrome:query=google "), "output: {out}");
    fs::remove_dir_all(root).expect("could not remove test root");
}

#[test]
fn dynamic_items_source_metadata_is_resolved_at_execution() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config.items]
        source = "{{ view.query.source }}"
        file = "{{ view.query.file }}"
        max_output_bytes = "{{ view.query.limit }}"

        [plugins.core.views.default.query]
        type = "object"
        source = { type = "string", default = "script" }
        file = { type = "string", default = "scripts/dynamic-items.sh" }
        limit = { type = "integer", default = 1024 }
        "#,
    )
    .unwrap();
    let scripts = root.join("plugins/core/scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(
        scripts.join("dynamic-items.sh"),
        "printf '%s\\n' '[{\"display\":\"Dynamic source\"}]'\n",
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let output = wait_for_text(&process.master, "Dynamic source");
    assert!(String::from_utf8_lossy(&output).contains("Dynamic source"));
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn run_command_args_resolve_to_exact_positional_arguments() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{ display = "Item", value = "value with spaces", metadata = { option = "selected mode", detail = { kind = "app" } } }]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/args.sh" }
        args = [
          "--option={{ selection.metadata.option }}",
          "{{ selection.value }}",
          "{{ view.query.extra }}",
          "--detail={{ selection.metadata.detail }}",
          "$(printf literal)",
        ]
        exit = true

        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        extra = { type = "string", default = "query value" }
        "#,
    )
    .unwrap();
    write_plugin_script(
        &root,
        "core",
        "scripts/args.sh",
        r#"
        printf 'argc=<%s>\n' "$#"
        for argument in "$@"; do
          printf 'arg=<%s>\n' "$argument"
        done
        "#,
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Item");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    let output = String::from_utf8_lossy(&output);
    for expected in [
        "argc=<5>",
        "arg=<--option=selected mode>",
        "arg=<value with spaces>",
        "arg=<query value>",
        "arg=<--detail={\"kind\":\"app\"}>",
        "arg=<$(printf literal)>",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?}: {output:?}"
        );
    }
    fs::remove_dir_all(root).unwrap();
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

        [plugins.apps.views.main]
        [plugins.apps.views.main.engine]
        type = "picker"
        [plugins.apps.views.main.engine.config]
        items = [{ display = "App", value = "fixture.desktop" }]

        [plugins.apps.views.main.commands.open]
        key = "enter"
        label = "Open"
        type = "run"
        [plugins.apps.views.main.commands.open.payload]
        handler = { source = "script", file = "scripts/open.sh" }
        exit = true
        "#,
    )
    .unwrap();
    write_plugin_script(
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
fn complete_dynamic_command_handler_source_resolves_as_a_script_object() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{ display = "Item", value = "value" }]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        [plugins.core.views.default.commands.run.payload]
        handler = "{{ view.query.handler }}"
        exit = true

        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        handler = { type = "object", default = { source = "script", file = "scripts/object.sh" } }
        "#,
    )
    .unwrap();
    write_plugin_script(
        &root,
        "core",
        "scripts/object.sh",
        "printf 'object-handler:%s\\n' \"$LAUNCHER_VALUE\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Item");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output).contains("object-handler:value"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_dynamic_command_args_resolve_to_an_argv_array() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{ display = "Item", value = "value" }]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/args.sh" }
        args = "{{ view.query.arguments }}"
        exit = true

        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        arguments = { type = "array<string>", default = ["--mode=dynamic", "two words"] }
        "#,
    )
    .unwrap();
    write_plugin_script(
        &root,
        "core",
        "scripts/args.sh",
        "printf 'argc=<%s> first=<%s> second=<%s>\\n' \"$#\" \"$1\" \"$2\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Item");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output)
            .contains("argc=<2> first=<--mode=dynamic> second=<two words>"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_backed_command_handler_keeps_template_text_opaque_at_execution() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/run.sh" }
        exit = true
        "#,
    )
    .unwrap();
    let scripts = root.join("plugins/core/scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(
        scripts.join("run.sh"),
        "printf '%s:%s\\n' '{{ user_template }}' \"$LAUNCHER_VALUE\"\n",
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output).contains("{{ user_template }}:value"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dynamic_command_handler_source_preserves_literal_template_text() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{ display = "Item", value = "value" }]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.core.views.default.commands.run.payload]
        handler = { source = "{{ view.query.source }}", file = "{{ view.query.file }}" }
        exit = true

        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        source = { type = "string", default = "script" }
        file = { type = "string", default = "scripts/opaque.sh" }
        "#,
    )
    .expect("could not write opaque handler config");
    write_plugin_script(
        &root,
        "core",
        "scripts/opaque.sh",
        "printf '%s:%s\\n' '{{ user_template }}' \"$LAUNCHER_VALUE\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "output: {:?}", output);
    assert!(
        String::from_utf8_lossy(&output).contains("{{ user_template }}:value"),
        "output: {:?}",
        output
    );
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{ display = "Static item", value = "static-value" }]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/static.sh" }
        exit = true
        "#,
    )
    .expect("could not write static items config");
    write_plugin_script(
        &root,
        "core",
        "scripts/static.sh",
        "printf 'static-marker:%s\\n' \"$LAUNCHER_VALUE\"\n",
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
fn explicit_capture_view_receives_typed_query_state() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "capture"
        [plugins.core.views.direct.engine.config]
        output = "{{ view.query.message }}"
        [plugins.core.views.direct.query]
        type = "object"
        message = { type = "string" }
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:direct", "--message=from-option"]);
    let output = wait_for_text(&process.master, "from-option");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("from-option"));
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible
            .lines()
            .last()
            .is_some_and(|footer| footer.contains("core:direct")),
        "route location is not in the footer: {visible}"
    );

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_does_not_render_its_private_query_as_a_host_input_row() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:capture"

        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config]
        output = "fixed-capture"
        [plugins.core.views.capture.query]
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
fn explicit_capture_view_receives_typed_runtime_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "capture"
        [plugins.core.views.direct.engine.config]
        output = "{{ page.input }}"
        [plugins.core.views.direct.query]
        type = "object"
        input_order = ["text"]
        text = { type = "string", default = "" }
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:direct", "--text=from-option"]);
    let output = wait_for_text(&process.master, "from-option");
    assert!(String::from_utf8_lossy(&output).contains("from-option"));

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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "embedded"
        [plugins.core.views.direct.engine.config]
        command = ["sh", "-lc", "printf 'input=%s\\n' \"$LAUNCHER_INPUT\""]
        [plugins.core.views.direct.query]
        type = "object"
        input_order = ["text"]
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

        [plugins.core.views.embedded.engine]
        type = "embedded"
        [plugins.core.views.embedded.engine.config]
        command = ["sh", "-c", "printf 'fixed-embedded'; trap 'exit 0' INT TERM; while :; do sleep 1; done"]
        [plugins.core.views.embedded.query]
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "embedded"
        [plugins.core.views.direct.engine.config]
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
fn btop_fixture_route_tab_and_escape_restore_the_empty_default() {
    let root = temporary_root();
    let marker = root.join("resource-marker");
    let bin = root.join("bin");
    let user_config = root.join("xdg-config").join("btop");
    let fake_btop = bin.join("btop");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&user_config).unwrap();
    fs::write(
        user_config.join("btop.conf"),
        "shown_boxes = \"cpu mem\"\nupdate_ms = 777\n",
    )
    .unwrap();
    fs::write(
        &fake_btop,
        "#!/bin/sh\nset -eu\nprintf '%s|%s\\n' \"$(sed -n '/^shown_boxes = /p' \"$2\")\" \"$(sed -n '/^update_ms = /p' \"$2\")\" >> \"$MONITOR_MARKER\"\ntrap 'exit 0' INT TERM\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    fs::set_permissions(&fake_btop, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let mut process = spawn_launcher_with_args_and_env(
        &fixture_config(),
        &[],
        &[
            ("MONITOR_MARKER", marker.to_str().unwrap()),
            ("PATH", &path),
            ("XDG_CONFIG_HOME", root.join("xdg-config").to_str().unwrap()),
        ],
    );
    wait_for_ready(&process.master);
    process.master.write_all(b"btop:main ").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "btop / cpu");
    let initial = wait_for_nonempty_file(&marker);
    assert!(initial.contains("shown_boxes = ") && initial.contains("cpu"));
    assert!(
        !initial.contains("cpu mem"),
        "user box selection leaked: {initial}"
    );
    assert!(initial.contains("update_ms = 777"), "marker: {initial}");

    process.master.write_all(b"\t").unwrap();
    process.master.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let switched = loop {
        let contents = fs::read_to_string(&marker).unwrap_or_default();
        if contents.lines().count() >= 2 {
            break contents;
        }
        assert!(Instant::now() < deadline, "Tab did not restart the monitor");
        std::thread::sleep(Duration::from_millis(10));
    };
    let switched_line = switched.lines().nth(1).unwrap();
    assert!(switched_line.contains("shown_boxes = ") && switched_line.contains("mem"));
    assert!(
        !switched_line.contains("cpu mem"),
        "user box selection leaked: {switched}"
    );
    assert!(
        switched_line.contains("update_ms = 777"),
        "marker: {switched}"
    );

    discard_pending_master_output(&process.master);
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_fresh_screen(&process.master, |visible| {
        visible.contains("Enter Open")
            && visible
                .lines()
                .nth(1)
                .is_some_and(|line| line.trim().is_empty())
            && !visible.contains("btop /")
    });
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(
        visible
            .lines()
            .nth(1)
            .is_some_and(|line| line.trim().is_empty()),
        "screen: {visible}"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_view_removes_stale_launcher_environment() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "embedded"
        [plugins.core.views.direct.engine.config]
        command = ["sh", "-lc", "printf 'managed=%s|%s|%s|%s|%s\\n' \"${LAUNCHER_COMMAND-unset}\" \"${LAUNCHER_ITEM-unset}\" \"${LAUNCHER_QUERY-unset}\" \"${LAUNCHER_VIEW-unset}\" \"${LAUNCHER_PLUGIN_DIR-unset}\""]
"#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(
        &config,
        &["core:direct"],
        &[
            ("LAUNCHER_COMMAND", "stale"),
            ("LAUNCHER_ITEM", "stale"),
            ("LAUNCHER_QUERY", "stale"),
            ("LAUNCHER_VIEW", "stale"),
            ("LAUNCHER_PLUGIN_DIR", "/tmp/stale"),
        ],
    );
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("managed=unset|unset|unset|unset|"),
        "output: {:?}",
        output
    );
    assert!(output.contains("/plugins/core"), "output: {:?}", output);
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/picker.sh" }
        exit = true
        "#,
    )
    .expect("could not write launcher integration config");
    write_plugin_script(
        &root,
        "core",
        "scripts/picker.sh",
        "printf 'picker-marker:%s\\n' \"$LAUNCHER_VALUE\"\n",
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
fn route_query_and_activate_share_one_input_batch() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [plugins.apps.views.main]
        alias = "app"
        [plugins.apps.views.main.engine]
        type = "picker"
        [plugins.apps.views.main.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.apps.views.main.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.apps.views.main.commands.run.payload]
        handler = { source = "script", file = "scripts/route.sh" }
        exit = true
        "#,
    )
    .expect("could not write route batch integration config");
    write_plugin_script(
        &root,
        "apps",
        "scripts/route.sh",
        "printf 'route-batch:%s:%s\\n' \"$LAUNCHER_QUERY\" \"$LAUNCHER_VALUE\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"app needle\r")
        .expect("could not write routed query and activation");
    process
        .master
        .flush()
        .expect("could not flush routed query and activation");

    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("route-batch:needle:value"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).expect("could not remove route batch integration config");
}

#[test]
fn view_commands_accept_unreserved_control_bindings() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.default.commands.run]
        key = "ctrl+r"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/ctrl.sh" }
        exit = true
        "#,
    )
    .expect("could not write control command config");
    write_plugin_script(
        &root,
        "core",
        "scripts/ctrl.sh",
        "printf 'ctrl-command:%s\\n' \"$LAUNCHER_VALUE\"\n",
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
fn edit_input_command_updates_the_picker_owned_editor() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value"}]

        [plugins.core.views.default.commands.rewrite]
        key = "ctrl+r"
        label = "Rewrite"
        requires = "input"
        type = "edit-input"

        [plugins.core.views.default.commands.rewrite.payload]
        value = "rewritten"
        cursor = 3
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &[]);
    wait_for_ready(&process.master);
    process.master.write_all(b"draft").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |visible| {
        visible.lines().any(|line| line.trim() == "draft")
    });
    process.master.write_all(b"\x12").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_fresh_screen(&process.master, |visible| {
        visible.lines().any(|line| line.trim() == "rewritten") && visible.contains("Rewrite")
    });
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.lines().any(|line| line.trim() == "rewritten"),
        "edit-input did not update the Picker surface"
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn view_command_overrides_printable_picker_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [
            { display = "First", value = "first" },
            { display = "Second", value = "second" },
        ]
        [plugins.core.views.default.keymap]
        "space" = "select_next"
        [plugins.core.views.default.commands.space]
        key = "space"
        label = "HiddenSpace"
        type = "run"

        [plugins.core.views.default.commands.space.payload]
        handler = { source = "script", file = "scripts/hidden-space.sh" }
        exit = true

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "run"

        [plugins.core.views.default.commands.accept.payload]
        handler = { source = "script", file = "scripts/space-selection.sh" }
        exit = true
        "#,
    )
    .expect("could not write printable keymap config");
    write_plugin_script(
        &root,
        "core",
        "scripts/hidden-space.sh",
        "printf 'hidden-space-command\\n'\n",
    );
    write_plugin_script(
        &root,
        "core",
        "scripts/space-selection.sh",
        "printf 'space-selection:%s\\n' \"$LAUNCHER_VALUE\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let initial = wait_for_text(&process.master, "First");
    let initial = String::from_utf8_lossy(&initial);
    assert!(initial.contains("HiddenSpace"), "output: {initial}");
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
fn unavailable_toggle_preview_consumes_an_unbound_key() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "First", value = "first"}]
        [plugins.core.views.default.keymap]
        space = "toggle_preview"

        [plugins.core.views.default.commands.inspect]
        key = "enter"
        label = "Inspect"
        scope = "view"
        requires = "input"
        type = "run"

        [plugins.core.views.default.commands.inspect.payload]
        handler = { source = "script", file = "scripts/inspect.sh" }
        exit = true
        "#,
    )
    .expect("could not write unavailable preview config");
    write_plugin_script(
        &root,
        "core",
        "scripts/inspect.sh",
        "printf 'toggle-query:%s:end\\n' \"$LAUNCHER_QUERY\"\n",
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [
            { display = "First", value = "first" },
            { display = "Second", value = "second" },
        ]
        [plugins.core.views.default.keymap]
        a = "select_next"
        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "run"

        [plugins.core.views.default.commands.accept.payload]
        handler = { source = "script", file = "scripts/uppercase-selection.sh" }
        exit = true
        "#,
    )
    .expect("could not write uppercase keymap config");
    write_plugin_script(
        &root,
        "core",
        "scripts/uppercase-selection.sh",
        "printf 'uppercase-selection:%s\\n' \"$LAUNCHER_VALUE\"\n",
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = []
        [plugins.core.views.default.keymap]
        a = "select_next"
        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        requires = "input"
        type = "run"

        [plugins.core.views.default.commands.accept.payload]
        handler = { source = "script", file = "scripts/uppercase-input.sh" }
        exit = true
        "#,
    )
    .expect("could not write uppercase input config");
    write_plugin_script(
        &root,
        "core",
        "scripts/uppercase-input.sh",
        "printf 'uppercase-input:%s\\n' \"$LAUNCHER_QUERY\"\n",
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

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]

        [plugins.core.views.default.commands.run]
        key = "tab"
        label = "Run"
        type = "run"

        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/tab.sh" }
        exit = true
        "#,
    )
    .expect("could not write Tab command config");
    write_plugin_script(&root, "core", "scripts/tab.sh", "printf 'tab-command'\n");

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
fn unknown_input_closes_route_completion_before_escape() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
    let plugin_root = root.join("plugins/core");
    let script_root = plugin_root.join("scripts");
    fs::create_dir_all(&script_root).expect("could not create cancellation script directory");
    fs::write(
        plugin_root.join("plugin.toml"),
        "[plugin]\nname = \"core\"\n\n[views.placeholder.engine]\ntype = \"picker\"\n[views.placeholder.engine.config]\n",
    )
    .expect("could not write cancellation plugin manifest");
    let old_pid_path = root.join("old.pid");
    fs::write(
        script_root.join("items.sh"),
        format!(
            r#"query=${{1:-}}
if [ -z "$query" ]; then
    printf '%s\n' "$$" > "{}"
    sleep 10
    printf '[{{"display":"old-result"}}]\n'
else
    printf '[{{"display":"new-result"}}]\n'
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [plugins.core.views.default.engine.config.items]
        source = "script"
        file = "scripts/items.sh"
        args = ["{{ view.query }}"]
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
fn ctrl_k_passes_page_commands_through_selector_query_and_invokes_an_opaque_ref() {
    let config = fixture_config();
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"sys ").unwrap();
    process.master.flush().unwrap();
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
fn command_selector_does_not_expose_an_owner_from_stale_items() {
    let config = fixture_config();
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"sys ").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"no-match\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "(no matches)");
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(!visible.contains("Run"), "screen: {visible}");

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    discard_pending_master_output(&process.master);
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}

#[test]
fn tab_opens_builtin_route_completion_and_escape_cancels_it() {
    let config = fixture_config();
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\t").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "app");
    assert!(String::from_utf8_lossy(&output).contains("sys"));

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_ready(&process.master);

    process.master.write_all(b"sys\t").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "sys:main");
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    assert!(!visible.contains("apps:main"), "screen: {visible}");

    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show system information");

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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = "{{ page.query }}"
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
            .contains("items must resolve to an array")
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
fn feeds_page_commands_remain_available_with_selected_owner_item() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.core.views.default.commands.page]
        key = "ctrl+r"
        label = "Page"
        type = "run"
        [plugins.core.views.default.commands.page.payload]
        handler = { source = "script", file = "scripts/page.sh" }
        exit = true

        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = [{display = "Row", value = "row"}]
        [plugins.apps.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "run"
        [plugins.apps.views.default.commands.open.payload]
        handler = { source = "script", file = "scripts/owner.sh" }
        exit = true

        [catalog]
        items = [{display = "Row", value = "row"}]
        "#,
    )
    .unwrap();
    write_plugin_script(
        &root,
        "core",
        "scripts/page.sh",
        "printf 'page-command\\n'\n",
    );
    write_plugin_script(
        &root,
        "apps",
        "scripts/owner.sh",
        "printf 'owner-command\\n'\n",
    );

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
fn pending_feed_owner_command_overrides_picker_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.core.views.default.keymap]
        space = "select_next"

        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = [{display = "Row", value = "row"}]
        [plugins.apps.views.default.commands.open]
        key = "space"
        label = "Open"
        scope = "selection"
        requires = "input"
        type = "run"
        [plugins.apps.views.default.commands.open.payload]
        handler = { source = "script", file = "scripts/owner.sh" }
        exit = true
        "#,
    )
    .unwrap();
    write_plugin_script(
        &root,
        "apps",
        "scripts/owner.sh",
        "printf 'pending-owner-command:%s\\n' \"$LAUNCHER_VALUE\"\n",
    );

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b" ").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("pending-owner-command:row"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn feed_owners_apply_independent_query_defaults() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ view.query.items }}"
        [plugins.apps.views.default.query]
        type = "object"
        items = { type = "array<object>", default = [{display = "VALUE:source-default"}] }

        [catalog]
        items = [{display = "VALUE:{{ view.query.text }}"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let output = wait_for_text(&process.master, "VALUE:source-default");
    assert!(String::from_utf8_lossy(&output).contains("VALUE:source-default"));

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn routed_picker_restores_alias_prefix_and_top_spacing() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"

        [plugins.apps.views.default]
        alias = "app"
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = [{display = "Needle", value = "needle"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"app needle\r").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_fresh_screen(&process.master, |visible| {
        visible.lines().any(|line| line.trim() == "app needle")
            && visible
                .lines()
                .last()
                .is_some_and(|footer| footer.trim_start().starts_with("app "))
    });
    let output = String::from_utf8_lossy(&output);
    let visible = output.rsplit("--- visible screen ---").next().unwrap();
    let mut lines = visible.lines().filter(|line| !line.is_empty());
    assert!(lines.next().is_some_and(|line| line.trim().is_empty()));
    assert!(visible.lines().any(|line| line.trim() == "app needle"));

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn picker_back_clears_routed_query_before_returning_to_default() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "sys:default"
        [plugins.apps.views.default]
        alias = "app"
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.sys.views.default]
        alias = "sys"
        [plugins.sys.views.default.engine]
        type = "picker"
        [plugins.sys.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
"#,
    )
    .expect("could not write route input config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"app ")
        .expect("could not write app route");
    process.master.flush().expect("could not flush app route");
    let _ = wait_for_text(&process.master, "app");

    process
        .master
        .write_all(b"aa")
        .expect("could not write app query");
    process.master.flush().expect("could not flush app query");
    let _ = wait_for_text(&process.master, "aa");

    process
        .master
        .write_all(b"\x1b")
        .expect("could not clear the routed query");
    process
        .master
        .flush()
        .expect("could not flush the routed query clear");
    wait_for_ready(&process.master);

    process
        .master
        .write_all(b"\x1b")
        .expect("could not return from the routed picker");
    process
        .master
        .flush()
        .expect("could not flush the routed picker return");
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"\x03")
        .expect("could not close route input launcher");
    process
        .master
        .flush()
        .expect("could not flush route input launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove route input config");
}

#[test]
fn empty_picker_input_returns_to_parent_before_a_new_root_route() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "sys:default"
        [plugins.apps.views.default]
        alias = "app"
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.sys.views.default]
        alias = "sys"
        [plugins.sys.views.default.engine]
        type = "picker"
        [plugins.sys.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
"#,
    )
    .expect("could not write route editing config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"app aa")
        .expect("could not write app route query");
    process
        .master
        .flush()
        .expect("could not flush app route query");
    let _ = wait_for_text(&process.master, "app");

    process
        .master
        .write_all(b"\x7f\x7f")
        .expect("could not delete route parameters");
    process
        .master
        .flush()
        .expect("could not flush route parameter deletion");
    let _ = wait_for_text(&process.master, "Item");

    process
        .master
        .write_all(b"\x7f")
        .expect("could not return from the empty child picker");
    process
        .master
        .flush()
        .expect("could not flush the child picker return");
    process
        .master
        .write_all(b"\x7f\x7f\x7f")
        .expect("could not backspace at the empty default root");
    process
        .master
        .flush()
        .expect("could not flush default root backspace");
    process
        .master
        .write_all(b"sys ")
        .expect("could not write replacement route");
    process
        .master
        .flush()
        .expect("could not flush replacement route");
    let _ = wait_for_text(&process.master, "sys");

    process
        .master
        .write_all(b"\x03")
        .expect("could not close route editing launcher");
    process
        .master
        .flush()
        .expect("could not flush route editing launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove route editing config");
}

#[test]
fn navigation_without_query_uses_the_target_view_default() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "navigate"

        [plugins.core.views.default.commands.open.payload]
        target = "core:capture"

        [plugins.core.views.capture]
        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config]
        output = "{{ view.query.text }}"
        [plugins.core.views.capture.query]
        type = "object"
        input_order = ["text"]
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:capture"
        query = "capture-marker:{{ selection.value }}"

        [plugins.core.views.capture]
        alias = "cap"
        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config]
        output = "{{ page.input }}\u001b[31m\n\u4e16\u754c\u001b[0m"
        title = "Capture"

        [plugins.core.views.capture.keymap]
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
    let copied = wait_for_output(
        &process.master,
        b"\x1b]52;c;Y2FwdHVyZS1tYXJrZXI6dmFsdWUbWzMxbQrkuJbnlYwbWzBt\x07",
    );
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
fn failed_capture_cannot_copy_its_diagnostic_text() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
        output = "{{ selection.missing }}"
        "#,
    )
    .expect("could not write failed capture config");

    let mut process = spawn_launcher(&config);
    let initial = wait_for_text(&process.master, "failed");
    assert!(
        !String::from_utf8_lossy(&initial).contains("Copy"),
        "failed capture still advertises Copy: {initial:?}"
    );

    process
        .master
        .write_all(b"\r\x1b")
        .expect("could not write failed capture actions");
    process
        .master
        .flush()
        .expect("could not flush failed capture actions");
    let (status, remaining) = wait_for_launcher_exit(&mut process);

    let mut observed = initial;
    observed.extend(remaining);
    assert_eq!(status, 0);
    assert!(
        !String::from_utf8_lossy(&observed).contains("\x1b]52;"),
        "failed capture copied its diagnostic output: {observed:?}"
    );
    fs::remove_dir_all(root).expect("could not remove failed capture config");
}

#[test]
fn capture_keeps_session_commands_available() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [defaults.capture.bindings]
        copy = ["ctrl+k"]

        [commands.bindings.details]
        key = "ctrl+k"
        label = "Details"
        type = "call"

        [commands.bindings.details.payload]
        target = "core:details"

        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
        output = "xxxxxxxxxxxxxx"

        [plugins.core.views.details.engine]
        type = "capture"
        [plugins.core.views.details.engine.config]
        output = "footer-capture"
        "#,
    )
    .expect("could not write capture footer config");

    let mut process = spawn_launcher(&config);
    let root_output = wait_for_text(&process.master, "Details");
    assert!(String::from_utf8_lossy(&root_output).contains("Details"));

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
fn root_capture_defaults_resolve_against_the_consuming_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [defaults.capture.bindings]
        back = ["{{ view.query.back_key }}"]

        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
        output = "root-default-owner"

        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        back_key = { type = "string", default = "ctrl+b" }
        "#,
    )
    .expect("could not write dynamic root-default config");

    let mut process = spawn_launcher(&config);
    let output = wait_for_text(&process.master, "root-default-owner");
    assert!(String::from_utf8_lossy(&output).contains("root-default-owner"));
    process.master.write_all(b"\x02").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove dynamic root-default config");
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]
        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:embedded"
        query = '''printf 'embedded-marker:%s\n' '{{ selection.value }}'; exit 0'''

        [plugins.core.views.embedded]
        alias = "emb"
        [plugins.core.views.embedded.engine]
        type = "embedded"
        [plugins.core.views.embedded.engine.config]
        command = ["sh", "-lc", "{{ page.input }}"]
        title = "Embedded"
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
fn failed_view_creation_returns_to_the_current_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [plugins.core.views.broken]
        [plugins.core.views.broken.engine]
        type = "embedded"
        [plugins.core.views.broken.engine.config]
        command = "{{ selection.missing }}"
"#,
    )
    .expect("could not write failed navigation integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"core:broken ")
        .expect("could not write broken view route");
    process
        .master
        .flush()
        .expect("could not flush broken route");
    let output = wait_for_text(&process.master, "ERROR");
    assert!(
        String::from_utf8_lossy(&output).contains(" core:broken"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x03")
        .expect("could not close launcher after failed navigation");
    process
        .master
        .flush()
        .expect("could not flush launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove failed navigation config");
}

#[test]
fn qualified_view_path_navigates_to_any_engine() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [plugins.core.views.embedded]
        [plugins.core.views.embedded.engine]
        type = "embedded"
        [plugins.core.views.embedded.engine.config]
        command = ["sh", "-lc", "{{ page.input }}"]
        title = "Embedded"
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
    let launcher = wait_for_text(&process.master, "0 of 0");
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
fn ctrl_k_in_embedded_view_displays_embedded_commands() {
    let root = temporary_root();
    let marker = root.join("resource-marker");
    let bin = root.join("bin");
    let fake_btop = bin.join("btop");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        &fake_btop,
        "#!/bin/sh\ntrap 'exit 0' INT TERM\nwhile :; do sleep 1; done\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_btop, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let mut process = spawn_launcher_with_args_and_env(
        &fixture_config(),
        &[],
        &[
            ("MONITOR_MARKER", marker.to_str().unwrap()),
            ("PATH", &path),
        ],
    );
    wait_for_ready(&process.master);
    process.master.write_all(b"btop:main ").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "btop / cpu");

    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Next resource");

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "btop / cpu");

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "core:default");

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_selector_displays_keybindings_for_commands() {
    let config = fixture_config();
    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"sys ").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");

    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let output = wait_for_text(&process.master, "Run");
    let screen = String::from_utf8_lossy(&output);
    assert!(screen.contains("Run"), "screen should contain command label 'Run': {screen}");
    assert!(screen.contains("enter"), "screen should contain command keybinding 'enter': {screen}");
    assert!(screen.contains("Info"), "screen should contain command label 'Info': {screen}");

    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Show date");
    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}
