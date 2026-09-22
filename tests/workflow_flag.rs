mod support;

use std::{fs, process::Command};
use support::{binary_path, quick_picker_fixture, temporary_root};

fn launcher_command() -> Command {
    Command::new(binary_path())
}

#[test]
fn check_validates_isolated_single_file_workflow() {
    let root = temporary_root();
    let workflow_file = root.join("calculator.toml");
    fs::write(
        &workflow_file,
        r#"
        [workflow]
        api = 1
        name = "Standalone Calc"
        entrypoint = "main"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "-w"])
        .arg(&workflow_file)
        .output()
        .expect("could not run tflow --check -w");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stdout.contains("configuration is valid:"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&workflow_file.display().to_string()),
        "stdout: {stdout}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn check_validates_isolated_workflow_directory() {
    let root = temporary_root();
    let package_dir = root.join("standalone_pack");
    fs::create_dir_all(&package_dir).unwrap();
    let manifest = package_dir.join("workflow.toml");
    fs::write(
        &manifest,
        r#"
        [workflow]
        api = 1
        name = "Directory Workflow"
        entrypoint = "main"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(&package_dir)
        .output()
        .expect("could not run tflow --check --workflow");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stdout.contains("configuration is valid:"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&package_dir.display().to_string()),
        "stdout: {stdout}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inspect_isolated_workflow_inspects_main_view() {
    let root = temporary_root();
    let workflow_file = root.join("tool.toml");
    fs::write(
        &workflow_file,
        r#"
        [workflow]
        api = 1
        name = "Standalone Tool"
        entrypoint = "main"

        [views.main]

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["-w"])
        .arg(&workflow_file)
        .args(["--inspect", "tool"])
        .output()
        .expect("could not inspect view");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json output");
    assert_eq!(parsed["view"], "tool:main");
    assert_eq!(parsed["alias"], "tool");
    assert_eq!(parsed["engine"], "picker");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_workflow_works_without_global_config() {
    let root = temporary_root();
    let workflow_file = root.join("demo.toml");
    fs::write(
        &workflow_file,
        r#"
        [workflow]
        api = 1
        name = "Demo"
        entrypoint = "main"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // Unset TFLOW_SETTINGS and point XDG_CONFIG_HOME to an empty dir
    let output = launcher_command()
        .env_remove("TFLOW_SETTINGS")
        .env_remove("TFLOW_SUITE")
        .env("XDG_CONFIG_HOME", root.join("empty_xdg"))
        .args(["--check", "-w"])
        .arg(&workflow_file)
        .output()
        .expect("could not run tflow without config");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "should succeed without global config. stderr: {stderr}"
    );
    assert!(stdout.contains("configuration is valid:"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn suite_inspect_resolves_alias_main() {
    let root = temporary_root();
    let suite = root.join("suite.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();

    fs::write(&suite, "[suite]\napi = 1\nname = \"Main\"\nentrypoint = \"main_tool:entry\"\n[workflows.main_tool]\nfile = \"./workflows/main_tool.toml\"\n[aliases]\nmain = \"main_tool:entry\"\n").unwrap();

    let tool_workflow = workflows_dir.join("main_tool.toml");
    fs::write(
        &tool_workflow,
        r#"
        [workflow]
        api = 1
        name = "Main Tool"
        entrypoint = "entry"

        [views.entry]

        [views.entry.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // inspect "main" should resolve to "main_tool:entry"
    let output = launcher_command()
        .args(["--suite"])
        .arg(&suite)
        .args(["--inspect", "main"])
        .output()
        .expect("could not inspect view");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json output");
    assert_eq!(parsed["view"], "main_tool:entry");
    assert_eq!(parsed["alias"], "main");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_workflow_runs_interactively_through_pty() {
    use support::run_tty_invocation_with_redirected_stdout_after_marker;

    let root = temporary_root();
    let workflow_file = root.join("hello.toml");
    fs::write(
        &workflow_file,
        r#"
        [workflow]
        api = 1
        name = "Hello Standalone"
        entrypoint = "main"

        [views.main]

        [views.main.engine]
        type = "picker"

        [views.main.engine.config]
        items = [
            { display = "First Option", value = "first", metadata = {} }
        ]

        [views.main.keymap]
        enter = "run"

        [commands.run]
        label = "Run"
        type = "return"
        producer = "script"
        [commands.run.handler]
        script = '''#!/bin/sh
        printf '{"version":1,"operation":{"type":"return","value":{"result":"ran_standalone"}}}\n'
        '''
        "#,
    )
    .unwrap();

    let result = run_tty_invocation_with_redirected_stdout_after_marker(
        &["-w", workflow_file.to_str().unwrap()],
        "First Option",
        b"\r",
    );
    assert_eq!(result.status, 0);
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("ran_standalone"), "stdout: {stdout}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_workflow_without_main_alias_fails_with_available_views_hint() {
    let root = temporary_root();
    let workflow_file = root.join("no_main.toml");
    fs::write(
        &workflow_file,
        r#"
        [workflow]
        api = 1
        name = "No Main"
        entrypoint = "main"

        [views.custom.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["-w"])
        .arg(&workflow_file)
        .output()
        .expect("could not run command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("entrypoint \"main\" does not match any declared view"),
        "stderr: {stderr}"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_multi_workflow_directory_requires_a_manifest() {
    let root = temporary_root();
    let multi_dir = root.join("my_workflows");
    fs::create_dir_all(&multi_dir).unwrap();

    // 1. Single-file workflow foo.toml (with entrypoint = "main")
    let foo_file = multi_dir.join("foo.toml");
    fs::write(
        &foo_file,
        r#"
        [workflow]
        api = 1
        name = "Foo Workflow"
        entrypoint = "main"

        [views.index]

        [views.index.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // 2. Directory package workflow bar/workflow.toml (with alias = "bar")
    let bar_dir = multi_dir.join("bar");
    fs::create_dir_all(&bar_dir).unwrap();
    let bar_file = bar_dir.join("workflow.toml");
    fs::write(
        &bar_file,
        r#"
        [workflow]
        api = 1
        name = "Bar Workflow"
        entrypoint = "main"

        [views.main]
        alias = "bar"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // Test 1: Verify --check rejects a multi-workflow directory without a manifest
    let output = launcher_command()
        .args(["--check", "-w"])
        .arg(&multi_dir)
        .output()
        .expect("could not run --check");
    assert!(!output.status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn shebang_workflow_fixture_invoked_directly_as_executable_script() {
    let fixture = quick_picker_fixture();
    let bin_dir = binary_path().parent().unwrap().to_path_buf();
    let original_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), original_path);

    // Directly execute quick-picker.toml with executable permissions to verify shebang dispatch (#!/usr/bin/env -S tflow -w)
    let output = Command::new(&fixture)
        .env("PATH", &new_path)
        .arg("--check")
        .output()
        .expect("could not directly execute workflow script via shebang");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "direct shebang execution failed; stderr: {stderr}"
    );
    assert!(
        stdout.contains("configuration is valid:"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&fixture.display().to_string()),
        "stdout: {stdout}"
    );
}

#[test]
fn shebang_workflow_fixture_supports_direct_inspection_and_query_schema() {
    let fixture = quick_picker_fixture();
    let bin_dir = binary_path().parent().unwrap().to_path_buf();
    let original_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), original_path);

    // Pass --inspect main directly to verify alias resolution and contract structure
    let output = Command::new(&fixture)
        .env("PATH", &new_path)
        .args(["--inspect", "main"])
        .output()
        .expect("could not inspect shebang workflow");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "direct inspect failed; stderr: {stderr}"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["view"], "quick-picker:main");
    assert_eq!(parsed["alias"], "quick-picker");
    assert_eq!(parsed["engine"], "picker");
    assert_eq!(parsed["query"]["prompt"]["default"], "Select an action:");
}

#[test]
fn shebang_workflow_fixture_runs_interactively_through_pty() {
    use support::run_tty_invocation_with_redirected_stdout_after_marker;

    let fixture = quick_picker_fixture();
    let result = run_tty_invocation_with_redirected_stdout_after_marker(
        &["-w", fixture.to_str().unwrap()],
        "First Option",
        b"\r",
    );
    assert_eq!(result.status, 0);
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "selected_option");
}

#[test]
fn streamed_workflow_uses_tty_input_and_returns_clean_stdout() {
    let source = br#"
[workflow]
api = 1
name = "Streamed picker"
entrypoint = "choose"
[views.choose.engine]
type = "picker"
[views.choose.engine.config]
items = [{ display = "Streamed choice", value = "chosen" }]
[views.choose.keymap]
enter = "accept"
[commands.accept]
type = "return"
producer = "script"
[commands.accept.handler]
script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
item = request["context"]["engine"]["state"]["item"]
json.dump({"version": 1, "operation": {"type": "return", "value": item["value"]}}, sys.stdout)
'''
"#;
    let result = support::run_invocation(&["--workflow", "-"], source, b"\r");
    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, b"chosen\n");
}
