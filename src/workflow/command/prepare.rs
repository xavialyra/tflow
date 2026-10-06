use crate::execution::PreparedProcess;
use crate::identity::ENV_WORKFLOW_DIR;
use crate::lifecycle::CancellationToken;
use crate::workflow::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    NavigationMode, NavigationRequest,
};
use crate::workflow::config::{CommandAction, CompiledConfig, ExecutionMode};
use anyhow::{Context, Result};
use serde_json::Value;
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
    InvokeCommand {
        command: crate::workflow::command::CommandRef,
    },
    Execute {
        prepared: PreparedProcess,
        exit: bool,
        success_message: Option<String>,
    },
    Companion {
        target: String,
        query: Option<Value>,
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
    prepare_action_execution(
        config,
        invocation,
        action,
        command_invocation,
        context,
        cancellation,
    )
}

fn prepare_action_execution(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    action: &CommandAction,
    command_invocation: CommandInvocation,
    context: CommandContext,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let execution = action.execution_mode();
    let payload = action.payload();
    let source_label = format!(
        "{}.commands.{}",
        command_invocation.source_view(),
        command_invocation.id()
    );
    let outcome = match execution {
        ExecutionMode::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                action.operation_type(),
                payload,
                &source_label,
            )?;
            crate::protocol::ProtocolOutcome::Operation(operation)
        }
        ExecutionMode::Script => {
            let root = command_root(config, &command_invocation);
            let source = crate::workflow::config::parse_script_source(payload, root)?;
            let active_view = &context.page.view_ref;
            let query = config
                .query_definition(active_view)
                .unwrap_or_else(|_| serde_json::json!({"type": "string"}));
            let view = serde_json::json!({
                "ref": active_view,
                "query": query,
                "values": context.page.parameters.values(),
                "raw_input": context.page.parameters.raw_input(),
            });
            let request = crate::protocol::command_request(
                &context.owner,
                command_invocation.id(),
                action.operation_type(),
                invocation.input_value(),
                &context.current,
                &context.engine_type,
                &context.commands,
                &view,
            );
            crate::protocol::run_script_response(
                command_invocation.source_view(),
                &source_label,
                root,
                &source,
                &request,
                (execution == ExecutionMode::Declared).then_some(action.operation_type()),
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
    // The command id is the unique FQID; the current View is the execution
    // context, so the same lookup serves every binding layer.
    let command = config
        .find_command(
            crate::workflow::config::package_id(origin.source_view()),
            origin.id(),
        )
        .cloned()
        .context("return processor origin is not configured")?;
    let command_invocation = CommandInvocation::from_origin(origin.clone(), command);
    let source_label = format!(
        "{}.commands.{}.return_processor",
        command_invocation.source_view(),
        command_invocation.id()
    );
    let outcome = match processor.execution {
        ExecutionMode::Declared => {
            let operation = crate::protocol::parse_declared_operation(
                processor
                    .operation
                    .as_deref()
                    .context("declared return processor requires type")?,
                &processor.payload,
                &source_label,
            )?;
            crate::protocol::ProtocolOutcome::Operation(operation)
        }
        ExecutionMode::Script => {
            let root = command_root(config, &command_invocation);
            let source = crate::workflow::config::parse_script_source(&processor.payload, root)?;
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
                (processor.execution == ExecutionMode::Declared)
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
            let caller_view = command_invocation.source_view();
            let target = config.resolve_view_scoped(&target, caller_view)?;
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
            let caller_view = command_invocation.source_view();
            let target = config.resolve_view_scoped(&target, caller_view)?;
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
        crate::protocol::ProtocolOperation::InvokeCommand { command } => {
            Ok(PreparedAction::InvokeCommand { command })
        }
        crate::protocol::ProtocolOperation::Run {
            argv,
            exit,
            success_message,
            timeout_ms,
            ..
        } => {
            let root = command_root(config, &command_invocation);
            let mut prepared = prepared_direct_process(root, argv)?;
            if let Some(ms) = timeout_ms {
                prepared.timeout = Some(std::time::Duration::from_millis(ms));
            }
            Ok(PreparedAction::Execute {
                prepared,
                exit,
                success_message,
            })
        }
        crate::protocol::ProtocolOperation::Companion { target, query } => {
            let caller_view = command_invocation.source_view();
            let target = config.resolve_view_scoped(&target, caller_view)?;
            Ok(PreparedAction::Companion { target, query })
        }
    }
}

fn command_root<'a>(
    config: &'a CompiledConfig,
    command_invocation: &CommandInvocation,
) -> Option<&'a Path> {
    // A command reference is either a fully qualified `<workflow>.<command>`
    // or a local id resolved against the view that declares the binding. The
    // owning workflow determines where relative script files are read from, so
    // an item binding published by another workflow must not borrow the
    // aggregate view's script root.
    let id = command_invocation.id();
    let owner = id.split_once('.').map_or_else(
        || command_invocation.source_view(),
        |(workflow, _)| workflow,
    );
    config.workflow_root(owner)
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
        timeout: None,
    })
}

#[cfg(test)]
pub(crate) fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
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
    fn fully_qualified_command_resolves_its_own_workflow_script_root() {
        // An aggregate view dispatches `apps.open`; the command's relative
        // script files must resolve under the apps workflow, not the aggregate.
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let command = config
            .find_command("core", "apps.open")
            .cloned()
            .expect("fixture exposes apps.open");
        let invocation = CommandInvocation::view(
            "core:default",
            crate::workflow::command::CommandRef {
                id: "apps.open".to_string(),
                revision: 0,
            },
            command,
        );
        let root = command_root(&config, &invocation).expect("apps workflow root");
        assert!(
            root.ends_with("workflows/apps"),
            "cross-workflow command must resolve its own root, got {}",
            root.display()
        );

        // A local binding in the aggregate keeps using the aggregate root.
        let local = config
            .find_command("core", "complete")
            .cloned()
            .expect("fixture exposes core.complete");
        let local_invocation = CommandInvocation::view(
            "core:default",
            crate::workflow::command::CommandRef {
                id: "complete".to_string(),
                revision: 0,
            },
            local,
        );
        let local_root = command_root(&config, &local_invocation).expect("core workflow root");
        assert!(
            local_root.ends_with("workflows/core"),
            "local command must resolve the caller root, got {}",
            local_root.display()
        );
    }
}
