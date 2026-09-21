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
        [suite]
        api = 1
        name = "Invalid plugins"
        entrypoint = "unknown:main"

        [plugins.unknown.views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
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
        entrypoint = "main"

        [views.main.engine]
        type = "picker"

        [plugin]
        name = "Unknown"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(&manifest)
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
fn check_rejects_suite_command_registration() {
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
        .args(["--check", "--suite"])
        .arg(&config)
        .output()
        .expect("could not validate the session command override");
    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown field `commands`"),
        "stderr: {:?}",
        output.stderr
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_reserved_builtin_selectors_workflow() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let manifest = root.join("workflows/__selectors/workflow.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(&config, "default_view = \"core:default\"\n").unwrap();
    fs::write(
        &manifest,
        r#"
        [workflow]
        api = 1
        name = "User selectors"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(manifest.parent().unwrap())
        .output()
        .expect("could not validate the reserved workflow ID");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("workflow ID \"__selectors\" is reserved for built-in workflows; user workflow IDs cannot start with '__'"),
        "stderr: {stderr}"
    );

    // Also verify single-file workflows with __ prefix are rejected
    fs::remove_file(&manifest).unwrap();
    let single_file = root.join("workflows/__custom.toml");
    fs::write(
        &single_file,
        r#"
        [workflow]
        api = 1
        name = "User custom"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    let output2 = launcher_command()
        .args(["--check", "--workflow"])
        .arg(&single_file)
        .output()
        .expect("could not validate single-file reserved workflow ID");
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    assert!(!output2.status.success(), "stderr: {stderr2}");
    assert!(
        stderr2.contains("workflow ID \"__custom\" is reserved for built-in workflows; user workflow IDs cannot start with '__'"),
        "stderr: {stderr2}"
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
            .args(["--check", "--suite"])
            .arg(&config)
            .arg("--settings")
            .arg(root.join("settings.toml"))
            .output()
            .expect("could not run tlaunch --check");

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
fn check_rejects_picker_binding_conflicts_with_defaults() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [defaults.picker.bindings]
        exit = ["up"]

        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(root.join("settings.toml"))
        .output()
        .expect("could not run tlaunch --check");

    assert!(!output.status.success(), "stderr: {:?}", output.stderr);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("picker key \"up\" is assigned to both"),
        "stderr: {:?}",
        output.stderr
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_capture_binding_conflicts_with_defaults() {
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
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(root.join("settings.toml"))
        .output()
        .expect("could not run tlaunch --check");

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
            .args(["-c", "rmdir \"$PWD\" && exec \"$@\"", "tlaunch"])
            .arg(binary_path())
            .args(["--check", "--suite"])
            .arg(fixture_config())
            .args(["--theme", theme])
            .output()
            .expect("could not run tlaunch from a missing current directory");

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
        output = "query output"
        [workflows.core.views.default.query]
        type = "object"
        message = { type = "string", default = "" }
        "#,
    )
    .unwrap();

    let positional = launcher_command()
        .arg("--suite")
        .arg(&config)
        .args(["core:default", "message"])
        .output()
        .unwrap();
    assert!(!positional.status.success());
    assert!(
        String::from_utf8_lossy(&positional.stderr).contains("does not accept positional argument")
    );

    let unknown = launcher_command()
        .arg("--suite")
        .arg(&config)
        .args(["core:default", "--unknown=value"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("does not declare query parameter"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_picker_preview_field_shape_mismatches() {
    for (field, expected) in [
        (
            "preview_ratio = \"not-a-number\"",
            "picker preview_ratio must be a number",
        ),
        (
            "preview_min_width = -1",
            "picker preview_min_width must be an unsigned 16-bit integer",
        ),
        (
            "preview_default_open = \"yes\"",
            "picker preview_default_open must be a boolean",
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
            .args(["--check", "--suite"])
            .arg(&config)
            .output()
            .expect("could not validate picker preview field shape");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted {field}: {stderr}");
        assert!(stderr.contains(expected), "stderr: {stderr}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn check_validates_static_producer_sources_without_running_them() {
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
        [workflows.core.views.default.engine.config.preview]
        producer = "script"
        handler = { file = "scripts/preview.sh" }

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
    let preview_marker = root.join("preview-script-ran");
    let scripts = root.join("workflows/core/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(
        scripts.join("preview.sh"),
        format!("#!/bin/sh\nprintf ran > {:?}\n", preview_marker),
    )
    .unwrap();
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
        .args(["--check", "--suite"])
        .arg(&config)
        .output()
        .expect("could not validate static producer sources");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(!items_marker.exists(), "--check executed the items script");
    assert!(
        !preview_marker.exists(),
        "--check executed the preview script"
    );
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
    std::fs::write(scripts.join("run.sh"), "printf 'ok\\n'\\n").unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
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
        printf 'ok\\n'
        """
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
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
            .args(["--check", "--suite"])
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
        .args(["--check", "--suite"])
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
        .args(["--check", "--suite"])
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
        r#"items = [{ display = "Show date", value = "date" }]"#,
        r#"items = [{ display = "System information", metadata = {} }]"#,
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
            .args(["--check", "--suite"])
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

    for source in ["items = 42", r#"items = "static""#] {
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
            .args(["--check", "--suite"])
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
            .args(["--check", "--suite"])
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
fn check_accepts_inline_script_handlers() {
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
        script = "printf '[]\\n'"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
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
            .args(["--check", "--suite"])
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
    let settings = root.join("settings.toml");
    fs::write(&settings, r##"theme = "work""##).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(
        root.join("themes/work.toml"),
        "[scheme]\naccent = \"ansi:magenta\"\nmuted = \"ansi:gray\"\n",
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(&settings)
        .output()
        .expect("could not validate a named theme");

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_invalid_theme_colors_and_references() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let settings = root.join("settings.toml");
    fs::write(&settings, r##"theme = "work""##).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    for (source, expected) in [
        ("[scheme]\nunused = \"scheme:accent\"\n", "scheme.unused"),
        (
            "[picker.badge.selected]\nbackground = \"#bad\"\n",
            "picker.badge.selected.background has invalid color",
        ),
    ] {
        std::fs::write(root.join("themes/work.toml"), source).unwrap();
        let output = launcher_command()
            .args(["--check", "--suite"])
            .arg(&config)
            .arg("--settings")
            .arg(&settings)
            .output()
            .expect("could not validate theme");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "accepted theme: {source}");
        assert!(stderr.contains(expected), "{stderr}");
        assert!(stderr.contains("themes/work.toml"), "{stderr}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_rejects_an_inline_theme_value() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let settings = root.join("settings.toml");
    fs::write(
        &settings,
        r##"        [theme]
        base = { foreground = "terminal", background = "#102030" }
        accent = { foreground = "cyan" }
        highlight = { foreground = "yellow", background = "blue", bold = true }

"##,
    )
    .unwrap();
    write_test_config(
        &config,
        r##"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "##,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(&settings)
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
    let settings = root.join("settings.toml");
    fs::write(&settings, r##"theme = "missing-root-theme""##).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(&settings)
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
    let settings = root.join("settings.toml");
    fs::write(&settings, r##"theme = "work""##).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(&settings)
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
    let settings = root.join("settings.toml");
    fs::write(&settings, r##"theme = "work""##).unwrap();
    write_test_config(
        &config,
        r#"
        default_view = "core:default"

        [workflows.core.views.default]
        [workflows.core.views.default.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&config)
        .arg("--settings")
        .arg(&settings)
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
        entrypoint = "main"
        [views.main.engine]
        type = "picker"
        [commands.run]
        type = "run"
        producer = "script"
        [commands.run.handler]
        script = "foo.sh"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(root.join("workflows/demo.toml"))
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_workflow_fixture_inspect_and_query_mapping() {
    let inspect = launcher_command()
        .args(["--suite"])
        .arg(fixture_config())
        .args(["--inspect", "echo"])
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

#[test]
fn fixture_inspect_all_returns_sorted_view_contracts() {
    let inspect = launcher_command()
        .args(["--suite"])
        .arg(fixture_config())
        .args(["--inspect", "--all"])
        .output()
        .expect("could not inspect all fixture views");

    assert!(
        inspect.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let views = json["views"].as_array().expect("views must be an array");
    let view_refs = views
        .iter()
        .map(|view| view["view"].as_str().expect("view ref must be a string"))
        .collect::<Vec<_>>();
    let mut sorted = view_refs.clone();
    sorted.sort_unstable();

    assert!(!views.is_empty());
    assert_eq!(view_refs, sorted);
    assert!(view_refs.contains(&"apps:main"));
    assert!(view_refs.contains(&"sys:main"));
    assert!(view_refs.contains(&"sys:output"));
    assert!(view_refs.contains(&"echo:main"));

    let sys = views
        .iter()
        .find(|view| view["view"] == "sys:main")
        .expect("sys:main must be inspectable");
    assert_eq!(sys["alias"], "sys");
    assert_eq!(sys["engine"], "picker");
    assert_eq!(sys["commands"]["sys:run"]["label"], "Run");
    assert_eq!(sys["keymap"]["enter"], "sys:run");

    let output_view = views
        .iter()
        .find(|view| view["view"] == "sys:output")
        .expect("sys:output must be inspectable");
    assert_eq!(output_view["alias"], serde_json::Value::Null);
    assert_eq!(output_view["engine"], "capture");
}

#[test]
fn inspect_all_is_self_sufficient_and_rejects_view_arguments() {
    // `--all` on its own, `--inspect` without a View, and `--inspect --all` all dump every View.
    for args in [vec!["--all"], vec!["--inspect"], vec!["--inspect", "--all"]] {
        let output = launcher_command()
            .args(["--suite"])
            .arg(fixture_config())
            .args(&args)
            .output()
            .expect("could not run inspect-all command");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            json["views"]
                .as_array()
                .is_some_and(|views| !views.is_empty()),
            "{args:?} must dump at least one View"
        );
    }

    // A View reference cannot be combined with all-View inspection.
    let with_view = launcher_command()
        .args(["--suite"])
        .arg(fixture_config())
        .args(["--all", "echo"])
        .output()
        .expect("could not run invalid inspect command");
    assert!(!with_view.status.success());
    assert!(String::from_utf8_lossy(&with_view.stderr).contains("cannot be combined"));

    let with_check = launcher_command()
        .args(["--check", "--all", "--suite"])
        .arg(fixture_config())
        .output()
        .expect("could not run invalid check command");
    assert!(!with_check.status.success());
    assert!(String::from_utf8_lossy(&with_check.stderr).contains("inspection options"));
}

#[test]
fn positional_inspect_is_a_view_selector_not_a_subcommand() {
    // `tlaunch inspect --all` is no longer a valid headless mode; `inspect`
    // stays a plain positional View selector.
    let legacy = launcher_command()
        .args(["--suite"])
        .arg(fixture_config())
        .args(["inspect", "--all"])
        .output()
        .expect("could not run legacy inspect invocation");
    assert!(!legacy.status.success());
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("cannot be combined"));

    // Views literally named `inspect` and `items` remain addressable.
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    fs::write(
        &workflow,
        r#"
[workflow]
api = 1
name = "Tool"
entrypoint = "inspect"
[views.inspect.engine]
type = "picker"
[views.items.engine]
type = "picker"
"#,
    )
    .unwrap();
    for target in ["inspect", "items"] {
        let output = launcher_command()
            .args(["-w"])
            .arg(&workflow)
            .args(["--inspect", target])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "inspecting {target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["view"], format!("tool:{target}"));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovered_settings_resolve_themes_from_the_settings_directory() {
    let root = temporary_root();
    let settings_dir = root.join("xdg/tlaunch");
    fs::create_dir_all(settings_dir.join("themes")).unwrap();
    fs::write(settings_dir.join("settings.toml"), "theme = 'global'\n").unwrap();
    fs::write(
        settings_dir.join("themes/global.toml"),
        "[scheme]\naccent = 'ansi:blue'\n",
    )
    .unwrap();
    let workflow = root.join("tool.toml");
    fs::write(&workflow, "[workflow]\napi = 1\nname = 'Tool'\nentrypoint = 'main'\n[views.main.engine]\ntype = 'picker'\n").unwrap();
    let output = launcher_command()
        .env_remove("TLAUNCH_SETTINGS")
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .args(["--check", "--workflow"])
        .arg(&workflow)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(
        settings_dir.join("settings.toml"),
        "[suite]\nname = 'invalid settings'\n",
    )
    .unwrap();
    let output = launcher_command()
        .env_remove("TLAUNCH_SETTINGS")
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .args(["--check", "--workflow"])
        .arg(&workflow)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("purity"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manifest_dispatch_rejects_wrong_types_and_nested_suites() {
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    fs::write(&workflow, "[workflow]\napi = 1\nname = 'Tool'\nentrypoint = 'main'\n[views.main.engine]\ntype = 'picker'\n").unwrap();
    let suite = root.join("suite.toml");
    fs::write(&suite, "[suite]\napi = 1\nname = 'Suite'\nentrypoint = 'tool:main'\n[workflows]\ntool = { file = 'tool.toml' }\n").unwrap();
    for (flag, path, tip) in [
        ("--workflow", &suite, "use -s/--suite"),
        ("--suite", &workflow, "use -w/--workflow"),
    ] {
        let output = launcher_command()
            .args(["--check", flag])
            .arg(path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(tip));
    }
    let outer = root.join("outer.toml");
    fs::write(&outer, "[suite]\napi = 1\nname = 'Outer'\nentrypoint = 'nested:main'\n[workflows]\nnested = { file = 'suite.toml' }\n").unwrap();
    let output = launcher_command()
        .args(["--check", "--suite"])
        .arg(&outer)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("suites cannot be nested"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_launch_uses_only_manifest_members() {
    let root = temporary_root();
    let config_dir = root.join("tlaunch");
    fs::create_dir_all(config_dir.join("workflows")).unwrap();
    fs::write(config_dir.join("workflows/tool.toml"), "[workflow]\napi = 1\nname = 'Tool'\nentrypoint = 'main'\n[views.main.engine]\ntype = 'picker'\n").unwrap();
    // Invalid unmounted files must not affect an explicitly mounted session.
    fs::write(
        config_dir.join("workflows/unmounted.toml"),
        "not valid TOML!",
    )
    .unwrap();
    fs::write(config_dir.join("default.toml"), "[suite]\napi = 1\nname = 'Default'\nentrypoint = 'tool:main'\n[workflows]\ntool = { file = 'workflows/tool.toml' }\n").unwrap();
    let output = launcher_command()
        .env_remove("TLAUNCH_SUITE")
        .env_remove("TLAUNCH_SETTINGS")
        .env("XDG_CONFIG_HOME", &root)
        .args(["--inspect", "--all"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let catalog: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let views = catalog["views"].as_array().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0]["view"], "tool:main");
    assert_eq!(views[0]["alias"], "tool");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn suite_alias_cannot_redirect_a_member_shorthand() {
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    fs::write(&workflow, "[workflow]\napi=1\nname='Tool'\nentrypoint='main'\n[views.main.engine]\ntype='picker'\n[views.other.engine]\ntype='picker'\n").unwrap();
    let suite = root.join("suite.toml");
    fs::write(&suite, "[suite]\napi=1\nname='Suite'\nentrypoint='tool:main'\n[workflows]\ntool={file='tool.toml'}\n[aliases]\ntool='tool:other'\nz='tool'\n").unwrap();
    let output = launcher_command()
        .args(["--suite"])
        .arg(&suite)
        .args(["--inspect", "z"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicts with member shorthand"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn suites_reject_host_environment_fields() {
    let root = temporary_root();
    let suite = root.join("suite.toml");
    for field in [
        "theme = 'private'",
        "image_protocol = 'kitty'",
        "log_file = '/tmp/private.log'",
        "defaults = {}",
    ] {
        fs::write(
            &suite,
            format!(
                "{field}\n[suite]\napi = 1\nname = 'Invalid suite'\nentrypoint = 'tool:main'\n"
            ),
        )
        .unwrap();
        let output = launcher_command()
            .args(["--suite"])
            .arg(&suite)
            .arg("--check")
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {field}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unknown field"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn view_level_commands_are_rejected_with_adr_0006_diagnostic() {
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    fs::write(
        &workflow,
        r#"
[workflow]
api=1
name="Tool"
entrypoint="main"
[commands.accept]
label="Shared accept"
type="return"
producer="declared"
handler={value="shared"}
[views.main.engine]
type="picker"
[views.other.engine]
type="picker"
[views.other.commands.accept]
label="Local accept"
type="return"
producer="declared"
handler={value="local"}
"#,
    )
    .unwrap();
    let output = launcher_command()
        .args(["-w"])
        .arg(&workflow)
        .args(["--inspect", "--all"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ADR 0006 promotes business commands"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn settings_cannot_register_business_commands() {
    let root = temporary_root();
    let settings = root.join("settings.toml");
    fs::write(&settings, "[commands.bindings.open]\nkey='f1'\ntype='navigate'\nproducer='declared'\nhandler={target='external:main'}\n").unwrap();
    let output = launcher_command()
        .args(["-w"])
        .arg(support::quick_picker_fixture())
        .arg("--settings")
        .arg(&settings)
        .arg("--check")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown field `commands`"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_settings_environment_path_must_exist() {
    let root = temporary_root();
    let output = launcher_command()
        .env("TLAUNCH_SETTINGS", root.join("missing.toml"))
        .args(["--workflow"])
        .arg(support::quick_picker_fixture())
        .arg("--check")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not read settings file"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn atomic_workflows_reject_dependency_and_global_alias_declarations() {
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    let base =
        "[workflow]\napi=1\nname='Tool'\nentrypoint='main'\n[views.main.engine]\ntype='picker'\n";
    for extra in [
        "\n[dependencies]\nother='other.toml'\n",
        "\n[imports]\nother='other.toml'\n",
        "\n[workflows]\nother={file='other.toml'}\n",
        "\n[aliases]\nglobal='tool:main'\n",
    ] {
        fs::write(&workflow, format!("{base}{extra}")).unwrap();
        let output = launcher_command()
            .args(["--check", "--workflow"])
            .arg(&workflow)
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {extra}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported fields"));
    }
    fs::write(&workflow, format!("{base}\n[views.main]\nalias='global'\n")).unwrap();
    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(&workflow)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot declare alias"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn standalone_workflow_defers_sibling_navigation_until_runtime() {
    let root = temporary_root();
    let workflow = root.join("tool.toml");
    for kind in ["navigate", "call"] {
        fs::write(&workflow, format!("[workflow]\napi=1\nname='Tool'\nentrypoint='main'\n[views.main.engine]\ntype='picker'\n[commands.open]\ntype='{kind}'\nproducer='declared'\nhandler={{target='sibling:main'}}\n")).unwrap();
        let output = launcher_command()
            .args(["--workflow"])
            .arg(&workflow)
            .arg("--check")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn items_query_mode_outputs_valid_json_array_exit_0() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "core:default"
        [workflows.core.views.default.engine]
        type = "picker"
        [workflows.core.views.default.engine.config]
        items = [
            { display = "Item 1", value = "val1" },
            { display = "Item 2", value = "val2" }
        ]
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--suite"])
        .arg(&config)
        .args(["--items", "core:default"])
        .output()
        .expect("could not run items query");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let items: serde_json::Value = serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(items.is_array());
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[0]["display"], "Item 1");
    assert_eq!(items[1]["value"], "val2");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn items_query_mode_missing_view_exit_2() {
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
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--suite"])
        .arg(&config)
        .args(["--items", "core:missing"])
        .output()
        .expect("could not run items query");
    assert_eq!(output.status.code(), Some(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn items_query_mode_validation_failure_exit_1() {
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

        [workflows.core.views.cap.engine]
        type = "capture"
        [workflows.core.views.cap.engine.config]
        output = "hello"
        "#,
    )
    .unwrap();

    // Non-picker view -> exit code 1
    let output = launcher_command()
        .args(["--suite"])
        .arg(&config)
        .args(["--items", "core:cap"])
        .output()
        .expect("could not run items query");
    assert_eq!(output.status.code(), Some(1));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn items_query_mode_producer_script_failure_exit_1() {
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
        script = "echo 'not-json-array'; exit 1"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--suite"])
        .arg(&config)
        .args(["--items", "core:default"])
        .output()
        .expect("could not run items query");
    assert_eq!(output.status.code(), Some(1));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn items_query_mode_accepts_plain_and_object_positional_queries() {
    let suite = std::path::PathBuf::from("tests/fixtures/config/default.toml");

    // 1. Plain string query (calculator:main "2+2")
    let output = launcher_command()
        .args(["--suite"])
        .arg(&suite)
        .args(["--items", "calculator:main", "2+2"])
        .output()
        .expect("could not run calculator items query");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let items: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON array");
    assert!(items.is_array());
    assert_eq!(items[0]["value"], "4");

    // 2. Object query (apps:main "network")
    let output = launcher_command()
        .args(["--suite"])
        .arg(&suite)
        .args(["--items", "apps:main", "network"])
        .output()
        .expect("could not run apps items query");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let items: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON array");
    assert!(items.is_array());
    assert!(stdout.contains("Advanced Network Configuration"));

    // 3. Aggregation query (core:default "2+2")
    let output = launcher_command()
        .args(["--suite"])
        .arg(&suite)
        .args(["--items", "core:default", "2+2"])
        .output()
        .expect("could not run core items query with calculation");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let items: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON array");
    assert!(items.is_array());
    assert_eq!(items[0]["value"], "4");
    assert_eq!(items[0]["bindings"]["enter"], "calculator:copy");
}

