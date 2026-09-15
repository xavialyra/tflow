mod support;

use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf};
use support::{
    spawn_launcher_with_args_and_env, temporary_root, wait_for_fresh_screen, wait_for_launcher_exit,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native-form/config.toml")
}
fn send(process: &mut support::LauncherProcess, bytes: &[u8]) {
    process.master.write_all(bytes).unwrap();
    process.master.flush().unwrap();
}

#[test]
fn native_form_commands_receive_validated_drafts_after_popup_and_paste() {
    let root = temporary_root();
    let result = root.join("result.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture(),
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
    let result = root.join("result.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture(),
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
    let submitted = root.join("submitted.json");
    let returned = root.join("returned.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture(),
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
    let submitted = root.join("submitted.json");
    let returned = root.join("returned.json");
    let mut process = spawn_launcher_with_args_and_env(
        &fixture(),
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
    let mut process = spawn_launcher_with_args_and_env(&fixture(), &["native-form:failed"], &[]);
    wait_for_fresh_screen(&process.master, |s| {
        s.contains("unsupported form-content protocol version 2")
    });
    send(&mut process, b"\x1b");
    let (status, output) = wait_for_launcher_exit(&mut process);
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&output));
}

#[test]
fn view_command_precedes_session_command_and_editor_even_when_form_is_invalid() {
    use std::io::Read;

    let root = temporary_root();
    let config = root.join("config.toml");
    support::write_test_config(
        &config,
        r#"
        default_view = "example:form"

        [commands.bindings.cancel]
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
        scope = "view"
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
