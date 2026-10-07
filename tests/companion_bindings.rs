mod support;

use std::fs;
use std::io::Write;
use support::{
    spawn_launcher_with_args_and_env, temporary_root, wait_for_fresh_screen,
    wait_for_launcher_exit, write_test_config,
};

#[test]
fn companion_static_binding_and_dynamic_literal_have_distinct_semantics() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"
        image_protocol = "halfblocks"

        [workflows.sample.views.main]
        engine = "picker"

        [workflows.sample.views.main.picker]
        items = [{ display = "Selected entry", value = "selected-value" }]

        [workflows.sample.views.main.bindings]
        "alt+s" = "sample.static_details"
        "alt+d" = "sample.dynamic_details"

        [workflows.sample.commands.static_details]
        label = "Static Details"
        companion = "details"
        query = "static-query"

        [workflows.sample.commands.dynamic_details]
        label = "Dynamic Details"
        type = "companion"
        script = '''#!/usr/bin/env python3
import json, sys
json.dump({"version": 1, "companion": {"target": "details", "query": "dynamic-query"}}, sys.stdout)
'''

        [workflows.sample.views.details]
        engine = "capture"

        [workflows.sample.views.details.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
ctx = request.get("context", {})
raw = ctx.get("input") or ""
val = raw if isinstance(raw, str) else json.dumps(raw)
json.dump({"version": 1, "output": "VAL_" + val}, sys.stdout)
'''
    "#,
    )
    .unwrap();
    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Selected entry"));
    process.master.write_all(b"\x1bs").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("VAL_static-query")
    });
    // Same target toggles off, then a dynamic response mounts its query.
    process.master.write_all(b"\x1bs").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Selected entry") && !screen.contains("VAL_static-query")
    });
    process.master.write_all(b"\x1bd").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("VAL_dynamic-query")
    });
    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ctrl_p_without_a_binding_or_after_unbind_does_not_toggle_companion() {
    for unbind in [false, true] {
        let root = temporary_root();
        let config = root.join("config.toml");
        let binding = if unbind {
            r#"
            [workflows.sample.views.main.bindings]
            "ctrl+p" = "sample.details"
            [workflows.sample.views.main.unbind]
            keys = ["ctrl+p"]
            "#
        } else {
            ""
        };
        write_test_config(
            &config,
            &format!(
                r#"
                default_view = "sample:main"
                [workflows.sample.views.main]
                engine = "picker"
                [workflows.sample.views.main.picker]
                items = [{{ display = "Selected entry", value = "entry" }}]
                {binding}
                [workflows.sample.commands.details]
                label = "Toggle Details"
                companion = "details"
                [workflows.sample.views.details]
                engine = "capture"
                [workflows.sample.views.details.capture.output]
                script = '''#!/usr/bin/env python3
import json, pathlib, sys
pathlib.Path("{marker}").write_text("started")
json.dump({{"version": 1, "output": "UNEXPECTED_COMPANION"}}, sys.stdout)
'''
                "#,
                marker = root.join("companion-started").display(),
            ),
        )
        .unwrap();
        let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
        wait_for_fresh_screen(&process.master, |screen| screen.contains("Selected entry"));
        // A printable query is a redraw barrier, avoiding a negative timeout.
        process.master.write_all(b"\x10barrier").unwrap();
        process.master.flush().unwrap();
        let screen = wait_for_fresh_screen(&process.master, |screen| screen.contains("barrier"));
        assert!(!String::from_utf8_lossy(&screen).contains("UNEXPECTED_COMPANION"));
        process.master.write_all(b"\x04").unwrap();
        process.master.flush().unwrap();
        let (status, _) = wait_for_launcher_exit(&mut process);
        assert_eq!(status, 0);
        assert!(!root.join("companion-started").exists(), "unbind={unbind}");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ctrl_p_bound_to_an_ordinary_run_command_only_executes_that_command() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        &format!(
            r#"
            default_view = "sample:main"
            [workflows.sample.views.main]
            engine = "picker"
            [workflows.sample.views.main.picker]
            items = [{{ display = "Selected entry", value = "entry" }}]
            [workflows.sample.views.main.bindings]
            "ctrl+p" = "sample.marker"
            [workflows.sample.commands.marker]
            label = "Run Marker"
            type = "run"
            argv = ["sh", "-c", "printf 'ORDINARY_COMMAND_EXECUTED\\n'"]
            exit = true
            [workflows.sample.views.details]
            engine = "capture"
            [workflows.sample.views.details.capture.output]
            script = '''#!/usr/bin/env python3
import json, pathlib, sys
pathlib.Path("{marker}").write_text("started")
json.dump({{"version": 1, "output": "UNEXPECTED_COMPANION"}}, sys.stdout)
'''
            "#,
            marker = root.join("companion-started").display(),
        ),
    )
    .unwrap();
    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Selected entry"));
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    assert!(String::from_utf8_lossy(&output).contains("ORDINARY_COMMAND_EXECUTED"));
    assert!(!root.join("companion-started").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_ctrl_p_companion_command_and_an_alternate_key_toggle_equivalently() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"
        [workflows.sample.views.main]
        engine = "picker"
        [workflows.sample.views.main.picker]
        items = [{ display = "Selected entry", value = "selected-value" }]
        [workflows.sample.views.main.bindings]
        "ctrl+p" = "sample.details"
        "alt+p" = "sample.details"
        [workflows.sample.commands.details]
        label = "Toggle Details"
        companion = "details"
        [workflows.sample.views.details]
        engine = "capture"
        [workflows.sample.views.details.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
ctx = request.get("context", {})
raw = ctx.get("parameters") or ctx.get("input") or ""
if isinstance(raw, str):
    try:
        data = json.loads(raw)
    except Exception:
        data = {}
else:
    data = raw or {}
item = data.get("item") or data
val = item.get("value", "val") if isinstance(item, dict) else str(item)
json.dump({"version": 1, "output": "DETAILS_" + val}, sys.stdout)
'''
        "#,
    )
    .unwrap();
    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Selected entry"));
    // Either key can open or close the same attachment, across key changes.
    for keys in [b"\x10".as_slice(), b"\x1bp".as_slice()] {
        process.master.write_all(keys).unwrap();
        process.master.flush().unwrap();
        wait_for_fresh_screen(&process.master, |screen| {
            screen.contains("DETAILS_selected-value")
        });
        process.master.write_all(keys).unwrap();
        process.master.flush().unwrap();
        wait_for_fresh_screen(&process.master, |screen| {
            screen.contains("Selected entry") && !screen.contains("DETAILS_selected-value")
        });
    }
    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn companion_defined_purely_as_command_without_view_slot_table() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"

        [workflows.sample.views.main]
        engine = "picker"

        [workflows.sample.views.main.picker]
        items = [
            { display = "Database Server", value = "db-01" },
            { display = "Cache Server", value = "redis-01" },
        ]

        [workflows.sample.views.main.bindings]
        "ctrl+p" = "sample.preview"
        "ctrl+i" = "sample.inspector"

        [workflows.sample.commands.preview]
        label = "Toggle Preview"
        type = "companion"
        target = "details"
        args = { title = "$selection.display", code = "$selection.value" }

        [workflows.sample.commands.inspector]
        label = "Toggle Inspector"
        type = "companion"
        target = "inspector"
        args = { id = "$selection.value" }

        [workflows.sample.views.details]
        engine = "capture"
        [workflows.sample.views.details.query]
        type = "object"
        title = { type = "string", default = "" }
        code = { type = "string", default = "" }

        [workflows.sample.views.details.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
params = request.get("context", {}).get("parameters", {})
title = params.get("title", "no-title")
code = params.get("code", "no-code")
json.dump({"version": 1, "output": f"COMMAND_PREVIEW: {title} [{code}]"}, sys.stdout)
'''

        [workflows.sample.views.inspector]
        engine = "capture"
        [workflows.sample.views.inspector.query]
        type = "object"
        id = { type = "string", default = "" }

        [workflows.sample.views.inspector.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
params = request.get("context", {}).get("parameters", {})
ident = params.get("id", "no-id")
json.dump({"version": 1, "output": f"COMMAND_INSPECTOR: {ident}"}, sys.stdout)
'''
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Database Server"));

    // 1. Press Ctrl-P to toggle companion command
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();

    // Verify projected parameters reached companion script and rendered
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMMAND_PREVIEW: Database Server")
    });

    // 2. Press Down arrow to change active selection
    process.master.write_all(b"\x1b[B").unwrap();
    process.master.flush().unwrap();

    // Verify companion dynamically re-evaluates projection on selection change
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMMAND_PREVIEW: Cache Server")
    });

    // 3. Hot-swap to inspector companion command via Ctrl-I (Tab)
    process.master.write_all(b"\x09").unwrap();
    process.master.flush().unwrap();

    // Verify second companion command renders correctly with its own projected args
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMMAND_INSPECTOR: redis-01") && !screen.contains("COMMAND_PREVIEW:")
    });

    // 4. Press Ctrl-I again to close companion
    process.master.write_all(b"\x09").unwrap();
    process.master.flush().unwrap();

    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Database Server") && !screen.contains("COMMAND_INSPECTOR:")
    });

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn companion_size_auto_layout_integration() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let _ = write_test_config(
        &config,
        r#"
        default_view = "sample:main"

        [workflows.sample.views.main]
        engine = "picker"
        companion = "details"
        companion_size = ["40%", 10]

        [workflows.sample.views.main.picker]
        items = [{ display = "Entry", value = "val" }]

        [workflows.sample.views.details]
        engine = "capture"

        [workflows.sample.views.details.query]
        type = "object"
        item = { type = "object", default = {} }

        [workflows.sample.views.details.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
json.dump({"version": 1, "output": "PREVIEW_CONTENT"}, sys.stdout)
'''
        "#,
    );

    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Entry") && screen.contains("PREVIEW_CONTENT")
    });

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_companion_command_target_toggles_off_on_first_key_press() {
    let root = temporary_root();
    let config = root.join("config.toml");
    let _ = write_test_config(
        &config,
        r#"
        default_view = "sample:main"

        [workflows.sample.views.main]
        engine = "picker"
        companion = "toggle_details"

        [workflows.sample.views.main.picker]
        items = [{ display = "Entry", value = "val" }]

        [workflows.sample.views.main.bindings]
        "alt+p" = "sample.toggle_details"

        [workflows.sample.commands.toggle_details]
        label = "Toggle Details"
        type = "companion"
        target = "details"

        [workflows.sample.views.details]
        engine = "capture"

        [workflows.sample.views.details.query]
        type = "object"
        item = { type = "object", default = {} }

        [workflows.sample.views.details.capture.output]
        script = '''#!/usr/bin/env python3
import json, sys
json.dump({"version": 1, "output": "PREVIEW_CONTENT"}, sys.stdout)
'''
        "#,
    );

    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Entry") && screen.contains("PREVIEW_CONTENT")
    });

    process.master.write_all(b"\x1bp").unwrap();
    process.master.flush().unwrap();

    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Entry") && !screen.contains("PREVIEW_CONTENT")
    });

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn files_workflow_default_companion_toggle_first_time() {
    let workflow = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/config/workflows/files/workflow.toml");
    let root = temporary_root();
    let suite = root.join("default.toml");
    fs::write(
        &suite,
        format!(
            "[suite]\napi=1\nname='Files'\nentrypoint='files:main'\n[workflows]\nfiles={{file={:?}}}\n",
            workflow.to_str().unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        root.join("settings.toml"),
        format!(
            "image_protocol = 'halfblocks'\nlog_file = {:?}\n",
            root.join("runtime.log").to_str().unwrap()
        ),
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&suite, &[], &[]);
    // Wait for the files picker and companion border to render
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Search files") && (screen.contains("│") || screen.contains("─"))
    });

    // Press Ctrl+P (0x10) to toggle preview off on FIRST press
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();

    // Verify companion pane vertical border disappears on first press
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Search files") && !screen.contains("│")
    });

    // Press Ctrl+P again to toggle preview back ON
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Search files") && screen.contains("│")
    });

    // Press Ctrl+P once more to toggle preview back OFF
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Search files") && !screen.contains("│")
    });

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn multi_companion_switching_and_toggle() {
    let root = temporary_root();
    let workflow = root.join("workflow.toml");
    fs::write(
        &workflow,
        r#"
[workflow]
api = 1
name = "Multi Companion Switching"

[views.main]
engine = "picker"
companion = "comp_a"
companion_size = ["60%", "40%"]

[views.main.picker]
items = [
  { display = "First Entry", value = "entry-1" },
  { display = "Second Entry", value = "entry-2" },
]

[views.main.bindings]
"alt+a" = "switch_a"
"alt+b" = "switch_b"
"alt+c" = "switch_c"

[commands.switch_a]
label = "Companion A"
type = "companion"
target = "comp_a"

[commands.switch_b]
label = "Companion B"
type = "companion"
target = "comp_b"

[commands.switch_c]
label = "Companion C"
type = "companion"
target = "comp_c"

[views.comp_a]
engine = "capture"
[views.comp_a.capture.output]
content = "COMP_A_VIEW_ACTIVE"

[views.comp_b]
engine = "capture"
[views.comp_b.capture.output]
content = "COMP_B_VIEW_ACTIVE"

[views.comp_c]
engine = "capture"
[views.comp_c.capture.output]
content = "COMP_C_VIEW_ACTIVE"
"#,
    )
    .unwrap();

    let suite = root.join("default.toml");
    fs::write(
        &suite,
        format!(
            "[suite]\napi=1\nname='Multi Companion Suite'\nentrypoint='multi:main'\n[workflows]\nmulti={{file={:?}}}\n",
            workflow.to_str().unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        root.join("settings.toml"),
        format!(
            "image_protocol = 'halfblocks'\nlog_file = {:?}\n",
            root.join("runtime.log").to_str().unwrap()
        ),
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&suite, &[], &[]);

    // 1. Initial state: Main view rendered with default companion (comp_a) open
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMP_A_VIEW_ACTIVE") && screen.contains("│")
    });

    // 2. Switch from comp_a to comp_b via Alt+B
    process.master.write_all(b"\x1bb").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMP_B_VIEW_ACTIVE") && screen.contains("│")
    });

    // 3. Switch from comp_b to comp_c via Alt+C
    process.master.write_all(b"\x1bc").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMP_C_VIEW_ACTIVE") && screen.contains("│")
    });

    // 4. Toggle comp_c OFF by pressing Alt+C again
    process.master.write_all(b"\x1bc").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("First Entry")
            && !screen.contains("COMP_C_VIEW_ACTIVE")
            && !screen.contains("│")
    });

    // 5. Open comp_b directly from closed state via Alt+B
    process.master.write_all(b"\x1bb").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMP_B_VIEW_ACTIVE") && screen.contains("│")
    });

    // 6. Switch directly from comp_b back to comp_a via Alt+A
    process.master.write_all(b"\x1ba").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("COMP_A_VIEW_ACTIVE") && screen.contains("│")
    });

    // 7. Toggle comp_a OFF by pressing Alt+A again
    process.master.write_all(b"\x1ba").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("First Entry")
            && !screen.contains("COMP_A_VIEW_ACTIVE")
            && !screen.contains("│")
    });

    // Exit cleanly
    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_with_companion_loaded_directly_via_w_flag() {
    let workflow = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/config/workflows/files/workflow.toml");
    let result = support::run_tty_invocation_with_redirected_stdout(
        &["-w", workflow.to_str().unwrap()],
        b"\x04",
    );
    assert_eq!(result.status, 0);
}

#[test]
fn companion_preview_navigate_shortcut_and_return() {
    let root = temporary_root();
    let workflow = root.join("workflow.toml");
    fs::write(
        &workflow,
        r#"
[workflow]
api = 1
name = "Preview Navigation"

[views.main]
engine = "picker"
companion = "preview"
companion_size = ["55%", "45%"]

[views.main.picker]
items = [
  { display = "Selected Target Item", value = "target-123" },
]

[views.main.bindings]
"ctrl+p" = "preview"
"ctrl+l" = "inspect"

[commands.preview]
label = "Companion Preview"
type = "companion"
target = "preview"
args = { item = "$selection" }

[commands.inspect]
label = "Inspect Preview"
type = "navigate"
target = "preview"
args = { item = "$selection" }

[views.preview]
engine = "capture"

[views.preview.query]
type = "object"
item = { type = "object", default = {} }

[views.preview.capture.output]
script = '''#!/usr/bin/env python3
import json, sys
req = json.load(sys.stdin)
item = req.get("context", {}).get("parameters", {}).get("item", {})
val = item.get("value", "NONE")
json.dump({"version": 1, "output": f"FULLSCREEN_PREVIEW_ACTIVE: {val}"}, sys.stdout)
'''
"#,
    )
    .unwrap();

    let suite = root.join("default.toml");
    fs::write(
        &suite,
        format!(
            "[suite]\napi=1\nname='Preview Navigation Suite'\nentrypoint='nav:main'\n[workflows]\nnav={{file={:?}}}\n",
            workflow.to_str().unwrap(),
        ),
    )
    .unwrap();
    fs::write(
        root.join("settings.toml"),
        format!(
            "image_protocol = 'halfblocks'\nlog_file = {:?}\n",
            root.join("runtime.log").to_str().unwrap()
        ),
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&suite, &[], &[]);

    // 1. Initial state: Main view rendered with split-screen companion preview
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("FULLSCREEN_PREVIEW_ACTIVE: target-123") && screen.contains("│")
    });

    // 2. Press Ctrl+L to navigate (push) directly into the preview view
    process.master.write_all(b"\x0c").unwrap();
    process.master.flush().unwrap();

    // 3. Screen now displays the full-screen preview without split divider
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("FULLSCREEN_PREVIEW_ACTIVE: target-123") && !screen.contains("│")
    });

    // 4. Press Escape to return back to the main picker view
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();

    // 5. Main picker view is restored with companion split-screen intact
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Selected Target Item") && screen.contains("│")
    });

    // Exit cleanly
    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}
