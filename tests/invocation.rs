mod support;

use std::fs;
use support::{
    run_invocation, run_invocation_steps, run_tty_invocation_with_redirected_stdout,
    temporary_root, write_test_config,
};

#[test]
fn redirected_stdout_contains_only_the_default_completion_result() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [plugins.core.views.default]
        type = "picker"
        items = "{{ config:test_items.items }}"

        [plugins.core.views.default.commands.accept]
        key = "enter"
        label = "Accept"
        complete = true
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
fn root_result_handler_receives_query_and_controls_raw_output_and_status() {
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
        type = "picker"
        items = "{{ config:catalog.items }}"
        result_handler = "scripts/result.sh"

        [views.default.query]
        type = "object"
        tag = '''{{ state("string", null) }}'''

        [views.default.commands.accept]
        key = "enter"
        label = "Accept"
        complete = true
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"query":{' || exit 3
printf '%s' "$payload" | grep -q '"tag":"value"' || exit 3
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
fn root_result_handler_keeps_the_root_query_after_child_completion() {
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
        type = "picker"
        items = "{{ config:catalog.items }}"
        result_handler = "scripts/result.sh"

        [views.default.query]
        type = "object"
        tag = '''{{ state("string", null) }}'''

        [views.default.commands.next]
        key = "enter"
        label = "Next"
        view = "custom:child"

        [views.child]
        type = "picker"
        items = "{{ config:catalog.items }}"

        [views.child.commands.accept]
        key = "enter"
        label = "Accept"
        complete = true
        "#,
    )
    .unwrap();
    fs::write(
        plugin.join("scripts/result.sh"),
        r#"#!/bin/sh
payload=$(cat)
printf '%s' "$payload" | grep -q '"tag":"root-value"' || exit 3
printf 'root-query'
"#,
    )
    .unwrap();

    let config = config.to_str().unwrap();
    let result = run_invocation_steps(
        &["--config", config, "custom:default", "--tag=root-value"],
        b"raw input",
        &[b"\r", b"\r"],
    );

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"root-query");
    fs::remove_dir_all(root).unwrap();
}
