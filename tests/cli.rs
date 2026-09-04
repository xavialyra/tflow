mod support;

use std::{fs, process::Command};

use support::{binary_path, fixture_config, temporary_root, write_test_config};

fn launcher_command() -> Command {
    Command::new(binary_path())
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
        type = "call"
        [commands.bindings.commands.payload]
        target = "core:default"

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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

                [plugins.core.views.default]
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
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
fn check_rejects_values_that_reference_unavailable_evaluation_stages() {
    for (default_view, source, expected) in [
        (
            "{{ input.mode }}",
            "",
            "default_view is consumed during the bootstrap evaluation stage",
        ),
        (
            "core:default",
            r#"
            [plugins.core.views.default.query]
            type = "object"
            input_order = ["value"]
            value = { type = "string", default = "{{ page.input }}" }
            "#,
            "query schema is consumed during the bootstrap evaluation stage",
        ),
        (
            "core:default",
            r#"
            layout = "{{ result }}"
            "#,
            "engine field \"layout\" is consumed during the operation evaluation stage",
        ),
        (
            "core:default",
            r#"
            [plugins.core.views.default.commands.run]
            key = "enter"
            label = "Run"
            type = "run"
            [plugins.core.views.default.commands.run.payload]
            handler = { source = "script", file = "scripts/run.sh" }
            args = ["{{ result }}"]
            "#,
            "command \"run\" args is consumed during the operation evaluation stage",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "{default_view}"
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
                items = []
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stderr: {stderr}");
        assert!(stderr.contains(expected), "stderr: {stderr}");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_rejects_static_picker_conflicts_with_dynamic_defaults() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        keymaps = { back = "escape" }

        [defaults.picker.bindings]
        exit = ["enter"]
        back = ["{{ view.input }}"]

        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
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
            .contains("picker key \"enter\" is assigned to both \"activate\" and \"exit\""),
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
        keymaps = { extra = "enter" }

        [defaults.capture.bindings]
        back = ["enter", "{{ view.input }}"]

        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
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
            .contains("capture key \"enter\" is assigned to both \"copy\" and \"back\""),
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
        type = "capture"
        [plugins.core.views.default.engine.config]
        output = "{{ view.query.message }}"
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
fn check_rejects_picker_runtime_field_shape_mismatches() {
    for (field, expected) in [
        (
            "layout = 42",
            "picker field \"layout\" must be a table or complete dynamic path",
        ),
        (
            "preview = \"not-a-table\"",
            "picker field \"preview\" must be a table or complete dynamic path",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
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
fn check_rejects_unsupported_dynamic_syntax_and_namespaces() {
    for expression in [
        "{{ page.value == 1 }}",
        "{{ {\"value\": page.value} }}",
        "{{ unknown.value }}",
        "{{ page.typo }}",
        "{{ view.typo }}",
        "{{ session.typo }}",
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [plugins.core.views.default.engine]
                type = "capture"
                [plugins.core.views.default.engine.config]
                output = {expression:?}
                "#
            ),
        )
        .unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate rejected dynamic syntax");
        assert!(
            !output.status.success(),
            "accepted {expression}; stderr: {:?}",
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
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config.items]
        source = "script"
        file = "scripts/items.sh"
        args = ["{{ view.query }}"]

        [plugins.core.views.capture.engine]
        type = "capture"
        [plugins.core.views.capture.engine.config.output]
        source = "script"
        file = "scripts/output.sh"
        args = ["{{ view.query }}"]
        "#,
    )
    .unwrap();
    let items_marker = root.join("items-script-ran");
    let capture_marker = root.join("capture-script-ran");
    let scripts = root.join("plugins/core/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("items.sh"),
        format!("printf ran > {:?}\nprintf '[]\\n'\n", items_marker),
    )
    .unwrap();
    std::fs::write(
        scripts.join("output.sh"),
        format!(
            "printf ran > {:?}\nprintf '\"output\"\\n'\n",
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

        [plugins.core.views.default.engine]
        type = "picker"

        [plugins.core.views.default.commands.run]
        key = "enter"
        label = "Run"
        type = "run"

        [plugins.core.views.default.commands.run.payload]
        handler = { source = "script", file = "scripts/run.sh" }
        "#,
    )
    .unwrap();
    let scripts = root.join("plugins/core/scripts");
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
fn check_rejects_invalid_run_handler_sources() {
    for (handler, expected) in [
        (
            r#"{ source = "script", file = "scripts/run.sh", max_output_bytes = 1024 }"#,
            "cannot define max_output_bytes",
        ),
        (
            r#"{ source = "script", file = "../run.sh" }"#,
            "invalid command handler file",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"

                [plugins.core.views.default.engine]
                type = "picker"

                [plugins.core.views.default.commands.run]
                key = "enter"
                label = "Run"
                type = "run"

                [plugins.core.views.default.commands.run.payload]
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
fn check_rejects_invalid_run_command_args() {
    for (args, expected) in [
        (
            r#""not-an-array""#,
            "command args must be an array or complete dynamic path",
        ),
        (
            r#"{ value = 1 }"#,
            "command args must be an array or complete dynamic path",
        ),
        (
            r#""prefix {{ view.query.value }}""#,
            "command args must be an array or complete dynamic path",
        ),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"

                [plugins.core.views.default.engine]
                type = "picker"

                [plugins.core.views.default.commands.run]
                key = "enter"
                label = "Run"
                type = "run"

                [plugins.core.views.default.commands.run.payload]
                handler = {{ source = "script", file = "scripts/run.sh" }}
                args = {args}
                "#
            ),
        )
        .unwrap();
        let scripts = root.join("plugins/core/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("run.sh"), ":\n").unwrap();

        let output = launcher_command()
            .args(["--check", "--config"])
            .arg(&config)
            .output()
            .expect("could not validate run command arguments");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted args {args}: {stderr}");
        assert!(stderr.contains(expected), "stderr: {stderr}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_validates_static_return_handler_targets() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config]
        items = []
        [plugins.core.views.default.commands.done]
        key = "enter"
        label = "Done"
        type = "return"
        [plugins.core.views.default.commands.done.payload]
        handler = "scripts/missing.sh"
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
        String::from_utf8_lossy(&output.stderr).contains("invalid result handler target"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_validates_items_shape_before_runtime() {
    for source in [
        r#"items = [{ display = "{{ page.input }}" }]"#,
        r#"items = "{{ page.items }}""#,
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "core:default"
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
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
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
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
                .contains("items must be an array, complete dynamic path, or script source object"),
            "items {source:?} reported unexpected stderr: {:?}",
            output.stderr
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_accepts_dynamic_script_source_fields() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [plugins.core.views.default.engine]
        type = "picker"
        [plugins.core.views.default.engine.config.items]
        source = "{{ page.query.source }}"
        file = "{{ page.query.file }}"
        max_output_bytes = "{{ page.query.limit }}"
        args = ["{{ page.query }}"]
        "#,
    )
    .unwrap();
    let scripts = root.join("plugins/core/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("items.sh"), "printf '[]\\n'\n").unwrap();

    let output = launcher_command()
        .args(["--check", "--config"])
        .arg(&config)
        .output()
        .expect("could not validate dynamic items source");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_invalid_items_sources() {
    for (source, expected) in [
        (
            "source = \"script\"\nfile = \"scripts/missing.sh\"",
            "could not read script",
        ),
        (
            "source = \"command\"\nfile = \"scripts/items.sh\"",
            "unsupported script source",
        ),
        (
            "source = \"script\"\nfile = \"scripts/items.sh\"\nmax_output_bytes = \"limit={{ page.query.limit }}\"",
            "script max_output_bytes must be an integer or complete dynamic path",
        ),
        (
            "source = \"script\"\nfile = \"scripts/items.sh\"\nunknown = true",
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
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config.items]
                {source}
                "#
            ),
        )
        .unwrap();
        let scripts = root.join("plugins/core/scripts");
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

        [plugins.core.views.default]
        [plugins.core.views.default.engine]
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
fn theme_errors_precede_dynamic_config_compilation_errors() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        theme = "work"
        default_view = "{{ page.input }}"

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
        .expect("could not validate configuration error ordering");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("could not resolve theme"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("themes/work.toml"), "stderr: {stderr}");
    assert!(
        !stderr.contains("default_view is consumed during the bootstrap evaluation stage"),
        "Theme errors must be reported before config compilation: {stderr}"
    );
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
