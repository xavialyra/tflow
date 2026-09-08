use crate::execution::{ensure_script_success, run_resolved_script_with_stdin_outcome_with_limit};
use crate::lifecycle::CancellationStatus;
use crate::view::ViewResult;
use crate::workflow::command::{CommandOwnerContext, CommandRef};
use crate::workflow::config::{ResolvedScriptSource, ViewPresentation};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: u64 = 1;
const MAX_ITEMS_PRODUCER_STDOUT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) enum ProtocolOperation {
    Navigate {
        target: String,
        query: Option<Value>,
        presentation: ViewPresentation,
        replace: bool,
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
    },
    EditInput {
        value: String,
        cursor: Option<u64>,
    },
    Invoke {
        command: CommandRef,
    },
}

impl ProtocolOperation {
    pub(crate) fn operation_type(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Call { .. } => "call",
            Self::Return { .. } => "return",
            Self::Run { .. } => "run",
            Self::EditInput { .. } => "edit-input",
            Self::Invoke { .. } => "invoke",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResponse {
    version: u64,
    operation: RawOperation,
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
    },
    EditInput {
        value: String,
        #[serde(default)]
        cursor: Option<u64>,
    },
    Invoke {
        command: CommandRef,
    },
}

pub(crate) fn parse_response(
    stdout: &[u8],
    expected_operation: &str,
    source_label: &str,
) -> Result<ProtocolOperation> {
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
    let operation = match response.operation {
        RawOperation::Navigate {
            target,
            query,
            presentation,
            replace,
        } => ProtocolOperation::Navigate {
            target,
            query,
            presentation,
            replace,
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
        RawOperation::Run { mode, argv, exit } => ProtocolOperation::Run { mode, argv, exit },
        RawOperation::EditInput { value, cursor } => ProtocolOperation::EditInput { value, cursor },
        RawOperation::Invoke { command } => ProtocolOperation::Invoke { command },
    };
    if operation.operation_type() != expected_operation {
        bail!(
            "{} producer returned operation {:?}, expected {:?}",
            source_label,
            operation.operation_type(),
            expected_operation
        );
    }
    validate_operation(&operation, source_label)?;
    Ok(operation)
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
        ProtocolOperation::EditInput { value, cursor } => {
            if let Some(cursor) = cursor {
                let cursor =
                    usize::try_from(*cursor).context("edit-input cursor does not fit in usize")?;
                anyhow::ensure!(
                    cursor <= value.len() && value.is_char_boundary(cursor),
                    "{} edit-input cursor is not a UTF-8 boundary in the new value",
                    source_label
                );
            }
        }
        ProtocolOperation::Return { .. } | ProtocolOperation::Invoke { .. } => {}
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
    parse_response(&bytes, operation_type, source_label)
}

pub(crate) fn run_script_response(
    source_view: &str,
    source_label: &str,
    root: Option<&std::path::Path>,
    source: &ResolvedScriptSource,
    request: &Value,
    expected_operation: &str,
    cancellation: &dyn CancellationStatus,
) -> Result<ProtocolOperation> {
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
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "command",
        "command": {"id": command_id, "type": operation_type},
        "context": producer_context(
            owner.parameters.values(),
            input,
            engine_type,
            engine_state,
        ),
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
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "return",
        "context": producer_context(parameters, input, engine_type, engine_state),
        "result": result.value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_response_requires_a_value_but_accepts_null() {
        let missing = br#"{"version":1,"operation":{"type":"return"}}"#;
        let null = br#"{"version":1,"operation":{"type":"return","value":null}}"#;
        assert!(parse_response(missing, "return", "test").is_err());
        let null = parse_response(null, "return", "test").unwrap();
        assert!(matches!(
            null,
            ProtocolOperation::Return { value: Value::Null }
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
            let parsed =
                parse_response(&serde_json::to_vec(&response).unwrap(), "return", "test").unwrap();
            assert!(
                matches!(parsed, ProtocolOperation::Return { value: parsed_value } if parsed_value == value)
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
        let expected_context = json!({
            "parameters": parameters,
            "input": input,
            "engine": {"type": "picker", "state": engine_state},
        });
        let owner = CommandOwnerContext {
            view_ref: "core:main".to_string(),
            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                parameters.clone(),
                String::new(),
                crate::input::InputSourceIdentity::default(),
                0,
            ),
            binding_raw: String::new(),
        };
        let command = command_request(&owner, "open", "navigate", &input, &engine_state, "picker");
        let items = items_request(&parameters, &input, "picker", &engine_state);
        let capture = capture_request(&parameters, &input, "picker", &engine_state);
        let returned = return_request(
            &parameters,
            &input,
            &crate::view::ViewResult {
                value: json!({"raw": true}),
            },
            "picker",
            &engine_state,
        );

        for request in [command, items, capture, returned.clone()] {
            assert_eq!(request["context"], expected_context);
            assert!(request.get("view").is_none());
            assert!(request.get("parameters").is_none());
            assert!(request.get("invocation").is_none());
            assert!(request.get("engine_output").is_none());
        }
        assert_eq!(returned["result"], json!({"raw": true}));
    }

    #[test]
    fn response_rejects_unknown_fields_and_trailing_documents() {
        let unknown = br#"{"version":1,"operation":{"type":"return","extra":true}}"#;
        assert!(parse_response(unknown, "return", "test").is_err());
        let trailing = br#"{"version":1,"operation":{"type":"return","value":null}}{}"#;
        assert!(parse_response(trailing, "return", "test").is_err());
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
