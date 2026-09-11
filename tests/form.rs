mod support;

use serde_json::{Value, json};
use std::io::{Read, Write};
use support::{
    fixture_config, spawn_launcher_with_args, spawn_launcher_with_args_and_redirected_stdout,
    wait_for_fresh_screen, wait_for_launcher_exit, wait_for_text,
};

fn send(process: &mut support::LauncherProcess, bytes: &[u8]) {
    process.master.write_all(bytes).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn route_defaults_are_rendered_and_submit_returns_typed_values() {
    let mut process =
        spawn_launcher_with_args_and_redirected_stdout(&fixture_config(), &["form:input"]);
    wait_for_text(&process.master, "name");
    send(&mut process, b"route-name\t\x15prod\t \r");
    let (status, _terminal) = wait_for_launcher_exit(&mut process);
    let mut stdout = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let output = stdout.into_bytes();
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        json!({"name":"route-name","environment":"prod","enabled":false})
    );
}

#[test]
fn redirected_stdout_contains_only_submitted_json() {
    let mut process =
        spawn_launcher_with_args_and_redirected_stdout(&fixture_config(), &["form:input"]);
    wait_for_text(&process.master, "name");
    send(&mut process, b"stdout-name\t\x15dev\t \r");
    let (status, terminal) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&terminal));
    let mut stdout = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(stdout.trim()).unwrap(),
        json!({"name":"stdout-name","environment":"dev","enabled":false})
    );
}

#[test]
fn initial_values_from_cli_are_used_by_the_native_form() {
    let mut process = spawn_launcher_with_args_and_redirected_stdout(
        &fixture_config(),
        &[
            "form:input",
            "--name=initial",
            "--environment=prod",
            "--enabled=false",
        ],
    );
    wait_for_text(&process.master, "initial");
    send(&mut process, b"\r\r\r\r");
    let (status, _terminal) = wait_for_launcher_exit(&mut process);
    let mut stdout = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let output = stdout.into_bytes();
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        json!({"name":"initial","environment":"prod","enabled":false})
    );
}

#[test]
fn required_name_and_environment_validation_are_enforced() {
    let mut process =
        spawn_launcher_with_args_and_redirected_stdout(&fixture_config(), &["form:input"]);
    wait_for_text(&process.master, "name");
    send(&mut process, b"valid\t\x15prod\r");
    send(&mut process, b"\t\r\r");
    let (status, _terminal) = wait_for_launcher_exit(&mut process);
    let mut stdout = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let output = stdout.into_bytes();
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap(),
        json!({"name":"valid","environment":"prod","enabled":true})
    );
}

#[test]
fn escape_cancels_with_exit_code_one() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["form:input"]);
    wait_for_text(&process.master, "name");
    send(&mut process, b"\x1b");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 1);
}

#[test]
fn caller_receives_submitted_values_and_cancel_skips_return_processor() {
    let mut process = spawn_launcher_with_args(&fixture_config(), &["form:main"]);
    wait_for_text(&process.master, "Collect parameters");
    send(&mut process, b"\r");
    wait_for_text(&process.master, "name");
    send(&mut process, b"caller-name\t\x15prod\t \r");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Caller received:")
            && screen.contains("caller-name")
            && screen.contains("prod")
    });
    send(&mut process, b"\x1b");
    wait_for_text(&process.master, "Collect parameters");
    send(&mut process, b"\r\x1b");
    send(&mut process, b"\x03");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
}
