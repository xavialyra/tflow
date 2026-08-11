use super::picker::{Item, PickerView};
use crate::config::{
    Command, CommandAction as ConfigCommandAction, Config, ConfigReadContext, ConfigScope,
    normalize_key,
};
use crate::engine::{CommandInvocation, PreparedProcess};
use crate::input::Key;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) enum CommandAction {
    Navigate {
        target: String,
        input: String,
    },
    Execute {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
    },
    Complete {
        invocation: CommandInvocation,
        state: crate::state::StateInstance,
    },
}

fn prepare_command(
    config: &Config,
    active_view: &str,
    query: &str,
    invocation: &CommandInvocation,
    item: Option<&Item>,
    log_file: Option<&Path>,
) -> Result<PreparedProcess> {
    let view = config
        .view(&invocation.source_view)
        .with_context(|| format!("view {:?} disappeared", invocation.source_view))?;
    let ConfigCommandAction::Run { payload } = &invocation.command.action else {
        anyhow::bail!("command is not a run action");
    };
    let script = &payload.handler;
    let command_shell = &payload.shell;
    let shell = command_shell
        .as_deref()
        .or(view.run_shell.as_deref())
        .unwrap_or("sh");
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
    let source_view = item
        .map(|item| item.source_view.clone())
        .unwrap_or_else(|| invocation.source_view.clone());
    let plugin_root = config
        .plugin_root(&source_view)
        .map(|path| path.to_path_buf());
    let mut environment = vec![
        ("LAUNCHER_ITEM".to_string(), item_text),
        ("LAUNCHER_VALUE".to_string(), value.to_string()),
        ("LAUNCHER_METADATA".to_string(), metadata),
        (
            "LAUNCHER_PLUGIN".to_string(),
            package_id(&source_view).to_string(),
        ),
        ("LAUNCHER_VIEW".to_string(), active_view.to_string()),
        ("LAUNCHER_VIEW_REF".to_string(), source_view.clone()),
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
            script.clone(),
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

fn command_key(key: Key) -> Option<String> {
    key.binding_name()
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

pub(super) fn runtime_item_value(item: &Item) -> Value {
    json!({
        "prefix": item.prefix,
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
        "source_view": item.source_view,
    })
}

impl PickerView {
    pub(crate) fn resolve_command(&self, config: &Config, key: Key) -> Option<CommandInvocation> {
        let frame = self.current();
        if frame.command_owner.is_some() {
            let binding = frame
                .items
                .get(frame.selected)
                .and_then(|item| item.value.as_deref())?;
            return find_command(config, frame.command_owner.as_deref()?, binding);
        }
        let key = command_key(key)?;
        find_command(config, self.command_owner()?, &key)
    }

    pub(crate) fn prepare_command_action(
        &self,
        config: &Config,
        active_state: &crate::state::StateInstance,
        runtime: &Value,
        key: Key,
        log_file: Option<&Path>,
    ) -> Result<Option<CommandAction>> {
        let Some(invocation) = self.resolve_command(config, key) else {
            return Ok(None);
        };
        let command_view = self.current().command_owner.is_some();
        let item = if command_view {
            self.command_parent_item().cloned()
        } else {
            self.current().items.get(self.current().selected).cloned()
        };
        let state = if invocation.source_view == active_state.view_ref() {
            active_state
        } else {
            self.source_states
                .get(&invocation.source_view)
                .unwrap_or(active_state)
        };
        match invocation.command.action.clone() {
            ConfigCommandAction::Complete { .. } => Ok(Some(CommandAction::Complete {
                invocation: invocation.clone(),
                state: state.clone(),
            })),
            ConfigCommandAction::Navigate { .. } => {
                let request = config
                    .get(
                        ConfigReadContext {
                            scope: ConfigScope::View(state),
                            runtime,
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
                let input = request
                    .get("query")
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .context("navigation input query must evaluate to a string or null")
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(Some(CommandAction::Navigate { target, input }))
            }
            ConfigCommandAction::Run { payload } => {
                let exit = payload.exit;
                let prepared = prepare_command(
                    config,
                    self.current_view_ref(),
                    &self.current().query,
                    &invocation,
                    item.as_ref(),
                    log_file,
                )?;
                Ok(Some(CommandAction::Execute {
                    invocation,
                    prepared,
                    exit,
                }))
            }
        }
    }
}
