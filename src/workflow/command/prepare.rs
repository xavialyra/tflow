use crate::execution::PreparedProcess;
use crate::identity::ENV_WORKFLOW_DIR;
use crate::lifecycle::CancellationToken;
use crate::workflow::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    NavigationMode, NavigationRequest,
};
use crate::workflow::config::{
    Command, CommandAction, CompiledConfig, ProducerKind, normalize_key,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) enum PreparedAction {
    Navigate {
        request: NavigationRequest,
        mode: NavigationMode,
        clear_input: bool,
    },
    Call(Box<CallRequest>),
    Return {
        value: Value,
    },
    Execute {
        prepared: PreparedProcess,
        exit: bool,
        success_message: Option<String>,
    },
    Feedback {
        message: String,
        level: crate::protocol::FeedbackLevel,
    },
    Noop,
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
    let outcome = match producer {
        ProducerKind::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                action.operation_type(),
                handler,
                &source_label,
            )?;
            crate::protocol::ProtocolOutcome::Operation(operation)
        }
        ProducerKind::Script => {
            let root = command_root(config, &command_invocation);
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
                (producer == ProducerKind::Declared).then_some(action.operation_type()),
                cancellation,
            )?
        }
    };
    match outcome {
        crate::protocol::ProtocolOutcome::Noop => Ok(PreparedAction::Noop),
        crate::protocol::ProtocolOutcome::Operation(operation) => prepare_protocol_operation(
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
        ),
        crate::protocol::ProtocolOutcome::Feedback { message, level } => {
            Ok(PreparedAction::Feedback { message, level })
        }
    }
}

#[allow(clippy::too_many_arguments)]
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
        CommandOrigin::View(reference) => {
            let member_id = crate::workflow::config::package_id(&reference.view);
            config.find_command(member_id, &reference.id).cloned()
        }
        CommandOrigin::Session { definition, .. } => Some((**definition).clone()),
    }
    .context("return processor origin is not configured")?;
    let command_invocation = CommandInvocation::from_origin(origin.clone(), command);
    let source_label = format!(
        "{}.commands.{}.return_processor",
        command_invocation.source_view(),
        command_invocation.id()
    );
    let outcome = match processor.producer {
        ProducerKind::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                processor
                    .operation
                    .as_deref()
                    .context("declared return processor requires type")?,
                &processor.handler,
                &source_label,
            )?;
            crate::protocol::ProtocolOutcome::Operation(operation)
        }
        ProducerKind::Script => {
            let root = command_root(config, &command_invocation);
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
                (processor.producer == ProducerKind::Declared)
                    .then_some(processor.operation.as_deref().unwrap_or("")),
                cancellation,
            )?
        }
    };
    match outcome {
        crate::protocol::ProtocolOutcome::Noop => Ok(PreparedAction::Noop),
        crate::protocol::ProtocolOutcome::Operation(operation) => prepare_protocol_operation(
            config,
            invocation,
            None,
            command_invocation,
            context,
            operation,
            cancellation,
        ),
        crate::protocol::ProtocolOutcome::Feedback { message, level } => {
            Ok(PreparedAction::Feedback { message, level })
        }
    }
}

fn producer_handler(action: &CommandAction) -> Result<&toml::Value> {
    match action {
        CommandAction::Run { handler, .. }
        | CommandAction::Navigate { handler, .. }
        | CommandAction::Call { handler, .. }
        | CommandAction::Return { handler, .. } => Ok(handler),
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
            clear_input,
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
                clear_input,
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
            Ok(PreparedAction::Call(Box::new(CallRequest {
                request,
                origin: command_invocation.origin(),
                context,
                return_processor,
            })))
        }
        crate::protocol::ProtocolOperation::Return { value } => {
            Ok(PreparedAction::Return { value })
        }
        crate::protocol::ProtocolOperation::Run {
            argv,
            exit,
            success_message,
            ..
        } => {
            let root = command_root(config, &command_invocation);
            let prepared = prepared_direct_process(root, argv)?;
            Ok(PreparedAction::Execute {
                prepared,
                exit,
                success_message,
            })
        }
    }
}

fn command_root<'a>(
    config: &'a CompiledConfig,
    command_invocation: &CommandInvocation,
) -> Option<&'a Path> {
    config
        .workflow_root(command_invocation.id())
        .or_else(|| config.workflow_root(command_invocation.source_view()))
}

fn prepared_direct_process(root: Option<&Path>, argv: Vec<String>) -> Result<PreparedProcess> {
    anyhow::ensure!(!argv.is_empty(), "run operation argv must not be empty");
    let mut environment = Vec::new();
    if let Some(root) = root {
        environment.push((
            ENV_WORKFLOW_DIR.to_string(),
            root.to_string_lossy().into_owned(),
        ));
    }
    Ok(PreparedProcess {
        argv,
        environment,
        current_dir: None,
    })
}

pub(crate) const COMMANDS_POPUP_WIDTH: u16 = 50;
pub(crate) const COMMANDS_POPUP_HEIGHT: u16 = 10;
pub(crate) const QUERY_POPUP_WIDTH: u16 = 54;
pub(crate) const QUERY_POPUP_HEIGHT: u16 = 12;

pub(crate) fn is_commands_view(view: &str, target: &str) -> bool {
    view == target
        || view == "__commands:main"
        || crate::workflow::config::package_id(view) == "__commands"
}

pub(crate) fn is_query_view(view: &str, target: &str) -> bool {
    view == target
        || view == "__query:main"
        || view == "__form:main"
        || crate::workflow::config::package_id(view) == "__query"
        || crate::workflow::config::package_id(view) == "__form"
}

fn prepare_builtin_commands(
    config: &CompiledConfig,
    command_invocation: CommandInvocation,
    context: CommandContext,
) -> Result<PreparedAction> {
    let target = config.resolve_view("__commands:main")?;
    if is_commands_view(&context.page.view_ref, &target) {
        return Ok(PreparedAction::Noop);
    }
    let commands = collect_available_commands(config, &context.page.view_ref, true)?
        .into_values()
        .collect::<Vec<_>>();
    let request = NavigationRequest::new(target, "")
        .with_parameters(json!({"commands": commands}))
        .with_presentation(
            crate::workflow::config::ViewPresentation::popup(
                COMMANDS_POPUP_WIDTH,
                COMMANDS_POPUP_HEIGHT,
            )
            .with_anchor(crate::workflow::config::PopupAnchor::BottomRight),
        );
    Ok(PreparedAction::Call(Box::new(CallRequest {
        request,
        origin: command_invocation.origin(),
        context,
        return_processor: None,
    })))
}

fn prepare_builtin_parameters(
    config: &CompiledConfig,
    command_invocation: CommandInvocation,
    context: CommandContext,
) -> Result<PreparedAction> {
    let target = config
        .resolve_view("__query:main")
        .or_else(|_| config.resolve_view("__form:main"))?;
    if is_query_view(&context.page.view_ref, &target) {
        return Ok(PreparedAction::Noop);
    }
    let payload = serde_json::to_string(&json!({
        "target": context.page.view_ref,
        "query": config.query_definition(&context.page.view_ref)?,
        "values": context.page.parameters.values().clone(),
    }))
    .context("could not serialize parameter form payload")?;
    let request = NavigationRequest::with_defaults(target)
        .with_parameters(json!({"payload": payload}))
        .with_presentation(crate::workflow::config::ViewPresentation::popup(
            QUERY_POPUP_WIDTH,
            QUERY_POPUP_HEIGHT,
        ));
    Ok(PreparedAction::Call(Box::new(CallRequest {
        request,
        origin: command_invocation.origin(),
        context,
        return_processor: None,
    })))
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
    let member_id = crate::workflow::config::package_id(page_view);
    for (id, command) in config.workflow_commands(member_id) {
        let local_id = id.strip_prefix(&format!("{member_id}:")).unwrap_or(&id);
        commands.insert(
            format!("{page_view}/{local_id}"),
            runtime_command_value(page_view, local_id, &command)?,
        );
    }
    Ok(commands)
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

    #[test]
    fn is_commands_view_identifies_builtin_command_view() {
        assert!(is_commands_view("__commands:main", "__commands:main"));
        assert!(is_commands_view("__commands:detail", "__commands:main"));
        assert!(!is_commands_view("core:default", "__commands:main"));
        assert!(!is_commands_view("sys:main", "__commands:main"));
    }

    #[test]
    fn prepare_builtin_commands_returns_noop_when_already_in_commands_view() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let invocation = CommandInvocation::session_command(
            "__commands:main",
            "commands",
            crate::workflow::config::CommandBinding::builtin_commands()
                .as_command("commands")
                .unwrap(),
        );
        let param_snap = crate::workflow::parameter::ParameterSnapshot::from_parts(
            serde_json::Value::Null,
            String::new(),
            crate::input::InputSourceIdentity::default(),
            0,
        );
        let context = CommandContext {
            page: super::super::CommandOwnerContext {
                view_ref: "__commands:main".to_string(),
                parameters: param_snap.clone(),
            },
            owner: super::super::CommandOwnerContext {
                view_ref: "__commands:main".to_string(),
                parameters: param_snap,
            },
            current: Value::Null,
            engine_type: "picker".to_string(),
        };
        let prepared = prepare_builtin_commands(&config, invocation, context).unwrap();
        assert!(matches!(prepared, PreparedAction::Noop));
    }

    #[test]
    fn is_query_view_identifies_builtin_query_view() {
        assert!(is_query_view("__query:main", "__query:main"));
        assert!(is_query_view("__query:detail", "__query:main"));
        assert!(is_query_view("__form:main", "__query:main"));
        assert!(!is_query_view("core:default", "__query:main"));
        assert!(!is_query_view("sys:main", "__query:main"));
    }

    #[test]
    fn prepare_builtin_parameters_returns_noop_when_already_in_query_view() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let invocation = CommandInvocation::session_command(
            "__query:main",
            "parameters",
            crate::workflow::config::CommandBinding::builtin_parameters()
                .as_command("parameters")
                .unwrap(),
        );
        let param_snap = crate::workflow::parameter::ParameterSnapshot::from_parts(
            serde_json::Value::Null,
            String::new(),
            crate::input::InputSourceIdentity::default(),
            0,
        );
        let context = CommandContext {
            page: super::super::CommandOwnerContext {
                view_ref: "__query:main".to_string(),
                parameters: param_snap.clone(),
            },
            owner: super::super::CommandOwnerContext {
                view_ref: "__query:main".to_string(),
                parameters: param_snap,
            },
            current: Value::Null,
            engine_type: "form".to_string(),
        };
        let prepared = prepare_builtin_parameters(&config, invocation, context).unwrap();
        assert!(matches!(prepared, PreparedAction::Noop));
    }
}
