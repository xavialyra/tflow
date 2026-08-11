use crate::config::{
    Command, CommandAction as ConfigCommandAction, Config, ConfigReadContext, ConfigScope,
    normalize_key,
};
use crate::engine::{CommandInvocation, PreparedProcess};
use crate::input::Key;
use crate::state::StateInstance;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) enum CommandAction {
    Navigate {
        target: String,
        input: Option<String>,
    },
    Execute {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
    },
    Complete {
        invocation: CommandInvocation,
        state: StateInstance,
    },
}

pub(crate) struct CommandItem<'a> {
    pub(crate) text: &'a str,
    pub(crate) value: Option<&'a str>,
    pub(crate) metadata: &'a Value,
    pub(crate) source_view: &'a str,
}

pub(crate) struct CommandContext<'a> {
    pub(crate) active_view: &'a str,
    pub(crate) query: &'a str,
    pub(crate) state: &'a StateInstance,
    pub(crate) runtime: &'a Value,
    pub(crate) item: Option<CommandItem<'a>>,
    pub(crate) log_file: Option<&'a Path>,
}

pub(crate) fn prepare_command_action(
    config: &Config,
    invocation: CommandInvocation,
    context: CommandContext<'_>,
) -> Result<CommandAction> {
    match invocation.command.action.clone() {
        ConfigCommandAction::Complete { .. } => Ok(CommandAction::Complete {
            invocation,
            state: context.state.clone(),
        }),
        ConfigCommandAction::Navigate { .. } => {
            let request = config
                .get(
                    ConfigReadContext {
                        scope: ConfigScope::View(context.state),
                        runtime: context.runtime,
                        input: &config.input_value,
                        cancellation: None,
                    },
                    &["commands", invocation.id.as_str(), "payload"],
                )?
                .context("navigation command input is not configured")?;
            let target = request
                .get("target")
                .and_then(Value::as_str)
                .context("navigation input target must evaluate to a string")?;
            let target = config.resolve_view(target)?;
            let input = match request.get("query") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .context("navigation input query must evaluate to a string or null")?
                        .to_string(),
                ),
            };
            Ok(CommandAction::Navigate { target, input })
        }
        ConfigCommandAction::Run { payload } => {
            let exit = payload.exit;
            let prepared = prepare_run_command(
                config,
                context.active_view,
                context.query,
                &invocation,
                context.item,
                context.log_file,
            )?;
            Ok(CommandAction::Execute {
                invocation,
                prepared,
                exit,
            })
        }
    }
}

fn prepare_run_command(
    config: &Config,
    active_view: &str,
    query: &str,
    invocation: &CommandInvocation,
    item: Option<CommandItem<'_>>,
    log_file: Option<&Path>,
) -> Result<PreparedProcess> {
    let view = config
        .view(&invocation.source_view)
        .with_context(|| format!("view {:?} disappeared", invocation.source_view))?;
    let ConfigCommandAction::Run { payload } = &invocation.command.action else {
        anyhow::bail!("command is not a run action");
    };
    let shell = payload
        .shell
        .as_deref()
        .or(view.run_shell.as_deref())
        .unwrap_or("sh");
    let value = item
        .as_ref()
        .and_then(|item| item.value)
        .or_else(|| item.as_ref().map(|item| item.text))
        .unwrap_or("");
    let metadata = item
        .as_ref()
        .map(|item| serde_json::to_string(item.metadata))
        .transpose()
        .context("could not serialize selected item metadata")?
        .unwrap_or_else(|| Value::Null.to_string());
    let item_text = item
        .as_ref()
        .map(|item| item.text.to_string())
        .unwrap_or_default();
    let source_view = item
        .as_ref()
        .map(|item| item.source_view)
        .unwrap_or(&invocation.source_view);
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
        ("LAUNCHER_VIEW".to_string(), active_view.to_string()),
        ("LAUNCHER_VIEW_REF".to_string(), source_view.to_string()),
        ("LAUNCHER_COMMAND".to_string(), invocation.id.clone()),
        ("LAUNCHER_QUERY".to_string(), query.to_string()),
    ];
    if let Some(root) = &plugin_root {
        environment.push((
            "LAUNCHER_PLUGIN_DIR".to_string(),
            root.to_string_lossy().to_string(),
        ));
    }
    if let Some(path) = log_file {
        environment.push((
            "LAUNCHER_LOG_FILE".to_string(),
            path.to_string_lossy().to_string(),
        ));
    }
    Ok(PreparedProcess {
        argv: vec![
            shell.to_string(),
            "-c".to_string(),
            payload.handler.clone(),
            "tui-launcher".to_string(),
        ],
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

pub(super) fn find_command(
    config: &Config,
    view_ref: &str,
    key: &str,
) -> Option<CommandInvocation> {
    let view = config.view(view_ref)?;
    view.commands.iter().find_map(|(id, command)| {
        (normalize_key(&command.key).ok().as_deref() == Some(key)).then(|| CommandInvocation {
            id: id.clone(),
            source_view: view_ref.to_string(),
            command: command.clone(),
        })
    })
}

pub(super) fn find_command_for_key(
    config: &Config,
    view_ref: &str,
    key: Key,
) -> Option<CommandInvocation> {
    find_command(config, view_ref, &key.binding_name()?)
}

pub(super) fn add_view_commands(
    config: &Config,
    commands: &mut BTreeMap<String, String>,
    view_ref: &str,
) {
    let Some(view) = config.view(view_ref) else {
        return;
    };
    for command in view.commands.values() {
        if let Ok(key) = normalize_key(&command.key) {
            commands.entry(key).or_insert_with(|| command.label.clone());
        }
    }
}

pub(super) fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

pub(super) fn runtime_command_value(owner: &str, id: &str, command: &Command) -> Result<Value> {
    let key = normalize_key(&command.key)
        .with_context(|| format!("invalid command key for {owner}/{id}"))?;
    Ok(json!({
        "id": format!("{owner}/{id}"),
        "owner": owner,
        "key": key,
        "label": command.label,
        "action": command.action,
    }))
}
