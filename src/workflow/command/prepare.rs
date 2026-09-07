use crate::execution::PreparedProcess;
use crate::lifecycle::CancellationToken;
use crate::workflow::command::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    CommandOwnerContext, CommandRef, NavigationMode, NavigationRequest, ReturnAdapter, ViewOutput,
    ViewOutputItem, ViewReturn,
};
use crate::workflow::config::{
    Command, CommandAction, CompiledConfig, EvaluationSnapshot, InvocationScope, NavigatePayload,
    OwnerViewScope, ResolvedScriptSource, ReturnScope, RunPayload, SessionScope, normalize_key,
};
use crate::workflow::expression::EvaluationStage;
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
        None,
        true,
        cancellation,
    )
}

pub(crate) fn prepare_continuation(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
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
        invocation,
        action,
        CommandInvocation::from_origin(origin, command),
        context,
        Some(returned),
        false,
        cancellation,
    )
}

fn prepare_action(
    config: &CompiledConfig,
    invocation: &crate::workflow::InvocationContext,
    action: &CommandAction,
    command_invocation: CommandInvocation,
    context: CommandContext,
    returned: Option<&Value>,
    root_adapter: bool,
    cancellation: &CancellationToken,
) -> Result<PreparedAction> {
    let owner = &context.owner;
    let owner_scope = OwnerViewScope::new(&owner.view_ref, &owner.parameters)
        .with_binding_raw(Some(&owner.binding_raw));
    let snapshot = EvaluationSnapshot::new(
        InvocationScope::new(invocation.input_value()),
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
        CommandAction::Run { .. } => {
            let payload = action.run_payload().context("invalid run action")?;
            let prepared = prepare_run_command(
                config,
                &payload,
                &command_invocation,
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
            let engine_options = payload
                .engine
                .as_ref()
                .map(|value| evaluate_value(config, &snapshot, stage, value))
                .transpose()?;
            let mut request = match parameters {
                Some(parameters) => NavigationRequest::new(target, "").with_parameters(parameters),
                None => NavigationRequest::with_defaults(target),
            }
            .with_presentation(payload.presentation.clone());
            if let Some(engine) = engine_options {
                request = request.with_engine_options(engine);
            }
            Ok(PreparedAction::Call(CallRequest {
                request,
                origin: command_invocation.origin(),
                context,
                then: payload.then.clone(),
            }))
        }
        CommandAction::Return { .. } => {
            let payload = action.return_payload().unwrap_or_default();
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
                    command: command_invocation
                        .view_reference()
                        .context("return action cannot originate from a session command")?
                        .clone(),
                    context: context.clone(),
                })
            } else {
                None
            };
            Ok(PreparedAction::Return(ViewReturn {
                source_view: command_invocation.source_view().to_string(),
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
    config: &CompiledConfig,
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
    config: &CompiledConfig,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
    value: &toml::Value,
) -> Result<Value> {
    config.evaluate_value(snapshot, stage, value)
}

fn prepare_run_command(
    config: &CompiledConfig,
    payload: &RunPayload,
    invocation: &CommandInvocation,
    _context: &CommandContext,
    _owner: &CommandOwnerContext,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
) -> Result<PreparedProcess> {
    let source_view = invocation.source_view();
    let (workflow_id, _view_name) = source_view
        .split_once(':')
        .unwrap_or((source_view, source_view));
    let command_id = invocation.id();
    let source_label = format!("{source_view}.commands.{command_id}");
    let arguments = config.evaluate_argv(payload.args.as_ref(), snapshot, stage, "command args")?;

    let workflow_root = config.workflow_root(source_view);
    let mut environment = Vec::new();
    if let Some(root) = workflow_root {
        environment.push((
            "WORKFLOW_DIR".to_string(),
            root.to_string_lossy().into_owned(),
        ));
    }

    let argv = if let Some(script_body) = &payload.script {
        crate::execution::prepare_inline_script_command(
            workflow_id,
            &source_label,
            script_body,
            &arguments,
        )?
    } else if let Some(handler_raw) = &payload.handler {
        let handler_value = evaluate_run_handler(config, snapshot, stage, handler_raw)?;
        let handler_source = ResolvedScriptSource::parse(&handler_value)
            .context("command handler must resolve to a script source")?;
        match handler_source.command_target()? {
            crate::workflow::config::ResolvedScriptTarget::Inline(script_body) => {
                crate::execution::prepare_inline_script_command(
                    workflow_id,
                    &source_label,
                    script_body,
                    &arguments,
                )?
            }
            crate::workflow::config::ResolvedScriptTarget::File(file) => {
                let (script_path, script_content) = if std::path::Path::new(file).is_absolute() {
                    let path = std::path::PathBuf::from(file);
                    let content = std::fs::read_to_string(&path)
                        .with_context(|| format!("could not read script {}", path.display()))?;
                    (path, content)
                } else if let Some(root) = workflow_root {
                    let content = crate::execution::read_script(root, file)?;
                    (root.join(file), content)
                } else {
                    bail!(
                        "single-file workflow {:?} cannot reference relative script file {:?}",
                        source_view,
                        file
                    );
                };
                let shebang = crate::execution::parse_shebang(&script_content);
                let interpreter = crate::execution::verify_interpreter(&shebang.interpreter)?;
                let mut argv = Vec::new();
                argv.push(interpreter.to_string_lossy().into_owned());
                argv.extend(shebang.args);
                argv.push(script_path.to_string_lossy().into_owned());
                argv.extend(arguments);
                argv
            }
        }
    } else {
        bail!(
            "command {:?} has neither script nor handler",
            invocation.id()
        );
    };

    Ok(PreparedProcess {
        argv,
        environment,
        current_dir: None,
    })
}

fn evaluate_run_handler(
    config: &CompiledConfig,
    snapshot: &EvaluationSnapshot<'_>,
    stage: EvaluationStage,
    handler: &toml::Value,
) -> Result<Value> {
    let mut handler = handler.clone();
    let script_body = handler
        .as_table_mut()
        .and_then(|handler| handler.remove("script"));
    let mut evaluated = config.evaluate_value(snapshot, stage, &handler)?;
    if let Some(script_body) = script_body {
        evaluated
            .as_object_mut()
            .context("command handler must evaluate to a script source object")?
            .insert(
                "script".to_string(),
                crate::workflow::config::toml_to_json(&script_body)?,
            );
    }
    Ok(evaluated)
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

pub(crate) fn return_value(returned: &ViewReturn) -> Value {
    json!({
        "source": returned.source_view,
        "output": returned.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::config::CommandScope;

    fn test_parameters(config: &CompiledConfig) -> crate::workflow::parameter::ParameterSnapshot {
        let state = config.instantiate_parameters("core:default").unwrap();
        config
            .parameter_snapshot(&state, Default::default())
            .unwrap()
    }

    fn test_invocation(
        config: &CompiledConfig,
        input: Value,
    ) -> crate::workflow::InvocationContext {
        crate::workflow::InvocationContext::new(
            "core:default".to_string(),
            input,
            config.instantiate_parameters("core:default").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn default_output_reads_picker_and_capture_current_shapes() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
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
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let invocation_context = test_invocation(
            &config,
            serde_json::json!({
                "stdin": {"path": "/tmp/tui-launcher-captured-stdin"}
            }),
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
        let invocation = CommandInvocation::view(
            CommandRef {
                view: "core:default".to_string(),
                id: "run".to_string(),
            },
            Command {
                key: Some("enter".to_string()),
                label: "Run".to_string(),
                scope: CommandScope::View,
                requires: crate::workflow::config::CommandRequirement::Input,
                passthrough: false,
                action: CommandAction::new_run(crate::workflow::config::RunPayload {
                    script: None,
                    handler: Some(
                        crate::workflow::config::ScriptSourceSpec::script_file("scripts/items.sh")
                            .as_toml_value(),
                    ),
                    args: None,
                    shell: None,
                    exit: false,
                }),
            },
        );
        let prepared = prepare_command_action(
            &config,
            &invocation_context,
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
                .any(|(key, _)| key.contains("LOG"))
        );
    }

    #[test]
    fn inline_run_scripts_preserve_literal_template_braces() {
        let mut config = crate::workflow::config::load_test_fixture().unwrap();
        let script = "printf '%s\\n' '{{ user_template }}'\n";
        let action = CommandAction::new_run(crate::workflow::config::RunPayload {
            script: Some(script.to_string()),
            ..Default::default()
        });
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .commands
            .insert(
                "literal-script".to_string(),
                Command {
                    key: None,
                    label: "literal-script".to_string(),
                    scope: CommandScope::View,
                    requires: crate::workflow::config::CommandRequirement::Input,
                    passthrough: false,
                    action: action.clone(),
                },
            );
        config.test_config_value_mut()["literal-script"] = serde_json::json!({
            "type": "run",
            "payload": {"script": script},
        });
        config.test_rebuild_compiled().unwrap();

        let parameters = test_parameters(&config);
        let context = CommandContext {
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
            current: Value::Null,
            current_fields: &[],
            runtime: Value::Null,
        };
        let prepared = prepare_command_action(
            &config,
            &test_invocation(&config, Value::Null),
            CommandExecution {
                invocation: CommandInvocation::view(
                    CommandRef {
                        view: "core:default".to_string(),
                        id: "literal-script".to_string(),
                    },
                    config.view("core:default").unwrap().commands["literal-script"].clone(),
                ),
                context,
            },
            &CancellationToken::new(),
        )
        .unwrap();
        let PreparedAction::Execute { prepared, .. } = prepared else {
            panic!("inline run action was not prepared for execution");
        };
        let output = std::process::Command::new(&prepared.argv[0])
            .args(&prepared.argv[1..])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{{ user_template }}\n");
    }

    #[test]
    fn edit_input_rejects_a_cursor_between_utf8_code_units() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
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
            payload: crate::workflow::config::EditInputPayload {
                value: toml::Value::String("\u{e9}".to_string()),
                cursor: Some(toml::Value::Integer(1)),
            },
        };
        assert!(
            prepare_continuation(
                &config,
                &test_invocation(&config, Value::Null),
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
    fn continuation_retains_invocation_input() {
        let mut config = crate::workflow::config::load_test_fixture().unwrap();
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
            runtime: Value::Null,
        };
        let action = CommandAction::EditInput {
            payload: crate::workflow::config::EditInputPayload {
                value: toml::Value::String("{{ input.stdin.path }}".to_string()),
                cursor: None,
            },
        };
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .commands
            .insert(
                "continued".to_string(),
                Command {
                    key: None,
                    label: "continued".to_string(),
                    scope: CommandScope::View,
                    requires: crate::workflow::config::CommandRequirement::Input,
                    passthrough: false,
                    action: action.clone(),
                },
            );
        *config.test_config_value_mut() = serde_json::json!({"template": "{{ input.stdin.path }}"});
        config.test_rebuild_compiled().unwrap();
        let invocation = test_invocation(
            &config,
            serde_json::json!({"stdin": {"path": "/tmp/continued-input"}}),
        );

        let prepared = prepare_continuation(
            &config,
            &invocation,
            &action,
            CommandOrigin::View(CommandRef {
                view: "core:default".to_string(),
                id: "continued".to_string(),
            }),
            context,
            &Value::Null,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(matches!(
            prepared,
            PreparedAction::EditInput { value, cursor }
                if value == "/tmp/continued-input" && cursor == value.len()
        ));
    }

    #[test]
    fn view_command_ids_cannot_be_misclassified_as_session_origins() {
        let mut config = crate::workflow::config::load_test_fixture().unwrap();
        config
            .test_views_mut()
            .get_mut("core:default")
            .unwrap()
            .commands
            .insert(
                "__session_local".to_string(),
                Command {
                    key: Some("ctrl+l".to_string()),
                    label: "Local".to_string(),
                    scope: CommandScope::View,
                    requires: crate::workflow::config::CommandRequirement::Input,
                    passthrough: false,
                    action: CommandAction::new_return(
                        crate::workflow::config::ReturnPayload::default(),
                    ),
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
            payload: crate::workflow::config::EditInputPayload {
                value: toml::Value::String("restored".to_string()),
                cursor: None,
            },
        };

        let prepared = prepare_continuation(
            &config,
            &test_invocation(&config, Value::Null),
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
        let config = crate::workflow::config::load_test_fixture().unwrap();
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
