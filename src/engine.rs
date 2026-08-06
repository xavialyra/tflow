use crate::command::EngineContext;
use crate::config::{Command, Config, ENGINE_LAUNCHER, EngineDefinition, normalize_key};
use crate::discovery::Item;
use crate::expression::validate_json_value;
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

#[path = "engine_runtime.rs"]
mod runtime;
#[path = "engine_session.rs"]
mod session;
#[path = "engine_types.rs"]
mod types;

pub(crate) use runtime::RuntimeStore;
pub(crate) use session::{DiscoveryEvent, LauncherFrame, LauncherSession};
pub(crate) use types::{
    CommandAction, CommandExecution, CommandInvocation, EngineKeyAction, EngineResult, Key,
    LauncherFocus, LauncherRenderState, PreparedCommand,
};

#[path = "engine_provider.rs"]
mod provider;

pub(crate) use provider::DataProviderRegistry;

#[path = "engine_capture.rs"]
mod capture_backend;
#[path = "engine_embedded.rs"]
mod embedded_backend;
#[path = "engine_launcher.rs"]
mod launcher_backend;

use capture_backend::CaptureEngine;
use embedded_backend::EmbeddedEngine;
use launcher_backend::LauncherCommandEngine;

pub(crate) trait Engine {
    fn engine_type(&self) -> &'static str;

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()>;

    fn execute(
        &self,
        execution: CommandExecution,
        context: &mut EngineContext<'_>,
        terminal: &mut Terminal,
    ) -> Result<EngineResult>;
}

pub(crate) trait EngineSession {
    fn handle_key(&mut self, key: Key, command_view: &str) -> EngineKeyAction;

    fn collect_events(&mut self) -> Vec<DiscoveryEvent>;

    fn render(&self, config: &Config, terminal: &Terminal, status: Option<&str>) -> Result<()>;
}

pub(crate) struct EngineRegistry {
    engines: BTreeMap<&'static str, Box<dyn Engine>>,
}

impl EngineRegistry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            engines: BTreeMap::new(),
        };
        registry.register(Box::new(LauncherCommandEngine));
        registry.register(Box::new(CaptureEngine));
        registry.register(Box::new(EmbeddedEngine));
        registry
    }

    pub(crate) fn register(&mut self, engine: Box<dyn Engine>) {
        self.engines.insert(engine.engine_type(), engine);
    }

    pub(crate) fn contains(&self, engine_type: &str) -> bool {
        self.engines.contains_key(engine_type)
    }

    pub(crate) fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        let engine = self
            .engines
            .get(definition.engine_type.as_str())
            .with_context(|| format!("unsupported viewtype engine {:?}", definition.engine_type))?;
        engine.validate_config(name, definition)
    }

    pub(crate) fn execute(
        &self,
        engine_type: &str,
        execution: CommandExecution,
        context: &mut EngineContext<'_>,
        terminal: &mut Terminal,
    ) -> Result<EngineResult> {
        let engine = self
            .engines
            .get(engine_type)
            .with_context(|| format!("unsupported command target engine {:?}", engine_type))?;
        engine.execute(execution, context, terminal)
    }
}

fn validate_fields(name: &str, definition: &EngineDefinition, allowed: &[&str]) -> Result<()> {
    for (field, value) in &definition.config {
        if !allowed.contains(&field.as_str()) {
            bail!(
                "viewtype {:?} engine {:?} has unsupported config field {:?}",
                name,
                definition.engine_type,
                field
            );
        }
        let value = serde_json::to_value(value).context("engine config is not valid JSON")?;
        validate_json_value(&value)
            .with_context(|| format!("viewtype {:?} engine field {:?}", name, field))?;
    }
    Ok(())
}

pub struct LauncherEngine<'a> {
    config: &'a Config,
    view_ref: String,
    runtime: &'a RuntimeStore,
}

impl<'a> LauncherEngine<'a> {
    pub fn new(config: &'a Config, view_ref: &str, runtime: &'a RuntimeStore) -> Self {
        Self {
            config,
            view_ref: view_ref.to_string(),
            runtime,
        }
    }

    pub fn evaluate_field(&self, field: &str) -> Result<Option<Value>> {
        let script_root = self
            .config
            .plugin_root(&self.view_ref)
            .unwrap_or_else(|| Path::new("."));
        let mut providers = DataProviderRegistry::new(
            &self.config.config_value,
            self.runtime.snapshot(),
            self.runtime.revision(),
            script_root,
        );
        self.config.evaluate_engine_field(
            &self.view_ref,
            field,
            self.runtime.snapshot(),
            &mut providers,
        )
    }

    pub(crate) fn handle_key(
        session: &mut LauncherSession,
        key: Key,
        command_view: &str,
    ) -> EngineKeyAction {
        if session.focus() != LauncherFocus::Launcher {
            return EngineKeyAction::Continue;
        }
        match key {
            Key::CtrlC | Key::CtrlD => EngineKeyAction::Exit,
            Key::CtrlK => {
                if session.current().view != command_view {
                    EngineKeyAction::OpenCommandView
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::Escape => {
                if !session.current().input.is_empty() {
                    session.current_mut().input.clear();
                    EngineKeyAction::Refresh
                } else if session.len() > 1 {
                    EngineKeyAction::PopView
                } else {
                    EngineKeyAction::Exit
                }
            }
            Key::Enter | Key::Alt(_) => {
                if session.current().command_owner.is_some() && !matches!(key, Key::Enter) {
                    EngineKeyAction::Continue
                } else {
                    EngineKeyAction::Command(key)
                }
            }
            Key::Up => {
                if !session.current().items.is_empty() {
                    session.current_mut().selected = session.current().selected.saturating_sub(1);
                }
                EngineKeyAction::ClearError
            }
            Key::Down => {
                if !session.current().items.is_empty() {
                    let last = session.current().items.len() - 1;
                    session.current_mut().selected = (session.current().selected + 1).min(last);
                }
                EngineKeyAction::ClearError
            }
            Key::Backspace => {
                if session.current_mut().input.pop().is_some() {
                    EngineKeyAction::Refresh
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::CtrlU => {
                if session.current().input.is_empty() {
                    EngineKeyAction::Continue
                } else {
                    session.current_mut().input.clear();
                    EngineKeyAction::Refresh
                }
            }
            Key::CtrlW => {
                let input = &mut session.current_mut().input;
                let previous_length = input.len();
                while input.chars().last().is_some_and(char::is_whitespace) {
                    input.pop();
                }
                while !input.chars().last().is_some_and(char::is_whitespace) && !input.is_empty() {
                    input.pop();
                }
                if input.len() != previous_length {
                    EngineKeyAction::Refresh
                } else {
                    EngineKeyAction::Continue
                }
            }
            Key::Char(character) if !character.is_control() => {
                session.current_mut().input.push(character);
                EngineKeyAction::Refresh
            }
            Key::Char(_) => EngineKeyAction::Continue,
        }
    }

    pub(crate) fn resolve_command(
        config: &Config,
        session: &LauncherSession,
        key: Key,
    ) -> Option<CommandInvocation> {
        let frame = session.current();
        if frame.command_owner.is_some() {
            let binding = frame
                .items
                .get(frame.selected)
                .and_then(|item| item.value.as_deref())?;
            return find_command(config, frame.command_owner.as_deref()?, binding);
        }
        let key = command_key(key)?;
        find_command(config, session.command_owner()?, &key)
    }

    pub(crate) fn prepare_command_action(
        config: &Config,
        session: &mut LauncherSession,
        key: Key,
        log_file: Option<&Path>,
    ) -> Result<Option<CommandAction>> {
        let Some(invocation) = Self::resolve_command(config, session, key) else {
            return Ok(None);
        };
        let command_view = session.current().command_owner.is_some();
        let item = if command_view {
            let parent = session
                .parent()
                .context("command view has no parent frame")?;
            parent.items.get(parent.selected).cloned()
        } else {
            let frame = session.current();
            frame.items.get(frame.selected).cloned()
        };
        let command = invocation.command.clone();
        let title = format!(
            "{} / {}",
            item.as_ref()
                .map(|item| item.text.as_str())
                .unwrap_or(&invocation.id),
            invocation.id
        );
        if command_view {
            session.pop().context("command view has no parent frame")?;
        }
        let current_view = session.current().view.clone();

        let Some(_script) = command.run.as_ref() else {
            return Ok(Some(match command.view {
                Some(target) => CommandAction::OpenView { target },
                None => CommandAction::Report {
                    message: format!("{} has no command", invocation.id),
                    invocation,
                },
            }));
        };

        let prepared = prepare_command(config, session, &invocation, item.as_ref(), log_file)?;
        let engine_type = if command.exit {
            ENGINE_LAUNCHER.to_string()
        } else {
            command
                .view
                .as_deref()
                .map(|target| config.engine(target))
                .transpose()?
                .unwrap_or(ENGINE_LAUNCHER)
                .to_string()
        };
        let next_view = (!command.exit && engine_type == ENGINE_LAUNCHER)
            .then_some(command.view)
            .flatten()
            .filter(|target| target != &current_view);
        Ok(Some(CommandAction::Execute(CommandExecution {
            invocation,
            prepared,
            engine_type,
            exit: command.exit,
            title,
            next_view,
        })))
    }

    pub(crate) fn publish_runtime(
        config: &Config,
        session: &LauncherSession,
        runtime: &mut RuntimeStore,
    ) -> Result<()> {
        let frame = session.current();
        let owner = session.command_owner().map(str::to_string);
        let commands = owner
            .as_deref()
            .and_then(|owner| config.view(owner).map(|view| (owner, view)))
            .map(|(owner, view)| {
                view.commands
                    .iter()
                    .map(|(id, command)| runtime_command_value(owner, id, command))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let items = frame
            .items
            .iter()
            .map(runtime_item_value)
            .collect::<Vec<_>>();
        let selected_item = frame.items.get(frame.selected).map(runtime_item_value);
        runtime.set(
            "",
            json!({
                "view": {
                    "current": {
                        "ref": frame.view,
                        "input": frame.input,
                        "query": frame.query,
                        "selected_index": frame.selected,
                        "selected_item": selected_item,
                        "items": items,
                        "command": commands,
                        "command_owner": owner,
                    }
                }
            }),
        )?;
        Ok(())
    }

    pub(crate) fn visible_commands(
        config: &Config,
        session: &LauncherSession,
    ) -> Vec<(String, String)> {
        let Some(owner) = session.command_owner() else {
            return Vec::new();
        };
        let mut commands = BTreeMap::new();
        add_view_commands(config, &mut commands, owner);
        let mut commands = commands.into_iter().collect::<Vec<_>>();
        commands.sort_by(|left, right| compare_bindings(&left.0, &right.0));
        commands
    }

    pub(crate) fn render_state(config: &Config, session: &LauncherSession) -> LauncherRenderState {
        let frame = session.current();
        LauncherRenderState {
            view: frame.view.clone(),
            input: frame.input.clone(),
            items: frame.items.clone(),
            selected: frame.selected,
            searching: frame.refresh_deadline.is_some() || frame.discovery_pending,
            commands: Self::visible_commands(config, session),
        }
    }
}

fn prepare_command(
    config: &Config,
    session: &LauncherSession,
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
    let frame = session.current();
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
        ("LAUNCHER_RULE".to_string(), frame.active_rule.clone()),
        ("LAUNCHER_QUERY".to_string(), frame.query.clone()),
        // Keep the old name available to existing scripts.
        (
            "LAUNCHER_PROVIDER".to_string(),
            plugin_name(&invocation.source_view).to_string(),
        ),
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

fn find_command(config: &Config, view_ref: &str, key: &str) -> Option<CommandInvocation> {
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
    match key {
        Key::Enter => Some("enter".to_string()),
        Key::Alt(character) if character.is_ascii_graphic() => {
            Some(format!("alt+{}", character.to_ascii_lowercase()))
        }
        _ => None,
    }
}

fn add_view_commands(config: &Config, commands: &mut BTreeMap<String, String>, view_ref: &str) {
    let Some(view) = config.view(view_ref) else {
        return;
    };
    for command in view.commands.values() {
        if let Ok(key) = normalize_key(&command.key) {
            commands.entry(key).or_insert_with(|| command.label.clone());
        }
    }
}

fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

fn runtime_command_value(owner: &str, id: &str, command: &Command) -> Result<Value> {
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

fn runtime_item_value(item: &Item) -> Value {
    json!({
        "prefix": item.prefix,
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
        "source_view": item.source_view,
    })
}

#[cfg(test)]
mod tests {
    use super::provider::{DataProvider, ProviderContext};
    use super::*;
    use crate::expression::{EvalContext, Template, TreeReferences};
    use std::env;
    use std::fs;

    struct StaticProvider;

    impl DataProvider for StaticProvider {
        fn fetch(
            &self,
            _target: Option<&str>,
            _params: Option<&Value>,
            _context: &ProviderContext<'_>,
        ) -> Result<Value> {
            Ok(serde_json::json!({"source": "static"}))
        }
    }

    #[test]
    fn registry_accepts_custom_engine_implementations() {
        struct TestEngine;

        impl Engine for TestEngine {
            fn engine_type(&self) -> &'static str {
                "test"
            }

            fn validate_config(&self, _name: &str, _definition: &EngineDefinition) -> Result<()> {
                Ok(())
            }

            fn execute(
                &self,
                _execution: CommandExecution,
                _context: &mut EngineContext<'_>,
                _terminal: &mut Terminal,
            ) -> Result<EngineResult> {
                bail!("test engine is not executable in this unit test")
            }
        }

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(TestEngine));
        assert!(registry.engines.contains_key("test"));
    }

    #[test]
    fn runtime_store_tracks_revisions_and_json_pointer_updates() {
        let mut store = RuntimeStore::new();
        assert_eq!(store.revision(), 0);
        assert_eq!(
            store.replace(serde_json::json!({"view": {"current": {}}})),
            1
        );
        assert_eq!(
            store
                .set("/view/current/items", serde_json::json!([1, 2]))
                .unwrap(),
            2
        );
        assert_eq!(
            store.snapshot(),
            &serde_json::json!({"view": {"current": {"items": [1, 2]}}})
        );
        assert_eq!(
            store
                .set("/view/current/items/0", serde_json::json!(3))
                .unwrap(),
            3
        );
        assert_eq!(store.snapshot()["view"]["current"]["items"][0], 3);
    }

    #[test]
    fn registry_resolves_a_runtime_request() {
        let config = Value::Null;
        let runtime = serde_json::json!({"items": [1, 2]});
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
        };
        let mut providers = DataProviderRegistry::new(&config, &runtime, 0, Path::new("."));
        let mut context = EvalContext {
            references: &references,
            methods: &mut providers,
        };
        assert_eq!(
            Template::parse(r#"{{ datafetch(provider = "runtime", match = "$.items") }}"#)
                .unwrap()
                .evaluate_value(&mut context)
                .unwrap(),
            serde_json::json!([1, 2])
        );
    }

    #[test]
    fn registry_accepts_engine_specific_providers() {
        let config = Value::Null;
        let runtime = Value::Null;
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
        };
        let mut providers = DataProviderRegistry::new(&config, &runtime, 0, Path::new("."));
        providers.register("static", StaticProvider);
        let mut context = EvalContext {
            references: &references,
            methods: &mut providers,
        };
        assert_eq!(
            Template::parse(r#"{{ datafetch(provider = "static") }}"#)
                .unwrap()
                .evaluate_value(&mut context)
                .unwrap(),
            serde_json::json!({"source": "static"})
        );
    }

    #[test]
    fn registry_passes_script_params_to_the_provider() {
        let root = env::temp_dir().join(format!(
            "tui-launcher-engine-provider-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("params.sh"), "cat\n").unwrap();
        let config = Value::Null;
        let runtime = Value::Null;
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
        };
        let mut providers = DataProviderRegistry::new(&config, &runtime, 0, &root);
        let mut context = EvalContext {
            references: &references,
            methods: &mut providers,
        };
        assert_eq!(
            Template::parse(
                r#"{{ datafetch(provider = "script", target = "params.sh", params = {query = "fire"}) }}"#,
            )
            .unwrap()
            .evaluate_value(&mut context)
            .unwrap(),
            serde_json::json!({"query": "fire"})
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn script_provider_paths_cannot_escape_the_root() {
        let config = Value::Null;
        let runtime = Value::Null;
        let references = TreeReferences {
            config: &config,
            runtime: &runtime,
        };
        let mut providers = DataProviderRegistry::new(&config, &runtime, 0, Path::new("."));
        let mut context = EvalContext {
            references: &references,
            methods: &mut providers,
        };
        let error =
            Template::parse(r#"{{ datafetch(provider = "script", target = "../test.sh") }}"#)
                .unwrap()
                .evaluate_value(&mut context)
                .expect_err("provider paths must remain below the root");
        assert!(error.to_string().contains("must stay below"));
    }
}
