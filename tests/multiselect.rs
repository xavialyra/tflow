mod support;

use serde_json::Value;
use std::{fs, io::Write};
use support::{
    fixture_config, spawn_launcher_with_args_and_env, temporary_root, wait_for_fresh_screen,
    wait_for_launcher_exit,
};

fn send(process: &mut support::LauncherProcess, bytes: &[u8]) {
    process.master.write_all(bytes).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn picker_multiselect_toggles_items_updates_preview_and_submits() {
    let root = temporary_root();
    let result = root.join("result.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture_config(),
        &["multiselect:main"],
        &[("MULTISELECT_RESULT", result.to_str().unwrap())],
    );

    // Wait for initial screen: unselected items and preview summary
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("[ ] ripgrep") && screen.contains("Selected (0 items)")
    });

    // Press Tab to toggle ripgrep
    send(&mut process, b"\t");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("[x] ripgrep") && screen.contains("Selected (1 items)")
    });

    // Move down to bat and press Tab to toggle bat
    send(&mut process, b"\x1b[B\t");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("[x] bat")
            && screen.contains("Selected (2 items)")
            && screen.contains("A cat clone with syntax highlighting")
    });

    // Press Tab again to toggle bat off - cursor should stay on bat
    send(&mut process, b"\t");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("[ ] bat")
            && screen.contains("Selected (1 items)")
            && screen.contains("A cat clone with syntax highlighting")
    });

    // Press Ctrl+A to select all
    send(&mut process, b"\x01");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Selected (5 items)")
    });

    // Press Ctrl+R to clear all
    send(&mut process, b"\x12");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Selected (0 items)")
    });

    // Toggle ripgrep again and submit with Enter
    send(&mut process, b"\t");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("[x] ripgrep") && screen.contains("Selected (1 items)")
    });

    send(&mut process, b"\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));

    assert!(result.exists(), "result file was not written");
    let content: Value = serde_json::from_slice(&fs::read(&result).unwrap()).unwrap();
    assert_eq!(content["selected"], serde_json::json!(["ripgrep"]));

    fs::remove_dir_all(root).unwrap();
}
