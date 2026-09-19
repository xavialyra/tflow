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

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "-w"])
        .arg(&workflow_file)
        .output()
        .expect("could not run tlaunch --check -w");
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

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--workflow"])
        .arg(&package_dir)
        .output()
        .expect("could not run tlaunch --check --workflow");
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

        [views.main]
        alias = "my-tool"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["-w"])
        .arg(&workflow_file)
        .args(["--inspect", "my-tool"])
        .output()
        .expect("could not inspect view");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "stderr: {stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json output");
    assert_eq!(parsed["view"], "tool:main");
    assert_eq!(parsed["alias"], "my-tool");
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

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // Point TLAUNCH_CONFIG to a non-existent path
    let nonexistent_config = root.join("does_not_exist/config.toml");
    let output = launcher_command()
        .env("TLAUNCH_CONFIG", &nonexistent_config)
        .args(["--check", "-w"])
        .arg(&workflow_file)
        .output()
        .expect("could not run tlaunch without config");
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
fn global_launch_resolves_alias_main() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();

    // Notice: config.toml does NOT declare default_view!
    fs::write(&config, "# empty config with no default_view\n").unwrap();

    let tool_workflow = workflows_dir.join("main_tool.toml");
    fs::write(
        &tool_workflow,
        r#"
        [workflow]
        api = 1
        name = "Main Tool"

        [views.entry]
        alias = "main"

        [views.entry.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // inspect "main" should resolve to "main_tool:entry"
    let output = launcher_command()
        .args(["--config"])
        .arg(&config)
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

        [views.main]
        alias = "main"

        [views.main.engine]
        type = "picker"

        [views.main.engine.config]
        items = [
            { display = "First Option", value = "first", metadata = {} }
        ]

        [views.main.commands.run]
        key = "enter"
        label = "Run"
        type = "return"
        producer = "script"
        [views.main.commands.run.handler]
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

        [views.custom]
        alias = "my-custom"

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
        stderr.contains("no default view with alias = \"main\" found in workflow"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("no_main:custom"), "stderr: {stderr}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_multi_workflow_directory_supports_multiple_workflows() {
    let root = temporary_root();
    let multi_dir = root.join("my_workflows");
    fs::create_dir_all(&multi_dir).unwrap();

    // 1. 单文件工作流 foo.toml (带有 alias = "main")
    let foo_file = multi_dir.join("foo.toml");
    fs::write(
        &foo_file,
        r#"
        [workflow]
        api = 1
        name = "Foo Workflow"

        [views.index]
        alias = "main"

        [views.index.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // 2. 目录包工作流 bar/workflow.toml (带有 alias = "bar")
    let bar_dir = multi_dir.join("bar");
    fs::create_dir_all(&bar_dir).unwrap();
    let bar_file = bar_dir.join("workflow.toml");
    fs::write(
        &bar_file,
        r#"
        [workflow]
        api = 1
        name = "Bar Workflow"

        [views.main]
        alias = "bar"

        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    // 测试 1: --check 整个多工作流目录有效
    let output = launcher_command()
        .args(["--check", "-w"])
        .arg(&multi_dir)
        .output()
        .expect("could not run --check");
    assert!(output.status.success());

    // 测试 2: 不传 view 时，自动命中带有 alias = "main" 的 foo:index
    let output = launcher_command()
        .args(["-w"])
        .arg(&multi_dir)
        .args(["--inspect", "main"])
        .output()
        .expect("could not run inspect main");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["view"], "foo:index");

    // 测试 3: 显式指定 bar 视图，命中 bar:main
    let output = launcher_command()
        .args(["-w"])
        .arg(&multi_dir)
        .args(["--inspect", "bar"])
        .output()
        .expect("could not run inspect bar");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["view"], "bar:main");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn shebang_workflow_fixture_invoked_directly_as_executable_script() {
    let fixture = quick_picker_fixture();
    let bin_dir = binary_path().parent().unwrap().to_path_buf();
    let original_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.display(), original_path);

    // 直接执行赋予了执行权限的 quick-picker.toml，验证内核 Shebang (#!/usr/bin/env -S tlaunch -w) 正常分发
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

    // 通过直接执行脚本传递 --inspect main 验证别名解析与契约结构
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
    assert_eq!(parsed["alias"], "main");
    assert_eq!(parsed["engine"], "picker");
    assert_eq!(
        parsed["query"]["prompt"]["default"],
        "Select an action:"
    );
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

