mod support;

use std::fs;
use std::io::Write;

use support::{
    spawn_launcher, spawn_launcher_with_args, temporary_root, wait_for_launcher_exit,
    wait_for_nonempty_file, wait_for_process_exit, wait_for_ready, wait_for_text,
    write_test_config,
};

#[test]
fn loads_items_from_an_expression() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:catalog.items }}"

        [catalog]
        items = [{label = "Item", value = "value"}]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = '''printf 'expression-marker:%s\\n' "$LAUNCHER_VALUE"'''
        exit = true
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
fn explicit_capture_view_receives_typed_query_state() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.direct]
        type = "capture"
        output = "{{ this:query.message }}"

        [plugins.core.views.direct.query]
        type = "object"
        message = '''{{ state("string") }}'''
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args(&config, &["core:direct", "--message=from-option"]);
    let output = wait_for_text(&process.master, "from-option");
    assert!(String::from_utf8_lossy(&output).contains("from-option"));

    process.master.write_all(b"\x03").unwrap();
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
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.direct]
        type = "embedded"
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

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = '''printf 'picker-marker:%s\n' "$LAUNCHER_VALUE"'''
        exit = true
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

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "ctrl+r"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = '''printf 'ctrl-command:%s\n' "$LAUNCHER_VALUE"'''
        exit = true
        "#,
    )
    .expect("could not write control command config");

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
fn replacing_items_request_cancels_the_previous_script() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin_root = root.join("plugins/core");
    let script_root = plugin_root.join("scripts");
    fs::create_dir_all(&script_root).expect("could not create cancellation script directory");
    fs::write(
        plugin_root.join("plugin.toml"),
        "[plugin]\nname = \"core\"\n\n[views.placeholder]\ntype = \"picker\"\n",
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
        type = "picker"
        show_prefix = true
        items = '{{ script("scripts/items.sh", runtime:view.active) }}'
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
fn ctrl_k_opens_the_command_picker_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = ":"

        [plugins.core.views.default.commands.apps]
        key = "alt+a"
        label = "Apps"
        type = "run"


        [plugins.core.views.default.commands.apps.payload]
        handler = '''printf 'command-marker:%s\n' "$LAUNCHER_VALUE"'''
        exit = true

        [plugins.core.views.default.commands.shell]
        key = "alt+s"
        label = "Shell"
        type = "run"


        [plugins.core.views.default.commands.shell.payload]
        handler = ":"

        [plugins.core.views.command]
        type = "picker"
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
    let output = wait_for_text(&process.master, "core:command");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("Enter"), "output: {output}");
    assert!(output.contains("Run"), "output: {output}");
    assert!(output.contains("Alt-A"), "output: {output}");
    assert!(output.contains("Apps"), "output: {output}");
    assert!(output.contains("Alt-S"), "output: {output}");
    assert!(output.contains("Shell"), "output: {output}");

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
fn tab_opens_view_completion_and_escape_closes_it() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true

        [plugins.apps.views.main]
        type = "picker"
        alias = "app"

        [plugins.sys.views.main]
        type = "picker"
        alias = "sys"
        "#,
    )
    .expect("could not write view completion config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\t")
        .expect("could not open view completion");
    process
        .master
        .flush()
        .expect("could not flush view completion key");
    let output = wait_for_text(&process.master, "apps:main");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("apps:main"), "output: {output}");
    assert!(output.contains("sys:main"), "output: {output}");
    assert!(!output.contains("\x1b[7m> "), "output: {output}");
    assert!(!output.contains(" > "), "output: {output}");

    process
        .master
        .write_all(b"\x1b")
        .expect("could not close view completion");
    process
        .master
        .flush()
        .expect("could not flush completion close key");

    process
        .master
        .write_all(b"\t\r")
        .expect("could not accept a completed view");
    process
        .master
        .flush()
        .expect("could not flush completed view");
    let output = wait_for_text(&process.master, "apps:main");
    assert!(
        String::from_utf8_lossy(&output).contains("apps:main"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x03")
        .expect("could not close completion test launcher");
    process
        .master
        .flush()
        .expect("could not flush completion test close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove view completion config");
}

#[test]
fn picker_bindings_can_override_a_default_shortcut() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.bindings]
        open_commands = ["ctrl+p"]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = ":"

        [plugins.core.views.command]
        type = "picker"
        "#,
    )
    .expect("could not write custom picker binding config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"\x10")
        .expect("could not write configured Ctrl-P shortcut");
    process
        .master
        .flush()
        .expect("could not flush configured shortcut");
    let output = wait_for_text(&process.master, "core:command");
    let output_text = String::from_utf8_lossy(&output);
    assert!(output_text.contains("Enter"), "output: {:?}", output);
    assert!(output_text.contains("Run"), "output: {:?}", output);

    process
        .master
        .write_all(b"\x03")
        .expect("could not close launcher");
    process
        .master
        .flush()
        .expect("could not flush launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove custom binding config");
}

#[test]
fn command_picker_navigation_keeps_the_parent_item_context() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.inspect]
        key = "enter"
        label = "Inspect"
        type = "navigate"

        [plugins.core.views.default.commands.inspect.payload]
        target = "{{ runtime:view.active.selected_item.metadata.target }}"
        query = "parent-value:{{ runtime:view.active.selected_item.value }}"

        [plugins.core.views.command]
        type = "picker"

        [plugins.core.views.capture]
        type = "capture"
        output = "{{ runtime:view.active.input }}"
        title = "Capture"
        "#,
    )
    .expect("could not write command navigation integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"\x0b")
        .expect("could not open command picker");
    process.master.flush().expect("could not flush Ctrl-K");
    let _ = wait_for_text(&process.master, "core:command");
    process
        .master
        .write_all(b"\r")
        .expect("could not navigate from command picker");
    process
        .master
        .flush()
        .expect("could not flush command navigation");
    let output = wait_for_text(&process.master, "parent-value:value");
    assert!(
        String::from_utf8_lossy(&output).contains("parent-value:value"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\r")
        .expect("could not return from capture view");
    process
        .master
        .flush()
        .expect("could not flush capture return");
    let _ = wait_for_text(&process.master, "core:default");
    process
        .master
        .write_all(b"\x03")
        .expect("could not close launcher");
    process
        .master
        .flush()
        .expect("could not flush launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove command navigation config");
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
        type = "picker"
        show_prefix = true
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
fn aggregate_sources_own_independent_view_state() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        sources = ["apps:default"]

        [plugins.apps.views.default]
        type = "picker"
        items = "{{ config:catalog.items }}"

        [plugins.apps.views.default.query]
        type = "object"
        input_order = ["text"]
        text = '''{{ state("string", "source-default") }}'''

        [catalog]
        items = [{label = "VALUE:{{ this:query.text }}"}]
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
fn route_input_survives_navigation_and_esc_restores_the_parent_input() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        sources = ["apps:default", "sys:default"]

        [plugins.apps.views.default]
        type = "picker"
        alias = "app"
        items = "{{ config:test_items.items }}"

        [plugins.sys.views.default]
        type = "picker"
        alias = "sys"
        items = "{{ config:test_items.items }}"
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
    let output = wait_for_text(&process.master, "apps:default");
    assert!(
        String::from_utf8_lossy(&output).contains(" app "),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"aa")
        .expect("could not write app query");
    process.master.flush().expect("could not flush app query");
    let output = wait_for_text(&process.master, " app aa");
    assert!(
        String::from_utf8_lossy(&output).contains(" app aa"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x1b")
        .expect("could not write route escape");
    process
        .master
        .flush()
        .expect("could not flush route escape");
    let output = wait_for_text(&process.master, "0/0");
    assert!(
        String::from_utf8_lossy(&output).contains(" app "),
        "output: {:?}",
        output
    );
    assert!(
        !String::from_utf8_lossy(&output).contains("core:default"),
        "root divider unexpectedly included the view name: {:?}",
        output
    );

    drop(process);
    fs::remove_dir_all(root).expect("could not remove route input config");
}

#[test]
fn deleting_route_input_returns_to_parent_before_switching_aliases() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        sources = ["apps:default", "sys:default"]

        [plugins.apps.views.default]
        type = "picker"
        alias = "app"
        items = "{{ config:test_items.items }}"

        [plugins.sys.views.default]
        type = "picker"
        alias = "sys"
        items = "{{ config:test_items.items }}"
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
    let _ = wait_for_text(&process.master, "apps:default");

    process
        .master
        .write_all(b"\x7f\x7f")
        .expect("could not delete route parameters");
    process
        .master
        .flush()
        .expect("could not flush route parameter deletion");
    let output = wait_for_text(&process.master, " app ");
    assert!(
        String::from_utf8_lossy(&output).contains(" app "),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x7f")
        .expect("could not delete route separator");
    process
        .master
        .flush()
        .expect("could not flush route separator deletion");
    let output = wait_for_text(&process.master, "1/2");
    assert!(
        String::from_utf8_lossy(&output).contains(" app"),
        "output: {:?}",
        output
    );
    assert!(
        !String::from_utf8_lossy(&output).contains("core:default"),
        "root divider unexpectedly included the view name: {:?}",
        output
    );

    process
        .master
        .write_all(b"\x7f\x7f\x7f")
        .expect("could not delete route selector");
    process
        .master
        .flush()
        .expect("could not flush route selector deletion");
    let output = wait_for_text(&process.master, "1/2");
    assert!(
        String::from_utf8_lossy(&output).contains("1/2"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"sys ")
        .expect("could not write replacement route");
    process
        .master
        .flush()
        .expect("could not flush replacement route");
    let output = wait_for_text(&process.master, "sys:default");
    assert!(
        String::from_utf8_lossy(&output).contains(" sys "),
        "output: {:?}",
        output
    );

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
fn view_alias_routes_to_the_configured_messages_picker() {
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
        "[plugin]\nname = \"core\"\n\n[views.placeholder]\ntype = \"picker\"\n",
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
        type = "picker"
        show_prefix = true

        [plugins.core.views.messages]
        type = "picker"
        alias = "log"
        items = '{{ script("scripts/items.sh", runtime:view.active) }}'
        "#,
    )
    .expect("could not write messages integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"log ")
        .expect("could not write view alias");
    process.master.flush().expect("could not flush view alias");
    let _ = wait_for_text(&process.master, "core:messages (log)");
    let output = wait_for_text(&process.master, "preexisting log");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("preexisting log"), "output: {output}");

    process
        .master
        .write_all(b"\x03")
        .expect("could not close messages picker");
    process
        .master
        .flush()
        .expect("could not flush messages picker close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove messages integration config");
}

#[test]
fn duplicate_view_alias_reports_an_error_when_invoked() {
    let root = temporary_root();
    let config = root.join("config.toml");
    for package in ["package-a", "package-b"] {
        let plugin_root = root.join("plugins").join(package);
        fs::create_dir_all(&plugin_root).expect("could not create conflicting plugin");
        fs::write(
            plugin_root.join("plugin.toml"),
            r#"[plugin]
name = "template"

[views.default]
type = "picker"
alias = "temp"
"#,
        )
        .expect("could not write conflicting plugin manifest");
    }
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        show_prefix = true
        "#,
    )
    .expect("could not write conflicting-alias config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"temp ")
        .expect("could not write conflicting view alias");
    process
        .master
        .flush()
        .expect("could not flush conflicting view alias");
    let output = wait_for_text(&process.master, "ambiguous");
    assert!(
        String::from_utf8_lossy(&output).contains("view alias \"temp\" is ambiguous"),
        "output: {:?}",
        output
    );
    let runtime_log = fs::read_to_string(root.join("runtime.jsonl"))
        .expect("could not read conflicting-alias runtime log");
    assert!(
        runtime_log.contains("package-a:default, package-b:default"),
        "runtime log: {runtime_log}"
    );

    process
        .master
        .write_all(b"\x03")
        .expect("could not close conflicting-alias launcher");
    process
        .master
        .flush()
        .expect("could not flush conflicting-alias launcher close");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove conflicting-alias config");
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
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:capture"
        query = "capture-marker:{{ runtime:view.active.selected_item.value }}"

        [plugins.core.views.capture]
        type = "capture"
        alias = "cap"
        output = "{{ runtime:view.active.input }}"
        title = "Capture"
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
    let launcher = wait_for_text(&process.master, "core:default");

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
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("capture-marker:value"), "output: {output}");
    assert!(output.contains("core:capture (cap)"), "output: {output}");
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
        type = "picker"
        show_prefix = true
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:embedded"
        query = '''printf 'embedded-marker:%s\n' '{{ runtime:view.active.selected_item.value }}'; exit 0'''

        [plugins.core.views.embedded]
        type = "embedded"
        alias = "emb"
        command = ["sh", "-lc", "{{ runtime:view.active.input }}"]
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
    assert!(output.contains("core:embedded (emb)"), "output: {output}");
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
        type = "picker"
        show_prefix = true

        [plugins.core.views.broken]
        type = "embedded"
        command = "{{ runtime:missing }}"
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
        type = "picker"
        show_prefix = true

        [plugins.core.views.embedded]
        type = "embedded"
        command = ["sh", "-lc", "{{ runtime:view.active.input }}"]
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

    let mut output = wait_for_text(&process.master, "route-marker");
    let launcher = wait_for_text(&process.master, "0/0");
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
