mod support;

use std::fs;
use support::{
    run_invocation, run_invocation_steps, run_tty_invocation_with_redirected_stdout,
    temporary_root, wait_for_process_exit, write_test_config,
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
        items = "{{ config:test_items.items }}"
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
fn called_picker_fields_and_commands_can_read_request_args() {
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
        type = "call"

        [plugins.core.views.default.commands.open.payload]
        target = "forms:main"
        args = { show_prefix = true, result = { name = "Ada" } }

        [plugins.core.views.default.commands.open.payload.then]
        type = "return"

        [plugins.core.views.default.commands.open.payload.then.payload]
        value = "{{ return:output.value }}"

        [plugins.forms.views.main]
        [plugins.forms.views.main.engine]
        type = "picker"
        [plugins.forms.views.main.engine.config]
        show_prefix = "{{ request:args.show_prefix }}"
        items = "{{ config:test_items.items }}"

        [plugins.forms.views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [plugins.forms.views.main.commands.accept.payload]
        value = "{{ request:args.result }}"
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
fn request_namespace_is_unavailable_at_the_root() {
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

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [plugins.core.views.default.commands.accept.payload]
        value = "{{ request:args }}"
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
        command = ["sh", "-c", "stty raw -echo; set -- $(dd bs=1 count=2 2>/dev/null | od -An -tu1); printf '%s,%s' \"$1\" \"$2\""]
        escape-cancels = false
        result = { format = "text", required = true }
        "#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_tty_invocation_with_redirected_stdout(
        &["--config", config, "core:default"],
        b"\x1b\x03",
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"27,3\n");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn embedded_input_does_not_dispatch_chrome_footer_bindings() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [chrome.footer.bindings.commands]
        key = "ctrl+k"
        label = "Commands"
        type = "call"

        [chrome.footer.bindings.commands.payload]
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
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.form]
        key = "enter"
        label = "Form"
        type = "call"

        [plugins.core.views.default.commands.form.payload]
        target = "forms:profile"
        args = { initial = "Ada" }

        [plugins.core.views.default.commands.form.payload.then]
        type = "return"

        [plugins.core.views.default.commands.form.payload.then.payload]
        value = "{{ return:output.value }}"

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
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.form]
        key = "enter"
        label = "Form"
        type = "call"

        [plugins.core.views.default.commands.form.payload]
        target = "forms:main"

        [plugins.core.views.default.commands.form.payload.then]
        type = "return"

        [plugins.core.views.default.commands.form.payload.then.payload]
        value = "{{ return:output.value }}"

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
fn return_handler_receives_explicit_params_and_controls_raw_output_and_status() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let plugin = root.join("plugins/custom");
    fs::create_dir_all(plugin.join("scripts")).unwrap();
    fs::write(
        &config,
        r#"
        default_view = "custom:default"

        [catalog]
        items = [{label = "Item", value = "selected-value"}]
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
        items = "{{ config:catalog.items }}"
        [views.default.query]
        type = "object"
        tag = { type = "string", nullable = true }

        [views.default.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [views.default.commands.accept.payload]
        handler = "scripts/result.sh"

        [views.default.commands.accept.payload.params]
        options = "{{ this:query }}"
        stdin = "{{ input:stdin }}"
        selected = "{{ runtime:view.current.selected_item }}"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"options":{' || exit 3
printf '%s' "$payload" | grep -q '"tag":"value"' || exit 3
printf '%s' "$payload" | grep -q '"value":"selected-value"' || exit 3
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
        items = [{label = "Item", value = "selected-value"}]
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
        items = "{{ config:catalog.items }}"
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
        items = "{{ config:catalog.items }}"
        [views.child.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"

        [views.child.commands.accept.payload]
        handler = "scripts/result.sh"

        [views.child.commands.accept.payload.params]
        selected = "{{ runtime:view.current.selected_item }}"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"value":"selected-value"' || exit 3
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
    fs::write(
        &config,
        r#"
        default_view = "core:default"

        [catalog]
        items = [{label = "Item", value = "selected-value"}]

        [plugins.core]
        name = "core"
        [plugins.core.views.default.engine]
        type = "picker"
        [[plugins.core.views.default.engine.config.feeds]]
        view = "custom:main"
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
        items = "{{ config:catalog.items }}"

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

        [views.main.commands.accept.payload.params]
        query = "{{ this:query }}"
        raw = "{{ this:raw_input }}"
        selected = "{{ runtime:view.current.selected_item }}"
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"text":"owner-default"' || exit 3
printf '%s' "$payload" | grep -q '"raw":""' || exit 3
printf '%s' "$payload" | grep -q '"value":"selected-value"' || exit 3
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
