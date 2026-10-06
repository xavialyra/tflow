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
fn host_bindings_customize_open_companion_and_allow_disabling() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"

        [host.bindings]
        "ctrl+l" = false
        "ctrl+o" = "@host:open_companion"

        [workflows.sample.views.main]
        engine = "picker"
        companion = "details"

        [workflows.sample.views.main.picker]
        items = [{ display = "First App", value = "app1" }]

        [workflows.sample.views.details]
        engine = "capture"

        [workflows.sample.views.details.capture.output]
        content = "DETAILS_PAGE"
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("First App") && screen.contains("DETAILS_PAGE")
    });

    // 1. Pressing Ctrl-L followed by a barrier input should keep First App visible and not open companion
    process.master.write_all(b"\x0cxyz").unwrap();
    process.master.flush().unwrap();
    let screen = wait_for_fresh_screen(&process.master, |screen| screen.contains("xyz"));
    assert!(String::from_utf8_lossy(&screen).contains("First App"));
    assert!(String::from_utf8_lossy(&screen).contains("│"));

    // 2. Clear query barrier with Ctrl-U
    process.master.write_all(b"\x15").unwrap();
    wait_for_fresh_screen(&process.master, |screen| !screen.contains("xyz"));

    // 3. Pressing Ctrl-O should navigate into companion view
    process.master.write_all(b"\x0f").unwrap();
    process.master.flush().unwrap();
    let screen = wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("DETAILS_PAGE") && !screen.contains("First App")
    });
    assert!(String::from_utf8_lossy(&screen).contains("DETAILS_PAGE"));

    // 4. Escape unwinds back to main
    process.master.write_all(b"\x1b").unwrap();
    process.master.flush().unwrap();
    wait_for_fresh_screen(&process.master, |screen| screen.contains("First App"));

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn companion_slot_declaration_with_dynamic_projection_and_pure_command_toggle() {
    let root = temporary_root();
    let config = root.join("config.toml");
    write_test_config(
        &config,
        r#"
        default_view = "sample:main"

        [workflows.sample.views.main]
        engine = "picker"

        [workflows.sample.views.main.companions.preview]
        target = "details"
        args = { title = "$selection.display", code = "$selection.value" }

        [workflows.sample.views.main.companions.inspector]
        target = "inspector"
        args = { id = "$selection.value" }

        [workflows.sample.views.main.picker]
        items = [
            { display = "Database Server", value = "db-01" },
            { display = "Cache Server", value = "redis-01" },
        ]

        [workflows.sample.views.main.bindings]
        "ctrl+p" = "sample.toggle_preview"
        "ctrl+i" = "sample.toggle_inspector"

        [workflows.sample.commands.toggle_preview]
        label = "Toggle Preview"
        type = "companion"
        slot = "preview"

        [workflows.sample.commands.toggle_inspector]
        label = "Toggle Inspector"
        type = "companion"
        slot = "inspector"

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
json.dump({"version": 1, "output": f"PREVIEW: {title} [{code}]"}, sys.stdout)
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
json.dump({"version": 1, "output": f"INSPECTOR: {ident}"}, sys.stdout)
'''
        "#,
    )
    .unwrap();

    let mut process = spawn_launcher_with_args_and_env(&config, &[], &[]);
    wait_for_fresh_screen(&process.master, |screen| screen.contains("Database Server"));

    // Press Ctrl-P to toggle companion slot
    process.master.write_all(b"\x10").unwrap();
    process.master.flush().unwrap();

    // Verify projected parameters reached companion script and rendered
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("PREVIEW: Database Server [db-01]")
    });

    // Press Down arrow to change active selection
    process.master.write_all(b"\x1b[B").unwrap();
    process.master.flush().unwrap();

    // Verify companion dynamically re-evaluates projection on selection change
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("PREVIEW: Cache Server [redis-01]")
    });

    // Hot-swap to second companion slot via Ctrl-I
    process.master.write_all(b"\x09").unwrap();
    process.master.flush().unwrap();

    // Verify second companion slot renders correctly with its own projected args
    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("INSPECTOR: redis-01") && !screen.contains("PREVIEW:")
    });

    // Press Ctrl-I again to close companion slot
    process.master.write_all(b"\x09").unwrap();
    process.master.flush().unwrap();

    wait_for_fresh_screen(&process.master, |screen| {
        screen.contains("Database Server") && !screen.contains("INSPECTOR:")
    });

    process.master.write_all(b"\x04").unwrap();
    process.master.flush().unwrap();
    let (status, _) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0);
    fs::remove_dir_all(root).unwrap();
}
