mod support;

use std::{fs, io::Write, path::PathBuf, process::Command};
use support::{
    binary_path, spawn_launcher_with_args, temporary_root, wait_for_fresh_screen,
    wait_for_launcher_exit,
};

fn example_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/companion-matrix")
}

fn start_example(name: &str) -> (PathBuf, support::LauncherProcess) {
    let root = temporary_root();
    let workflow = example_root().join(name);
    let suite = root.join("default.toml");
    fs::write(&suite, format!(
        "[suite]\napi=1\nname='Companion Example Test'\nentrypoint='demo:main'\n[workflows]\ndemo={{file={:?}}}\n",
        workflow.to_str().unwrap(),
    )).unwrap();
    fs::write(
        root.join("settings.toml"),
        format!(
            "image_protocol = 'halfblocks'\nlog_file = {:?}\n",
            root.join("runtime.log").to_str().unwrap()
        ),
    )
    .unwrap();
    let process = spawn_launcher_with_args(&suite, &[]);
    (root, process)
}

fn send(process: &mut support::LauncherProcess, keys: &[u8]) {
    process.master.write_all(keys).unwrap();
    process.master.flush().unwrap();
}

fn finish(root: PathBuf, mut process: support::LauncherProcess) {
    send(&mut process, b"\x04");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn slow_picker_companion_does_not_block_foreground_picker() {
    let root = temporary_root();
    let workflow = root.join("workflow.toml");
    fs::write(
        &workflow,
        r#"
[workflow]
api = 1
name = "Scheduling regression"
entrypoint = "main"
[views.main]
engine = "picker"
companion = "slow"
[views.main.picker.items]
script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
query = request["context"]["engine"]["state"]["input"]
print(json.dumps({"version": 1, "items": [{"display": "RESULT " + query, "value": query}]}))
'''
[views.slow]
engine = "picker"
[views.slow.picker.items]
script = '''#!/usr/bin/env python3
import json, sys, time
json.load(sys.stdin)
time.sleep(30)
print(json.dumps({"version": 1, "items": []}))
'''
"#,
    )
    .unwrap();
    let suite = root.join("default.toml");
    fs::write(&suite, "[suite]\napi=1\nname='Scheduling'\nentrypoint='demo:main'\n[workflows]\ndemo={file='workflow.toml'}\n").unwrap();
    let mut process = spawn_launcher_with_args(&suite, &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("RESULT"));
    send(&mut process, b"updated");
    wait_for_fresh_screen(&process.master, |screen| screen.contains("RESULT updated"));
    finish(root, process);
}

#[test]
fn standalone_companion_examples_validate() {
    let mut files = fs::read_dir(example_root())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(
        files.len(),
        8,
        "keep every combination under configuration validation"
    );
    for file in files {
        let result = Command::new(binary_path())
            .args(["--check", "--workflow"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}: {}",
            file.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn form_capture_example_shows_initialized_fields_and_live_draft_updates() {
    let (root, mut process) = start_example("form_capture.toml");
    let screen = wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("LIVE FORM SUMMARY")
            && screen.contains("Name: demo")
            && screen.contains("Enabled: true")
    });
    let screen = String::from_utf8_lossy(&screen);
    // The field value must survive in the left pane, not be centered across
    // the complete viewport and then overwritten by the companion.
    assert!(
        screen
            .lines()
            .any(|line| line.contains("demo") && line.find("demo").unwrap() < 40),
        "{screen}"
    );
    send(&mut process, b"\x15new-name");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Name: new-name") && screen.contains("Dirty: true")
    });
    send(&mut process, b"\t\x1b[C");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Environment: staging")
    });
    send(&mut process, b"\t ");
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Enabled: false"));
    finish(root, process);
}

#[test]
fn picker_picker_example_cascades_and_returns_one_level_at_a_time() {
    let (root, mut process) = start_example("picker_picker.toml");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Databases") && screen.contains("PostgreSQL")
    });
    send(&mut process, b"\x1b[B");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("RabbitMQ") && !screen.contains("PostgreSQL")
    });
    send(&mut process, b"\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("RabbitMQ") && !screen.contains("Databases")
    });
    send(&mut process, b"\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Single Node") && screen.contains("Cluster")
    });
    send(&mut process, b"\x1b[B\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("CONFIRMED")
            && screen.contains("Service: rabbitmq")
            && screen.contains("Variant: Cluster")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Single Node") && !screen.contains("CONFIRMED")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("RabbitMQ") && !screen.contains("Single Node")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Databases") && screen.contains("RabbitMQ")
    });
    finish(root, process);
}

#[test]
fn picker_form_example_initializes_foreground_editor_from_selected_snapshot() {
    let (root, mut process) = start_example("picker_form.toml");
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Build release"));
    send(&mut process, b"\x1b[B\x1be");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Run tests") && screen.contains("Retries")
    });
    send(&mut process, b"\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Retries") && screen.contains("tests") && !screen.contains("Build release")
    });
    send(&mut process, b"\x15edited-job\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(String::from_utf8_lossy(&output).contains("\"job\":\"edited-job\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn capture_picker_example_opens_selected_action_and_returns_to_report() {
    let (root, mut process) = start_example("capture_picker.toml");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("DEPLOYMENT REPORT") && screen.contains("View deployment logs")
    });
    send(&mut process, b"\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("View deployment logs") && !screen.contains("DEPLOYMENT REPORT")
    });
    send(&mut process, b"\x1b[B\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("SELECTED ACTION: config")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Inspect configuration") && !screen.contains("SELECTED ACTION:")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("DEPLOYMENT REPORT")
    });
    send(&mut process, b"\x1b");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cascading_picker_enter_shortcut_preserves_category_context() {
    let (root, mut process) = start_example("picker_picker.toml");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Databases") && screen.contains("PostgreSQL")
    });
    send(&mut process, b"\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("PostgreSQL") && !screen.contains("Databases")
    });
    send(&mut process, b"\x1b[B\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("In-memory") && screen.contains("Persistent")
    });
    send(&mut process, b"\x1b[B\r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Service: sqlite") && screen.contains("Variant: Persistent")
    });
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| screen.contains("In-memory"));
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| screen.contains("PostgreSQL"));
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Databases") && screen.contains("PostgreSQL")
    });
    finish(root, process);
}
