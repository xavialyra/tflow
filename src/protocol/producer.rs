use crate::execution::{ensure_script_success, run_resolved_script_with_stdin_outcome_with_limit};
use crate::lifecycle::CancellationStatus;
use crate::view::{ViewLocation, ViewResult};
use crate::workflow::command::{CommandOwnerContext, CommandRef, ViewOutput};
use crate::workflow::config::{ResolvedScriptSource, ViewPresentation};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::Deserializer;
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
        value: Option<Value>,
        value_present: bool,
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
        #[serde(default, deserialize_with = "deserialize_present_value")]
        value: Option<Value>,
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

fn deserialize_present_value<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Value::deserialize(deserializer)?))
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
        RawOperation::Return { value } => ProtocolOperation::Return {
            value_present: value.is_some(),
            value,
        },
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

pub(crate) fn command_request(
    owner: &CommandOwnerContext,
    page: &CommandOwnerContext,
    command: &CommandRef,
    operation_type: &str,
    invocation: &Value,
    current: &Value,
) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "command",
        "view": owner.view_ref,
        "mounted_view": page.view_ref,
        "command": {"view": command.view, "id": command.id, "type": operation_type},
        "parameters": owner.parameters.values(),
        "invocation": invocation,
        "engine_output": engine_output(current, &page.binding_raw),
    })
}

pub(crate) fn items_request(
    view: &str,
    parameters: &Value,
    invocation: &Value,
    input: &Value,
) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "picker-items",
        "view": view,
        "parameters": parameters,
        "invocation": invocation,
        "request": {"input": input},
    })
}

pub(crate) fn capture_request(view: &str, parameters: &Value, invocation: &Value) -> Value {
    json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "capture-output",
        "view": view,
        "parameters": parameters,
        "invocation": invocation,
    })
}

pub(crate) fn return_request(
    caller: &ViewLocation,
    command: &CommandRef,
    parameters: &Value,
    invocation: &Value,
    result: &ViewResult,
) -> Result<Value> {
    Ok(json!({
        "version": PROTOCOL_VERSION,
        "entrypoint": "return",
        "view": caller.target,
        "parameters": parameters,
        "invocation": invocation,
        "caller": {"view": command.view, "command": command.id},
        "result": return_result(result)?,
    }))
}

fn engine_output(current: &Value, input: &str) -> Value {
    let Some(fields) = current.as_object() else {
        return json!({"kind": "value", "value": current});
    };
    if let Some(item) = fields.get("item") {
        let selected_item = if item.is_null() {
            Value::Null
        } else {
            let mut selected = item.as_object().cloned().unwrap_or_default();
            let source_view = selected
                .get("owner_view")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            selected.remove("owner_view");
            selected.insert("source_view".to_string(), Value::String(source_view));
            Value::Object(selected)
        };
        return json!({
            "kind": "picker",
            "input": fields.get("input").and_then(Value::as_str).unwrap_or(input),
            "selected_item": selected_item,
        });
    }
    if let Some(value) = fields.get("value") {
        return json!({"kind": "capture", "output": value});
    }
    json!({"kind": "value", "value": current})
}

fn return_result(result: &ViewResult) -> Result<Value> {
    let output: ViewOutput = serde_json::from_value(result.value.clone())
        .context("called View returned an invalid command output")?;
    Ok(match output {
        ViewOutput::Selected { item, input } => {
            json!({"kind": "selected", "input": input, "item": item})
        }
        ViewOutput::Value { value } => json!({"kind": "value", "value": value}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_response_distinguishes_missing_and_null_values() {
        let missing = br#"{"version":1,"operation":{"type":"return"}}"#;
        let null = br#"{"version":1,"operation":{"type":"return","value":null}}"#;
        let missing = parse_response(missing, "return", "test").unwrap();
        let null = parse_response(null, "return", "test").unwrap();
        assert!(matches!(
            missing,
            ProtocolOperation::Return {
                value_present: false,
                ..
            }
        ));
        assert!(matches!(
            null,
            ProtocolOperation::Return {
                value_present: true,
                value: Some(Value::Null)
            }
        ));
    }

    #[test]
    fn response_rejects_unknown_fields_and_trailing_documents() {
        let unknown = br#"{"version":1,"operation":{"type":"return","extra":true}}"#;
        assert!(parse_response(unknown, "return", "test").is_err());
        let trailing = br#"{"version":1,"operation":{"type":"return"}}{}"#;
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
