use super::Item;
use crate::config::{Command, Config, normalize_key};
use crate::engine::{EngineHost, PreparedProcess};
use crate::input::Key;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    pub(crate) id: String,
    pub(crate) source_view: String,
    pub(crate) command: Command,
}

pub(crate) enum CommandAction {
    Report {
        invocation: CommandInvocation,
        message: String,
    },
    Navigate {
        target: String,
        input: String,
    },
    Execute {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
    },
}

fn prepare_command(
    config: &Config,
    driver: &super::LauncherView,
    invocation: &CommandInvocation,
    item: Option<&Item>,
    log_file: Option<&Path>,
) -> Result<PreparedProcess> {
    let view = config
        .view(&invocation.source_view)
        .with_context(|| format!("view {:?} disappeared", invocation.source_view))?;
    let script = invocation
        .command
        .run
        .as_ref()
        .context("command has no run script")?;
    let shell = invocation
        .command
        .shell
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
    let frame = driver.current();
    let plugin_root = config
        .plugin_root(&source_view)
        .map(|path| path.to_path_buf());
    let mut environment = vec![
        ("LAUNCHER_ITEM".to_string(), item_text),
        ("LAUNCHER_VALUE".to_string(), value.to_string()),
        ("LAUNCHER_METADATA".to_string(), metadata),
        (
            "LAUNCHER_PLUGIN".to_string(),
            plugin_name(&source_view).to_string(),
        ),
        ("LAUNCHER_VIEW".to_string(), frame.view.clone()),
        ("LAUNCHER_VIEW_REF".to_string(), source_view.clone()),
        ("LAUNCHER_COMMAND".to_string(), invocation.id.clone()),
        ("LAUNCHER_QUERY".to_string(), frame.query.clone()),
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

fn plugin_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(plugin, _)| plugin)
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
        "run": command.run,
        "shell": command.shell,
        "view": command.view,
        "input": command.input,
        "exit": command.exit,
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

impl super::LauncherView {
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
        let command = invocation.command.clone();

        if let Some(target) = command.view {
            let input = config.evaluate_command_input(
                &invocation.source_view,
                command.input.as_deref().unwrap_or(""),
                runtime,
            )?;
            return Ok(Some(CommandAction::Navigate { target, input }));
        }

        let Some(_script) = command.run.as_ref() else {
            return Ok(Some(CommandAction::Report {
                message: format!("{} has no action", invocation.id),
                invocation,
            }));
        };
        let prepared = prepare_command(config, self, &invocation, item.as_ref(), log_file)?;
        Ok(Some(CommandAction::Execute {
            invocation,
            prepared,
            exit: command.exit,
        }))
    }
}

pub(super) fn execute_local(
    host: &mut EngineHost<'_>,
    prepared: PreparedProcess,
    terminal: &mut Terminal,
    invocation: &CommandInvocation,
    exit: bool,
) -> Result<()> {
    terminal.leave()?;
    let status = prepared.command().status();
    if !exit {
        terminal.reenter()?;
    }
    match status {
        Ok(status) => {
            let message = status_message(&status);
            host.record_command_status(invocation, &message, status.success());
        }
        Err(error) => host.record_error(invocation, &error.to_string()),
    }
    Ok(())
}

fn status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}
