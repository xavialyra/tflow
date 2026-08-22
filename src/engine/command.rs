use crate::cancellation::CancellationToken;
use crate::config::{
    Command, CommandAction, CommandScope, Config, EvaluationSnapshot, InvocationScope,
    NavigatePayload, OwnerViewScope, ResolvedScriptSource, ReturnScope, RunPayload, SessionScope,
    normalize_key,
};
use crate::engine::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    CommandOwnerContext, CommandRef, NavigationRequest, PreparedProcess, ReturnAdapter, ViewOutput,
    ViewReturn,
};
use crate::expression::EvaluationStage;
use crate::input::Key;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) enum PreparedAction {
    Navigate(NavigationRequest),
    Call(CallRequest),
    Return(ViewReturn),
    EditInput {
        value: String,
        cursor: usize,
    },
    Invoke(CommandExecution),
    Execute {
        invocation: CommandInvocation,
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
        CommandOrigin::ChromeFooter { binding, .. } => config
            .chrome
            .footer
            .bindings
            .get(binding)
            .map(|binding| binding.as_command()),
    }
    .with_context(|| {
        format!(
            "continuation origin {:?} is no longer configured",
            origin.id()
        )
    })?;
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
    let owner = command_owner(&context, invocation.source_view())?;
    let owner_scope = OwnerViewScope::new(&owner.state).with_binding_raw(Some(&owner.binding_raw));
    let snapshot = EvaluationSnapshot::new(
        InvocationScope::new(&config.input_value),
        SessionScope::new(&context.runtime),
        Some(owner_scope),
        Some(cancellation),
    )
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
                invocation,
                prepared,
                exit: payload.exit,
            })
        }
        CommandAction::Navigate { payload } => {
            let (target, query) = evaluate_target(config, payload, &snapshot, stage)?;
            let request = match query {
                Some(query) => NavigationRequest::new(target, "").with_query(query),
                None => NavigationRequest::with_defaults(target),
            };
            Ok(PreparedAction::Navigate(request))
        }
        CommandAction::Call { payload } => {
            let target = evaluate_value(config, &snapshot, stage, &payload.target)?
                .as_str()
                .context("call target must evaluate to a string")?
                .to_string();
            let target = config.resolve_view(&target)?;
            let query = payload
                .query
                .as_ref()
                .map(|value| evaluate_value(config, &snapshot, stage, value))
                .transpose()?;
            let request = match query {
                Some(query) => NavigationRequest::new(target, "").with_query(query),
                None => NavigationRequest::with_defaults(target),
            };
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
                None => context.output.clone().context(
                    "return command has no engine output; configure payload.value explicitly",
                )?,
            };
            let adapter = if root_adapter {
                Some(ReturnAdapter {
                    command: invocation
                        .view_reference()
                        .context("return action cannot originate from chrome")?
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

fn command_owner<'a>(
    context: &'a CommandContext,
    source_view: &str,
) -> Result<&'a CommandOwnerContext> {
    if context.page.view_ref == source_view {
        return Ok(&context.page);
    }
    context
        .selection
        .as_ref()
        .filter(|selection| selection.owner.view_ref == source_view)
        .map(|selection| &selection.owner)
        .with_context(|| {
            format!(
                "command owner {:?} is not the active page or selected item owner",
                source_view
            )
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
    let query = payload
        .query
        .as_ref()
        .map(|value| evaluate_value(config, snapshot, stage, value))
        .transpose()?;
    Ok((target, query))
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
        .unwrap_or("sh");
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
    let handler = crate::script_runner::read_script(root, handler_file)?;
    if shell.is_empty() {
        bail!("command shell must not be empty");
    }
    let arguments = config.evaluate_argv(payload.args.as_ref(), snapshot, stage, "command args")?;
    let item = context.selection.as_ref().map(|selection| &selection.item);
    let value = item
        .and_then(|item| item.value.as_deref())
        .or_else(|| item.map(|item| item.text.as_str()))
        .unwrap_or("");
    let metadata = item
        .map(|item| serde_json::to_string(&item.metadata))
        .transpose()
        .context("could not serialize selected item metadata")?
        .unwrap_or_else(|| Value::Null.to_string());
    let item_text = item.map(|item| item.text.clone()).unwrap_or_default();
    let item_view = item.map(|item| item.source_view.as_str());
    let item_plugin = item_view.map(package_id).unwrap_or("");
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
            item_view.unwrap_or("").to_string(),
        ),
        ("LAUNCHER_ITEM_PLUGIN".to_string(), item_plugin.to_string()),
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

fn package_id(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(view_ref)
}

pub(crate) fn find_command(
    config: &Config,
    view_ref: &str,
    key: &str,
) -> Option<CommandInvocation> {
    let view = config.view(view_ref)?;
    view.commands.iter().find_map(|(id, command)| {
        (normalize_key(&command.key).ok().as_deref() == Some(key)).then(|| {
            CommandInvocation::view(
                CommandRef {
                    view: view_ref.to_string(),
                    id: id.clone(),
                },
                command.clone(),
            )
        })
    })
}

pub(crate) fn find_command_for_key(
    config: &Config,
    view_ref: &str,
    key: Key,
) -> Option<CommandInvocation> {
    find_command(config, view_ref, &key.binding_name()?)
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
            if command.scope != CommandScope::Selection {
                continue;
            }
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
    let owner = context
        .selection
        .as_ref()
        .map(|selection| selection.owner.view_ref.as_str());
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

    #[test]
    fn run_command_does_not_export_runtime_log_environment() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.input_value = serde_json::json!({
            "stdin": {"path": "/tmp/tui-launcher-captured-stdin"}
        });
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                state: config.instantiate_state("core:default").unwrap(),
                binding_raw: String::new(),
            },
            selection: None,
            runtime: serde_json::json!({}),
            output: None,
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
                state: config.instantiate_state("core:default").unwrap(),
                binding_raw: String::new(),
            },
            selection: None,
            runtime: serde_json::json!({}),
            output: None,
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
    fn view_command_ids_cannot_be_misclassified_as_chrome_origins() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config
            .views
            .get_mut("core:default")
            .unwrap()
            .commands
            .insert(
                "__chrome_footer_local".to_string(),
                Command {
                    key: "ctrl+l".to_string(),
                    label: "Local".to_string(),
                    scope: CommandScope::View,
                    requires: crate::config::CommandRequirement::Input,
                    action: CommandAction::Return {
                        payload: crate::config::ReturnPayload::default(),
                    },
                },
            );
        let context = CommandContext {
            page: CommandOwnerContext {
                view_ref: "core:default".to_string(),
                state: config.instantiate_state("core:default").unwrap(),
                binding_raw: String::new(),
            },
            selection: None,
            runtime: serde_json::json!({}),
            output: None,
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
                id: "__chrome_footer_local".to_string(),
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
                state: config.instantiate_state("core:default").unwrap(),
                binding_raw: String::new(),
            },
            selection: None,
            runtime: serde_json::json!({}),
            output: None,
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
