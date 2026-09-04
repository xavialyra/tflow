mod support;

use std::fs;
use std::io::Write;
use support::{
    run_invocation, run_invocation_steps, run_tty_invocation_with_redirected_stdout,
    run_tty_invocation_with_redirected_stdout_after_marker, spawn_launcher_with_args_and_env,
    temporary_root, wait_for_launcher_exit, wait_for_nonempty_file, wait_for_process_exit,
    wait_for_ready, write_test_config,
};

#[test]
fn redirected_stdout_contains_only_the_default_return_result() {
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
        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"value\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn requires_input_return_uses_the_latest_committed_input() {
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

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        scope = "view"
        requires = "input"
        type = "return"

        [plugins.core.views.default.commands.accept.payload]
        value = "{{ view.raw_input }}"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_tty_invocation_with_redirected_stdout(
        &["--config", config, "core:default"],
        b"latest\r",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"latest\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn called_picker_fields_and_commands_read_declared_query_values() {
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
        type = "call"

        [plugins.core.views.default.commands.open.payload]
        target = "forms:main"
        query = { result = { name = "Ada" } }

        [plugins.core.views.default.commands.open.payload.then]
        type = "return"

        [plugins.core.views.default.commands.open.payload.then.payload]
        value = "{{ result.output.value }}"

        [plugins.forms.views.main]
        [plugins.forms.views.main.engine]
        type = "picker"
        [plugins.forms.views.main.engine.config]
        items = [{display = "Item", value = "value", metadata = {target = "core:capture"}}]

        [plugins.forms.views.main.query]
        type = "object"
        input_order = []
        result = { type = "object", default = {} }

        [plugins.forms.views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [plugins.forms.views.main.commands.accept.payload]
        value = "{{ view.query.result }}"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(&["--config", config, "core:default"], b"", &[b"\r", b"\r"]);

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"{\"name\":\"Ada\"}\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn picker_layout_and_preview_resolve_from_the_operation_scope() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        layout = { type = "object", default = { direction = "horizontal", gap = 1, panes = [{ slot = "items", grow = 1, min = 1 }, { slot = "preview", size = 12, min = 1 }] } }
        preview = { type = "object", default = { blocks = [{ type = "separator" }] } }

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        layout = "{{ view.query.layout }}"
        preview = "{{ view.query.preview }}"
        items = [{display = "Item", value = "value"}]

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"value\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn result_namespace_is_rejected_outside_a_return_consumption_stage() {
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

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [plugins.core.views.default.commands.accept.payload]
        value = "{{ result }}"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 1);
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_view_commands_use_the_same_return_dispatcher() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "capture"

        [plugins.core.views.default.engine.config]
        output = "captured value"

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        scope = "view"
        requires = "input"
        type = "return"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"captured value\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn passthrough_view_command_remains_available_in_normal_mode() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
        output = "normal-mode"

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        passthrough = true
        type = "return"
        [plugins.core.views.default.commands.accept.payload]
        value = "normal-passthrough-command"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"normal-passthrough-command\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_capture_is_not_available_as_implicit_return_output() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "capture"

        [plugins.core.views.default.engine.config]
        output = "{{ selection.missing }}"

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        scope = "view"
        requires = "input"
        type = "return"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\r");

    assert_eq!(result.status, 1);
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_bare_escape_cancels_the_process() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "trap 'exit 0' INT TERM; printf 'READY' >&2; while :; do sleep 1; done"]
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(&["--config", config, "core:default"], b"", &[b"\x1b"]);

    assert_eq!(result.status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_escape_and_ctrl_c_reach_the_child_when_cancellation_is_disabled() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "stty raw -echo; printf '__TUI_LAUNCHER_CHILD_READY__\\n' >&2; set -- $(dd bs=1 count=2 2>/dev/null | od -An -tu1); printf '%s,%s' \"$1\" \"$2\""]
        escape-cancels = false
        result = { format = "text", required = true }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_tty_invocation_with_redirected_stdout_after_marker(
        &["--config", config, "core:default"],
        "__TUI_LAUNCHER_CHILD_READY__",
        b"\x1b\x03",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"27,3\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_input_does_not_dispatch_session_commands_without_a_passthrough_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [commands.bindings.details]
        key = "ctrl+k"
        label = "Details"
        type = "call"

        [commands.bindings.details.payload]
        target = "selectors:main"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"
        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "stty raw -echo; dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' '"]
        result = { format = "text", required = true }

        [plugins.selectors.views.main]
        [plugins.selectors.views.main.engine]
        type = "picker"
        [plugins.selectors.views.main.engine.config]
        items = []
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\x0b");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"11\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_passthrough_command_consumes_a_switch_key() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"
        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "sleep 5"]

        [plugins.core.views.default.commands.finish]
        key = "ctrl+b"
        label = "Finish"
        passthrough = true
        type = "return"

        [plugins.core.views.default.commands.finish.payload]
        value = "switch-consumed"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\x02");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"switch-consumed\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_passthrough_view_command_overrides_engine_cancel_binding() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"
        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "sleep 5"]

        [plugins.core.views.default.commands.finish]
        key = "escape"
        label = "Finish"
        passthrough = true
        type = "return"

        [plugins.core.views.default.commands.finish.payload]
        value = "view-overrode-engine"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\x1b");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"view-overrode-engine\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_passthrough_command_selector_returns_to_the_embedded_view() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"
        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "sleep 5"]

        [plugins.core.views.default.commands.finish]
        label = "Finish"
        passthrough = true
        type = "return"
        [plugins.core.views.default.commands.finish.payload]
        value = "overlay-returned"

        [plugins.selectors.views.commands]
        [plugins.selectors.views.commands.engine]
        type = "picker"
        [plugins.selectors.views.commands.engine.config]
        items = [{display = "Finish", metadata = {command = {view = "core:default", id = "finish"}}}]
        [plugins.selectors.views.commands.query]
        type = "object"
        input_order = ["search"]
        search = {type = "string", default = ""}
        commands = {type = "array<object>", default = []}
        [plugins.selectors.views.commands.commands.accept]
        key = "enter"
        label = "Select"
        type = "return"
        [plugins.selectors.views.commands.commands.accept.payload]
        value = "{{ selection.metadata.command }}"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"\x0b\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"overlay-returned\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_leader_exit_cleans_up_its_process_group() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let pid_file = root.join("child.pid");
    let source = format!(
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "sleep 30 & echo $! > '{}'; exit 0"]
        "#,
        pid_file.display()
    );
    write_test_config(&config, &source).unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation(&["--config", config, "core:default"], b"", b"");

    assert_eq!(result.status, 0);
    let pid = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for_process_exit(pid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_successful_exit_returns_json_to_the_caller() {
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

        [plugins.core.views.default.commands.form]
        key = "enter"
        label = "Form"
        type = "call"

        [plugins.core.views.default.commands.form.payload]
        target = "forms:profile"

        [plugins.core.views.default.commands.form.payload.then]
        type = "return"

        [plugins.core.views.default.commands.form.payload.then.payload]
        value = "{{ result.output.value }}"

        [plugins.forms.views.profile]
        [plugins.forms.views.profile.engine]
        type = "embedded"

        [plugins.forms.views.profile.engine.config]
        command = ["sh", "-c", "printf 'FORM' >&2; IFS= read -r _; printf 'WAIT' >&2; IFS= read -r _; printf '%s' '{\"name\":\"Ada\",\"enabled\":true}'"]
        result = { format = "json", required = true, max_bytes = 4096 }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(
        &["--config", config, "core:default"],
        b"",
        &[b"\r", b"\r", b"confirm\r"],
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"{\"enabled\":true,\"name\":\"Ada\"}\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_nonzero_exit_does_not_return_a_result() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"
        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "exit 7"]
        result = { format = "text", required = false }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"");

    assert_eq!(result.status, 0);
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_launcher_bytes_are_transferred_to_embedded_input() {
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

        [plugins.core.views.default.commands.form]
        key = "enter"
        label = "Form"
        type = "call"

        [plugins.core.views.default.commands.form.payload]
        target = "forms:main"

        [plugins.core.views.default.commands.form.payload.then]
        type = "return"

        [plugins.core.views.default.commands.form.payload.then.payload]
        value = "{{ result.output.value }}"

        [plugins.forms.views.main]
        [plugins.forms.views.main.engine]
        type = "embedded"
        [plugins.forms.views.main.engine.config]
        command = ["sh", "-c", "IFS= read -r value; printf '%s' \"$value\""]
        result = { format = "text", required = true }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(
        &["--config", config, "core:default"],
        b"",
        &[b"\r\xc3", b"\xa9\r"],
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, [0xc3, 0xa9, b'\n']);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_result_is_returned_from_a_direct_invocation() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "printf '%s' '{\"source\":\"process\"}'"]
        result = { format = "json", required = true }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"{\"source\":\"process\"}\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_options_resolve_from_the_operation_scope() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.query]
        type = "object"
        input_order = []
        escape_cancels = { type = "boolean", default = false }
        result_config = { type = "object", default = { format = "text", required = true } }

        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "printf dynamic-result"]
        escape-cancels = "{{ view.query.escape_cancels }}"
        result = "{{ view.query.result_config }}"
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"dynamic-result\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_missing_required_result_fails_after_successful_exit() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", ":"]
        result = { format = "text", required = true }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"");

    assert_eq!(result.status, 1);
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_result_pipe_enforces_its_byte_limit() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "embedded"

        [plugins.core.views.default.engine.config]
        command = ["sh", "-c", "printf '12345'"]
        result = { format = "text", required = true, max_bytes = 4 }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result =
        run_tty_invocation_with_redirected_stdout(&["--config", config, "core:default"], b"");

    assert_eq!(result.status, 1);
    assert!(result.stdout.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn return_handler_receives_argv_and_controls_raw_output_and_status() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        default_view = "custom:default"

        [catalog]
        items = [{display = "Item", value = "selected-value"}]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"

        [views.default]
        [views.default.engine]
        type = "picker"
        [views.default.engine.config]
        items = [{display = "Item", value = "selected-value"}]
        [views.default.query]
        type = "object"
        tag = { type = "string", nullable = true }

        [views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [views.default.commands.accept.payload]
        handler = "scripts/result.sh"
        args = [
            "--options={{ view.query }}",
            "{{ input.stdin }}",
            "{{ selection }}",
            "{{ page.input }}",
            "{{ result }}",
        ]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
options=${1#--options=}
stdin_info=$2
selected=$3
typed=$4
returned=$5
[ "$#" -eq 5 ] || exit 3
if read -r unexpected; then exit 3; fi
printf '%s' "$options" | grep -q '"tag":"value"' || exit 3
printf '%s' "$stdin_info" | grep -q '"is_tty":false' || exit 3
printf '%s' "$selected" | grep -q '"value":"selected-value"' || exit 3
printf '%s' "$returned" | grep -q '"source":"custom:default"' || exit 3
printf '%s' "$typed" >/dev/null
printf 'handled-output'
printf 'handled-warning\n' >&2
exit 7
"#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation(
        &["--config", config, "custom:default", "--tag=value"],
        b"raw input",
        b"\r",
    );

    assert_eq!(result.status, 7);
    assert_eq!(result.stdout, b"handled-output");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn signal_exit_terminates_a_running_return_handler() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    let pid_file = root.join("handler.pid");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        default_view = "custom:main"
        [catalog]
        items = [{display = "Return"}]
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
        [views.main.engine.config]
        items = [{display = "Item", value = "selected-value"}]
        [views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        [views.main.commands.accept.payload]
        handler = "scripts/result.sh"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        "#!/bin/sh\nprintf '%s' $$ > \"$PID_FILE\"\nsleep 30\n",
    )
    .unwrap();
    let pid_path = pid_file.to_string_lossy().to_string();
    let mut process = spawn_launcher_with_args_and_env(
        &config,
        &["custom:main"],
        &[("PID_FILE", pid_path.as_str())],
    );
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
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
fn return_handler_belongs_to_the_returning_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        default_view = "custom:default"

        [catalog]
        items = [{display = "Item", value = "selected-value"}]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"

        [views.default]
        [views.default.engine]
        type = "picker"
        [views.default.engine.config]
        items = [{display = "Item", value = "selected-value"}]
        [views.default.commands.next]
        key = "enter"
        label = "Next"
        type = "navigate"

        [views.default.commands.next.payload]
        target = "custom:child"

        [views.child]
        [views.child.engine]
        type = "picker"
        [views.child.engine.config]
        items = [{display = "Item", value = "selected-value"}]
        [views.child.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [views.child.commands.accept.payload]
        handler = "scripts/result.sh"
        args = ["{{ selection }}"]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
selected=$1
printf '%s' "$selected" | grep -q '"value":"selected-value"' || exit 3
printf 'child-command'
"#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(
        &["--config", config, "custom:default"],
        b"raw input",
        &[b"\r", b"\r"],
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"child-command");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn root_return_handler_uses_the_selected_feed_owner_context() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    let core = root.join("plugins/core");
    fs::create_dir_all(&core).unwrap();
    fs::write(
        core.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "core"

        [views.default.engine]
        type = "picker"
        [[views.default.engine.config.feeds]]
        view = "custom:main"
        "#,
    )
    .unwrap();
    fs::write(
        &config,
        r#"
        default_view = "core:default"

        [catalog]
        items = [{display = "Item", value = "selected-value"}]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        r#"
        [plugin]
        api = 1
        name = "custom"

        [views.main]
        [views.main.engine]
        type = "picker"
        [views.main.engine.config]
        items = [{display = "Item", value = "selected-value"}]

        [views.main.query]
        type = "object"
        input_order = ["text"]
        text = { type = "string", default = "owner-default" }

        [views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [views.main.commands.accept.payload]
        handler = "scripts/result.sh"
        args = [
            "{{ view.query }}",
            "{{ view.raw_input }}",
            "{{ selection }}",
        ]
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
query=$1
raw=$2
selected=$3
printf '%s' "$query" | grep -q '"text":"owner-default"' || exit 3
[ -z "$raw" ] || exit 3
printf '%s' "$selected" | grep -q '"value":"selected-value"' || exit 3
printf 'feed-owner'
"#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation(&["--config", config, "core:default"], b"", b"\r");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"feed-owner");
    fs::remove_dir_all(root).unwrap();
}
