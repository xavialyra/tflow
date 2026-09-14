use crate::execution::PreparedProcess;
use crate::lifecycle::CancellationToken;
use crate::workflow::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin, CommandRef,
    NavigationMode, NavigationRequest,
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
    Return {
        kind: Option<String>,
        value: Value,
    },
    EditInput {
        value: String,
        cursor: usize,
    },
    Invoke(CommandExecution),
    Execute {
        prepared: PreparedProcess,
        exit: bool,
        success_message: Option<String>,
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
        CommandAction::OpenParameters => {
            prepare_builtin_parameters(config, command_invocation, context)
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
            let request = crate::protocol::command_request(
                &context.owner,
                command_invocation.id(),
                action.operation_type(),
                invocation.input_value(),
                &context.current,
                &context.engine_type,
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
    _caller: &crate::view::ViewContext,
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
            let request = crate::protocol::return_request(
                context.owner.parameters.values(),
                invocation.input_value(),
                result,
                &context.engine_type,
                &context.current,
            );
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
        CommandAction::OpenCommands | CommandAction::OpenParameters => {
            bail!("built-in actions do not have a producer handler")
        }
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
            }))
        }
        crate::protocol::ProtocolOperation::Return { kind, value } => {
            Ok(PreparedAction::Return { kind, value })
        }
        crate::protocol::ProtocolOperation::Run {
            argv,
            exit,
            success_message,
            ..
        } => {
            let prepared = prepared_direct_process(config, command_invocation.source_view(), argv)?;
            Ok(PreparedAction::Execute {
                prepared,
                exit,
                success_message,
            })
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

fn prepare_builtin_commands(
    config: &CompiledConfig,
    command_invocation: CommandInvocation,
    context: CommandContext,
) -> Result<PreparedAction> {
    let target = config.resolve_view("__selectors:commands")?;
    let commands = collect_available_commands(config, &context.page.view_ref, true)?
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
    }))
}

fn prepare_builtin_parameters(
    config: &CompiledConfig,
    command_invocation: CommandInvocation,
    context: CommandContext,
) -> Result<PreparedAction> {
    let target = config.resolve_view("__selectors:form")?;
    let payload = serde_json::to_string(&json!({
        "target": context.page.view_ref,
        "query": config.query_definition(&context.page.view_ref)?,
        "values": context.page.parameters.values().clone(),
    }))
    .context("could not serialize parameter form payload")?;
    let request = NavigationRequest::with_defaults(target)
        .with_parameters(json!({"payload": payload}))
        .with_presentation(crate::workflow::config::ViewPresentation {
            mode: crate::workflow::config::ViewPresentationMode::Popup,
            width: Some(72),
            height: Some(20),
        });
    Ok(PreparedAction::Call(CallRequest {
        request,
        origin: command_invocation.origin(),
        context,
        return_processor: None,
    }))
}

pub(crate) fn collect_available_commands(
    config: &CompiledConfig,
    page_view: &str,
    include_globals: bool,
) -> Result<BTreeMap<String, Value>> {
    let mut commands = BTreeMap::new();
    if include_globals {
        for (id, command) in config.session_commands() {
            if id != "commands" && id != "parameters" {
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
    Ok(commands)
}

pub(crate) fn resolve_visible_command(
    config: &CompiledConfig,
    context: &CommandContext,
    reference: &CommandRef,
) -> Result<CommandInvocation> {
    let visible = collect_available_commands(config, &context.page.view_ref, true)?;
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

#[cfg(test)]
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
        let commands = collect_available_commands(&config, "dmenu:main", true).unwrap();
        let (key, value) = commands.iter().next().expect("fixture exposes commands");
        let (view, id) = key.split_once('/').expect("command key has an owner");
        assert_eq!(value["ref"]["view"], view);
        assert_eq!(value["ref"]["id"], id);
    }
}
