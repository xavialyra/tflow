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
        query: Option<Value>,
    },
    Execute {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
    },
    Complete {
        invocation: CommandInvocation,
        state: StateInstance,
        binding_raw: String,
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
    pub(crate) state: &'a StateInstance,
    pub(crate) binding_raw: &'a str,
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
            binding_raw: context.binding_raw.to_string(),
        }),
        ConfigCommandAction::Navigate { .. } => {
            let request = config
                .get(
                    ConfigReadContext {
                        scope: ConfigScope::View(context.state),
                        runtime: context.runtime,
                        input: &config.input_value,
                        cancellation: None,
                        binding_raw: Some(context.binding_raw),
                    },
                    &["commands", invocation.id.as_str(), "payload"],
                )?
                .context("navigation command input is not configured")?;
            let target = request
                .get("target")
                .and_then(Value::as_str)
                .context("navigation input target must evaluate to a string")?;
            let target = config.resolve_view(target)?;
            let query = match request.get("query") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.clone()),
            };
            Ok(CommandAction::Navigate { target, query })
        }
        ConfigCommandAction::Run { payload } => {
            let exit = payload.exit;
            let prepared = prepare_run_command(
                config,
                context.active_view,
                context.binding_raw,
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
    binding_raw: &str,
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
    let item_view = item.as_ref().map(|item| item.source_view);
    let item_plugin = item_view.map(package_id).unwrap_or("");
    let command_view = invocation.source_view.as_str();
    let plugin_root = config
        .plugin_root(command_view)
        .map(|path| path.to_path_buf());
    let mut environment = vec![
        ("LAUNCHER_ITEM".to_string(), item_text),
        ("LAUNCHER_VALUE".to_string(), value.to_string()),
        ("LAUNCHER_METADATA".to_string(), metadata),
        (
            "LAUNCHER_PLUGIN".to_string(),
            package_id(command_view).to_string(),
        ),
        ("LAUNCHER_VIEW".to_string(), active_view.to_string()),
        ("LAUNCHER_VIEW_REF".to_string(), command_view.to_string()),
        (
            "LAUNCHER_ITEM_VIEW_REF".to_string(),
            item_view.unwrap_or("").to_string(),
        ),
        ("LAUNCHER_ITEM_PLUGIN".to_string(), item_plugin.to_string()),
        ("LAUNCHER_COMMAND".to_string(), invocation.id.clone()),
        ("LAUNCHER_QUERY".to_string(), binding_raw.to_string()),
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

/// Resolve a key against views in priority order (first match wins).
pub(super) fn find_command_for_key_on_views<'a>(
    config: &Config,
    views: impl IntoIterator<Item = &'a str>,
    key: Key,
) -> Option<CommandInvocation> {
    let key_name = key.binding_name()?;
    for view_ref in views {
        if let Some(command) = find_command(config, view_ref, &key_name) {
            return Some(command);
        }
    }
    None
}

/// Collect page commands, then overwrite normalized-key conflicts with owner commands.
/// The returned values are the same public command objects used by runtime and Ctrl-K.
pub(super) fn collect_page_owner_commands(
    config: &Config,
    page_view: &str,
    owner_view: Option<&str>,
) -> Result<BTreeMap<String, Value>> {
    let mut commands = BTreeMap::new();
    let Some(page) = config.view(page_view) else {
        return Ok(commands);
    };
    for (id, command) in &page.commands {
        let key = normalize_key(&command.key)
            .with_context(|| format!("invalid command key for {page_view}/{id}"))?;
        commands.insert(key, runtime_command_value(page_view, id, command)?);
    }
    if let Some(owner_view) = owner_view.filter(|owner| *owner != page_view) {
        let Some(owner) = config.view(owner_view) else {
            return Ok(commands);
        };
        for (id, command) in &owner.commands {
            let key = normalize_key(&command.key)
                .with_context(|| format!("invalid command key for {owner_view}/{id}"))?;
            commands.insert(key, runtime_command_value(owner_view, id, command)?);
        }
    }
    Ok(commands)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Command, CommandAction as ConfigCommandAction, RunPayload};
    use std::path::Path;

    #[test]
    fn page_command_uses_command_owner_process_context_and_item_provenance() {
        let mut config = Config::load(Path::new("config/config.toml")).unwrap();
        config.views.get_mut("core:default").unwrap().run_shell = Some("bash".to_string());
        let state = config.instantiate_state("core:default").unwrap();
        let invocation = CommandInvocation {
            id: "page".to_string(),
            source_view: "core:default".to_string(),
            command: Command {
                key: "ctrl+r".to_string(),
                label: "Page".to_string(),
                action: ConfigCommandAction::Run {
                    payload: RunPayload {
                        handler: ":".to_string(),
                        shell: None,
                        exit: false,
                    },
                },
            },
        };
        let metadata = serde_json::json!({"kind": "app"});
        let action = prepare_command_action(
            &config,
            invocation,
            CommandContext {
                active_view: "core:default",
                state: &state,
                binding_raw: "committed page query",
                runtime: &Value::Null,
                item: Some(CommandItem {
                    text: "Terminal",
                    value: Some("terminal.desktop"),
                    metadata: &metadata,
                    source_view: "apps:default",
                }),
                log_file: None,
            },
        )
        .unwrap();
        let CommandAction::Execute { prepared, .. } = action else {
            panic!("expected prepared run command");
        };
        let environment = prepared.environment.into_iter().collect::<BTreeMap<_, _>>();
        assert_eq!(prepared.argv[0], "bash");
        assert_eq!(
            prepared.current_dir.as_deref(),
            config.plugin_root("core:default")
        );
        assert_eq!(environment["LAUNCHER_PLUGIN"], "core");
        assert_eq!(environment["LAUNCHER_VIEW"], "core:default");
        assert_eq!(environment["LAUNCHER_VIEW_REF"], "core:default");
        assert_eq!(environment["LAUNCHER_QUERY"], "committed page query");
        assert_eq!(environment["LAUNCHER_ITEM_VIEW_REF"], "apps:default");
        assert_eq!(environment["LAUNCHER_ITEM_PLUGIN"], "apps");
        assert_eq!(environment["LAUNCHER_VALUE"], "terminal.desktop");
    }
}
