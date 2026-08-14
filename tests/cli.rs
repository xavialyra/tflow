mod support;

use std::process::Command;

use support::{binary_path, fixture_config, temporary_root, write_test_config};

#[test]
fn check_loads_the_fixture_configuration() {
    let output = Command::new(binary_path())
        .args(["--check", "--config"])
        .arg(fixture_config())
        .output()
        .expect("could not run tui-launcher --check");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("configuration is valid: {}\n", fixture_config().display())
    );
}

#[test]
fn view_query_rejects_cli_positionals_and_unknown_keys() {
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
        output = "{{ this:query.message }}"
        [plugins.core.views.default.query]
        type = "object"
        message = { type = "string", default = "" }
        "#,
    )
    .unwrap();

    let positional = Command::new(binary_path())
        .arg("--config")
        .arg(&config)
        .args(["core:default", "message"])
        .output()
        .unwrap();
    assert!(!positional.status.success());
    assert!(
        String::from_utf8_lossy(&positional.stderr).contains("does not accept positional argument")
    );

    let unknown = Command::new(binary_path())
        .arg("--config")
        .arg(&config)
        .args(["core:default", "--unknown=value"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("does not declare query parameter"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_removed_picker_fields() {
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
        max_rows = 0
"#,
    )
    .unwrap();

    let output = Command::new(binary_path())
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has unsupported field \"max_rows\""),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}
