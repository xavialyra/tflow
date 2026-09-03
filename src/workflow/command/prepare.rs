use crate::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    CommandOwnerContext, CommandRef, NavigationMode, NavigationRequest, ReturnAdapter, ViewOutput,
    ViewOutputItem, ViewReturn,
};
use crate::config::{
    Command, CommandAction, Config, EvaluationSnapshot, InvocationScope, NavigatePayload,
    OwnerViewScope, ResolvedScriptSource, ReturnScope, RunPayload, SessionScope, normalize_key,
};
use crate::execution::PreparedProcess;
use crate::expression::EvaluationStage;
use crate::lifecycle::CancellationToken;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) enum PreparedAction {
    Navigate {
        request: NavigationRequest,
        mode: NavigationMode,
    },
    Call(CallRequest),
    Return(ViewReturn),
    EditInput {
        value: String,
        cursor: usize,
    },
    Invoke(CommandExecution),
    Execute {
        prepared: PreparedProcess,
        exit: bool,
    },
}

pub(crate) fn prepare_command_action(
    config: &Config,
    execution: CommandExecution,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let action = execution.invocation.command.action.clone();
    prepare_action(
        config,
        &action,
        execution.invocation,
        execution.context,
        None,
        true,
        cancellation,
    )
}

pub(crate) fn prepare_continuation(
    config: &Config,
    action: &CommandAction,
    origin: CommandOrigin,
    context: CommandContext,
    returned: &Value,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let command = match &origin {
        CommandOrigin::View(reference) => config
            .view(&reference.view)
            .and_then(|view| view.commands.get(&reference.id))
            .cloned(),
        CommandOrigin::Session { definition, .. } => Some((**definition).clone()),
    }
    .with_context(|| format!("continuation origin {:?} is not configured", origin.id()))?;
    prepare_action(
        config,
        action,
        CommandInvocation::from_origin(origin, command),
        context,
        Some(returned),
        false,
        cancellation,
    )
}

fn prepare_action(
    config: &Config,
    action: &CommandAction,
    invocation: CommandInvocation,
    context: CommandContext,
    returned: Option<&Value>,
    root_adapter: bool,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let owner = &context.owner;
    let owner_scope = OwnerViewScope::new(&owner.view_ref, &owner.parameters)
        .with_binding_raw(Some(&owner.binding_raw));
    let snapshot = EvaluationSnapshot::new(
        InvocationScope::new(&config.input_value),
        SessionScope::new(&context.runtime),
        Some(owner_scope),
        Some(cancellation),
    )
    .with_current(&context.current)
    .with_current_fields(context.current_fields)
    .with_return_scope(returned.map(ReturnScope::new));
    let stage = if returned.is_some() {
        EvaluationStage::Return
    } else {
        EvaluationStage::Operation
    };
    match action {
        CommandAction::Run { payload } => {
            let prepared = prepare_run_command(
                config,
                payload,
                &invocation,
                &context,
                owner,
                &snapshot,
                stage,
            )?;
            Ok(PreparedAction::Execute {
                prepared,
                exit: payload.exit,
            })
        }
        CommandAction::Navigate { payload } => {
            let (target, parameters) = evaluate_target(config, payload, &snapshot, stage)?;
            let request = match parameters {
                Some(parameters) => NavigationRequest::new(target, "").with_parameters(parameters),
                None => NavigationRequest::with_defaults(target),
            }
            .with_presentation(payload.presentation.clone());
            Ok(PreparedAction::Navigate {
                request,
                mode: if payload.replace {
                    NavigationMode::Replace
                } else {
                    NavigationMode::Push
                },
            })
        }
        CommandAction::Call { payload } => {
            let target = evaluate_value(config, &snapshot, stage, &payload.target)?
                .as_str()
                .context("call target must evaluate to a string")?
                .to_string();
            let target = config.resolve_view(&target)?;
            let parameters = payload
                .query
                .as_ref()
                .map(|value| evaluate_value(config, &snapshot, stage, value))
                .transpose()?;
            let request = match parameters {
                Some(parameters) => NavigationRequest::new(target, "").with_parameters(parameters),
                None => NavigationRequest::with_defaults(target),
            }
            .with_presentation(payload.presentation.clone());
            Ok(PreparedAction::Call(CallRequest {
                request,
                origin: invocation.origin(),
                context,
                then: payload.then.clone(),
            }))
        }
        CommandAction::Return { payload } => {
            let output = match &payload.value {
                Some(value) => ViewOutput::Value {
                    value: evaluate_value(config, &snapshot, stage, value)?,
                },
                None => current_output(&context).context(
                    "return command has no current ViewContext output; configure payload.value explicitly",
                )?,
            };
            let adapter = if root_adapter {
                Some(ReturnAdapter {
                    command: invocation
                        .view_reference()
                        .context("return action cannot originate from a session command")?
                        .clone(),
                    context: context.clone(),
                })
            } else {
                None
            };
            Ok(PreparedAction::Return(ViewReturn {
                source_view: invocation.source_view().to_string(),
                output,
                adapter,
            }))
        }
        CommandAction::EditInput { payload } => {
            let value = evaluate_value(config, &snapshot, stage, &payload.value)?
                .as_str()
                .context("edit-input value must evaluate to a string")?
                .to_string();
            let cursor = match &payload.cursor {
                Some(cursor) => evaluate_value(config, &snapshot, stage, cursor)?
                    .as_u64()
                    .context("edit-input cursor must evaluate to a non-negative integer")?
                    .try_into()
                    .context("edit-input cursor does not fit in usize")?,
                None => value.len(),
            };
            if cursor > value.len() || !value.is_char_boundary(cursor) {
                bail!("edit-input cursor {cursor} is not a UTF-8 boundary in the new value");
            }
            Ok(PreparedAction::EditInput { value, cursor })
        }
        CommandAction::Invoke { payload } => {
            let value = evaluate_value(config, &snapshot, stage, &payload.command)?;
            let reference: CommandRef = serde_json::from_value(value)
                .context("invoke command must evaluate to {view, id}")?;
            let invocation = resolve_visible_command(config, &context, &reference)?;
            Ok(PreparedAction::Invoke(CommandExecution {
                invocation,
                context,
            }))
        }
    }
}

fn current_output(context: &CommandContext) -> Option<ViewOutput> {
    let current = &context.current;
    if let Some(values) = current.as_object()
        && let Some(item_value) = values.get("item")
    {
        let item = (!item_value.is_null())
            .then(|| {
                let item = item_value.as_object()?;
                Some(ViewOutputItem {
                    text: item.get("text")?.as_str()?.to_string(),
                    value: item
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    metadata: item.get("metadata").cloned().unwrap_or(Value::Null),
                    source_view: values
                        .get("source")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("owner_view").and_then(Value::as_str))?
                        .to_string(),
                })
            })
            .flatten();
        if item.is_some() || !context.page.binding_raw.is_empty() {
            return Some(ViewOutput::Selected {
                item,
                input: values
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or(&context.page.binding_raw)
                    .to_string(),
            });
        }
        return None;
    }
    if let Some(value) = current.as_object().and_then(|values| values.get("value")) {
        return Some(ViewOutput::Value {
            value: value.clone(),
        });
    }
    (!current.is_null()).then(|| ViewOutput::Value {
        value: current.clone(),
    })
}

fn evaluate_target(
    config: &Config,
    payload: &NavigatePayload,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
) -> Result<(String, Option<Value>)> {
    let target = evaluate_value(config, snapshot, stage, &payload.target)?
        .as_str()
        .context("navigation target must evaluate to a string")?
        .to_string();
    let target = config.resolve_view(&target)?;
    let parameters = payload
        .query
        .as_ref()
        .map(|value| evaluate_value(config, snapshot, stage, value))
        .transpose()?;
    Ok((target, parameters))
}

fn evaluate_value(
    config: &Config,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
    value: &toml::Value,
) -> Result<Value> {
    config.evaluate_value(snapshot, stage, value)
}

fn evaluate_string_value(
    config: &Config,
    source: &str,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
    label: &str,
) -> Result<String> {
    evaluate_value(
        config,
        snapshot,
        stage,
        &toml::Value::String(source.to_string()),
    )?
    .as_str()
    .map(str::to_string)
    .with_context(|| format!("{label} must evaluate to a string"))
}

fn prepare_run_command(
    config: &Config,
    payload: &RunPayload,
    invocation: &CommandInvocation,
    context: &CommandContext,
    owner: &CommandOwnerContext,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
) -> Result<PreparedProcess> {
    let view = config
        .view(invocation.source_view())
        .with_context(|| format!("view {:?} disappeared", invocation.source_view()))?;
    let shell_source = payload
        .shell
        .as_deref()
        .or(view.run_shell.as_deref())
        .unwrap_or("/bin/sh");
    let shell = evaluate_string_value(config, shell_source, snapshot, stage, "command shell")?;
    let handler_value = config.evaluate_value(snapshot, stage, &payload.handler)?;
    let handler_source = ResolvedScriptSource::parse(&handler_value)
        .context("command handler must resolve to a script source")?;
    let handler_file = handler_source.command_file()?;
    let root = config
        .plugin_root(invocation.source_view())
        .with_context(|| {
            format!(
                "command {:?} has a file handler but no plugin root",
                invocation.id()
            )
        })?;
    let handler = crate::execution::read_script(root, handler_file)?;
    if shell.is_empty() {
        bail!("command shell must not be empty");
    }
    let arguments = config.evaluate_argv(payload.args.as_ref(), snapshot, stage, "command args")?;
    let item_text = current_string_field(&context.current, "text")
        .map(str::to_string)
        .unwrap_or_default();
    let value = current_string_field(&context.current, "value")
        .map(str::to_string)
        .unwrap_or_else(|| item_text.clone());
    let metadata = current_field(&context.current, "metadata")
        .map(serde_json::to_string)
        .transpose()
        .context("could not serialize current item metadata")?
        .unwrap_or_else(|| Value::Null.to_string());
    let item_view = current_string_field(&context.current, "source")
        .or_else(|| current_string_field(&context.current, "owner_view"))
        .map(str::to_string);
    let item_plugin = item_view
        .as_deref()
        .map(package_id)
        .unwrap_or("")
        .to_string();
    let source_view = invocation.source_view();
    let plugin_root = config
        .plugin_root(source_view)
        .map(|path| path.to_path_buf());
    let mut environment = vec![
        ("LAUNCHER_ITEM".to_string(), item_text),
        ("LAUNCHER_VALUE".to_string(), value.to_string()),
        ("LAUNCHER_METADATA".to_string(), metadata),
        (
            "LAUNCHER_PLUGIN".to_string(),
            package_id(source_view).to_string(),
        ),
        ("LAUNCHER_VIEW".to_string(), context.page.view_ref.clone()),
        ("LAUNCHER_VIEW_REF".to_string(), source_view.to_string()),
        (
            "LAUNCHER_ITEM_VIEW_REF".to_string(),
            item_view.unwrap_or_default(),
        ),
        ("LAUNCHER_ITEM_PLUGIN".to_string(), item_plugin),
        ("LAUNCHER_COMMAND".to_string(), invocation.id().to_string()),
        ("LAUNCHER_QUERY".to_string(), owner.binding_raw.clone()),
    ];
    if let Some(root) = &plugin_root {
        environment.push((
            "LAUNCHER_PLUGIN_DIR".to_string(),
            root.to_string_lossy().to_string(),
        ));
    }
    if let Some(path) = config
        .input_value
        .pointer("/stdin/path")
        .and_then(Value::as_str)
    {
        environment.push(("LAUNCHER_STDIN_FILE".to_string(), path.to_string()));
    }
    let mut argv = vec![shell, "-c".to_string(), handler, "tui-launcher".to_string()];
    // With `sh -c SOURCE tui-launcher ARG...`, the fixed fourth argument is
    // `$0` inside SOURCE and configured values become `$1`, `$2`, and `"$@"`.
    // They are appended as process arguments, not interpolated into SOURCE.
    argv.extend(arguments);
    Ok(PreparedProcess {
        argv,
        environment,
        current_dir: plugin_root,
    })
}

fn current_field<'a>(current: &'a Value, field: &str) -> Option<&'a Value> {
    current
        .get(field)
        .filter(|value| !value.is_null())
        .or_else(|| {
            current
                .get("item")
                .and_then(Value::as_object)
                .and_then(|item| item.get(field))
                .filter(|value| !value.is_null())
        })
}

fn current_string_field<'a>(current: &'a Value, field: &str) -> Option<&'a str> {
    current_field(current, field).and_then(Value::as_str)
}

fn package_id(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(view_ref)
}

pub(crate) fn collect_page_owner_commands(
    config: &Config,
    page_view: &str,
    owner_view: Option<&str>,
) -> Result<BTreeMap<String, Value>> {
    let mut commands = BTreeMap::new();
    if let Some(page) = config.view(page_view) {
        for (id, command) in &page.commands {
            let key = normalize_key(&command.key)
                .with_context(|| format!("invalid command key for {page_view}/{id}"))?;
            commands.insert(key, runtime_command_value(page_view, id, command)?);
        }
    }
    if let Some(owner_view) = owner_view.filter(|owner| *owner != page_view) {
        let Some(owner) = config.view(owner_view) else {
            return Ok(commands);
        };
        for (id, command) in &owner.commands {
            let key = normalize_key(&command.key)
                .with_context(|| format!("invalid command key for {owner_view}/{id}"))?;
            commands.insert(key, runtime_command_value(owner_view, id, command)?);
        }
    }
    Ok(commands)
}

pub(crate) fn resolve_visible_command(
    config: &Config,
    context: &CommandContext,
    reference: &CommandRef,
) -> Result<CommandInvocation> {
    let owner = (context.owner.view_ref != context.page.view_ref)
        .then_some(context.owner.view_ref.as_str());
    let visible = collect_page_owner_commands(config, &context.page.view_ref, owner)?;
    let is_visible = visible.values().any(|value| {
        value.get("ref").is_some_and(|value| {
            value.get("view").and_then(Value::as_str) == Some(reference.view.as_str())
                && value.get("id").and_then(Value::as_str) == Some(reference.id.as_str())
        })
    });
    if !is_visible {
        bail!(
            "command {}/{} is not available in the restored View context",
            reference.view,
            reference.id
        );
    }
    let command = config
        .view(&reference.view)
        .and_then(|view| view.commands.get(&reference.id))
        .cloned()
        .with_context(|| {
            format!(
                "command {:?} is not configured for view {:?}",
                reference.id, reference.view
            )
        })?;
    Ok(CommandInvocation::view(reference.clone(), command))
}

pub(crate) fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

pub(crate) fn runtime_command_value(owner: &str, id: &str, command: &Command) -> Result<Value> {
    let key = normalize_key(&command.key)
        .with_context(|| format!("invalid command key for {owner}/{id}"))?;
    Ok(json!({
        "ref": {"view": owner, "id": id},
        "owner": owner,
        "key": key,
        "label": command.label,
    }))
}

pub(crate) fn return_value(returned: &ViewReturn) -> Value {
    json!({
        "source": returned.source_view,
        "output": returned.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CommandScope;

    fn test_parameters(config: &Config) -> crate::parameter::ParameterSnapshot {
        let state = config.instantiate_parameters("core:default").unwrap();
        config
            .parameter_snapshot(&state, Default::default())
            .unwrap()
    }

    #[test]
    fn default_output_reads_picker_and_capture_current_shapes() {
        let config = crate::config::load_test_fixture().unwrap();
        let parameters = test_parameters(&config);
        let mut context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: parameters.clone(),
                binding_raw: String::new(),
            },
            owner: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters,
                binding_raw: String::new(),
            },
            current: serde_json::json!({"value": "captured"}),
            current_fields: &["value"],
            runtime: serde_json::json!({}),
        };
        assert!(matches!(
            current_output(&context),
            Some(ViewOutput::Value { value }) if value == serde_json::json!("captured")
        ));

        context.current = serde_json::json!({
            "item": {
                "text": "Editor",
                "value": "vim",
                "metadata": {},
                "owner_view": "apps:main"
            },
            "source": "apps:main",
            "input": ""
        });
        assert!(matches!(
            current_output(&context),
            Some(ViewOutput::Selected { item: Some(item), input })
                if item.text == "Editor"
                    && item.value.as_deref() == Some("vim")
                    && input.is_empty()
        ));

        context.current = Value::Null;
        assert!(current_output(&context).is_none());
    }

    #[test]
    fn run_command_does_not_export_runtime_log_environment() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.input_value = serde_json::json!({
            "stdin": {"path": "/tmp/tui-launcher-captured-stdin"}
        });
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            owner: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            current: Value::Null,
            current_fields: &[],
            runtime: serde_json::json!({}),
        };
        let invocation = CommandInvocation::view(
            CommandRef {
                view: "core:default".to_string(),
                id: "run".to_string(),
            },
            Command {
                key: "enter".to_string(),
                label: "Run".to_string(),
                scope: CommandScope::View,
                requires: crate::config::CommandRequirement::Input,
                passthrough: false,
                action: CommandAction::Run {
                    payload: crate::config::RunPayload {
                        handler: crate::config::ScriptSourceSpec::script_file("scripts/items.sh")
                            .as_toml_value(),
                        args: None,
                        shell: None,
                        exit: false,
                    },
                },
            },
        );
        let prepared = prepare_command_action(
            &config,
            CommandExecution {
                invocation,
                context,
            },
            &CancellationToken::new(),
        )
        .unwrap();
        let PreparedAction::Execute { prepared, .. } = prepared else {
            panic!("run action was not prepared for execution");
        };
        assert!(
            !prepared
                .environment
                .iter()
                .any(|(key, _)| key == "LAUNCHER_LOG_FILE")
        );
        assert_eq!(
            prepared
                .environment
                .iter()
                .find(|(key, _)| key == "LAUNCHER_STDIN_FILE")
                .map(|(_, value)| value.as_str()),
            Some("/tmp/tui-launcher-captured-stdin")
        );
    }

    #[test]
    fn edit_input_rejects_a_cursor_between_utf8_code_units() {
        let config = crate::config::load_test_fixture().unwrap();
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            owner: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            current: Value::Null,
            current_fields: &[],
            runtime: serde_json::json!({}),
        };
        let action = CommandAction::EditInput {
            payload: crate::config::EditInputPayload {
                value: toml::Value::String("\u{e9}".to_string()),
                cursor: Some(toml::Value::Integer(1)),
            },
        };
        assert!(
            prepare_continuation(
                &config,
                &action,
                CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context,
                &serde_json::json!({}),
                &CancellationToken::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn view_command_ids_cannot_be_misclassified_as_session_origins() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .commands
            .insert(
                "__session_local".to_string(),
                Command {
                    key: "ctrl+l".to_string(),
                    label: "Local".to_string(),
                    scope: CommandScope::View,
                    requires: crate::config::CommandRequirement::Input,
                    passthrough: false,
                    action: CommandAction::Return {
                        payload: crate::config::ReturnPayload::default(),
                    },
                },
            );
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            owner: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            current: Value::Null,
            current_fields: &[],
            runtime: serde_json::json!({}),
        };
        let action = CommandAction::EditInput {
            payload: crate::config::EditInputPayload {
                value: toml::Value::String("restored".to_string()),
                cursor: None,
            },
        };

        let prepared = prepare_continuation(
            &config,
            &action,
            CommandOrigin::View(CommandRef {
                view: "core:default".to_string(),
                id: "__session_local".to_string(),
            }),
            context,
            &serde_json::Value::Null,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(matches!(
            prepared,
            PreparedAction::EditInput { value, cursor }
                if value == "restored" && cursor == "restored".len()
        ));
    }

    #[test]
    fn opaque_command_refs_are_strict_and_revalidated_against_the_restored_context() {
        let config = crate::config::load_test_fixture().unwrap();
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            owner: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                parameters: test_parameters(&config),
                binding_raw: String::new(),
            },
            current: Value::Null,
            current_fields: &[],
            runtime: serde_json::json!({}),
        };
        let forged = CommandRef {
            view: "sys:main".to_string(),
            id: "run".to_string(),
        };
        assert!(resolve_visible_command(&config, &context, &forged).is_err());
        assert!(
            serde_json::from_value::<CommandRef>(serde_json::json!({
                "view": "core:default",
                "id": "views",
                "action": "run"
            }))
            .is_err()
        );
    }
}
