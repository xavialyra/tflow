mod support;

use std::fs;
use std::io::Write;

use support::{
    spawn_launcher, spawn_launcher_with_args, spawn_launcher_with_args_and_env, temporary_root,
    wait_for_launcher_exit, wait_for_nonempty_file, wait_for_process_exit, wait_for_ready,
    wait_for_text, write_test_config,
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "capture"
        [plugins.core.views.direct.engine.config]
        output = "{{ this:query.message }}"
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
    assert!(output.contains("core:direct"), "output: {output}");

    process.master.write_all(b"\x03").unwrap();
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
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.direct]
        [plugins.core.views.direct.engine]
        type = "capture"
        [plugins.core.views.direct.engine.config]
        output = "{{ runtime:view.current.input }}"
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

    process.master.write_all(b"\x03").unwrap();
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
        show_prefix = true
        items = "{{ config:test_items.items }}"
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
    assert!(output.contains("core:direct"), "output: {output}");
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
        show_prefix = true
        items = "{{ config:test_items.items }}"
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
        items = "{{ config:test_items.items }}"
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
    assert!(
        String::from_utf8_lossy(&output).contains("managed=unset|unset|unset|unset|unset"),
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
        items = "{{ config:test_items.items }}"
        [plugins.apps.views.main.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.apps.views.main.commands.run.payload]
        handler = '''printf 'route-batch:%s:%s\n' "$LAUNCHER_QUERY" "$LAUNCHER_VALUE"'''
        exit = true
        "#,
    )
    .expect("could not write route batch integration config");

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
        "[plugin]\nname = \"core\"\n\n[views.placeholder.engine]\ntype = \"picker\"\n[views.placeholder.engine.config]\n",
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        items = '{{ script("scripts/items.sh", {query = this:query, log_file = runtime:view.current.log_file}) }}'
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
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
    let output = wait_for_text(&process.master, "Run");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("Run"), "output: {output}");
    assert!(output.contains("Apps"), "output: {output}");
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
fn command_picker_waits_for_the_committed_selection() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        command_view = "core:command"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        [[plugins.core.views.default.engine.config.feeds]]
        view = "alpha:default"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "beta:default"
        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
        [plugins.alpha.views.default]
        [plugins.alpha.views.default.engine]
        type = "picker"
        [plugins.alpha.views.default.engine.config]
        items = "{{ config:catalog.alpha }}"
        [plugins.alpha.views.default.commands.open]
        key = "enter"
        label = "Alpha Action"
        type = "run"

        [plugins.alpha.views.default.commands.open.payload]
        handler = ":"

        [plugins.beta.views.default]
        [plugins.beta.views.default.engine]
        type = "picker"
        [plugins.beta.views.default.engine.config]
        items = "{{ config:catalog.beta }}"
        [plugins.beta.views.default.commands.open]
        key = "enter"
        label = "Beta Action"
        type = "run"

        [plugins.beta.views.default.commands.open.payload]
        handler = ":"

        [catalog]
        alpha = [{label = "Alpha"}]
        beta = [{label = "Beta"}]
        "#,
    )
    .expect("could not write pending command-view config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Beta");
    process
        .master
        .write_all(b"x\x1b[B\x0b")
        .expect("could not write query, selection, and command-view batch");
    process
        .master
        .flush()
        .expect("could not flush pending command-view batch");

    let output = wait_for_text(&process.master, "Beta Action");
    let output = String::from_utf8_lossy(&output);
    let command_screen = output
        .rsplit_once("core:command")
        .map(|(_, screen)| screen)
        .unwrap_or(&output);
    assert!(command_screen.contains("Beta Action"), "output: {output}");
    assert!(!command_screen.contains("Alpha Action"), "output: {output}");

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove pending command-view config");
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        [plugins.apps.views.main]
        alias = "app"
        [plugins.apps.views.main.engine]
        type = "picker"
        [plugins.apps.views.main.engine.config]
        [plugins.sys.views.main]
        alias = "sys"
        [plugins.sys.views.main.engine]
        type = "picker"
        [plugins.sys.views.main.engine.config]
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
    let output = wait_for_text(&process.master, "app");
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("app"), "output: {output}");
    assert!(output.contains("sys"), "output: {output}");
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
    let output = wait_for_text(&process.master, "app");
    assert!(
        String::from_utf8_lossy(&output).contains("app"),
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
fn typing_dismisses_completion_and_replays_the_character_batch() {
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
        [plugins.sys.views.main]
        alias = "sys"
        [plugins.sys.views.main.engine]
        type = "picker"
        [plugins.sys.views.main.engine.config]
"#,
    )
    .expect("could not write completion replay config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process
        .master
        .write_all(b"\tsys ")
        .expect("could not write completion replay batch");
    process
        .master
        .flush()
        .expect("could not flush completion replay batch");
    let output = wait_for_text(&process.master, "sys");
    assert!(
        String::from_utf8_lossy(&output).contains(" sys "),
        "output: {:?}",
        output
    );

    process.master.write_all(b"\x03").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).expect("could not remove completion replay config");
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.default.engine.config.bindings]
        open_commands = ["ctrl+p"]

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"


        [plugins.core.views.default.commands.run.payload]
        handler = ":"

        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
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
    let output = wait_for_text(&process.master, "Run");
    let output_text = String::from_utf8_lossy(&output);
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.default.commands.inspect]
        key = "enter"
        label = "Inspect"
        type = "navigate"

        [plugins.core.views.default.commands.inspect.payload]
        target = "{{ runtime:view.current.selected_item.metadata.target }}"
        query = "{{ runtime:view.current.input }}|{{ runtime:session.input.params }}|{{ runtime:view.current.selected_item.value }}"

        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
        [plugins.core.views.capture]
        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config]
        output = "{{ runtime:view.current.input }}"
        title = "Capture"
"#,
    )
    .expect("could not write command navigation integration config");

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    let _ = wait_for_text(&process.master, "Item");
    process
        .master
        .write_all(b"Item")
        .expect("could not write parent query");
    process
        .master
        .flush()
        .expect("could not flush parent query");
    let _ = wait_for_text(&process.master, "Item");

    process
        .master
        .write_all(b"\r")
        .expect("could not navigate with direct command");
    process
        .master
        .flush()
        .expect("could not flush direct navigation");
    let output = wait_for_text(&process.master, "Item|Item|value");
    assert!(
        String::from_utf8_lossy(&output).contains("Item|Item|value"),
        "output: {:?}",
        output
    );
    process
        .master
        .write_all(b"\r")
        .expect("could not return from direct capture view");
    process
        .master
        .flush()
        .expect("could not flush direct capture return");
    let _ = wait_for_text(&process.master, "Item");

    process
        .master
        .write_all(b"\x0b")
        .expect("could not open command picker");
    process.master.flush().expect("could not flush Ctrl-K");
    let _ = wait_for_text(&process.master, "Inspect");
    process
        .master
        .write_all(b"ins")
        .expect("could not filter command picker");
    process
        .master
        .write_all(b"\r")
        .expect("could not navigate from command picker");
    process
        .master
        .flush()
        .expect("could not flush command navigation");
    let output = wait_for_text(&process.master, "Item|Item|value");
    assert!(
        String::from_utf8_lossy(&output).contains("Item|Item|value"),
        "output: {:?}",
        output
    );

    process
        .master
        .write_all(b"\r")
        .expect("could not return from command-picker capture view");
    process
        .master
        .flush()
        .expect("could not flush command-picker capture return");
    let _ = wait_for_text(&process.master, "Item");
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
fn ctrl_k_feed_complete_uses_original_item_state_and_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/apps");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.core.views.default.commands.page]
        key = "ctrl+r"
        label = "Page Action"
        type = "run"
        [plugins.core.views.default.commands.page.payload]
        handler = ":"
        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "apps"

        [views.default]
        [views.default.engine]
        type = "picker"
        [views.default.engine.config]
        items = '{{ script("scripts/items.sh", this:query) }}'
        [views.default.query]
        type = "object"
        input_order = ["text"]
        text = { type = "string", default = "owner-default" }

        [views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "complete"

        [views.default.commands.accept.payload]
        handler = "scripts/result.sh"

        [views.default.commands.accept.payload.params]
        options = "{{ this:query }}"
        selected = "{{ runtime:view.current.selected_item }}"
        page_ref = "{{ runtime:view.current.ref }}"
        page_query = "{{ runtime:view.current.query }}"
        session_params = "{{ runtime:session.input.params }}"
        commands = "{{ runtime:view.current.command }}"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/items.sh"),
        r#"#!/bin/sh
text=$(cat | jq -r .text)
jq -cn --arg text "$text" '[{label:("ROW:" + $text), value: $text}]'
"#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"text":"typed-feed"' || exit 3
printf '%s' "$payload" | grep -q '"value":"typed-feed"' || exit 3
printf '%s' "$payload" | grep -q '"owner_view":"apps:default"' || exit 3
printf '%s' "$payload" | grep -q 'owner_query\|feed_id\|source_view\|binding_raw' && exit 3
printf '%s' "$payload" | grep -q '"page_ref":"core:default"' || exit 3
printf '%s' "$payload" | grep -q '"page_query":"typed-feed"' || exit 3
printf '%s' "$payload" | grep -q '"session_params":"typed-feed"' || exit 3
printf '%s' "$payload" | grep -q '"label":"Page Action"' || exit 3
printf '%s' "$payload" | grep -q '"label":"Accept"' || exit 3
printf 'owner-complete-ok'
exit 0
"#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"typed-feed").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "ROW:typed-feed");
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Accept");
    process.master.write_all(b"acc").unwrap();
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("owner-complete-ok"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_feed_binding_is_shared_by_items_and_direct_complete() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/apps");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
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
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "apps"

        [views.default]
        [views.default.engine]
        type = "picker"
        [views.default.engine.config]
        items = '{{ script("scripts/items.sh", this:$) }}'
        [views.default.query]
        type = "object"
        input_order = ["text"]
        text = { type = "string", default = "owner-default" }
        [views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "complete"
        [views.default.commands.accept.payload]
        handler = "scripts/result.sh"
        [views.default.commands.accept.payload.params]
        raw = "{{ this:raw_input }}"
        query = "{{ this:query }}"
        selected = "{{ runtime:view.current.selected_item }}"
        session_params = "{{ runtime:session.input.params }}"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/items.sh"),
        r#"#!/bin/sh
payload=$(cat)
raw=$(printf '%s' "$payload" | jq -r .raw_input)
text=$(printf '%s' "$payload" | jq -r .query.text)
jq -cn --arg raw "$raw" --arg text "$text" '[{label:("RAW:" + $raw + "|TEXT:" + $text), value:$text}]'
"#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"raw":""' || exit 4
printf '%s' "$payload" | grep -q '"text":"owner-default"' || exit 4
printf '%s' "$payload" | grep -q '"value":"owner-default"' || exit 4
printf '%s' "$payload" | grep -q '"session_params":""' || exit 4
printf 'empty-binding-ok'
"#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "RAW:|TEXT:owner-default");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("empty-binding-ok"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
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
        show_prefix = true
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.core.views.default.commands.page]
        key = "ctrl+r"
        label = "Page"
        type = "run"
        [plugins.core.views.default.commands.page.payload]
        handler = '''printf 'page-command\n' '''
        exit = true

        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ config:catalog.items }}"
        [plugins.apps.views.default.commands.open]
        key = "enter"
        label = "Open"
        type = "run"
        [plugins.apps.views.default.commands.open.payload]
        handler = '''printf 'owner-command\n' '''
        exit = true

        [catalog]
        items = [{label = "Row", value = "row"}]
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
fn ctrl_k_lists_page_commands_and_uses_owner_conflict_priority() {
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
        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
        [plugins.core.views.default.commands.conflict]
        key = "enter"
        label = "Page Conflict"
        type = "run"
        [plugins.core.views.default.commands.conflict.payload]
        handler = '''printf 'wrong-page-conflict\n' '''
        exit = true
        [plugins.core.views.default.commands.page]
        key = "ctrl+r"
        label = "Page Action"
        type = "run"
        [plugins.core.views.default.commands.page.payload]
        handler = '''printf 'ctrl-k-page\n' '''
        exit = true

        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ config:catalog.items }}"
        [plugins.apps.views.default.commands.open]
        key = "enter"
        label = "Owner Action"
        type = "run"
        [plugins.apps.views.default.commands.open.payload]
        handler = '''printf 'owner-action\n' '''
        exit = true

        [catalog]
        items = [{label = "Row", value = "row"}]
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "Row");
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    let screen = wait_for_text(&process.master, "Page Action");
    let screen = String::from_utf8_lossy(&screen);
    assert!(screen.contains("Owner Action"), "screen: {screen}");
    assert!(!screen.contains("Page Conflict"), "screen: {screen}");
    process.master.write_all(b"\x1b[B\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("ctrl-k-page"),
        "output: {:?}",
        output
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ctrl_k_shows_page_commands_without_a_selected_item() {
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
        items = "{{ config:catalog.items }}"
        [plugins.core.views.default.commands.page]
        key = "enter"
        label = "Page Only"
        type = "run"
        [plugins.core.views.default.commands.page.payload]
        handler = '''printf 'page-only-ok\n' '''
        exit = true
        [plugins.core.views.command]
        [plugins.core.views.command.engine]
        type = "picker"
        [plugins.core.views.command.engine.config]
        [catalog]
        items = []
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    wait_for_text(&process.master, "no matches");
    process.master.write_all(b"\x0b").unwrap();
    process.master.flush().unwrap();
    wait_for_text(&process.master, "Page Only");
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(
        String::from_utf8_lossy(&output).contains("page-only-ok"),
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
        show_prefix = true
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [plugins.apps.views.default]
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ config:catalog.items }}"
        [plugins.apps.views.default.query]
        type = "object"
        input_order = ["text"]
        text = { type = "string", default = "source-default" }

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
fn route_input_escape_removes_the_route_tag() {
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
        show_prefix = true
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "sys:default"
        [plugins.apps.views.default]
        alias = "app"
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ config:test_items.items }}"
        [plugins.sys.views.default]
        alias = "sys"
        [plugins.sys.views.default.engine]
        type = "picker"
        [plugins.sys.views.default.engine.config]
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
        .expect("could not write route escape");
    process
        .master
        .flush()
        .expect("could not flush route escape");
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
fn deleting_route_input_returns_to_parent_before_switching_aliases() {
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
        show_prefix = true
        [[plugins.core.views.default.engine.config.feeds]]
        view = "apps:default"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "sys:default"
        [plugins.apps.views.default]
        alias = "app"
        [plugins.apps.views.default.engine]
        type = "picker"
        [plugins.apps.views.default.engine.config]
        items = "{{ config:test_items.items }}"
        [plugins.sys.views.default]
        alias = "sys"
        [plugins.sys.views.default.engine]
        type = "picker"
        [plugins.sys.views.default.engine.config]
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
        .expect("could not delete route tag");
    process
        .master
        .flush()
        .expect("could not flush route tag deletion");
    process
        .master
        .write_all(b"\x7f\x7f\x7f")
        .expect("could not delete input after route tag removal");
    process
        .master
        .flush()
        .expect("could not flush input deletion after route tag removal");
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
        "[plugin]\nname = \"core\"\n\n[views.placeholder.engine]\ntype = \"picker\"\n[views.placeholder.engine.config]\n",
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        [plugins.core.views.messages]
        alias = "log"
        [plugins.core.views.messages.engine]
        type = "picker"
        [plugins.core.views.messages.engine.config]
        items = '{{ script("scripts/items.sh", {query = this:query, log_file = runtime:view.current.log_file}) }}'
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
    let _ = wait_for_text(&process.master, "log");
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
alias = "temp"
[views.default.engine]
type = "picker"
[views.default.engine.config]
"#,
        )
        .expect("could not write conflicting plugin manifest");
    }
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
        items = "{{ config:test_items.items }}"
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
        output = "{{ this:query.text }}"
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
        .write_all(b"\r")
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
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:capture"
        query = "capture-marker:{{ runtime:view.current.selected_item.value }}"

        [plugins.core.views.capture]
        alias = "cap"
        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config]
        output = "{{ runtime:view.current.input }}"
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
    output.extend(launcher);
    output.extend(remaining);
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("capture-marker:value"), "output: {output}");
    assert!(output.contains("core:capture"), "output: {output}");
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        items = "{{ config:test_items.items }}"
        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "navigate"

        [plugins.core.views.default.commands.run.payload]
        target = "core:embedded"
        query = '''printf 'embedded-marker:%s\n' '{{ runtime:view.current.selected_item.value }}'; exit 0'''

        [plugins.core.views.embedded]
        alias = "emb"
        [plugins.core.views.embedded.engine]
        type = "embedded"
        [plugins.core.views.embedded.engine.config]
        command = ["sh", "-lc", "{{ runtime:view.current.input }}"]
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
    assert!(output.contains("core:embedded"), "output: {output}");
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
        show_prefix = true
        [plugins.core.views.broken]
        [plugins.core.views.broken.engine]
        type = "embedded"
        [plugins.core.views.broken.engine.config]
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        show_prefix = true
        [plugins.core.views.embedded]
        [plugins.core.views.embedded.engine]
        type = "embedded"
        [plugins.core.views.embedded.engine.config]
        command = ["sh", "-lc", "{{ runtime:view.current.input }}"]
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
