use crate::cancellation::CancellationToken;
use crate::expression::{
    EvalContext, ExpressionMethods, Template, TreeReferences, evaluate_json_value,
};
use crate::input::Key;
use crate::state::{StateInstance, StateRegistry};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub type ViewRef = String;

pub const ENGINE_PICKER: &str = "picker";
pub const ENGINE_CAPTURE: &str = "capture";
pub const ENGINE_EMBEDDED: &str = "embedded";

#[derive(Debug, Clone)]
pub struct Config {
    pub default_view: ViewRef,
    pub command_view: ViewRef,
    pub views: BTreeMap<ViewRef, View>,
    pub plugins: BTreeMap<String, PluginMetadata>,
    pub(crate) defaults: Defaults,
    pub plugin_roots: BTreeMap<String, PathBuf>,
    pub(crate) config_value: Value,
    pub(crate) input_value: Value,
    pub(crate) state_registry: StateRegistry,
    pub(crate) invocation_state: StateInstance,
}

pub(crate) enum ConfigScope<'a> {
    Root,
    View(&'a StateInstance),
}

pub(crate) struct ConfigReadContext<'a> {
    pub scope: ConfigScope<'a>,
    pub runtime: &'a Value,
    pub input: &'a Value,
    pub cancellation: Option<CancellationToken>,
    /// When set, `this.input` / `this.raw_input` use this binding string
    /// instead of rendering the owner query state (feed default binding).
    pub binding_raw: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct PluginMetadata {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Defaults {
    #[serde(default)]
    pub(crate) picker: PickerDefaults,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct PickerDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EngineSpec {
    #[serde(rename = "type", default = "default_engine_type")]
    pub engine_type: String,
    #[serde(default)]
    pub config: EngineOptions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedSpec {
    pub view: ViewRef,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EngineOptions {
    #[serde(default)]
    pub feeds: Vec<FeedSpec>,
    #[serde(default)]
    pub items: Option<String>,
    #[serde(flatten)]
    pub fields: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub engine: EngineSpec,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default)]
    pub cancel_exit_code: Option<u8>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) query: Option<toml::Table>,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
}

impl View {
    pub(crate) fn selected_engine_type(&self) -> &str {
        &self.engine.engine_type
    }
    pub(crate) fn selected_engine_config(&self) -> &toml::Table {
        &self.engine.config.fields
    }
    pub(crate) fn selected_items(&self) -> Option<&str> {
        self.engine.config.items.as_deref()
    }
    pub(crate) fn selected_feeds(&self) -> &[FeedSpec] {
        &self.engine.config.feeds
    }
    pub(crate) fn is_feeds_page(&self) -> bool {
        !self.selected_feeds().is_empty()
    }
    pub(crate) fn engine_field(&self, field: &str) -> Option<&toml::Value> {
        self.selected_engine_config().get(field)
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CommandAction {
    Run {
        payload: RunPayload,
    },
    Navigate {
        payload: NavigatePayload,
    },
    Complete {
        #[serde(default)]
        payload: Option<CompletePayload>,
    },
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPayload {
    pub handler: String,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub exit: bool,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NavigatePayload {
    pub target: toml::Value,
    #[serde(default)]
    pub query: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletePayload {
    pub handler: String,
    #[serde(default)]
    pub params: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub action: CommandAction,
}

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    #[serde(default = "default_view_name")]
    default_view: String,
    #[serde(default = "default_command_view_name")]
    command_view: String,
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    viewtypes: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct Plugin {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    views: BTreeMap<String, View>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginHeader {
    #[serde(default = "default_plugin_api")]
    api: u32,
    name: String,
}

impl Config {
    pub fn load(user_path: &Path) -> Result<Self> {
        let engines = crate::engine::EngineRegistry::new();
        Self::load_with_engines(user_path, &engines)
    }

    pub(crate) fn load_with_engines(
        user_path: &Path,
        engines: &crate::engine::EngineRegistry,
    ) -> Result<Self> {
        let user_source = fs::read_to_string(user_path)
            .with_context(|| format!("could not read config {}", user_path.display()))?;
        let user_config: toml::Value = toml::from_str(&user_source)
            .with_context(|| format!("cannot parse config {}", user_path.display()))?;
        let disabled_plugins = disabled_plugins(Some(&user_config))?;
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let plugin_directory = user_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plugins");
        let plugin_roots = load_plugin_packages(&mut merged, &plugin_directory, &disabled_plugins)?;
        merge_values(&mut merged, user_config);
        remove_disabled_plugins(&mut merged, &disabled_plugins);

        let mut config_value =
            toml_to_json(&merged).context("merged configuration cannot be represented as JSON")?;
        normalize_engine_configs(&mut config_value);
        let raw: RawConfig = merged
            .try_into()
            .context("merged configuration does not match the picker schema")?;
        let mut config = Self::from_raw(raw, plugin_roots)?;
        config.config_value = config_value;
        config.state_registry = StateRegistry::compile(&config.config_value)?;
        config.validate_with_engines(engines)?;
        Ok(config)
    }

    pub(crate) fn bind_invocation_state(
        &self,
        view_ref: &str,
        arguments: &[String],
    ) -> Result<StateInstance> {
        self.state_registry.bind_cli(view_ref, arguments)
    }

    pub(crate) fn set_invocation(&mut self, input: Value, state: StateInstance) {
        self.input_value = input;
        self.invocation_state = state;
    }

    pub(crate) fn instantiate_state(&self, view_ref: &str) -> Result<StateInstance> {
        self.state_registry.instantiate(view_ref)
    }

    pub(crate) fn query_value(&self, state: &StateInstance) -> Result<Value> {
        self.state_registry.query_value(state)
    }

    pub(crate) fn update_query_value(
        &self,
        state: &mut StateInstance,
        value: &Value,
    ) -> Result<bool> {
        self.state_registry.update_value(state, value)
    }

    pub(crate) fn render_query_input(&self, state: &StateInstance) -> Result<String> {
        self.state_registry.render_input(state)
    }

    pub(crate) fn validate_query_state(&self, state: &StateInstance) -> Result<()> {
        self.state_registry.validate_instance(state)
    }

    pub(crate) fn update_query_input(
        &self,
        state: &mut StateInstance,
        source: &str,
    ) -> Result<bool> {
        self.state_registry.update_input(state, source)
    }

    fn view_config(&self, view_ref: &str) -> Result<&Value> {
        let (plugin, view) = view_ref
            .split_once(':')
            .with_context(|| format!("invalid view reference {:?}", view_ref))?;
        self.config_value
            .get("plugins")
            .and_then(|plugins| plugins.get(plugin))
            .and_then(|plugin| plugin.get("views"))
            .and_then(|views| views.get(view))
            .with_context(|| format!("view {:?} configuration disappeared", view_ref))
    }

    fn from_raw(raw: RawConfig, plugin_roots: BTreeMap<String, PathBuf>) -> Result<Self> {
        if raw.viewtypes.is_some() {
            bail!("viewtypes are no longer supported; configure the engine directly on each view");
        }
        let mut views = BTreeMap::new();
        let mut plugins = BTreeMap::new();
        for (package_id, plugin) in raw.plugins {
            let metadata = PluginMetadata {
                name: plugin.name.unwrap_or_else(|| package_id.clone()),
            };
            for (view_name, view) in plugin.views {
                let view_ref = qualify_view_ref(&package_id, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
            plugins.insert(package_id, metadata);
        }

        Ok(Self {
            default_view: raw.default_view,
            command_view: raw.command_view,
            views,
            plugins,
            defaults: raw.defaults,
            plugin_roots,
            config_value: Value::Object(serde_json::Map::new()),
            input_value: Value::Null,
            state_registry: StateRegistry::default(),
            invocation_state: StateInstance::default(),
        })
    }

    #[cfg(test)]
    pub fn validate(&self) -> Result<()> {
        let engines = crate::engine::EngineRegistry::new();
        self.validate_with_engines(&engines)
    }

    pub(crate) fn validate_with_engines(
        &self,
        engines: &crate::engine::EngineRegistry,
    ) -> Result<()> {
        self.engine(&self.default_view)?;

        let default_picker_bindings = self
            .defaults
            .picker
            .bindings
            .as_ref()
            .map(toml_to_json)
            .transpose()?;
        crate::engine::validate_picker_bindings(default_picker_bindings.as_ref(), None)
            .context("default picker bindings")?;

        for (package_id, plugin) in &self.plugins {
            if plugin.name.trim().is_empty() {
                bail!("plugin package {:?} has an empty name", package_id);
            }
        }

        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if let Some(alias) = &view.alias
                && (alias.trim().is_empty()
                    || alias.contains(':')
                    || alias.chars().any(char::is_whitespace))
            {
                bail!("view {:?} has an invalid alias {:?}", view_ref, alias);
            }
            let feeds = view.selected_feeds();
            let items = view.selected_items();
            if !feeds.is_empty() && items.is_some() {
                bail!("feeds view {:?} cannot define items", view_ref);
            }
            let engine = self.engine(view_ref)?;
            engines.validate_config(view_ref, view)?;
            if engine == ENGINE_PICKER {
                let view_bindings = view
                    .engine_field("bindings")
                    .map(toml_to_json)
                    .transpose()?;
                crate::engine::validate_picker_bindings(
                    default_picker_bindings.as_ref(),
                    view_bindings.as_ref(),
                )
                .with_context(|| format!("view {:?} picker bindings", view_ref))?;
            }
            if engine != ENGINE_PICKER && (!feeds.is_empty() || items.is_some()) {
                bail!(
                    "view {:?} using engine {:?} cannot provide picker items",
                    view_ref,
                    engine
                );
            }
            if let Some(items) = items {
                Template::parse(items)
                    .with_context(|| format!("view {:?} has invalid items expression", view_ref))?;
            }
            if let Some(shell) = &view.run_shell {
                validate_script(shell, "run_shell", view_ref)?;
            }

            let mut seen_feeds = BTreeSet::new();
            for feed in feeds {
                let feed_ref = &feed.view;
                if !seen_feeds.insert(feed_ref.clone()) {
                    bail!(
                        "view {:?} lists feed {:?} more than once",
                        view_ref,
                        feed_ref
                    );
                }
                let feed_view = self.views.get(feed_ref).with_context(|| {
                    format!("view {:?} references missing feed {:?}", view_ref, feed_ref)
                })?;
                if self.engine(feed_ref)? != ENGINE_PICKER {
                    bail!(
                        "view {:?} feed {:?} does not use the picker engine",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.is_feeds_page() {
                    bail!(
                        "view {:?} cannot use feeds view {:?} as a feed",
                        view_ref,
                        feed_ref
                    );
                }
                if feed_view.selected_items().is_none() {
                    bail!("view {:?} feed {:?} must define items", view_ref, feed_ref);
                }
            }

            let mut keys = BTreeMap::new();
            for (command_id, command) in &view.commands {
                if command.label.trim().is_empty() {
                    bail!(
                        "view {:?} command {:?} has an empty label",
                        view_ref,
                        command_id
                    );
                }
                let key = normalize_key(&command.key)
                    .with_context(|| format!("view {:?} command {:?}", view_ref, command_id))?;
                if keys.insert(key.clone(), command_id).is_some() {
                    bail!("view {:?} has duplicate command key {:?}", view_ref, key);
                }
                match &command.action {
                    CommandAction::Run { payload } => {
                        validate_script(
                            &payload.handler,
                            "command handler",
                            &format!("{}:{}", view_ref, command_id),
                        )?;
                    }
                    CommandAction::Navigate { payload } => {
                        validate_navigation_payload(view_ref, command_id, payload, &self.views)?;
                    }
                    CommandAction::Complete { payload } => {
                        if let Some(payload) = payload {
                            validate_result_handler(
                                &payload.handler,
                                &format!("{}:{}", view_ref, command_id),
                            )?;
                            validate_templates(&toml::Value::Table(payload.params.clone()))
                                .with_context(|| {
                                    format!(
                                        "view {:?} command {:?} has invalid completion params",
                                        view_ref, command_id
                                    )
                                })?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub(crate) fn resolve_view(&self, selector: &str) -> Result<ViewRef> {
        if self.views.contains_key(selector) {
            return Ok(selector.to_string());
        }
        let matches = self
            .views
            .iter()
            .filter_map(|(view_ref, view)| {
                (view.alias.as_deref() == Some(selector)).then_some(view_ref.clone())
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [view_ref] => Ok(view_ref.clone()),
            [] => bail!("view {:?} is not configured", selector),
            _ => bail!(
                "view alias {:?} is ambiguous: {}",
                selector,
                matches.join(", ")
            ),
        }
    }

    pub fn engine(&self, view_ref: &str) -> Result<&str> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        Ok(view.selected_engine_type())
    }

    pub(crate) fn get(
        &self,
        context: ConfigReadContext<'_>,
        path: &[&str],
    ) -> Result<Option<Value>> {
        let fallback;
        let (raw, this, script_root) = match context.scope {
            ConfigScope::Root => (&self.config_value, Value::Null, Path::new(".")),
            ConfigScope::View(state) => {
                let raw = match self.view_config(state.view_ref()) {
                    Ok(raw) => raw,
                    Err(_)
                        if self
                            .config_value
                            .as_object()
                            .is_some_and(|value| value.is_empty()) =>
                    {
                        let view = self.view(state.view_ref()).with_context(|| {
                            format!("view {:?} is not configured", state.view_ref())
                        })?;
                        let mut values = serde_json::Map::new();
                        if let Some(items) = view.selected_items() {
                            values.insert("items".to_string(), Value::String(items.to_string()));
                        }
                        for (name, value) in view.selected_engine_config() {
                            values.insert(name.clone(), toml_to_json(value)?);
                        }
                        fallback = Value::Object(values);
                        &fallback
                    }
                    Err(error) => return Err(error),
                };
                (
                    raw,
                    self.this_value(state, context.binding_raw)?,
                    self.plugin_root(state.view_ref())
                        .unwrap_or_else(|| Path::new(".")),
                )
            }
        };
        let Some(value) = path
            .iter()
            .try_fold(raw, |value, segment| value.get(segment))
        else {
            return Ok(None);
        };
        let references = TreeReferences {
            config: &self.config_value,
            this: &this,
            runtime: context.runtime,
            input: context.input,
        };
        let cancellation = context.cancellation.unwrap_or_else(CancellationToken::new);
        let mut methods = ExpressionMethods::with_cancellation(script_root, cancellation);
        let mut evaluator = EvalContext {
            references: &references,
            methods: &mut methods,
        };
        evaluate_json_value(value, &mut evaluator).map(Some)
    }

    pub fn command_view(&self) -> Result<&View> {
        let view = self
            .views
            .get(&self.command_view)
            .with_context(|| format!("command view {:?} is not defined", self.command_view))?;
        if self.engine(&self.command_view)? != ENGINE_PICKER {
            bail!(
                "command view {:?} must use the picker engine",
                self.command_view
            );
        }
        Ok(view)
    }

    pub fn plugin_root(&self, view_ref: &str) -> Option<&Path> {
        let plugin = package_id(view_ref);
        self.plugin_roots.get(plugin).map(PathBuf::as_path)
    }

    pub fn feed_views<'a>(&'a self, view_ref: &str) -> Result<Vec<(String, &'a View)>> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let feeds = view.selected_feeds();
        if feeds.is_empty() {
            return Ok(vec![(view_ref.to_string(), view)]);
        }
        feeds
            .iter()
            .map(|feed| {
                self.views
                    .get(&feed.view)
                    .map(|source| (feed.view.clone(), source))
                    .with_context(|| {
                        format!(
                            "view {:?} references missing feed {:?}",
                            view_ref, feed.view
                        )
                    })
            })
            .collect()
    }

    pub(crate) fn this_value(
        &self,
        state: &StateInstance,
        binding_raw: Option<&str>,
    ) -> Result<Value> {
        // Feed default binding must expose the coordinating page's committed
        // raw string (including ""). Rendering the owner state would replace
        // empty input with schema defaults and break this.raw_input == R.
        let input = match binding_raw {
            Some(raw) => raw.to_string(),
            None => self.render_query_input(state)?,
        };
        Ok(serde_json::json!({
            "ref": state.view_ref(),
            "query": self.query_value(state)?,
            "input": input,
            "raw_input": input,
            "state_revision": state.revision(),
        }))
    }

    pub(crate) fn ephemeral_feed_state(
        &self,
        feed_ref: &str,
        query_input: &str,
    ) -> Result<StateInstance> {
        let mut state = self.instantiate_state(feed_ref)?;
        self.state_registry
            .bind_feed_input(&mut state, query_input)?;
        self.validate_query_state(&state)?;
        Ok(state)
    }
}

fn package_id(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(package, _)| package)
        .unwrap_or(view_ref)
}

fn qualify_view_ref(plugin: &str, view: &str) -> Result<ViewRef> {
    if plugin.trim().is_empty()
        || view.trim().is_empty()
        || plugin.contains(':')
        || view.contains(':')
    {
        bail!("invalid view reference components {:?}:{:?}", plugin, view);
    }
    Ok(format!("{}:{}", plugin, view))
}

fn validate_view_ref(view_ref: &str) -> Result<()> {
    let Some((plugin, view)) = view_ref.split_once(':') else {
        bail!("view reference {:?} must use plugin:view form", view_ref);
    };
    if plugin.is_empty() || view.is_empty() || view.contains(':') {
        bail!("invalid view reference {:?}", view_ref);
    }
    Ok(())
}

pub fn normalize_key(key: &str) -> Result<String> {
    Key::parse_binding(key)?
        .binding_name()
        .with_context(|| format!("unsupported command key {:?}", key))
}

fn disabled_plugins(user_config: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let mut disabled = BTreeSet::new();
    let Some(table) = user_config.and_then(toml::Value::as_table) else {
        return Ok(disabled);
    };

    if table.contains_key("plugin_dirs") {
        bail!("plugin_dirs is no longer supported; use the config directory plugins/ path");
    }
    if let Some(value) = table.get("disabled_plugins") {
        let entries = value
            .as_array()
            .with_context(|| "disabled_plugins must be an array of plugin IDs")?;
        for entry in entries {
            let entry = entry
                .as_str()
                .with_context(|| "disabled_plugins entries must be strings")?;
            disabled.insert(entry.to_string());
        }
    }

    Ok(disabled)
}

fn load_plugin_packages(
    merged: &mut toml::Value,
    directory: &Path,
    disabled: &BTreeSet<String>,
) -> Result<BTreeMap<String, PathBuf>> {
    if !directory.exists() {
        return Ok(BTreeMap::new());
    }
    if !directory.is_dir() {
        bail!("plugin path {:?} is not a directory", directory);
    }

    let mut manifests = Vec::new();
    for entry in fs::read_dir(directory)
        .with_context(|| format!("could not read plugin directory {}", directory.display()))?
    {
        let entry = entry
            .with_context(|| format!("could not read plugin directory {}", directory.display()))?;
        let path = entry.path();
        if path.is_dir() {
            let manifest = path.join("plugin.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();

    let mut roots = BTreeMap::new();
    for manifest in manifests {
        let (plugin_id, package) = read_plugin_package(&manifest)?;
        if disabled.contains(&plugin_id) {
            continue;
        }
        let root = manifest
            .parent()
            .expect("plugin manifest has a parent directory")
            .to_path_buf();
        if roots.insert(plugin_id.clone(), root).is_some() {
            bail!(
                "duplicate plugin {:?} in the user plugin directory",
                plugin_id
            );
        }
        merge_values(merged, package);
    }
    Ok(roots)
}

fn read_plugin_package(manifest: &Path) -> Result<(String, toml::Value)> {
    let root = manifest
        .parent()
        .with_context(|| format!("plugin manifest {} has no parent", manifest.display()))?;
    let plugin_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| {
            format!(
                "plugin directory for {} has no valid name",
                manifest.display()
            )
        })?;
    validate_plugin_id(plugin_id)?;

    let source = fs::read_to_string(manifest)
        .with_context(|| format!("could not read plugin manifest {}", manifest.display()))?;
    let mut value: toml::Value = toml::from_str(&source)
        .with_context(|| format!("could not parse plugin manifest {}", manifest.display()))?;
    let header_value = value
        .get("plugin")
        .cloned()
        .with_context(|| format!("plugin manifest {} is missing [plugin]", manifest.display()))?;
    let header: PluginHeader = header_value
        .try_into()
        .with_context(|| format!("invalid [plugin] in {}", manifest.display()))?;
    if header.api != 1 {
        bail!(
            "plugin {:?} uses unsupported API version {}",
            plugin_id,
            header.api
        );
    }

    let table = value
        .as_table_mut()
        .context("plugin manifest root is not a table")?;
    table.remove("plugin");
    let mut views = table
        .remove("views")
        .with_context(|| format!("plugin {:?} is missing [views.*]", plugin_id))?;
    expand_script_refs(&mut views, root, plugin_id)?;

    let mut plugin_table = toml::map::Map::new();
    plugin_table.insert("name".to_string(), toml::Value::String(header.name));
    plugin_table.insert("views".to_string(), views);
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(plugin_id.to_string(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((plugin_id.to_string(), toml::Value::Table(package_table)))
}

fn expand_script_refs(value: &mut toml::Value, root: &Path, owner: &str) -> Result<()> {
    match value {
        toml::Value::Table(table) => {
            let keys = table.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                let child = table.get_mut(&key).expect("key collected from table");
                if (key == "run" || key == "handler")
                    && let Some(file) = child
                        .as_table()
                        .and_then(|script| script.get("file"))
                        .and_then(toml::Value::as_str)
                {
                    let text = read_plugin_script(root, file, owner, &key)?;
                    *child = toml::Value::String(text);
                    continue;
                }
                expand_script_refs(child, root, owner)?;
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                expand_script_refs(value, root, owner)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn read_plugin_script(root: &Path, relative: &str, owner: &str, field: &str) -> Result<String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!(
            "plugin {:?} {} script path {:?} must stay below the plugin directory",
            owner,
            field,
            relative
        );
    }
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("could not resolve plugin directory {}", root.display()))?;
    let path = root.join(relative_path);
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("could not read plugin script {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!(
            "plugin {:?} {} script path {:?} escapes the plugin directory",
            owner,
            field,
            relative
        );
    }
    fs::read_to_string(&canonical_path)
        .with_context(|| format!("could not read plugin script {}", canonical_path.display()))
}

fn remove_disabled_plugins(value: &mut toml::Value, disabled: &BTreeSet<String>) {
    if let Some(plugins) = value.get_mut("plugins").and_then(toml::Value::as_table_mut) {
        for plugin_id in disabled {
            plugins.remove(plugin_id);
        }
    }
}

fn merge_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge_values(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn toml_to_json(value: &toml::Value) -> Result<Value> {
    serde_json::to_value(value).context("could not convert TOML to JSON")
}

fn normalize_engine_configs(config: &mut Value) {
    let Some(plugins) = config.get_mut("plugins").and_then(Value::as_object_mut) else {
        return;
    };
    for plugin in plugins.values_mut() {
        let Some(views) = plugin.get_mut("views").and_then(Value::as_object_mut) else {
            continue;
        };
        for view in views.values_mut() {
            let Some(view) = view.as_object_mut() else {
                continue;
            };
            let Some(engine) = view.get("engine").cloned() else {
                continue;
            };
            let Some(engine) = engine.as_object() else {
                continue;
            };
            let Some(engine_type) = engine.get("type").cloned() else {
                continue;
            };
            let Some(config) = engine.get("config").and_then(Value::as_object) else {
                continue;
            };
            view.insert("type".to_string(), engine_type);
            for (key, value) in config {
                view.insert(key.clone(), value.clone());
            }
        }
    }
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.is_empty() || plugin_id.contains(':') || plugin_id.chars().any(char::is_whitespace)
    {
        bail!("plugin directory {:?} is not a valid package ID", plugin_id);
    }
    Ok(())
}

fn validate_script(script: &str, kind: &str, owner: &str) -> Result<()> {
    if script.trim().is_empty() {
        bail!("{} has an empty {} script", owner, kind);
    }
    Ok(())
}

fn validate_navigation_payload(
    view_ref: &str,
    command_id: &str,
    payload: &NavigatePayload,
    views: &BTreeMap<ViewRef, View>,
) -> Result<()> {
    validate_templates(&payload.target).with_context(|| {
        format!(
            "view {:?} command {:?} has invalid navigation payload",
            view_ref, command_id
        )
    })?;
    if let Some(query) = &payload.query {
        validate_templates(query)?;
    }
    let target = &payload.target;
    let target = target
        .as_str()
        .context("navigation input target must be a string or expression")?;
    if !target.contains("{{") && !views.contains_key(target) {
        bail!(
            "view {:?} command {:?} references missing view {:?}",
            view_ref,
            command_id,
            target
        );
    }
    Ok(())
}

fn validate_result_handler(handler: &str, owner: &str) -> Result<()> {
    let path = Path::new(handler);
    if handler.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("{} has invalid result handler {:?}", owner, handler);
    }
    Ok(())
}

fn validate_templates(value: &toml::Value) -> Result<()> {
    match value {
        toml::Value::String(source) => {
            Template::parse(source)?;
        }
        toml::Value::Array(values) => {
            for value in values {
                validate_templates(value)?;
            }
        }
        toml::Value::Table(values) => {
            for value in values.values() {
                validate_templates(value)?;
            }
        }
        toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Float(_)
        | toml::Value::Integer(_) => {}
    }
    Ok(())
}

fn default_engine_type() -> String {
    ENGINE_PICKER.to_string()
}

fn default_plugin_api() -> u32 {
    1
}

fn default_view_name() -> String {
    "core:default".to_string()
}

fn default_command_view_name() -> String {
    "core:command".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs};

    fn config(source: &str) -> Config {
        let value: toml::Value = toml::from_str(source).unwrap();
        let raw: RawConfig = value.try_into().unwrap();
        Config::from_raw(raw, BTreeMap::new()).unwrap()
    }

    #[test]
    fn plugin_views_are_namespaced() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        assert_eq!(config.default_view, "core:default");
        assert_eq!(
            config.views["apps:main"].engine.engine_type,
            ENGINE_PICKER.to_string()
        );
        assert_eq!(
            config.views["core:default"].selected_feeds()[0].view,
            "apps:main"
        );
    }

    #[test]
    fn view_type_selects_the_engine_directly() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
"#,
        );
        config.validate().unwrap();
        assert_eq!(config.engine("core:default").unwrap(), ENGINE_PICKER);
        assert_eq!(
            config.view("core:default").unwrap().engine.engine_type,
            ENGINE_PICKER
        );
    }

    #[test]
    fn legacy_viewtypes_are_rejected() {
        let value: toml::Value = toml::from_str(
            r#"
            [viewtypes.picker.engine]
            type = "picker"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
"#,
        )
        .unwrap();
        let raw: RawConfig = value.try_into().unwrap();
        let error = Config::from_raw(raw, BTreeMap::new())
            .expect_err("legacy viewtypes should be rejected");
        assert!(error.to_string().contains("no longer supported"));
    }

    #[test]
    fn engine_rejects_unknown_view_fields() {
        let config = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "capture"
            [plugins.core.views.default.engine.config]
            output = "ok"
            titel = "typo"
"#,
        );
        let error = config
            .validate()
            .expect_err("unknown engine fields should be rejected");
        assert!(error.to_string().contains("unsupported field \"titel\""));
    }

    #[test]
    fn feeds_views_may_define_page_commands() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            type = "run"

            [plugins.core.views.default.commands.open.payload]
            handler = ":"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        config
            .validate()
            .expect("feeds pages may define page-level commands");
    }

    #[test]
    fn nested_feeds_are_rejected() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "core:hub"
            [plugins.core.views.hub]
            [plugins.core.views.hub.engine]
            type = "picker"
            [plugins.core.views.hub.engine.config]
            [[plugins.core.views.hub.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        let error = config
            .validate()
            .expect_err("nested feeds should be rejected");
        assert!(error.to_string().contains("cannot use feeds view"));
    }

    #[test]
    fn feed_unknown_fields_are_rejected() {
        let value: Result<toml::Value, _> = toml::from_str(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            raw = "{{ this:input }}"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        let value = value.expect("toml should parse");
        let error = value
            .try_into::<RawConfig>()
            .expect_err("unknown feed fields should be rejected");
        assert!(
            error.to_string().contains("unknown field") || error.to_string().contains("raw"),
            "error={error}"
        );
    }

    #[test]
    fn feeds_views_cannot_define_items() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            items = "[]"
            [[plugins.core.views.default.engine.config.feeds]]
            view = "apps:main"
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        let error = config
            .validate()
            .expect_err("feeds+items should be rejected");
        assert!(error.to_string().contains("cannot define items"));
    }

    #[test]
    fn legacy_sources_field_is_rejected() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            sources = ["apps:main"]
            [plugins.apps.views.main]
            [plugins.apps.views.main.engine]
            type = "picker"
            [plugins.apps.views.main.engine.config]
            items = "[]"
"#,
        );
        let error = config
            .validate()
            .expect_err("legacy sources should be rejected");
        assert!(
            error.to_string().contains("unsupported field \"sources\""),
            "error={error}"
        );
    }

    #[test]
    fn navigation_target_must_reference_a_configured_view() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            type = "navigate"
            [plugins.core.views.default.commands.open.payload]
            target = "missing:view"
            query = "item"
            "#,
        );
        let error = config
            .validate()
            .expect_err("navigation target must reference a configured view");
        assert!(error.to_string().contains("references missing view"));
    }

    #[test]
    fn duplicate_plugin_names_and_view_aliases_are_allowed() {
        let config = config(
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.package-a]
            name = "template"
            [plugins.package-a.views.default]
            alias = "temp"
            [plugins.package-a.views.default.engine]
            type = "picker"
            [plugins.package-a.views.default.engine.config]
            [plugins.package-b]
            name = "template"
            [plugins.package-b.views.default]
            alias = "temp"
            [plugins.package-b.views.default.engine]
            type = "picker"
            [plugins.package-b.views.default.engine.config]
"#,
        );

        config.validate().unwrap();
        assert_eq!(config.plugins["package-a"].name, "template");
        assert_eq!(config.plugins["package-b"].name, "template");
        assert_eq!(
            config.views["package-a:default"].alias.as_deref(),
            Some("temp")
        );
        assert_eq!(
            config.views["package-b:default"].alias.as_deref(),
            Some("temp")
        );
    }

    #[test]
    fn view_alias_cannot_contain_route_syntax_or_whitespace() {
        for alias in ["", "bad:alias", "bad alias"] {
            let config = config(&format!(
                r#"
                [plugins.core.views.default]
                alias = {alias:?}
                [plugins.core.views.default.engine]
                type = "picker"
                [plugins.core.views.default.engine.config]
"#
            ));
            assert!(
                config.validate().is_err(),
                "alias {alias:?} should be invalid"
            );
        }
    }

    #[test]
    fn command_keys_are_validated_and_normalized() {
        assert_eq!(normalize_key("Alt+C").unwrap(), "alt+c");
        assert_eq!(normalize_key("enter").unwrap(), "enter");
        assert_eq!(normalize_key("Ctrl+R").unwrap(), "ctrl+r");
        assert_eq!(normalize_key("Ctrl+J").unwrap(), "enter");
        assert!(normalize_key("c").is_err());
    }

    #[test]
    fn plugin_api_defaults_to_one() {
        let header: PluginHeader = toml::from_str(
            r#"
            name = "template"
            "#,
        )
        .unwrap();
        assert_eq!(header.api, 1);
    }

    #[test]
    fn plugin_directory_names_are_valid_view_namespace_components() {
        assert!(validate_plugin_id("apps").is_ok());
        assert!(validate_plugin_id("my-app").is_ok());
        assert!(validate_plugin_id("bad:name").is_err());
        assert!(validate_plugin_id("bad name").is_err());
    }

    #[test]
    fn file_backed_plugin_scripts_are_loaded_and_rooted() {
        let root = env::temp_dir().join(format!("tui-launcher-plugin-test-{}", std::process::id()));
        let plugin_root = root.join("plugins/filetest");
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(plugin_root.join("scripts")).unwrap();
        fs::write(
            plugin_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "file test"

            [views.main]
            alias = "file"
            [views.main.engine]
            type = "picker"
            [views.main.engine.config]
            items = '{{ script("scripts/items.sh") }}'
            [views.main.commands.run]
            key = "enter"
            label = "Run"
            type = "run"

            [views.main.commands.run.payload]
            handler = { file = "scripts/run.sh" }
            "#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("scripts/items.sh"),
            "printf '%s\\n' '[{\"label\":\"from file\"}]'\\n",
        )
        .unwrap();
        fs::write(plugin_root.join("scripts/run.sh"), "printf 'run\\n'\\n").unwrap();
        let config_path = root.join("config.toml");
        let default_source = r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [[plugins.core.views.default.engine.config.feeds]]
            view = "filetest:main"
"#;
        fs::write(&config_path, default_source).unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.views["filetest:main"].engine.config.items.as_deref(),
            Some("{{ script(\"scripts/items.sh\") }}")
        );
        let CommandAction::Run { payload } = &config.views["filetest:main"].commands["run"].action
        else {
            panic!("file command did not deserialize as a run action");
        };
        assert_eq!(payload.handler, "printf 'run\\n'\\n");
        assert_eq!(
            config.plugin_root("filetest:main"),
            Some(plugin_root.as_path())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loaded_config_exposes_the_merged_json_tree() {
        let root = env::temp_dir().join(format!("tui-launcher-config-json-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            r#"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [aa.a]
            bb = 1

            [aa.b]
            bb = 2
            "#,
        )
        .unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            crate::expression::apply_path(config.config_value.clone(), "$.aa.*.bb").unwrap(),
            serde_json::json!([1, 2])
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_tables_extend_defaults() {
        let mut base: toml::Value = toml::from_str(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            [plugins.core.views.default.engine]
            type = "picker"
            [plugins.core.views.default.engine.config]
            [plugins.base.views.main]
            [plugins.base.views.main.engine]
            type = "picker"
            [plugins.base.views.main.engine.config]
            items = "{{ runtime:view.current.items }}"
"#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main.engine.config]
            items = "{{ config:items }}"
            "#,
        )
        .unwrap();
        merge_values(&mut base, overlay);
        let raw: RawConfig = base.try_into().unwrap();
        let config = Config::from_raw(raw, BTreeMap::new()).unwrap();
        assert_eq!(
            config.views["base:main"].engine.config.items.as_deref(),
            Some("{{ config:items }}")
        );
    }
}
