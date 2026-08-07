use super::{Item, ItemsTaskScheduler};
use crate::config::{Command, Config, ENGINE_LAUNCHER, EngineDefinition, normalize_key};
use crate::engine::{
    Engine, EngineDriver, EngineHost, LauncherDriver, RuntimeHandle, SessionEffect, validate_fields,
};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    pub(crate) id: String,
    pub(crate) source_view: String,
    pub(crate) command: Command,
}

pub(crate) struct PreparedCommand {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

impl PreparedCommand {
    pub(crate) fn process(&self) -> ProcessCommand {
        let mut process = ProcessCommand::new(&self.argv[0]);
        process.args(&self.argv[1..]);
        if let Some(current_dir) = &self.current_dir {
            process.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            process.env(key, value);
        }
        process
    }
}

pub(crate) struct CommandExecution {
    pub(crate) invocation: CommandInvocation,
    pub(crate) prepared: PreparedCommand,
    pub(crate) engine_type: String,
    pub(crate) exit: bool,
    pub(crate) title: String,
    pub(crate) next_view: Option<String>,
}

pub(crate) enum CommandAction {
    Report {
        invocation: CommandInvocation,
        message: String,
    },
    OpenView {
        target: String,
    },
    Execute(CommandExecution),
}

pub(super) fn prepare_command(
    config: &Config,
    driver: &LauncherDriver,
    invocation: &CommandInvocation,
    item: Option<&Item>,
    log_file: Option<&Path>,
) -> Result<PreparedCommand> {
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
    Ok(PreparedCommand {
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

pub(super) fn command_key(key: super::super::Key) -> Option<String> {
    match key {
        super::super::Key::Enter => Some("enter".to_string()),
        super::super::Key::Alt(character) if character.is_ascii_graphic() => {
            Some(format!("alt+{}", character.to_ascii_lowercase()))
        }
        _ => None,
    }
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

pub(crate) struct LauncherCommandEngine;

impl Engine for LauncherCommandEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_LAUNCHER
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["items", "commands"])
    }

    fn create_view(
        &self,
        _config: &Config,
        view_ref: &str,
        input: &str,
        log_file: Option<&Path>,
        _runtime: RuntimeHandle,
        items_scheduler: ItemsTaskScheduler,
    ) -> Result<Box<dyn EngineDriver>> {
        Ok(Box::new(LauncherDriver::new(
            view_ref,
            input,
            items_scheduler,
            log_file.map(PathBuf::from),
            None,
            None,
        )))
    }

    fn create_command(&self, execution: CommandExecution) -> Result<Box<dyn EngineDriver>> {
        Ok(Box::new(LauncherCommandDriver {
            execution: Some(execution),
        }))
    }
}

struct LauncherCommandDriver {
    execution: Option<CommandExecution>,
}

impl EngineDriver for LauncherCommandDriver {
    fn step(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<SessionEffect> {
        let execution = self
            .execution
            .take()
            .context("launcher command driver was already completed")?;
        let CommandExecution {
            invocation,
            prepared,
            exit,
            next_view,
            ..
        } = execution;
        if exit {
            execute_exit(host, prepared, terminal, &invocation)?;
            return Ok(SessionEffect::Exit);
        }
        execute_oneshot(host, prepared, terminal, &invocation)?;
        match next_view {
            Some(view_ref) => Ok(SessionEffect::OpenView {
                view_ref,
                input: String::new(),
                replace_current: true,
            }),
            None => Ok(SessionEffect::Back),
        }
    }

    fn render(&self, _host: &EngineHost<'_>, _terminal: &Terminal) -> Result<()> {
        Ok(())
    }
}

impl LauncherDriver {
    pub(crate) fn resolve_command(
        &self,
        config: &Config,
        key: crate::engine::Key,
    ) -> Option<CommandInvocation> {
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
        key: crate::engine::Key,
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
        let title = format!(
            "{} / {}",
            item.as_ref()
                .map(|item| item.text.as_str())
                .unwrap_or(&invocation.id),
            invocation.id
        );
        let current_view = self.current().view.clone();

        let Some(_script) = command.run.as_ref() else {
            return Ok(Some(match command.view {
                Some(target) => CommandAction::OpenView { target },
                None => CommandAction::Report {
                    message: format!("{} has no command", invocation.id),
                    invocation,
                },
            }));
        };

        let prepared = prepare_command(config, self, &invocation, item.as_ref(), log_file)?;
        let engine_type = if command.exit {
            crate::config::ENGINE_LAUNCHER.to_string()
        } else {
            command
                .view
                .as_deref()
                .map(|target| config.engine(target))
                .transpose()?
                .unwrap_or(crate::config::ENGINE_LAUNCHER)
                .to_string()
        };
        let next_view = (!command.exit && engine_type == crate::config::ENGINE_LAUNCHER)
            .then_some(command.view)
            .flatten()
            .filter(|target| target != &current_view);
        Ok(Some(execution(
            invocation,
            prepared,
            engine_type,
            command.exit,
            title,
            next_view,
        )))
    }
}

fn execute_oneshot(
    host: &mut EngineHost<'_>,
    prepared: PreparedCommand,
    terminal: &mut Terminal,
    invocation: &CommandInvocation,
) -> Result<()> {
    let result = (|| {
        terminal.leave()?;
        let result = prepared.process().status();
        terminal.reenter()?;
        Ok::<_, anyhow::Error>(result)
    })();
    let result = result?;
    match result {
        Ok(status) => {
            let message = status_message(&status);
            host.record_command_status(invocation, &message, status.success());
        }
        Err(error) => host.record_error(invocation, &error.to_string()),
    }
    Ok(())
}

fn execute_exit(
    host: &mut EngineHost<'_>,
    prepared: PreparedCommand,
    terminal: &mut Terminal,
    invocation: &CommandInvocation,
) -> Result<()> {
    terminal.leave()?;
    let status = prepared.process().status();
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

pub(super) fn execution(
    invocation: CommandInvocation,
    prepared: PreparedCommand,
    engine_type: String,
    exit: bool,
    title: String,
    next_view: Option<String>,
) -> CommandAction {
    CommandAction::Execute(CommandExecution {
        invocation,
        prepared,
        engine_type,
        exit,
        title,
        next_view,
    })
}
