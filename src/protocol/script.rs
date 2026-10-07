use crate::execution::{ensure_script_success, run_resolved_script_with_stdin_outcome_with_limit};
use crate::lifecycle::CancellationStatus;
use crate::view::ViewResult;
use crate::workflow::command::{CommandOwnerContext, CommandRef};
use crate::workflow::config::{ResolvedScriptSource, ViewPresentation};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: u64 = 1;
const MAX_ITEMS_SCRIPT_STDOUT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProtocolOperation {
    Navigate {
        target: String,
        query: Option<Value>,
        presentation: ViewPresentation,
        replace: bool,
        clear_input: bool,
    },
    Call {
        target: String,
        query: Option<Value>,
        presentation: ViewPresentation,
    },
    Return {
        value: Value,
    },
    InvokeCommand {
        command: CommandRef,
    },
    Run {
        argv: Vec<String>,
        exit: bool,
        success_message: Option<String>,
        timeout_ms: Option<u64>,
    },
    Companion {
        target: Option<String>,
        slot: Option<String>,
        query: Option<Value>,
    },
}

impl ProtocolOperation {
    pub(crate) fn operation_type(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Call { .. } => "call",
            Self::Return { .. } => "return",
            Self::InvokeCommand { .. } => "invoke-command",
            Self::Run { .. } => "run",
            Self::Companion { .. } => "companion",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProtocolOutcome {
    Noop,
    Operation(ProtocolOperation),
    Feedback {
        message: String,
        level: FeedbackLevel,
    },
}

impl ProtocolOutcome {
    #[cfg(test)]
    pub(crate) fn operation(&self) -> Option<&ProtocolOperation> {
        match self {
            Self::Operation(op) => Some(op),
            Self::Noop | Self::Feedback { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FeedbackLevel {
    #[default]
    Warning,
    Info,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawError {
    Message(String),
    Structured {
        message: String,
        #[serde(default)]
        level: FeedbackLevel,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCompanionOperation {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    slot: Option<String>,
    #[serde(default, alias = "args")]
    query: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResponse {
    version: u64,
    #[serde(default)]
    operation: Option<RawOperation>,
    #[serde(default)]
    companion: Option<RawCompanionOperation>,
    #[serde(default)]
    error: Option<RawError>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum RawOperation {
    Navigate {
        target: String,
        #[serde(default, alias = "args")]
        query: Option<Value>,
        #[serde(default)]
        presentation: ViewPresentation,
        #[serde(default)]
        replace: bool,
        #[serde(default)]
        clear_input: bool,
    },
    Call {
        target: String,
        #[serde(default, alias = "args")]
        query: Option<Value>,
        #[serde(default)]
        presentation: ViewPresentation,
    },
    Return {
        value: Value,
    },
    InvokeCommand {
        command: CommandRef,
    },
    Run {
        argv: Vec<String>,
        #[serde(default)]
        exit: bool,
        #[serde(default)]
        success_message: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Companion {
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        slot: Option<String>,
        #[serde(default, alias = "args")]
        query: Option<Value>,
    },
}

pub(crate) fn parse_response(
    stdout: &[u8],
    expected_operation: Option<&str>,
    source_label: &str,
) -> Result<ProtocolOutcome> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(ProtocolOutcome::Noop);
    }
    let mut response: RawResponse = serde_json::from_slice(stdout).with_context(|| {
        format!(
            "{} producer must write exactly one valid JSON protocol response",
            source_label
        )
    })?;
    anyhow::ensure!(
        response.operation.is_none() || response.companion.is_none(),
        "{} producer response cannot define both operation and companion",
        source_label
    );
    if let Some(c) = response.companion.take() {
        response.operation = Some(RawOperation::Companion {
            target: c.target,
            slot: c.slot,
            query: c.query,
        });
    }
    if response.version != PROTOCOL_VERSION {
        bail!(
            "{} producer protocol version {} is unsupported; expected {}",
            source_label,
            response.version,
            PROTOCOL_VERSION
        );
    }
    match (response.operation, response.error) {
        (Some(raw_op), None) => {
            let operation = match raw_op {
                RawOperation::Navigate {
                    target,
                    query,
                    presentation,
                    replace,
                    clear_input,
                } => ProtocolOperation::Navigate {
                    target,
                    query,
                    presentation,
                    replace,
                    clear_input,
                },
                RawOperation::Call {
                    target,
                    query,
                    presentation,
                } => ProtocolOperation::Call {
                    target,
                    query,
                    presentation,
                },
                RawOperation::Return { value } => ProtocolOperation::Return { value },
                RawOperation::InvokeCommand { command } => {
                    ProtocolOperation::InvokeCommand { command }
                }
                RawOperation::Run {
                    argv,
                    exit,
                    success_message,
                    timeout_ms,
                } => ProtocolOperation::Run {
                    argv,
                    exit,
                    success_message,
                    timeout_ms,
                },
                RawOperation::Companion {
                    target,
                    slot,
                    query,
                } => ProtocolOperation::Companion {
                    target,
                    slot,
                    query,
                },
            };
            if let Some(expected_operation) = expected_operation
                && operation.operation_type() != expected_operation
            {
                bail!(
                    "{} producer returned operation {:?}, expected {:?}",
                    source_label,
                    operation.operation_type(),
                    expected_operation
                );
            }
            validate_operation(&operation, source_label)?;
            Ok(ProtocolOutcome::Operation(operation))
        }
        (None, Some(raw_error)) => {
            let (message, level) = match raw_error {
                RawError::Message(msg) => (msg, FeedbackLevel::Warning),
                RawError::Structured { message, level } => (message, level),
            };
            anyhow::ensure!(
                !message.trim().is_empty(),
                "{} error message must not be empty",
                source_label
            );
            Ok(ProtocolOutcome::Feedback { message, level })
        }
        (Some(_), Some(_)) => {
            bail!(
                "{} producer response cannot define both operation and error",
                source_label
            );
        }
        (None, None) => {
            bail!(
                "{} producer response must define either operation or error",
                source_label
            );
        }
    }
}

fn validate_operation(operation: &ProtocolOperation, source_label: &str) -> Result<()> {
    match operation {
        ProtocolOperation::Navigate {
            target,
            presentation,
            ..
        }
        | ProtocolOperation::Call {
            target,
            presentation,
            ..
        } => {
            anyhow::ensure!(
                !target.is_empty(),
                "{} operation target must be non-empty",
                source_label
            );
            let is_popup =
                presentation.mode == crate::workflow::config::ViewPresentationMode::Popup;
            anyhow::ensure!(
                is_popup
                    || (presentation.width.is_none()
                        && presentation.height.is_none()
                        && presentation.anchor == crate::workflow::config::PopupAnchor::Center
                        && presentation.offset_x.is_none()
                        && presentation.offset_y.is_none()
                        && presentation.min_width.is_none()
                        && presentation.max_width.is_none()
                        && presentation.min_height.is_none()
                        && presentation.max_height.is_none()
                        && presentation.show_title),
                "{} operation width and height require popup mode",
                source_label
            );
            anyhow::ensure!(
                !presentation.width.as_ref().is_some_and(|w| w.is_zero())
                    && !presentation.height.as_ref().is_some_and(|h| h.is_zero()),
                "{} operation width and height must be positive",
                source_label
            );
            if let (Some(min), Some(max)) = (presentation.min_width, presentation.max_width) {
                anyhow::ensure!(
                    min <= max,
                    "{} operation min_width ({min}) cannot exceed max_width ({max})",
                    source_label
                );
            }
            if let (Some(min), Some(max)) = (presentation.min_height, presentation.max_height) {
                anyhow::ensure!(
                    min <= max,
                    "{} operation min_height ({min}) cannot exceed max_height ({max})",
                    source_label
                );
            }
        }
        ProtocolOperation::InvokeCommand { command } => {
            anyhow::ensure!(
                !command.id.is_empty(),
                "{} invoke-command reference must include a non-empty id",
                source_label
            );
        }
        ProtocolOperation::Run {
            argv, timeout_ms, ..
        } => {
            anyhow::ensure!(
                !argv.is_empty(),
                "{} run operation argv must not be empty",
                source_label
            );
            if let Some(timeout) = timeout_ms {
                anyhow::ensure!(
                    *timeout > 0,
                    "{} run operation timeout_ms must be positive",
                    source_label
                );
            }
            for (index, argument) in argv.iter().enumerate() {
                anyhow::ensure!(
                    !argument.contains('\0'),
                    "{} run operation argv[{}] cannot contain a NUL byte",
                    source_label,
                    index
                );
            }
        }
        ProtocolOperation::Return { .. } => {}
        ProtocolOperation::Companion { target, slot, .. } => {
            anyhow::ensure!(
                target.as_ref().is_some_and(|t| !t.is_empty())
                    || slot.as_ref().is_some_and(|s| !s.is_empty()),
                "{} companion operation requires non-empty target or slot",
                source_label
            );
        }
    }
    Ok(())
}

pub(crate) fn parse_declared_operation(
    operation_type: &str,
    handler: &toml::Value,
    source_label: &str,
) -> Result<ProtocolOperation> {
    let value = crate::workflow::config::toml_to_json(handler)?;
    let mut operation = value
        .as_object()
        .cloned()
        .with_context(|| format!("{} declared handler must be a table", source_label))?;
    if operation.contains_key("type") {
        bail!(
            "{} declared handler must not define the operation type field",
            source_label
        );
    }
    operation.insert(
        "type".to_string(),
        Value::String(operation_type.to_string()),
    );
    let envelope = json!({
        "version": PROTOCOL_VERSION,
        "operation": Value::Object(operation),
    });
    let bytes = serde_json::to_vec(&envelope)?;
    match parse_response(&bytes, Some(operation_type), source_label)? {
        ProtocolOutcome::Operation(operation) => Ok(operation),
        ProtocolOutcome::Noop | ProtocolOutcome::Feedback { .. } => unreachable!(),
    }
}

pub(crate) fn run_script_response(
    source_view: &str,
    source_label: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    expected_operation: Option<&str>,
    cancellation: &dyn CancellationStatus,
) -> Result<ProtocolOutcome> {
    let output = run_script_output(
        source_view,
        source_label,
        root,
        source,
        request,
        None,
        cancellation,
    );
    let output = output.result?;
    parse_response(&output.stdout, expected_operation, source_label)
}

pub(crate) struct ScriptResponseOutcome<T> {
    pub(crate) result: Result<T>,
    pub(crate) managed_child_reaped: bool,
}

pub(crate) fn run_script_items_response(
    source_view: &str,
    source_label: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    cancellation: &dyn CancellationStatus,
) -> ScriptResponseOutcome<Value> {
    let output = run_script_output(
        source_view,
        source_label,
        root,
        source,
        request,
        Some(MAX_ITEMS_SCRIPT_STDOUT),
        cancellation,
    );
    let managed_child_reaped = output.managed_child_reaped;
    let result = output.result.and_then(|output| {
        let response: RawItemsResponse =
            serde_json::from_slice(&output.stdout).with_context(|| {
                format!(
                    "{} producer must write exactly one valid items response",
                    source_label
                )
            })?;
        if response.version != PROTOCOL_VERSION {
            bail!(
                "{} producer protocol version {} is unsupported; expected {}",
                source_label,
                response.version,
                PROTOCOL_VERSION
            );
        }
        Ok(Value::Array(response.items))
    });
    ScriptResponseOutcome {
        result,
        managed_child_reaped,
    }
}

pub(crate) fn run_script_capture_response(
    source_view: &str,
    source_label: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    cancellation: &dyn CancellationStatus,
) -> ScriptResponseOutcome<Value> {
    let output = run_script_output(
        source_view,
        source_label,
        root,
        source,
        request,
        None,
        cancellation,
    );
    let managed_child_reaped = output.managed_child_reaped;
    let result = (|| {
        let output = output.result?;
        let response: RawCaptureResponse =
            serde_json::from_slice(&output.stdout).with_context(|| {
                format!(
                    "{} producer must write exactly one valid capture response",
                    source_label
                )
            })?;
        if response.version != PROTOCOL_VERSION {
            bail!(
                "{} producer protocol version {} is unsupported; expected {}",
                source_label,
                response.version,
                PROTOCOL_VERSION
            );
        }
        Ok(response.output)
    })();
    ScriptResponseOutcome {
        result,
        managed_child_reaped,
    }
}

pub(crate) fn form_request(parameters: &Value, input: &Value, state: &Value) -> Value {
    json!({"version": PROTOCOL_VERSION, "entrypoint": "form-content",
        "context": script_context(parameters, input, "form", state)})
}

pub(crate) fn run_script_form_response(
    owner: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    cancellation: &dyn CancellationStatus,
) -> ScriptResponseOutcome<Value> {
    let output = run_script_output(
        owner,
        "form-content",
        root,
        source,
        request,
        Some(1024 * 1024),
        cancellation,
    );
    ScriptResponseOutcome {
        managed_child_reaped: output.managed_child_reaped,
        result: output
            .result
            .and_then(|output| parse_form_response(&output.stdout)),
    }
}

fn parse_form_response(stdout: &[u8]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        version: u64,
        content: Value,
    }
    let response: Response = serde_json::from_slice(stdout)
        .context("form-content producer must write exactly one JSON response")?;
    anyhow::ensure!(
        response.version == PROTOCOL_VERSION,
        "unsupported form-content protocol version {}; expected 1",
        response.version
    );
    Ok(response.content)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItemsResponse {
    version: u64,
    items: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCaptureResponse {
    version: u64,
    output: Value,
}

struct ScriptOutputOutcome {
    result: Result<std::process::Output>,
    managed_child_reaped: bool,
}

fn run_script_output(
    source_view: &str,
    source_label: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    max_output_override: Option<usize>,
    cancellation: &dyn CancellationStatus,
) -> ScriptOutputOutcome {
    let stdin = match serde_json::to_vec(request)
        .with_context(|| format!("{} producer request could not be encoded", source_label))
    {
        Ok(stdin) => stdin,
        Err(error) => {
            return ScriptOutputOutcome {
                result: Err(error),
                managed_child_reaped: false,
            };
        }
    };
    let workflow_id = source_view
        .split_once(':')
        .map_or(source_view, |(workflow_id, _)| workflow_id);
    let outcome = run_resolved_script_with_stdin_outcome_with_limit(
        workflow_id,
        source_label,
        root,
        source,
        &[],
        Some(&stdin),
        max_output_override,
        cancellation,
    );
    let managed_child_reaped = outcome.managed_child_reaped();
    let result = outcome
        .into_result()
        .with_context(|| format!("{} producer execution failed", source_label))
        .and_then(|output| {
            ensure_script_success(&output)
                .with_context(|| format!("{} producer execution failed", source_label))?;
            Ok(output)
        });
    ScriptOutputOutcome {
        result,
        managed_child_reaped,
    }
}

fn script_context(
    parameters: &Value,
    input: &Value,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    let empty_obj = json!({});
    let parameters = if parameters.is_null() {
        &empty_obj
    } else {
        parameters
    };
    json!({
        "parameters": parameters,
        "input": input,
        "engine": {
            "type": engine_type,
            "state": engine_state,
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn command_request(
    owner: &CommandOwnerContext,
    command_id: &str,
    operation_type: &str,
    input: &Value,
    engine_state: &Value,
    engine_type: &str,
    commands: &Value,
    view: &Value,
) -> Value {
    let mut context = script_context(owner.parameters.values(), input, engine_type, engine_state);
    if let Value::Object(ref mut map) = context {
        map.insert(
            "command".to_string(),
            json!({"id": command_id, "type": operation_type}),
        );
        map.insert("commands".to_string(), commands.clone());
        map.insert("view".to_string(), view.clone());
    }
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "command",
        "context": context,
    })
}

pub(crate) fn items_request(
    parameters: &Value,
    input: &Value,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "picker-items",
        "context": script_context(parameters, input, engine_type, engine_state),
    })
}

pub(crate) fn capture_request(
    parameters: &Value,
    input: &Value,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "capture-output",
        "context": script_context(parameters, input, engine_type, engine_state),
    })
}

pub(crate) fn return_request(
    parameters: &Value,
    input: &Value,
    result: &ViewResult,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    let mut context = script_context(parameters, input, engine_type, engine_state);
    if let Value::Object(ref mut map) = context {
        map.insert("result".to_string(), result.value.clone());
    }
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "return",
        "context": context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_response_requires_a_value_but_accepts_null() {
        let missing = br#"{"version":1,"operation":{"type":"return"}}"#;
        let null = br#"{"version":1,"operation":{"type":"return","value":null}}"#;
        assert!(parse_response(missing, Some("return"), "test").is_err());
        let null = parse_response(null, Some("return"), "test").unwrap();
        assert!(matches!(
            null,
            ProtocolOutcome::Operation(ProtocolOperation::Return { value: Value::Null })
        ));
    }

    #[test]
    fn invoke_command_operations_carry_revision_aware_references() {
        let parsed = parse_response(
            br#"{"version":1,"operation":{"type":"invoke-command","command":{"id":"open","revision":42}}}"#,
            Some("invoke-command"),
            "test",
        )
        .unwrap();
        assert!(matches!(
            parsed,
            ProtocolOutcome::Operation(ProtocolOperation::InvokeCommand {
                command: CommandRef { revision: 42, .. }
            })
        ));

        let invalid = parse_response(
            br#"{"version":1,"operation":{"type":"invoke-command","command":{"id":"","revision":42}}}"#,
            None,
            "test",
        );
        assert!(invalid.is_err());

        // The runtime palette envelope only needs identity + revision; the
        // owning View is a dispatch-origin detail, not part of the payload.
        let payload = parse_response(
            br#"{"version":1,"operation":{"type":"invoke-command","command":{"id":"open","revision":42}}}"#,
            Some("invoke-command"),
            "test",
        )
        .unwrap();
        assert!(matches!(
            payload,
            ProtocolOutcome::Operation(ProtocolOperation::InvokeCommand {
                command: CommandRef { revision: 42, .. }
            })
        ));
    }

    #[test]
    fn navigate_clear_input_is_optional_and_parsed() {
        let explicit = parse_response(
            br#"{"version":1,"operation":{"type":"navigate","target":"a:b","clear_input":true}}"#,
            Some("navigate"),
            "test",
        )
        .unwrap();
        assert!(matches!(
            explicit,
            ProtocolOutcome::Operation(ProtocolOperation::Navigate {
                clear_input: true,
                ..
            })
        ));

        let defaulted = parse_response(
            br#"{"version":1,"operation":{"type":"navigate","target":"a:b"}}"#,
            Some("navigate"),
            "test",
        )
        .unwrap();
        assert!(matches!(
            defaulted,
            ProtocolOutcome::Operation(ProtocolOperation::Navigate {
                clear_input: false,
                ..
            })
        ));

        let with_args = parse_response(
            br#"{"version":1,"operation":{"type":"navigate","target":"a:b","args":{"item":123}}}"#,
            Some("navigate"),
            "test",
        )
        .unwrap();
        match with_args {
            ProtocolOutcome::Operation(ProtocolOperation::Navigate { query, .. }) => {
                assert_eq!(query, Some(json!({"item": 123})));
            }
            _ => panic!("expected navigate operation"),
        }
    }

    #[test]
    fn return_responses_preserve_arbitrary_json_values() {
        for value in [
            json!("text"),
            json!({"key": [1, true]}),
            json!(["item", null]),
            json!(7),
            json!(false),
            Value::Null,
        ] {
            let response = json!({
                "version": 1,
                "operation": {"type": "return", "value": value.clone()},
            });
            let parsed = parse_response(
                &serde_json::to_vec(&response).unwrap(),
                Some("return"),
                "test",
            )
            .unwrap();
            assert!(
                matches!(parsed, ProtocolOutcome::Operation(ProtocolOperation::Return { value: parsed_value }) if parsed_value == value)
            );
        }
    }

    #[test]
    fn script_requests_use_one_explicit_context_shape() {
        let parameters = json!({"mode": "normal"});
        let input = json!({
            "stdin": {"path": null, "length": 0, "is_tty": true}
        });
        let engine_state = json!({"input": "needle"});
        let owner = CommandOwnerContext {
            view_ref: "core:main".to_string(),
            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                parameters.clone(),
                String::new(),
                crate::input::InputSourceIdentity::default(),
                0,
            ),
        };
        let command = command_request(
            &owner,
            "open",
            "navigate",
            &input,
            &engine_state,
            "picker",
            &json!([]),
            &json!({"ref": "core:main"}),
        );
        let items = items_request(&parameters, &input, "picker", &engine_state);
        let capture = capture_request(&parameters, &input, "picker", &engine_state);
        let returned = return_request(
            &parameters,
            &input,
            &crate::view::ViewResult::new(json!({"raw": true})),
            "picker",
            &engine_state,
        );

        for request in [&command, &items, &capture, &returned] {
            assert_eq!(request["context"]["parameters"], parameters);
            assert_eq!(request["context"]["input"], input);
            assert_eq!(request["context"]["engine"]["type"], "picker");
            assert_eq!(request["context"]["engine"]["state"], engine_state);
            assert!(request.get("view").is_none());
            assert!(request.get("parameters").is_none());
            assert!(request.get("invocation").is_none());
            assert!(request.get("engine_output").is_none());
            assert!(request.get("command").is_none());
            assert!(request.get("result").is_none());
        }
        assert_eq!(
            command["context"]["command"],
            json!({"id": "open", "type": "navigate"})
        );
        assert_eq!(command["context"]["commands"], json!([]));
        assert_eq!(command["context"]["view"]["ref"], "core:main");
        assert_eq!(returned["context"]["result"], json!({"raw": true}));
    }

    #[test]
    fn response_rejects_unknown_fields_and_trailing_documents() {
        let unknown = br#"{"version":1,"operation":{"type":"return","extra":true}}"#;
        assert!(parse_response(unknown, Some("return"), "test").is_err());
        let trailing = br#"{"version":1,"operation":{"type":"return","value":null}}{}"#;
        assert!(parse_response(trailing, Some("return"), "test").is_err());
    }

    #[test]
    fn parse_response_allows_any_operation_when_expected_is_none() {
        let run = br#"{"version":1,"operation":{"type":"run","argv":["true"]}}"#;
        let parsed = parse_response(run, None, "test").unwrap();
        assert_eq!(parsed.operation().unwrap().operation_type(), "run");
        let call = br#"{"version":1,"operation":{"type":"call","target":"view:other"}}"#;
        let parsed = parse_response(call, None, "test").unwrap();
        assert_eq!(parsed.operation().unwrap().operation_type(), "call");
    }

    #[test]
    fn parse_response_handles_error_feedback() {
        let str_err = br#"{"version":1,"error":"simple warning"}"#;
        let parsed = parse_response(str_err, Some("run"), "test").unwrap();
        assert_eq!(
            parsed,
            ProtocolOutcome::Feedback {
                message: "simple warning".to_string(),
                level: FeedbackLevel::Warning,
            }
        );

        let warning = br#"{"version":1,"error":{"message":"branch exists","level":"warning"}}"#;
        let parsed = parse_response(warning, Some("navigate"), "test").unwrap();
        assert_eq!(
            parsed,
            ProtocolOutcome::Feedback {
                message: "branch exists".to_string(),
                level: FeedbackLevel::Warning,
            }
        );

        let error = br#"{"version":1,"error":{"message":"network timeout","level":"error"}}"#;
        let parsed = parse_response(error, None, "test").unwrap();
        assert_eq!(
            parsed,
            ProtocolOutcome::Feedback {
                message: "network timeout".to_string(),
                level: FeedbackLevel::Error,
            }
        );

        let info = br#"{"version":1,"error":{"message":"already up to date","level":"info"}}"#;
        let parsed = parse_response(info, None, "test").unwrap();
        assert_eq!(
            parsed,
            ProtocolOutcome::Feedback {
                message: "already up to date".to_string(),
                level: FeedbackLevel::Info,
            }
        );
    }

    #[test]
    fn response_rejects_both_or_neither_operation_and_error() {
        let both = br#"{"version":1,"operation":{"type":"return","value":1},"error":"failed"}"#;
        let err = parse_response(both, None, "test").unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot define both operation and error")
        );

        let neither = br#"{"version":1}"#;
        let err = parse_response(neither, None, "test").unwrap_err();
        assert!(
            err.to_string()
                .contains("must define either operation or error")
        );

        let empty_msg = br#"{"version":1,"error":""}"#;
        let err = parse_response(empty_msg, None, "test").unwrap_err();
        assert!(err.to_string().contains("error message must not be empty"));
    }

    #[test]
    fn declared_handler_rejects_an_operation_type_field() {
        let handler: toml::Value = toml::from_str("type = 'return'\nvalue = 'done'\n").unwrap();
        let error = parse_declared_operation("return", &handler, "test").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must not define the operation type")
        );
    }

    #[test]
    fn run_operation_accepts_valid_timeout_and_rejects_zero() {
        let valid =
            br#"{"version":1,"operation":{"type":"run","argv":["echo","ok"],"timeout_ms":5000}}"#;
        let outcome = parse_response(valid, None, "test").unwrap();
        match outcome {
            ProtocolOutcome::Operation(ProtocolOperation::Run { timeout_ms, .. }) => {
                assert_eq!(timeout_ms, Some(5000));
            }
            _ => panic!("expected Run operation"),
        }

        let zero =
            br#"{"version":1,"operation":{"type":"run","argv":["echo","ok"],"timeout_ms":0}}"#;
        let err = parse_response(zero, None, "test").unwrap_err();
        assert!(err.to_string().contains("timeout_ms must be positive"));
    }

    #[test]
    fn presentation_show_title_requires_popup_mode() {
        let inline_show_title_false = br#"{"version":1,"operation":{"type":"navigate","target":"a:b","presentation":{"mode":"inline","show_title":false}}}"#;
        let err = parse_response(inline_show_title_false, Some("navigate"), "test").unwrap_err();
        assert!(err.to_string().contains("require popup mode"));

        let popup_show_title_false = br#"{"version":1,"operation":{"type":"navigate","target":"a:b","presentation":{"mode":"popup","show_title":false}}}"#;
        let ok = parse_response(popup_show_title_false, Some("navigate"), "test").unwrap();
        assert!(matches!(
            ok,
            ProtocolOutcome::Operation(ProtocolOperation::Navigate {
                presentation: ViewPresentation {
                    show_title: false,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn response_rejects_ambiguous_companion_operations() {
        for response in [
            br#"{"version":1,"operation":{"type":"return","value":1},"companion":{"target":"details"}}"#.as_slice(),
            br#"{"version":1,"companion":{"target":"details"},"error":"failed"}"#.as_slice(),
        ] {
            assert!(parse_response(response, None, "test").is_err());
        }
    }

    #[test]
    fn companion_operation_parsed_from_standard_and_shorthand() {
        let standard = br#"{"version":1,"operation":{"type":"companion","target":"pod_logs","query":"pod-123"}}"#;
        let parsed = parse_response(standard, None, "test").unwrap();
        assert_eq!(
            parsed,
            ProtocolOutcome::Operation(ProtocolOperation::Companion {
                target: Some("pod_logs".to_string()),
                slot: None,
                query: Some(serde_json::Value::String("pod-123".to_string())),
            })
        );

        let shorthand = br#"{"version":1,"companion":{"target":"pod_logs","query":"pod-123"}}"#;
        let parsed_shorthand = parse_response(shorthand, None, "test").unwrap();
        assert_eq!(
            parsed_shorthand,
            ProtocolOutcome::Operation(ProtocolOperation::Companion {
                target: Some("pod_logs".to_string()),
                slot: None,
                query: Some(serde_json::Value::String("pod-123".to_string())),
            })
        );
    }
}
