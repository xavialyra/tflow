mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use support::{
    spawn_launcher_with_args_and_env, temporary_root, wait_for_fresh_screen,
    wait_for_launcher_exit, wait_for_process_exit, write_test_config,
};

fn started_previews(root: &Path) -> Vec<(i32, serde_json::Value)> {
    fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("preview-start-") {
                return None;
            }
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            Some((value["pid"].as_i64()? as i32, value))
        })
        .collect()
}

fn wait_for_preview_start(root: &Path, seen: &BTreeSet<i32>, item: &str, input: &str) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some((pid, _)) = started_previews(root).into_iter().find(|(pid, event)| {
            !seen.contains(pid) && event["item"] == item && event["input"] == input
        }) {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "preview did not start for item {item:?}, input {input:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn send(process: &mut support::LauncherProcess, keys: &[u8]) {
    process.master.write_all(keys).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn default_preview_is_collapsed_and_resolves_page_and_item_details() {
    let items = r#"items = [{ display = "Selected item", value = "selected-value", metadata = { summary = "DETAILS_MARKER" } }]"#;
    let page_preview = r#"preview = { producer = "declared", document = "PAGE_MARKER" }"#;
    for (page_config, expected) in [
        ("", "DETAILS_MARKER"),
        ("preview = { inherit = true }", "DETAILS_MARKER"),
        (page_preview, "PAGE_MARKER"),
    ] {
        let root = temporary_root();
        let config = root.join("config.toml");
        write_test_config(
            &config,
            &format!(
                r#"
            default_view = "sample:main"
            [workflows.sample.views.main.engine]
            type = "picker"
            [workflows.sample.views.main.engine.config]
            {items}
            {page_config}
        "#
            ),
        )
        .unwrap();
        let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
        let initial =
            wait_for_fresh_screen(&process.master, |screen| screen.contains("Selected item"));
        let initial = String::from_utf8_lossy(&initial);
        for marker in ["DETAILS_MARKER", "PAGE_MARKER"] {
            assert!(
                !initial.contains(marker),
                "preview started expanded: {initial}"
            );
        }
        send(&mut process, b"\x10");
        let expanded = wait_for_fresh_screen(&process.master, |screen| {
            screen.contains(if expected == "DETAILS_MARKER" {
                "selected-value"
            } else {
                expected
            })
        });
        let expanded = String::from_utf8_lossy(&expanded);
        if expected == "PAGE_MARKER" {
            assert!(expanded.contains("PAGE_MARKER"), "{expanded}");
        }
        if expected == "DETAILS_MARKER" {
            assert!(expanded.contains("selected-value"), "{expanded}");
            assert!(!expanded.contains("DETAILS_MARKER"), "{expanded}");
        }
        send(&mut process, b"\x10");
        wait_for_fresh_screen(&process.master, |screen| {
            screen.contains("Selected item") && !screen.contains(expected)
        });
        send(&mut process, b"\x04");
        let (status, _) = wait_for_launcher_exit(&mut process);
        assert_eq!(status, 0);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn slow_preview_keeps_items_responsive_and_selection_hide_and_exit_reap_children() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"
        [workflows.sample.views.main.engine]
        type = "picker"
        [workflows.sample.views.main.engine.config.items]
        producer = "script"
        handler = { file = "scripts/items.py" }
        [workflows.sample.views.main.engine.config.preview]
        producer = "script"
        handler = { file = "scripts/preview.py" }
        [workflows.sample.views.main.keymap]
        "ctrl+p" = "toggle_preview"
        "alt+k" = "preview_scroll_up"
        "alt+j" = "preview_scroll_down"
        "#,
    )
    .unwrap();
    let scripts = root.join("workflows/sample/scripts");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(
        scripts.join("items.py"),
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys
request = json.load(sys.stdin)
assert request['version'] == 1 and request['entrypoint'] == 'picker-items'
query = request['context']['engine']['state']['input']
pathlib.Path(os.environ['PREVIEW_EVENTS'], 'items-' + (query or 'initial')).write_text('loaded')
json.dump({'version': 1, 'items': [
    {'display': 'Slow item', 'value': 'slow', 'metadata': {'query': query}},
    {'display': 'Fast item', 'value': 'fast', 'metadata': {'query': query}},
]}, sys.stdout)
"#,
    )
    .unwrap();
    fs::write(
        scripts.join("preview.py"),
        r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
request = json.load(sys.stdin)
assert request['version'] == 1 and request['entrypoint'] == 'picker-preview'
context = request['context']
assert set(context) == {'parameters', 'input', 'engine'}
state = context['engine']['state']
assert set(state['item']) == {'text', 'value', 'metadata'}
assert state['item']['metadata']['query'] == state['input']
events = pathlib.Path(os.environ['PREVIEW_EVENTS'])
pid = os.getpid()
(events / f'preview-start-{pid}').write_text(json.dumps({
    'pid': pid, 'item': state['item']['value'], 'input': state['input'],
}))
if state['item']['value'] == 'slow':
    time.sleep(4)
    (events / f'preview-finished-{pid}').write_text('stale')
    text = 'PREVIEW_STALE'
else:
    text = 'PREVIEW_FAST'
json.dump({'version': 1, 'preview': {'type': 'paragraph', 'text': text}}, sys.stdout)
"#,
    )
    .unwrap();

    let events = root.to_string_lossy().into_owned();
    let mut process =
        spawn_launcher_with_args_and_env(&config, &[], &[("PREVIEW_EVENTS", &events)]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Slow item"));
    let hidden_until = Instant::now() + Duration::from_millis(250);
    while Instant::now() < hidden_until {
        assert!(
            started_previews(&root).is_empty(),
            "collapsed preview ran a script"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    send(&mut process, b"\x10");
    let mut seen = BTreeSet::new();
    let first = wait_for_preview_start(&root, &seen, "slow", "");
    seen.insert(first);

    // A fresh items request must complete while the first preview is sleeping.
    send(&mut process, b"q");
    let second = wait_for_preview_start(&root, &seen, "slow", "q");
    seen.insert(second);
    assert!(root.join("items-q").exists());
    wait_for_process_exit(first);

    // Cursor selection cancels the slow request and displays only the replacement.
    send(&mut process, b"\x1b[B");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("PREVIEW_FAST") && !screen.contains("PREVIEW_STALE")
    });
    wait_for_process_exit(second);
    seen.extend(started_previews(&root).into_iter().map(|(pid, _)| pid));

    // Explicit Ctrl+P is configured locally; Ctrl+D and Ctrl+U retain defaults.
    send(&mut process, b"\x10");
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Fast item") && !screen.contains("PREVIEW_FAST")
    });
    send(&mut process, b"\x1b[A");
    let hidden_until = Instant::now() + Duration::from_millis(250);
    while Instant::now() < hidden_until {
        assert!(
            started_previews(&root)
                .iter()
                .all(|(pid, _)| seen.contains(pid))
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    send(&mut process, b"\x10");
    let third = wait_for_preview_start(&root, &seen, "slow", "q");
    seen.insert(third);
    send(&mut process, b"\x10");
    wait_for_process_exit(third);

    send(&mut process, b"\x10");
    let fourth = wait_for_preview_start(&root, &seen, "slow", "q");
    send(&mut process, b"\x04");
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    wait_for_process_exit(fourth);
    for pid in [first, second, third, fourth] {
        assert!(
            !root.join(format!("preview-finished-{pid}")).exists(),
            "cancelled preview {pid} completed its stale response"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
