use crate::execution::{ensure_script_success, run_resolved_script_with_stdin_outcome_with_limit};
use crate::lifecycle::CancellationStatus;
use crate::view::ViewResult;
use crate::workflow::command::CommandOwnerContext;
use crate::workflow::config::{ResolvedScriptSource, ViewPresentation};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: u64 = 1;
const MAX_ITEMS_PRODUCER_STDOUT: usize = 64 * 1024 * 1024;

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
    Run {
        mode: String,
        argv: Vec<String>,
        exit: bool,
        success_message: Option<String>,
    },
}

impl ProtocolOperation {
    pub(crate) fn operation_type(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Call { .. } => "call",
            Self::Return { .. } => "return",
            Self::Run { .. } => "run",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProtocolOutcome {
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
            Self::Feedback { .. } => None,
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
struct RawResponse {
    version: u64,
    #[serde(default)]
    operation: Option<RawOperation>,
    #[serde(default)]
    error: Option<RawError>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum RawOperation {
    Navigate {
        target: String,
        #[serde(default)]
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
        #[serde(default)]
        query: Option<Value>,
        #[serde(default)]
        presentation: ViewPresentation,
    },
    Return {
        value: Value,
    },
    Run {
        mode: String,
        argv: Vec<String>,
        #[serde(default)]
        exit: bool,
        #[serde(default)]
        success_message: Option<String>,
    },
}

pub(crate) fn parse_response(
    stdout: &[u8],
    expected_operation: Option<&str>,
    source_label: &str,
) -> Result<ProtocolOutcome> {
    let response: RawResponse = serde_json::from_slice(stdout).with_context(|| {
        format!(
            "{} producer must write exactly one valid JSON protocol response",
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
                RawOperation::Run {
                    mode,
                    argv,
                    exit,
                    success_message,
                } => ProtocolOperation::Run {
                    mode,
                    argv,
                    exit,
                    success_message,
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
            anyhow::ensure!(
                presentation.mode == crate::workflow::config::ViewPresentationMode::Popup
                    || (presentation.width.is_none() && presentation.height.is_none()),
                "{} operation width and height require popup mode",
                source_label
            );
            anyhow::ensure!(
                presentation.width != Some(0) && presentation.height != Some(0),
                "{} operation width and height must be positive",
                source_label
            );
        }
        ProtocolOperation::Run { mode, argv, .. } => {
            anyhow::ensure!(
                mode == "foreground",
                "{} run operation mode must be foreground",
                source_label
            );
            anyhow::ensure!(
                !argv.is_empty(),
                "{} run operation argv must not be empty",
                source_label
            );
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
        ProtocolOutcome::Feedback { .. } => unreachable!(),
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
        Some(MAX_ITEMS_PRODUCER_STDOUT),
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
) -> ScriptResponseOutcome<String> {
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
    let result = output.result.and_then(|output| {
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
    });
    ScriptResponseOutcome {
        result,
        managed_child_reaped,
    }
}

pub(crate) fn form_request(parameters: &Value, input: &Value, state: &Value) -> Value {
    json!({"version": PROTOCOL_VERSION, "entrypoint": "form-content",
        "context": producer_context(parameters, input, "form", state)})
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

pub(crate) fn parse_preview_response(stdout: &[u8]) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        version: u64,
        preview: Value,
    }
    let response: Response = serde_json::from_slice(stdout)
        .context("picker-preview producer must write exactly one JSON response")?;
    anyhow::ensure!(
        response.version == PROTOCOL_VERSION,
        "unsupported picker-preview protocol version {}; expected 1",
        response.version
    );
    Ok(response.preview)
}

pub(crate) fn run_script_preview_response(
    owner: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    cancellation: &dyn CancellationStatus,
) -> ScriptResponseOutcome<Value> {
    let output = run_script_output(
        owner,
        "picker-preview",
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
            .and_then(|output| parse_preview_response(&output.stdout)),
    }
}

pub(crate) fn preview_request(parameters: &Value, input: &Value, state: &Value) -> Value {
    json!({"version": PROTOCOL_VERSION, "entrypoint": "picker-preview",
        "context": producer_context(parameters, input, "picker", state)})
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
    output: String,
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

fn producer_context(
    parameters: &Value,
    input: &Value,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    json!({
        "parameters": parameters,
        "input": input,
        "engine": {
            "type": engine_type,
            "state": engine_state,
        },
    })
}

pub(crate) fn command_request(
    owner: &CommandOwnerContext,
    command_id: &str,
    operation_type: &str,
    input: &Value,
    engine_state: &Value,
    engine_type: &str,
) -> Value {
    let mut context = producer_context(owner.parameters.values(), input, engine_type, engine_state);
    if let Value::Object(ref mut map) = context {
        map.insert(
            "command".to_string(),
            json!({"id": command_id, "type": operation_type}),
        );
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
        "context": producer_context(parameters, input, engine_type, engine_state),
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
        "context": producer_context(parameters, input, engine_type, engine_state),
    })
}

pub(crate) fn return_request(
    parameters: &Value,
    input: &Value,
    result: &ViewResult,
    engine_type: &str,
    engine_state: &Value,
) -> Value {
    let mut context = producer_context(parameters, input, engine_type, engine_state);
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
    fn producer_requests_use_one_explicit_context_shape() {
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
        let command = command_request(&owner, "open", "navigate", &input, &engine_state, "picker");
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
        let run =
            br#"{"version":1,"operation":{"type":"run","mode":"foreground","argv":["true"]}}"#;
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
}
