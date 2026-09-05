mod support;

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
};
use support::{
    binary_path, run_tty_invocation_with_redirected_stdout, spawn_launcher,
    temporary_root, wait_for_launcher_exit, wait_for_ready,
};

fn launcher_command() -> Command {
    Command::new(binary_path())
}

#[test]
fn single_file_workflow_loads_and_executes() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();

    fs::write(&config, "default_view = \"hello:main\"\n").unwrap();
    fs::write(
        workflows_dir.join("hello.toml"),
        r#"
        [workflow]
        api = 1
        name = "hello"

        [views.main]
        [views.main.engine]
        type = "capture"
        [views.main.engine.config]
        output = "Hello from single-file workflow!"

        [views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        "#,
    )
    .unwrap();

    let result = run_tty_invocation_with_redirected_stdout(
        &["--config", config.to_str().unwrap(), "hello:main"],
        b"\r",
    );

    assert_eq!(result.status, 0);
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "Hello from single-file workflow!\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_file_workflow_prohibits_relative_scripts() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();

    fs::write(&config, "default_view = \"invalid:main\"\n").unwrap();
    fs::write(
        workflows_dir.join("invalid.toml"),
        r#"
        [workflow]
        api = 1
        name = "invalid"

        [views.main]
        [views.main.engine]
        type = "picker"
        [views.main.commands.run]
        key = "enter"
        label = "Run"
        type = "run"
        [views.main.commands.run.payload]
        handler = "scripts/relative.sh"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config", config.to_str().unwrap()])
        .output()
        .expect("could not run check on config");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("single-file workflow \"invalid\"")
            && stderr.contains("cannot reference relative script file \"scripts/relative.sh\""),
        "unexpected stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_workflow_id_collision_rejected() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(workflows_dir.join("collision")).unwrap();

    fs::write(&config, "default_view = \"collision:main\"\n").unwrap();
    fs::write(
        workflows_dir.join("collision.toml"),
        r#"
        [workflow]
        api = 1
        name = "collision_file"
        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    fs::write(
        workflows_dir.join("collision/workflow.toml"),
        r#"
        [workflow]
        api = 1
        name = "collision_dir"
        [views.main.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config", config.to_str().unwrap()])
        .output()
        .expect("could not run check");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("duplicate workflow \"collision\" detected"),
        "unexpected stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_view_alias_collision_rejected() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflows_dir = root.join("workflows");
    fs::create_dir_all(&workflows_dir).unwrap();

    fs::write(&config, "default_view = \"w1:v1\"\n").unwrap();
    fs::write(
        workflows_dir.join("w1.toml"),
        r#"
        [workflow]
        api = 1
        name = "w1"
        [views.v1]
        alias = "myalias"
        [views.v1.engine]
        type = "picker"
        "#,
    )
    .unwrap();
    fs::write(
        workflows_dir.join("w2.toml"),
        r#"
        [workflow]
        api = 1
        name = "w2"
        [views.v2]
        alias = "myalias"
        [views.v2.engine]
        type = "picker"
        "#,
    )
    .unwrap();

    let output = launcher_command()
        .args(["--check", "--config", config.to_str().unwrap()])
        .output()
        .expect("could not run check");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("conflicting view alias \"myalias\""),
        "unexpected stderr: {stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inline_script_with_env_s_preserves_caller_pwd_and_sets_0600() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow_dir = root.join("workflows/tester");
    fs::create_dir_all(&workflow_dir).unwrap();

    let expected_dir = std::env::current_dir().unwrap().to_string_lossy().to_string();

    fs::write(&config, "default_view = \"tester:main\"\n").unwrap();
    fs::write(
        workflow_dir.join("workflow.toml"),
        format!(
            r#"
            [workflow]
            api = 1
            name = "tester"

            [views.main]
            [views.main.engine]
            type = "picker"
            [views.main.engine.config]
            items = [{{ display = "Run inline script", value = "hello_inline" }}]

            [views.main.commands.accept]
            key = "enter"
            label = "Accept"
            type = "run"
            [views.main.commands.accept.payload]
            args = ["{expected_dir}", "{{{{ selection.value }}}}"]
            exit = true
            script = '''
#!/usr/bin/env -S sh -eu
expected_pwd="$1"
val="$2"
if [ "$PWD" != "$expected_pwd" ]; then
    printf 'PWD mismatch: expected %s, got %s\n' "$expected_pwd" "$PWD" >&2
    exit 1
fi
printf 'INLINE_SUCCESS:%s' "$val"
'''
            "#
        ),
    )
    .unwrap();

    let mut process = spawn_launcher(&config);
    wait_for_ready(&process.master);
    process.master.write_all(b"\r").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);

    assert_eq!(status, 0, "output: {:?}", output);
    let output_text = String::from_utf8_lossy(&output);
    assert!(
        output_text.contains("INLINE_SUCCESS:hello_inline"),
        "output did not contain expected text: {output_text}"
    );

    // Verify materialized script permissions and attribution comment
    let cache_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_CACHE_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| Path::new("/tmp").to_path_buf())
        })
        .join("tui-launcher/scripts");

    if cache_dir.is_dir() {
        let mut found_attributed = false;
        for entry in fs::read_dir(&cache_dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_file() {
                let meta = fs::metadata(&path).unwrap();
                let perm = meta.permissions().mode() & 0o777;
                assert_eq!(perm, 0o600, "script file {:?} does not have 0600 permissions", path);

                let content = fs::read_to_string(&path).unwrap_or_default();
                if content.contains("# [tui-launcher] source: workflows/tester")
                    && content.contains("commands.accept]")
                {
                    found_attributed = true;
                }
            }
        }
        assert!(found_attributed, "did not find attributed materialized script in {:?}", cache_dir);
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_multiplexing_via_argv0_and_inspect() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let workflow_dir = root.join("workflows/tool");
    fs::create_dir_all(&workflow_dir).unwrap();

    fs::write(&config, "default_view = \"tool:main\"\n").unwrap();
    fs::write(
        workflow_dir.join("workflow.toml"),
        r#"
        [workflow]
        api = 1
        name = "tool"

        [views.main]
        alias = "mytool"

        [views.main.query]
        type = "object"
        name = { type = "string", default = "World" }

        [views.main.engine]
        type = "capture"
        [views.main.engine.config]
        output = "Hello {{ view.query.name }}!"

        [views.main.commands.accept]
        key = "enter"
        label = "Accept"
        type = "return"
        "#,
    )
    .unwrap();

    // 1. Test inspect <view>
    let inspect_output = launcher_command()
        .args(["--config", config.to_str().unwrap(), "inspect", "mytool"])
        .output()
        .expect("could not run inspect");

    assert!(inspect_output.status.success(), "stderr: {:?}", String::from_utf8_lossy(&inspect_output.stderr));
    let inspect_json: serde_json::Value = serde_json::from_slice(&inspect_output.stdout).unwrap();
    assert_eq!(inspect_json["view"], "tool:main");
    assert_eq!(inspect_json["alias"], "mytool");
    assert!(inspect_json["query"]["name"].is_object());

    // 2. Test inspect via --inspect flag
    let inspect_flag_output = launcher_command()
        .args(["--config", config.to_str().unwrap(), "--inspect", "mytool"])
        .output()
        .expect("could not run --inspect");
    assert!(inspect_flag_output.status.success());

    // 3. Test argv[0] multiplexing via symlink
    let symlink_path = root.join("mytool");
    std::os::unix::fs::symlink(binary_path(), &symlink_path).unwrap();

    // Test --inspect via symlink
    let mux_inspect = Command::new(&symlink_path)
        .args(["--config", config.to_str().unwrap(), "--inspect", "mytool"])
        .output()
        .expect("could not run symlinked inspect");
    assert!(mux_inspect.status.success(), "stderr: {:?}", String::from_utf8_lossy(&mux_inspect.stderr));

    // Test --check via symlink
    let mux_check = Command::new(&symlink_path)
        .args(["--config", config.to_str().unwrap(), "--check"])
        .output()
        .expect("could not run symlinked check");
    assert!(mux_check.status.success(), "stderr: {:?}", String::from_utf8_lossy(&mux_check.stderr));

    fs::remove_dir_all(root).unwrap();
}
