use crate::execution::PreparedProcess;
use crate::lifecycle::CancellationToken;
use crate::workflow::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin, CommandRef,
    NavigationMode, NavigationRequest, ViewOutput, ViewOutputItem, ViewReturn,
};
use crate::workflow::config::{
    Command, CommandAction, CompiledConfig, ProducerKind, normalize_key,
};
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
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    execution: CommandExecution,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let action = execution.invocation.command.action.clone();
    prepare_action(
        config,
        invocation,
        &action,
        execution.invocation,
        execution.context,
        cancellation,
    )
}

fn prepare_action(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    action: &CommandAction,
    command_invocation: CommandInvocation,
    context: CommandContext,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    match action {
        CommandAction::OpenCommands => {
            prepare_builtin_commands(config, command_invocation, context)
        }
        _ => prepare_producer_action(
            config,
            invocation,
            action,
            command_invocation,
            context,
            cancellation,
        ),
    }
}

fn prepare_producer_action(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    action: &CommandAction,
    command_invocation: CommandInvocation,
    context: CommandContext,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let producer = action
        .producer()
        .context("producer action is missing its producer kind")?;
    let handler = producer_handler(action)?;
    let source_label = format!(
        "{}.commands.{}",
        command_invocation.source_view(),
        command_invocation.id()
    );
    let operation = match producer {
        ProducerKind::Declared => crate::protocol::parse_declared_operation(
            action.operation_type(),
            handler,
            &source_label,
        )?,
        ProducerKind::Script => {
            let root = command_invocation
                .view_reference()
                .and_then(|_| config.workflow_root(command_invocation.source_view()));
            let source = crate::workflow::config::parse_producer_script_handler(handler, root)?;
            let command = command_invocation
                .view_reference()
                .cloned()
                .unwrap_or_else(|| CommandRef {
                    view: "session".to_string(),
                    id: command_invocation.id().to_string(),
                });
            let request = crate::protocol::command_request(
                &context.owner,
                &context.page,
                &command,
                action.operation_type(),
                invocation.input_value(),
                &context.current,
            );
            crate::protocol::run_script_response(
                command_invocation.source_view(),
                &source_label,
                root,
                &source,
                &request,
                action.operation_type(),
                cancellation,
            )?
        }
    };
    prepare_protocol_operation(
        config,
        invocation,
        match action {
            CommandAction::Call {
                return_processor, ..
            } => return_processor.clone(),
            _ => None,
        },
        command_invocation,
        context,
        operation,
        cancellation,
    )
}

pub(crate) fn prepare_return_processor(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    processor: &crate::workflow::config::ReturnProcessor,
    origin: CommandOrigin,
    context: CommandContext,
    caller: &crate::view::ViewContext,
    result: &crate::view::ViewResult,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let command = match &origin {
        CommandOrigin::View(reference) => config
            .view(&reference.view)
            .and_then(|view| view.commands.get(&reference.id))
            .cloned(),
        CommandOrigin::Session { definition, .. } => Some((**definition).clone()),
    }
    .context("return processor origin is not configured")?;
    let command_invocation = CommandInvocation::from_origin(origin.clone(), command);
    let source_label = format!(
        "{}.commands.{}.return_processor",
        command_invocation.source_view(),
        command_invocation.id()
    );
    let operation = match processor.producer {
        ProducerKind::Declared => crate::protocol::parse_declared_operation(
            &processor.operation,
            &processor.handler,
            &source_label,
        )?,
        ProducerKind::Script => {
            let root = command_invocation
                .view_reference()
                .and_then(|_| config.workflow_root(command_invocation.source_view()));
            let source =
                crate::workflow::config::parse_producer_script_handler(&processor.handler, root)?;
            let command = command_invocation
                .view_reference()
                .cloned()
                .unwrap_or_else(|| CommandRef {
                    view: "session".to_string(),
                    id: command_invocation.id().to_string(),
                });
            let request = crate::protocol::return_request(
                &caller.location,
                &command,
                context.owner.parameters.values(),
                invocation.input_value(),
                result,
            )?;
            crate::protocol::run_script_response(
                command_invocation.source_view(),
                &source_label,
                root,
                &source,
                &request,
                &processor.operation,
                cancellation,
            )?
        }
    };
    prepare_protocol_operation(
        config,
        invocation,
        None,
        command_invocation,
        context,
        operation,
        cancellation,
    )
}

fn producer_handler(action: &CommandAction) -> Result<&toml::Value> {
    match action {
        CommandAction::Run { handler, .. }
        | CommandAction::Navigate { handler, .. }
        | CommandAction::Call { handler, .. }
        | CommandAction::Return { handler, .. }
        | CommandAction::EditInput { handler, .. }
        | CommandAction::Invoke { handler, .. } => Ok(handler),
        CommandAction::OpenCommands => bail!("built-in commands do not have a producer handler"),
    }
}

fn prepare_protocol_operation(
    config: &CompiledConfig,
    _invocation: &crate::workflow::InvocationContext,
    return_processor: Option<crate::workflow::config::ReturnProcessor>,
    command_invocation: CommandInvocation,
    context: CommandContext,
    operation: crate::protocol::ProtocolOperation,
    _cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    match operation {
        crate::protocol::ProtocolOperation::Navigate {
            target,
            query,
            presentation,
            replace,
        } => {
            let target = config.resolve_view(&target)?;
            let request = match query {
                Some(query) => NavigationRequest::new(target, "").with_parameters(query),
                None => NavigationRequest::with_defaults(target),
            }
            .with_presentation(presentation);
            Ok(PreparedAction::Navigate {
                request,
                mode: if replace {
                    NavigationMode::Replace
                } else {
                    NavigationMode::Push
                },
            })
        }
        crate::protocol::ProtocolOperation::Call {
            target,
            query,
            presentation,
        } => {
            let target = config.resolve_view(&target)?;
            let request = match query {
                Some(query) => NavigationRequest::new(target, "").with_parameters(query),
                None => NavigationRequest::with_defaults(target),
            }
            .with_presentation(presentation);
            Ok(PreparedAction::Call(CallRequest {
                request,
                origin: command_invocation.origin(),
                context,
                return_processor,
                invoke_selected: false,
            }))
        }
        crate::protocol::ProtocolOperation::Return {
            value,
            value_present,
        } => {
            let output = if value_present {
                ViewOutput::Value {
                    value: value.unwrap_or(Value::Null),
                }
            } else {
                current_output(&context).context(
                    "return producer has no current ViewContext output; return a value explicitly",
                )?
            };
            Ok(PreparedAction::Return(ViewReturn { output }))
        }
        crate::protocol::ProtocolOperation::Run { argv, exit, .. } => {
            let prepared = prepared_direct_process(config, command_invocation.source_view(), argv)?;
            Ok(PreparedAction::Execute { prepared, exit })
        }
        crate::protocol::ProtocolOperation::EditInput { value, cursor } => {
            let cursor = cursor
                .map(usize::try_from)
                .transpose()
                .context("edit-input cursor does not fit in usize")?
                .unwrap_or(value.len());
            if cursor > value.len() || !value.is_char_boundary(cursor) {
                bail!("edit-input cursor {cursor} is not a UTF-8 boundary in the new value");
            }
            Ok(PreparedAction::EditInput { value, cursor })
        }
        crate::protocol::ProtocolOperation::Invoke { command } => {
            let invocation = resolve_visible_command(config, &context, &command)?;
            Ok(PreparedAction::Invoke(CommandExecution {
                invocation,
                context,
            }))
        }
    }
}

fn prepared_direct_process(
    config: &CompiledConfig,
    source_view: &str,
    argv: Vec<String>,
) -> Result<PreparedProcess> {
    anyhow::ensure!(!argv.is_empty(), "run operation argv must not be empty");
    let mut environment = Vec::new();
    if let Some(root) = config.workflow_root(source_view) {
        environment.push((
            "WORKFLOW_DIR".to_string(),
            root.to_string_lossy().into_owned(),
        ));
    }
    Ok(PreparedProcess {
        argv,
        environment,
        current_dir: None,
    })
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
                    source_view: item.get("owner_view").and_then(Value::as_str)?.to_string(),
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

fn prepare_builtin_commands(
    config: &CompiledConfig,
    command_invocation: CommandInvocation,
    context: CommandContext,
) -> Result<PreparedAction> {
    let target = config.resolve_view("selectors:commands")?;
    let owner_view = (context.owner.view_ref != context.page.view_ref)
        .then_some(context.owner.view_ref.as_str());
    let commands = collect_available_commands(config, &context.page.view_ref, owner_view, true)?
        .into_values()
        .collect::<Vec<_>>();
    let request = NavigationRequest::new(target, "")
        .with_parameters(json!({"commands": commands}))
        .with_presentation(crate::workflow::config::ViewPresentation {
            mode: crate::workflow::config::ViewPresentationMode::Popup,
            width: Some(72),
            height: Some(16),
        });
    Ok(PreparedAction::Call(CallRequest {
        request,
        origin: command_invocation.origin(),
        context,
        return_processor: None,
        invoke_selected: true,
    }))
}

pub(crate) fn collect_available_commands(
    config: &CompiledConfig,
    page_view: &str,
    owner_view: Option<&str>,
    include_globals: bool,
) -> Result<BTreeMap<String, Value>> {
    let mut commands = BTreeMap::new();
    if include_globals {
        for (id, command) in config.session_commands() {
            if id != "commands" {
                commands.insert(
                    format!("session/{id}"),
                    runtime_command_value("session", &id, &command)?,
                );
            }
        }
    }
    if let Some(page) = config.view(page_view) {
        for (id, command) in &page.commands {
            commands.insert(
                format!("{page_view}/{id}"),
                runtime_command_value(page_view, id, command)?,
            );
        }
    }
    if let Some(owner_view) = owner_view.filter(|owner| *owner != page_view) {
        let Some(owner) = config.view(owner_view) else {
            return Ok(commands);
        };
        for (id, command) in &owner.commands {
            commands.insert(
                format!("{owner_view}/{id}"),
                runtime_command_value(owner_view, id, command)?,
            );
        }
    }
    Ok(commands)
}

pub(crate) fn collect_page_owner_commands(
    config: &CompiledConfig,
    page_view: &str,
    owner_view: Option<&str>,
) -> Result<BTreeMap<String, Value>> {
    collect_available_commands(config, page_view, owner_view, false)
}

pub(crate) fn resolve_visible_command(
    config: &CompiledConfig,
    context: &CommandContext,
    reference: &CommandRef,
) -> Result<CommandInvocation> {
    let owner = (context.owner.view_ref != context.page.view_ref)
        .then_some(context.owner.view_ref.as_str());
    let visible = collect_available_commands(config, &context.page.view_ref, owner, true)?;
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
    if reference.view == "session" {
        if let Some(cmd) = config.session_command(&reference.id) {
            return Ok(CommandInvocation::session_command(
                &context.page.view_ref,
                &reference.id,
                cmd,
            ));
        }
    }
    if let Some(command) = config
        .view(&reference.view)
        .and_then(|view| view.commands.get(&reference.id))
        .cloned()
    {
        return Ok(CommandInvocation::view(reference.clone(), command));
    }
    if reference.view == context.page.view_ref {
        if let Some(cmd) = config.session_command(&reference.id) {
            return Ok(CommandInvocation::session_command(
                &context.page.view_ref,
                &reference.id,
                cmd,
            ));
        }
    }
    bail!(
        "command {:?} is not configured for view {:?}",
        reference.id,
        reference.view
    );
}

pub(crate) fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

pub(crate) fn runtime_command_value(owner: &str, id: &str, command: &Command) -> Result<Value> {
    let key = match &command.key {
        Some(raw) => {
            normalize_key(raw).with_context(|| format!("invalid command key for {owner}/{id}"))?
        }
        None => String::new(),
    };
    Ok(json!({
        "ref": {"view": owner, "id": id},
        "owner": owner,
        "key": key,
        "label": command.label,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(current: Value, raw_input: &str) -> CommandContext {
        let owner = crate::workflow::command::CommandOwnerContext {
            view_ref: "core:default".to_string(),
            parameters: crate::workflow::parameter::ParameterSnapshot::from_parts(
                Value::Null,
                String::new(),
                crate::input::InputSourceIdentity::default(),
                0,
            ),
            binding_raw: raw_input.to_string(),
        };
        CommandContext {
            page: owner.clone(),
            owner,
            current,
        }
    }

    #[test]
    fn current_output_converts_picker_publication() {
        let output = current_output(&context(
            serde_json::json!({
                "item": {
                    "text": "Item",
                    "value": "value",
                    "metadata": {"kind": "test"},
                    "owner_view": "core:items"
                },
                "input": "typed"
            }),
            "fallback",
        ))
        .unwrap();
        assert_eq!(
            output,
            ViewOutput::Selected {
                item: Some(ViewOutputItem {
                    text: "Item".to_string(),
                    value: Some("value".to_string()),
                    metadata: serde_json::json!({"kind": "test"}),
                    source_view: "core:items".to_string(),
                }),
                input: "typed".to_string(),
            }
        );
    }

    #[test]
    fn current_output_uses_binding_input_for_an_empty_selection() {
        let output = current_output(&context(serde_json::json!({"item": null}), "typed")).unwrap();
        assert_eq!(
            output,
            ViewOutput::Selected {
                item: None,
                input: "typed".to_string(),
            }
        );
    }

    #[test]
    fn command_binding_order_keeps_enter_first() {
        assert_eq!(
            compare_bindings("enter", "ctrl+k"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_bindings("ctrl+k", "enter"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_bindings("a", "b"), std::cmp::Ordering::Less);
    }

    #[test]
    fn fixture_commands_have_serializable_references() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let commands = collect_available_commands(&config, "dmenu:main", None, true).unwrap();
        let (key, value) = commands.iter().next().expect("fixture exposes commands");
        let (view, id) = key.split_once('/').expect("command key has an owner");
        assert_eq!(value["ref"]["view"], view);
        assert_eq!(value["ref"]["id"], id);
    }
}
