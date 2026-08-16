mod support;

use std::process::Command;

use support::{binary_path, fixture_config, temporary_root, write_test_config};

fn launcher_command() -> Command {
    Command::new(binary_path())
}

#[test]
fn check_loads_the_fixture_configuration() {
    let output = launcher_command()
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
fn fixture_can_switch_between_named_theme_files() {
    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(fixture_config())
        .args(["--theme", "contrast"])
        .output()
        .expect("could not select the contrast fixture theme");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
}

#[test]
fn cli_theme_selection_does_not_require_an_existing_current_directory() {
    for theme in ["terminal", "contrast"] {
        let current_dir = temporary_root();
        let output = Command::new("sh")
            .current_dir(&current_dir)
            .args(["-c", "rmdir \"$PWD\" && exec \"$@\"", "tui-launcher"])
            .arg(binary_path())
            .args(["--check", "--config"])
            .arg(fixture_config())
            .args(["--theme", theme])
            .output()
            .expect("could not run tui-launcher from a removed current directory");

        assert!(output.status.success(), "stderr: {:?}", output.stderr);
    }
}

#[test]
fn check_rejects_removed_theme_file_option() {
    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(fixture_config())
        .args(["--theme-file", "./theme.toml"])
        .output()
        .expect("could not validate the removed theme-file option");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument '--theme-file'"),
        "stderr: {:?}",
        output.stderr
    );
}

#[test]
fn check_rejects_removed_global_input_bindings() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [chrome.input.bindings.views]
        key = "tab"
        type = "call"
        payload = { target = "core:default" }

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown field `input`"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
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

    let positional = launcher_command()
        .arg("--config")
        .arg(&config)
        .args(["core:default", "message"])
        .output()
        .unwrap();
    assert!(!positional.status.success());
    assert!(
        String::from_utf8_lossy(&positional.stderr).contains("does not accept positional argument")
    );

    let unknown = launcher_command()
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
fn check_rejects_the_removed_complete_action() {
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
        type = "complete"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown variant `complete`"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_removed_picker_prompt() {
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
        prompt = "> "
"#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has unsupported field \"prompt\""),
        "stderr: {:?}",
        output.stderr
    );
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

    let output = launcher_command()
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

#[test]
fn check_loads_a_named_theme_from_the_config_directory() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "work"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(
        root.join("themes/work.toml"),
        "[tokens.accent]\nforeground = \"magenta\"\n\n[tokens.muted]\nforeground = \"gray\"\n",
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate a named theme");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_an_inline_theme_value() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r##"
        [theme]
        base = { foreground = "terminal", background = "#102030" }
        accent = { foreground = "cyan" }
        highlight = { foreground = "yellow", background = "blue", bold = true }

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "##,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate an inline theme");

    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expected a string"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_an_invalid_theme_override() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .args(["--theme-set", "accent.foreground=not-a-color"])
        .output()
        .expect("could not validate an invalid theme override");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unsupported color"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_removed_theme_modifiers() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .args(["--theme-set", "highlight.reverse=true"])
        .output()
        .expect("could not validate a removed theme modifier");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("unsupported"),
        "stderr: {:?}",
        output.stderr
    );
    assert!(stderr.contains("reverse"), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_theme_replaces_the_root_configuration() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "missing-root-theme"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .args(["--theme", "terminal"])
        .output()
        .expect("could not validate theme precedence");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_named_theme_reports_the_theme_path() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "work"

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate a theme reference typo");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("could not resolve theme"),
        "stderr: {:?}",
        output.stderr
    );
    assert!(
        stderr.contains("themes/work.toml"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}
