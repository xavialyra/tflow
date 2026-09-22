mod support;

use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf};
use support::{
    spawn_launcher_with_args_and_env, temporary_root, wait_for_fresh_screen, wait_for_launcher_exit,
};

fn setup_fixture(root: &std::path::Path) -> PathBuf {
    let suite = root.join("suite.toml");
    let wf_dir = root.join("workflows/native-form");
    let scripts_dir = wf_dir.join("scripts");
    let sel_dir = root.join("workflows/selectors");
    fs::create_dir_all(&scripts_dir).unwrap();
    fs::create_dir_all(&sel_dir).unwrap();

    fs::write(
        &suite,
        r#"[suite]
api = 1
name = "Native Form Fixtures"
entrypoint = "native-form:main"

[workflows]
native-form = { dir = "./workflows/native-form" }
selectors = { dir = "./workflows/selectors" }

[aliases]
native-form = "native-form:main"
"#,
    )
    .unwrap();

    fs::write(
        sel_dir.join("workflow.toml"),
        r#"[workflow]
api = 1
name = "Native form command selector"
entrypoint = "commands"

[views.commands.query]
type = "object"
commands = { type = "array<object>", default = [] }

[views.commands.engine]
type = "picker"
[views.commands.engine.config]
show_input = false
[views.commands.engine.config.items]
producer = "script"
[views.commands.engine.config.items.handler]
script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
items = [{"display": command["label"], "metadata": {"command": command["ref"]}}
         for command in request["context"]["parameters"]["commands"]]
json.dump({"version": 1, "items": items}, sys.stdout)
'''

[views.commands.keymap]
enter = "accept"

[commands.accept]
label = "Select"
type = "return"
producer = "script"
[commands.accept.handler]
script = '''#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
reference = request["context"]["engine"]["state"]["item"]["metadata"]["command"]
json.dump({"version": 1, "operation": {"type": "return", "value": reference}}, sys.stdout)
'''
"#,
    )
    .unwrap();

    fs::write(
        wf_dir.join("workflow.toml"),
        r#"[workflow]
api = 1
name = "Native form fixture"
entrypoint = "main"

[views.main]
[views.main.engine]
type = "form"
[views.main.engine.config.content]
producer = "declared"
[views.main.engine.config.content.handler]
fields = [
    { name = "name", label = "Project name", required = true },
    { name = "count", label = "Count", type = "integer", value = 2 },
    { name = "enabled", label = "Enabled", type = "boolean", value = false },
    { name = "options", label = "Options", type = "json", value = { tags = [] } },
]
[views.main.keymap]
enter = "submit"
"ctrl+l" = "details"
"ctrl+t" = "string_form"

[views.dynamic.query]
type = "object"
spec = { type = "object", default = { fields = [{ name = "message", label = "Dynamic message", value = "From query", required = true }, { name = "data", type = "json", value = { count = 1 } }] } }
[views.dynamic.engine]
type = "form"
[views.dynamic.engine.config.content]
producer = "script"
handler = { file = "scripts/content.py" }
[views.dynamic.keymap]
enter = "submit"

[commands.submit]
label = "Submit string values"
type = "return"
producer = "script"
handler = { file = "scripts/submit.py" }

[commands.details]
label = "Edit details"
type = "call"
producer = "declared"
handler = { target = "native-form:dynamic" }
[commands.details.return_processor]
type = "return"
producer = "script"
handler = { file = "scripts/returned.py" }

[commands.string_form]
label = "String form"
type = "call"
producer = "declared"
handler = { target = "native-form:string", query = "a:string:dd,b:number:null" }
[commands.string_form.return_processor]
type = "return"
producer = "script"
handler = { file = "scripts/returned.py" }

[views.string.query]
type = "string"
[views.string.engine]
type = "form"
[views.string.engine.config.content]
producer = "script"
handler = { file = "scripts/string-content.py" }
[views.string.keymap]
"" = "submit"

[views.failed.engine]
type = "form"
[views.failed.engine.config.content]
producer = "script"
[views.failed.engine.config.content.handler]
script = '''#!/bin/sh
printf '%s' '{"version":2,"content":{"fields":[]}}'
'''
"#,
    )
    .unwrap();

    fs::write(
        scripts_dir.join("content.py"),
        r#"#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
assert request["entrypoint"] == "form-content"
assert request["context"]["engine"]["type"] == "form"
content = request["context"]["parameters"]["spec"]
json.dump({"version": 1, "content": content}, sys.stdout)
"#,
    )
    .unwrap();

    fs::write(
        scripts_dir.join("returned.py"),
        r#"#!/usr/bin/env python3
import json, os, sys
request = json.load(sys.stdin)
assert request["entrypoint"] == "return"
if path := os.environ.get("NATIVE_FORM_RETURN"):
    with open(path, "w") as result:
        json.dump(request, result)
json.dump({
    "version": 1,
    "operation": {
        "type": "return",
        "value": {
            "caller": request["context"]["engine"]["state"]["values"],
            "child": request["context"]["result"],
        },
    },
}, sys.stdout)
"#,
    )
    .unwrap();

    fs::write(
        scripts_dir.join("string-content.py"),
        r#"#!/usr/bin/env python3
import json, sys
request = json.load(sys.stdin)
fields = []
for declaration in request["context"]["parameters"].split(","):
    name, kind, initial = declaration.split(":", 2)
    value = initial if kind == "string" else json.loads(initial)
    fields.append({"name": name, "label": f"{name} ({kind})", "type": kind, "value": value})
json.dump({"version": 1, "content": {"fields": fields}}, sys.stdout)
"#,
    )
    .unwrap();

    fs::write(
        scripts_dir.join("submit.py"),
        r#"#!/usr/bin/env python3
import json, os, sys
request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
if not state["valid"]:
    raise SystemExit("Please correct the form fields")
if path := os.environ.get("NATIVE_FORM_RESULT"):
    with open(path, "w") as result:
        json.dump(request, result)
json.dump({"version": 1, "operation": {"type": "return", "value": state["values"]}}, sys.stdout)
"#,
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        for script in [
            "content.py",
            "returned.py",
            "string-content.py",
            "submit.py",
        ] {
            fs::set_permissions(scripts_dir.join(script), perms.clone()).unwrap();
        }
    }

    suite
}

fn send(process: &mut support::LauncherProcess, bytes: &[u8]) {
    process.master.write_all(bytes).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn native_form_commands_receive_validated_drafts_after_popup_and_paste() {
    let root = temporary_root();
    let fixture = setup_fixture(&root);
    let result = root.join("result.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture,
        &[],
        &[("NATIVE_FORM_RESULT", result.to_str().unwrap())],
    );
    wait_for_fresh_screen(&process.master, |s| s.contains("Project name"));
    assert!(!result.exists());
    send(&mut process, "\x1b[200~é project\x1b[201~".as_bytes());
    wait_for_fresh_screen(&process.master, |s| s.contains("é project"));
    send(&mut process, b"\t\x15x");
    wait_for_fresh_screen(&process.master, |s| s.contains("Enter an integer"));
    send(
        &mut process,
        b"\x157\t \t\x15\x1b[200~{\"nested\":[true,3]}\x1b[201~",
    );
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("nested") && !s.contains("Enter an integer")
    });
    send(&mut process, b"\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    let request: Value = serde_json::from_slice(&fs::read(&result).unwrap()).unwrap();
    let state = &request["context"]["engine"]["state"];
    assert_eq!(request["entrypoint"], "command");
    assert_eq!(request["context"]["engine"]["type"], "form");
    assert_eq!(
        state["values"],
        json!({"name":"é project","count":7,"enabled":true,"options":{"nested":[true,3]}})
    );
    assert_eq!(state["valid"], true);
    assert_eq!(state["dirty"], true);
    assert_eq!(state["errors"], json!({}));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_form_script_interprets_object_query_and_commands_keep_original_parameters() {
    let root = temporary_root();
    let fixture = setup_fixture(&root);
    let result = root.join("result.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture,
        &["native-form:dynamic"],
        &[("NATIVE_FORM_RESULT", result.to_str().unwrap())],
    );
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("Dynamic message") && s.contains("From query")
    });
    send(&mut process, b" edited\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    let request: Value = serde_json::from_slice(&fs::read(&result).unwrap()).unwrap();
    assert_eq!(
        request["context"]["parameters"]["spec"]["fields"][0]["value"],
        "From query"
    );
    assert_eq!(
        request["context"]["engine"]["state"]["values"]["message"],
        "From query edited"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_form_call_return_scripts_receive_child_values_and_restored_caller_drafts() {
    let root = temporary_root();
    let fixture = setup_fixture(&root);
    let submitted = root.join("submitted.json");
    let returned = root.join("returned.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture,
        &[],
        &[
            ("NATIVE_FORM_RESULT", submitted.to_str().unwrap()),
            ("NATIVE_FORM_RETURN", returned.to_str().unwrap()),
        ],
    );
    wait_for_fresh_screen(&process.master, |s| s.contains("Project name"));
    send(
        &mut process,
        "\x1b[200~Caller é界\x1b[201~\t\x15invalid".as_bytes(),
    );
    wait_for_fresh_screen(&process.master, |s| s.contains("Enter an integer"));
    // Invalid caller drafts must not block a call/navigation command.
    send(&mut process, b"\x0c");
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("Dynamic message") && s.contains("From query")
    });
    send(
        &mut process,
        "\x15\x1b[200~Child é界\x1b[201~\t\x15\x1b[200~{\"nested\":[1,true]}\x1b[201~".as_bytes(),
    );
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("Child é界") && s.contains("nested")
    });
    send(&mut process, b"\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    let submitted: Value = serde_json::from_slice(&fs::read(submitted).unwrap()).unwrap();
    let returned: Value = serde_json::from_slice(&fs::read(returned).unwrap()).unwrap();
    let child_values = json!({"message":"Child é界", "data":{"nested":[1,true]}});
    assert_eq!(
        submitted["context"]["engine"]["state"]["values"],
        child_values
    );
    assert_eq!(
        submitted["context"]["parameters"]["spec"]["fields"][0]["value"],
        "From query"
    );
    assert_eq!(returned["entrypoint"], "return");
    assert_eq!(returned["context"]["result"], child_values);
    assert_eq!(returned["context"]["engine"]["type"], "form");
    let caller = &returned["context"]["engine"]["state"];
    assert_eq!(caller["values"]["name"], "Caller é界");
    assert_eq!(caller["values"]["count"], Value::Null);
    assert_eq!(caller["drafts"]["count"], "invalid");
    assert_eq!(caller["errors"]["count"], "Enter an integer");
    assert_eq!(caller["valid"], false);
    assert_eq!(caller["dirty"], true);
    assert_eq!(caller["focused"], "count");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn string_convention_form_submits_through_palette_and_call_return() {
    let root = temporary_root();
    let fixture = setup_fixture(&root);
    let submitted = root.join("submitted.json");
    let returned = root.join("returned.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture,
        &[],
        &[
            ("NATIVE_FORM_RESULT", submitted.to_str().unwrap()),
            ("NATIVE_FORM_RETURN", returned.to_str().unwrap()),
        ],
    );
    wait_for_fresh_screen(&process.master, |s| s.contains("Project name"));
    send(&mut process, b"\x14");
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("a (string)") && s.contains("dd") && s.contains("b (number)")
    });
    send(
        &mut process,
        "\x15\x1b[200~String é界\x1b[201~\tbad".as_bytes(),
    );
    wait_for_fresh_screen(&process.master, |s| s.contains("Enter a number"));
    // Even the command palette can be opened while a field is invalid.
    send(&mut process, b"\x0b");
    wait_for_fresh_screen(&process.master, |s| s.contains("Submit string values"));
    send(&mut process, b"\x1b");
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("Enter a number") && !s.contains("Submit string values")
    });
    send(&mut process, b"\x153.5");
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("3.5") && !s.contains("Enter a number")
    });
    send(&mut process, b"\x0b");
    wait_for_fresh_screen(&process.master, |s| s.contains("Submit string values"));
    std::thread::sleep(std::time::Duration::from_millis(50));
    send(&mut process, b"\r");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    let submitted: Value = serde_json::from_slice(&fs::read(submitted).unwrap()).unwrap();
    let returned: Value = serde_json::from_slice(&fs::read(returned).unwrap()).unwrap();
    assert_eq!(
        submitted["context"]["parameters"],
        "a:string:dd,b:number:null"
    );
    let values = json!({"a":"String é界", "b":3.5});
    assert_eq!(submitted["context"]["engine"]["state"]["values"], values);
    assert_eq!(submitted["context"]["engine"]["state"]["valid"], true);
    assert_eq!(returned["context"]["result"], values);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_form_content_failure_is_visible_and_can_be_cancelled() {
    let root = temporary_root();
    let fixture = setup_fixture(&root);
    let mut process = spawn_launcher_with_args_and_env(&fixture, &["native-form:failed"], &[]);
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("unsupported form-content protocol version 2")
    });
    send(&mut process, b"\x1b");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn view_command_precedes_workflow_command_and_editor_even_when_form_is_invalid() {
    use std::io::Read;

    let root = temporary_root();
    let config = root.join("config.toml");
    support::write_test_config(
        &config,
        r#"
        default_view = "example:form"

        [workflows.example.commands.cancel]
        key = "ctrl+u"
        label = "Session cancel"
        type = "return"
        producer = "declared"
        handler = { value = "SESSION_COMMAND_WON" }

        [workflows.example.views.form.engine]
        type = "form"
        [workflows.example.views.form.engine.config.content]
        producer = "declared"
        handler = { fields = [{ name = "name", label = "Required name", required = true }] }

        [workflows.example.views.form.commands.cancel]
        key = "ctrl+u"
        label = "View cancel"
        type = "return"
        producer = "declared"
        handler = { value = "VIEW_COMMAND_WON" }
    "#,
    )
    .unwrap();
    let mut process = support::spawn_launcher_with_redirected_stdout(&config);
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("Required name") && s.contains("Invalid")
    });
    send(&mut process, b"\x15");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
    let mut result = String::new();
    process
        .take_stdout()
        .unwrap()
        .read_to_string(&mut result)
        .unwrap();
    assert_eq!(result.trim(), "VIEW_COMMAND_WON");
    fs::remove_dir_all(root).unwrap();
}
