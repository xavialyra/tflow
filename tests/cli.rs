mod support;

use std::{fs, process::Command};

use support::{binary_path, fixture_config, temporary_root, write_test_config};

fn launcher_command() -> Command {
    Command::new(binary_path())
}

#[test]
fn check_rejects_unknown_root_plugins_field() {
    let root = temporary_root();
    let config = root.join("config.toml");
    fs::write(
        &config,
        r#"
        default_view = "unknown:main"

        [plugins.unknown.views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate unknown root field");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("unknown field `plugins`"),
        "stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_unknown_workflow_manifest_fields() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let manifest = root.join("workflows/unknown/workflow.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(&config, "default_view = \"unknown:main\"\n").unwrap();
    fs::write(
        &manifest,
        r#"
        [workflow]
        api = 1
        name = "Unknown"

        [views.main.engine]
        type = "picker"

        [plugin]
        name = "Unknown"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate workflow manifest");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("unsupported fields [\"plugin\"]"),
        "stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_builtin_session_command_action_overrides() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [commands.bindings.commands]
        key = "ctrl+k"
        label = "Override"

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate the session command override");
    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("session command \"commands\" is built in; configure only its key"),
        "stderr: {:?}",
        output.stderr
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_unknown_defaults_fields() {
    for (source, expected) in [
        (
            r#"
            [defaults.capture]
            binding = { copy = ["enter"] }
            "#,
            "unknown field `binding`",
        ),
        (
            r#"
            [defaults.captuer.bindings]
            copy = ["enter"]
            "#,
            "unknown field `captuer`",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"

                [workflows.core.views.default]
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                {source}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not run tui-launcher --check");

        assert!(!output.status.success(), "stderr: {:?}", output.stderr);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_rejects_dynamic_bootstrap_references() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "{{ input.mode }}"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = []
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("view \"{{ input.mode }}\" is not configured"),
        "stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_static_picker_conflicts_with_dynamic_defaults() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [defaults.picker.bindings]
        exit = ["enter"]

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("picker key \"enter\" is assigned to both"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_static_capture_conflicts_with_dynamic_defaults() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [defaults.capture.bindings]
        back = ["enter"]

        [workflows.core.views.default.engine]
        type = "capture"
        [workflows.core.views.default.engine.config]
        output = "captured"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not run tui-launcher --check");

    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("capture key \"enter\" is assigned to both"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
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
            .expect("could not run tui-launcher from a missing current directory");

        assert!(output.status.success(), "stderr: {:?}", output.stderr);
    }
}

#[test]
fn view_query_rejects_cli_positionals_and_unknown_keys() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "capture"
        [workflows.core.views.default.engine.config]
        output = "{{ view.query.message }}"
        [workflows.core.views.default.query]
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
fn check_rejects_picker_runtime_field_shape_mismatches() {
    for (field, expected) in [
        (
            "layout = 42\npreview = { blocks = [{ type = \"text\" }] }",
            "picker layout is invalid",
        ),
        (
            "layout = { panes = [{ slot = \"items\", grow = 1 }, { slot = \"preview\", grow = 1 }] }\npreview = \"not-a-table\"",
            "picker preview is invalid",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                items = []
                {field}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate picker runtime field shape");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted {field}: {stderr}");
        assert!(stderr.contains(expected), "stderr: {stderr}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_treats_template_looking_static_values_as_literals() {
    for value in [
        "{{ page.value == 1 }}",
        "{{ {\"value\": page.value} }}",
        "{{ unknown.value }}",
        "{{ page.typo }}",
        "{{ view.typo }}",
        "{{ session.typo }}",
        "{{ malformed",
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "capture"
                [workflows.core.views.default.engine.config]
                output = {value:?}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate literal capture output");
        assert!(
            output.status.success(),
            "rejected literal {value}; stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_validates_static_items_sources_without_running_them() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config.items]
        producer = "script"
        [workflows.core.views.default.engine.config.items.handler]
        file = "scripts/items.sh"

        [workflows.core.views.capture.engine]
        type = "capture"
        [workflows.core.views.capture.engine.config]
        [workflows.core.views.capture.engine.config.output]
        producer = "script"
        [workflows.core.views.capture.engine.config.output.handler]
        file = "scripts/output.sh"
        "#,
    )
    .unwrap();
    let items_marker = root.join("items-script-ran");
    let capture_marker = root.join("capture-script-ran");
    let scripts = root.join("workflows/core/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("items.sh"),
        format!("printf ran > {:?}\nprintf '[]\\n'\n", items_marker),
    )
    .unwrap();
    std::fs::write(
        scripts.join("output.sh"),
        format!(
            "printf ran > {:?}\nprintf '{{\\\"version\\\":1,\\\"output\\\":\\\"output\\\"}}\\n'\n",
            capture_marker
        ),
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate static items source");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(!items_marker.exists(), "--check executed the items script");
    assert!(
        !capture_marker.exists(),
        "--check executed the capture script"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_treats_file_backed_run_handlers_as_opaque_scripts() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default.engine]
        type = "picker"

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "script"
        [workflows.core.views.default.commands.run.handler]
        file = "scripts/run.sh"
        "#,
    )
    .unwrap();
    let scripts = root.join("workflows/core/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("run.sh"), "printf '{{ user_template }}\\n'\\n").unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate file-backed handler");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_treats_inline_run_script_bodies_as_opaque() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default.engine]
        type = "picker"

        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "script"
        [workflows.core.views.default.commands.run.handler]
        script = """
        printf '%s\\n' '{{ user_template }}'
        """
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate inline run script");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_invalid_run_handler_sources() {
    for (handler, expected) in [
        (
            r#"{ file = "scripts/run.sh", script = "printf ok" }"#,
            "must define exactly one of file or script",
        ),
        (r#""scripts/run.sh""#, "must be a table with file or script"),
        (
            r#"{ file = "../run.sh" }"#,
            "script path \"../run.sh\" must stay below",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"

                [workflows.core.views.default.engine]
                type = "picker"

                [workflows.core.views.default.commands.run]
                key = "enter"
                label = "Run"
                type = "run"
                producer = "script"
                handler = {handler}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate run handler source");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "accepted handler {handler}: {stderr}"
        );
        assert!(stderr.contains(expected), "stderr: {stderr}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_rejects_unknown_run_command_args() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        producer = "script"
        handler = { script = "printf ok" }
        args = ["literal"]
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate unknown run arguments");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "accepted unknown args: {stderr}");
    assert!(stderr.contains("unknown field `args`"), "stderr: {stderr}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_validates_static_return_handler_targets() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = []
        [workflows.core.views.default.commands.done]
        key = "enter"
        label = "Done"
        type = "return"
        producer = "script"
        handler = { file = "scripts/missing.sh" }
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate return handler target");
    assert!(!output.status.success(), "accepted missing return handler");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not read script"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_validates_items_shape_before_runtime() {
    for source in [
        r#"items = [{ display = "{{ page.input }}" }]"#,
        r#"items = [{ display = "{{ literal.value }}", value = "{{" }]"#,
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                {source}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate items shape");
        assert!(
            output.status.success(),
            "rejected items {source:?}; stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    for source in [
        "items = 42",
        r#"items = "static""#,
        r#"items = "prefix {{ page.items }}""#,
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config]
                {source}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not reject invalid items shape");
        assert!(
            !output.status.success(),
            "accepted items {source:?}; stderr: {:?}",
            output.stderr
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("items must be an array or a producer object"),
            "items {source:?} reported unexpected stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_rejects_unknown_producer_fields() {
    for (engine, source) in [
        (
            "picker",
            r#"
            [workflows.core.views.default.engine.config.items]
            producer = "script"
            extra = true
            [workflows.core.views.default.engine.config.items.handler]
            file = "scripts/items.sh"
            "#,
        ),
        (
            "capture",
            r#"
            [workflows.core.views.default.engine.config.output]
            producer = "declared"
            extra = true
            [workflows.core.views.default.engine.config.output.handler]
            output = "output"
            "#,
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "{engine}"
                [workflows.core.views.default.engine.config]
                {source}
                "#
            ),
        )
        .unwrap();
        let scripts = root.join("workflows/core/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("items.sh"),
            "printf '{\\\"version\\\":1,\\\"items\\\":[]}\\n'\n",
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate producer fields");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "accepted producer fields: {stderr}"
        );
        assert!(stderr.contains("unknown field"), "stderr: {stderr}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_accepts_template_looking_script_bodies_as_literal_handlers() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config.items]
        producer = "script"
        [workflows.core.views.default.engine.config.items.handler]
        script = "printf '{{ page.query.source }}\\n'"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate a literal script handler");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_invalid_items_producers() {
    for (source, expected) in [
        (
            "producer = \"script\"\nhandler = { file = \"scripts/missing.sh\" }",
            "could not read script",
        ),
        (
            "producer = \"command\"\nhandler = { file = \"scripts/items.sh\" }",
            "unknown variant `command`",
        ),
        (
            "producer = \"script\"\nhandler = { file = \"scripts/items.sh\", script = \"printf ok\" }",
            "must define exactly one of file or script",
        ),
        (
            "producer = \"script\"\nhandler = { file = \"scripts/items.sh\", unknown = true }",
            "unknown field `unknown`",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [workflows.core.views.default.engine]
                type = "picker"
                [workflows.core.views.default.engine.config.items]
                {source}
                "#
            ),
        )
        .unwrap();
        let scripts = root.join("workflows/core/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("items.sh"), "printf '[]\\n'\n").unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate rejected items source");
        assert!(
            !output.status.success(),
            "accepted source {source:?}; stderr: {:?}",
            output.stderr
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "source {source:?} did not report {expected:?}; stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_loads_a_named_theme_from_the_config_directory() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "work"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(
        root.join("themes/work.toml"),
        "[palette]\nbrand = \"magenta\"\nquiet = \"gray\"\n\n[scheme]\nprimary = \"palette:brand\"\non-surface-variant = \"palette:quiet\"\n",
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

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
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
fn cli_theme_replaces_the_root_configuration() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "missing-root-theme"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
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
fn missing_theme_is_reported_during_startup() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "work"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate missing theme");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("could not resolve theme"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("themes/work.toml"), "stderr: {stderr}");
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

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
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

#[test]
fn single_file_workflow_accepts_one_line_script_body_that_looks_like_a_filename() {
    let root = temporary_root();
    let config = root.join("config.toml");
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(&config, "default_view = \"demo:main\"\n").unwrap();
    std::fs::write(
        root.join("workflows/demo.toml"),
        r#"
        [workflow]
        api = 1
        name = "demo"
        [views.main.engine]
        type = "picker"
        [views.main.commands.run]
        type = "run"
        producer = "script"
        [views.main.commands.run.handler]
        script = "foo.sh"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_workflow_fixture_inspect_and_query_mapping() {
    let inspect = launcher_command()
        .args(["--config"])
        .arg(fixture_config())
        .args(["inspect", "echo"])
        .output()
        .expect("could not inspect echo workflow");

    assert!(
        inspect.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(json["view"], "echo:main");
    assert_eq!(json["alias"], "echo");
    assert_eq!(json["engine"], "capture");
    assert!(json["query"]["message"].is_object());
}
