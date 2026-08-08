use crate::expression::{
    EvalContext, MethodResolver, Template, TreeReferences, evaluate_json_value,
};
use crate::input::Key;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub type ViewRef = String;

pub const ENGINE_LAUNCHER: &str = "launcher";
pub const ENGINE_CAPTURE: &str = "capture";
pub const ENGINE_EMBEDDED: &str = "embedded";

#[derive(Debug, Clone)]
pub struct Config {
    pub default_view: ViewRef,
    pub dmenu_view: ViewRef,
    pub command_view: ViewRef,
    pub views: BTreeMap<ViewRef, View>,
    pub(crate) defaults: Defaults,
    pub plugin_roots: BTreeMap<String, PathBuf>,
    pub(crate) config_value: Value,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayType {
    #[default]
    Text,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Defaults {
    #[serde(default)]
    pub(crate) launcher: LauncherDefaults,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LauncherDefaults {
    #[serde(default)]
    pub(crate) bindings: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct View {
    #[serde(rename = "type", default = "default_engine_type")]
    pub engine_type: String,
    #[serde(default)]
    pub display: DisplayType,
    #[serde(default)]
    pub sources: Vec<ViewRef>,
    #[serde(default)]
    pub display_prefix: Option<String>,
    #[serde(default)]
    pub items: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
    #[serde(flatten)]
    pub(crate) engine_config: toml::Table,
}

impl View {
    pub(crate) fn engine_field(&self, field: &str) -> Option<&toml::Value> {
        self.engine_config.get(field)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub view: Option<ViewRef>,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub exit: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RawConfig {
    #[serde(default = "default_view_name")]
    default_view: String,
    #[serde(default = "default_dmenu_view_name")]
    dmenu_view: String,
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
    views: BTreeMap<String, View>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginHeader {
    #[serde(default = "default_plugin_api")]
    api: u32,
}

impl Default for PluginHeader {
    fn default() -> Self {
        Self {
            api: default_plugin_api(),
        }
    }
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

        let config_value =
            toml_to_json(&merged).context("merged configuration cannot be represented as JSON")?;
        let raw: RawConfig = merged
            .try_into()
            .context("merged configuration does not match the launcher schema")?;
        let mut config = Self::from_raw(raw, plugin_roots)?;
        config.config_value = config_value;
        config.validate_with_engines(engines)?;
        Ok(config)
    }

    fn from_raw(raw: RawConfig, plugin_roots: BTreeMap<String, PathBuf>) -> Result<Self> {
        if raw.viewtypes.is_some() {
            bail!("viewtypes are no longer supported; configure the engine directly on each view");
        }
        let mut views = BTreeMap::new();
        for (plugin_name, plugin) in raw.plugins {
            for (view_name, view) in plugin.views {
                let view_ref = qualify_view_ref(&plugin_name, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
        }

        Ok(Self {
            default_view: raw.default_view,
            dmenu_view: raw.dmenu_view,
            command_view: raw.command_view,
            views,
            defaults: raw.defaults,
            plugin_roots,
            config_value: Value::Object(serde_json::Map::new()),
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

        let default_launcher_bindings = self
            .defaults
            .launcher
            .bindings
            .as_ref()
            .map(toml_to_json)
            .transpose()?;
        crate::engine::validate_launcher_bindings(default_launcher_bindings.as_ref(), None)
            .context("default launcher bindings")?;

        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if let Some(prefix) = &view.display_prefix
                && (prefix.trim().is_empty() || prefix.chars().any(char::is_whitespace))
            {
                bail!(
                    "view {:?} has an invalid display prefix {:?}",
                    view_ref,
                    prefix
                );
            }
            if !view.sources.is_empty() && !view.commands.is_empty() {
                bail!("aggregate view {:?} cannot define commands", view_ref);
            }
            if !view.sources.is_empty() && view.items.is_some() {
                bail!("aggregate view {:?} cannot define items", view_ref);
            }
            let engine = self.engine(view_ref)?;
            engines.validate_config(view_ref, view)?;
            if engine == ENGINE_LAUNCHER {
                let view_bindings = view
                    .engine_field("bindings")
                    .map(toml_to_json)
                    .transpose()?;
                crate::engine::validate_launcher_bindings(
                    default_launcher_bindings.as_ref(),
                    view_bindings.as_ref(),
                )
                .with_context(|| format!("view {:?} launcher bindings", view_ref))?;
            }
            if engine != ENGINE_LAUNCHER && (!view.sources.is_empty() || view.items.is_some()) {
                bail!(
                    "view {:?} using engine {:?} cannot provide launcher items",
                    view_ref,
                    engine
                );
            }
            if let Some(items) = &view.items {
                Template::parse(items)
                    .with_context(|| format!("view {:?} has invalid items expression", view_ref))?;
            }
            if let Some(shell) = &view.run_shell {
                validate_script(shell, "run_shell", view_ref)?;
            }

            for source_ref in &view.sources {
                let source = self.views.get(source_ref).with_context(|| {
                    format!(
                        "view {:?} references missing source {:?}",
                        view_ref, source_ref
                    )
                })?;
                if self.engine(source_ref)? != ENGINE_LAUNCHER {
                    bail!(
                        "view {:?} source {:?} does not use the launcher engine",
                        view_ref,
                        source_ref
                    );
                }
                if !source.sources.is_empty() {
                    bail!(
                        "view {:?} cannot use aggregate view {:?} as a source",
                        view_ref,
                        source_ref
                    );
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
                if let Some(script) = &command.run {
                    validate_script(
                        script,
                        "command run",
                        &format!("{}:{}", view_ref, command_id),
                    )?;
                }
                if let Some(target) = &command.view
                    && !self.views.contains_key(target)
                {
                    bail!(
                        "view {:?} command {:?} references missing view {:?}",
                        view_ref,
                        command_id,
                        target
                    );
                }
                if command.run.is_none() && command.view.is_none() {
                    bail!(
                        "view {:?} command {:?} has neither a run script nor a target view",
                        view_ref,
                        command_id
                    );
                }
                if command.run.is_some() && command.view.is_some() {
                    bail!(
                        "view {:?} command {:?} cannot combine run and view",
                        view_ref,
                        command_id
                    );
                }
                if command.input.is_some() && command.view.is_none() {
                    bail!(
                        "view {:?} command {:?} has input without a target view",
                        view_ref,
                        command_id
                    );
                }
                if let Some(input) = &command.input {
                    Template::parse(input).with_context(|| {
                        format!(
                            "view {:?} command {:?} has invalid input expression",
                            view_ref, command_id
                        )
                    })?;
                }
                if command.shell.is_some() && command.run.is_none() {
                    bail!(
                        "view {:?} command {:?} selects a shell without a run script",
                        view_ref,
                        command_id
                    );
                }
                if command.exit && command.run.is_none() {
                    bail!(
                        "view {:?} command {:?} exits without a run script",
                        view_ref,
                        command_id
                    );
                }
            }
        }

        Ok(())
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub fn engine(&self, view_ref: &str) -> Result<&str> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        Ok(view.engine_type.as_str())
    }

    pub fn evaluate_view_field(
        &self,
        view_ref: &str,
        field: &str,
        runtime: &Value,
        methods: &mut dyn MethodResolver,
    ) -> Result<Option<Value>> {
        let view = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let Some(raw) = view.engine_field(field) else {
            return Ok(None);
        };
        self.evaluate_config_value(raw, runtime, methods).map(Some)
    }

    pub fn evaluate_default_launcher_bindings(
        &self,
        runtime: &Value,
        methods: &mut dyn MethodResolver,
    ) -> Result<Option<Value>> {
        let Some(raw) = &self.defaults.launcher.bindings else {
            return Ok(None);
        };
        self.evaluate_config_value(raw, runtime, methods).map(Some)
    }

    fn evaluate_config_value(
        &self,
        raw: &toml::Value,
        runtime: &Value,
        methods: &mut dyn MethodResolver,
    ) -> Result<Value> {
        let raw = toml_to_json(raw)?;
        let references = TreeReferences {
            config: &self.config_value,
            runtime,
        };
        let mut context = EvalContext {
            references: &references,
            methods,
        };
        if let Some(source) = raw.as_str() {
            Template::parse(source)?.evaluate_value(&mut context)
        } else {
            evaluate_json_value(&raw, &mut context)
        }
    }

    pub(crate) fn evaluate_command_input(
        &self,
        view_ref: &str,
        source: &str,
        runtime: &Value,
    ) -> Result<String> {
        let root = self.plugin_root(view_ref).unwrap_or_else(|| Path::new("."));
        let mut methods = crate::expression::ExpressionMethods::new(root);
        let references = TreeReferences {
            config: &self.config_value,
            runtime,
        };
        let mut context = EvalContext {
            references: &references,
            methods: &mut methods,
        };
        Template::parse(source)?.evaluate_text(&mut context)
    }

    pub fn evaluate_view_items(
        &self,
        view_ref: &str,
        runtime: &Value,
        methods: &mut dyn MethodResolver,
    ) -> Result<Option<Value>> {
        let Some(source) = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?
            .items
            .as_deref()
        else {
            return Ok(None);
        };
        let references = TreeReferences {
            config: &self.config_value,
            runtime,
        };
        let mut context = EvalContext {
            references: &references,
            methods,
        };
        Ok(Some(Template::parse(source)?.evaluate_value(&mut context)?))
    }

    pub fn dmenu_view(&self) -> Result<&View> {
        let view = self
            .views
            .get(&self.dmenu_view)
            .with_context(|| format!("dmenu view {:?} is not defined", self.dmenu_view))?;
        if self.engine(&self.dmenu_view)? != ENGINE_LAUNCHER {
            bail!(
                "dmenu view {:?} must use the launcher engine",
                self.dmenu_view
            );
        }
        Ok(view)
    }

    pub fn command_view(&self) -> Result<&View> {
        let view = self
            .views
            .get(&self.command_view)
            .with_context(|| format!("command view {:?} is not defined", self.command_view))?;
        if self.engine(&self.command_view)? != ENGINE_LAUNCHER {
            bail!(
                "command view {:?} must use the launcher engine",
                self.command_view
            );
        }
        Ok(view)
    }

    pub fn plugin_root(&self, view_ref: &str) -> Option<&Path> {
        let plugin = view_ref
            .split_once(':')
            .map(|(plugin, _)| plugin)
            .unwrap_or(view_ref);
        self.plugin_roots.get(plugin).map(PathBuf::as_path)
    }

    pub fn source_views<'a>(&'a self, view_ref: &str) -> Result<Vec<(String, &'a View)>> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        if view.sources.is_empty() {
            return Ok(vec![(view_ref.to_string(), view)]);
        }
        view.sources
            .iter()
            .map(|source_ref| {
                self.views
                    .get(source_ref)
                    .map(|source| (source_ref.clone(), source))
                    .with_context(|| {
                        format!(
                            "view {:?} references missing source {:?}",
                            view_ref, source_ref
                        )
                    })
            })
            .collect()
    }

    pub fn resolve_view_prefix(&self, view_ref: &str, input: &str) -> (Option<String>, String) {
        let Some((prefix, query)) = split_prefix(input) else {
            return (None, input.to_string());
        };

        let matches = self
            .source_views(view_ref)
            .ok()
            .into_iter()
            .flatten()
            .find_map(|(source_ref, view)| {
                let display_prefix = view.display_prefix.as_deref().unwrap_or_else(|| {
                    source_ref
                        .split_once(':')
                        .map(|(_, view_name)| view_name)
                        .unwrap_or(source_ref.as_str())
                });
                (display_prefix == prefix).then(|| prefix.to_string())
            });
        if matches.is_some() {
            (matches, query.to_string())
        } else {
            (None, input.to_string())
        }
    }

    pub fn resolve_view_route(
        &self,
        current_view_ref: &str,
        input: &str,
    ) -> Option<(ViewRef, String)> {
        let (target, query) = split_prefix(input)
            .map(|(target, query)| (target, query.to_string()))
            .unwrap_or((input, String::new()));
        if target.contains(':') && target != current_view_ref && self.views.contains_key(target) {
            return Some((target.to_string(), query));
        }

        let (prefix, query) = split_prefix(input)?;
        let source_owns_prefix =
            self.source_views(current_view_ref)
                .ok()?
                .into_iter()
                .any(|(source_ref, view)| {
                    let display_prefix = view.display_prefix.as_deref().unwrap_or_else(|| {
                        source_ref
                            .split_once(':')
                            .map(|(_, view_name)| view_name)
                            .unwrap_or(source_ref.as_str())
                    });
                    display_prefix == prefix
                });
        if source_owns_prefix {
            return None;
        }

        let mut routes = self
            .views
            .iter()
            .filter(|(view_ref, view)| {
                *view_ref != current_view_ref && view.display_prefix.as_deref() == Some(prefix)
            })
            .map(|(view_ref, _)| (view_ref.clone(), query.to_string()));
        let route = routes.next();
        route.filter(|_| routes.next().is_none())
    }
}

fn split_prefix(input: &str) -> Option<(&str, &str)> {
    let (prefix, query) = input.split_once(' ')?;
    (!prefix.is_empty()).then_some((prefix, query.trim_start()))
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
    let header: PluginHeader = if let Some(header_value) = value.get("plugin").cloned() {
        header_value
            .try_into()
            .with_context(|| format!("invalid [plugin] in {}", manifest.display()))?
    } else {
        PluginHeader::default()
    };
    if header.api != 2 {
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
    plugin_table.insert("views".to_string(), views);
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(plugin_id.to_string(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((plugin_id.to_string(), toml::Value::Table(package_table)))
}

fn expand_script_refs(value: &mut toml::Value, root: &Path, owner: &str) -> Result<()> {
    const SCRIPT_KEYS: [&str; 1] = ["run"];
    match value {
        toml::Value::Table(table) => {
            let keys = table.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                let child = table.get_mut(&key).expect("key collected from table");
                if SCRIPT_KEYS.contains(&key.as_str())
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

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.is_empty() || plugin_id.contains(':') || plugin_id.chars().any(char::is_whitespace)
    {
        bail!("plugin directory {:?} is not a valid plugin ID", plugin_id);
    }
    Ok(())
}

fn validate_script(script: &str, kind: &str, owner: &str) -> Result<()> {
    if script.trim().is_empty() {
        bail!("{} has an empty {} script", owner, kind);
    }
    Ok(())
}

fn default_engine_type() -> String {
    ENGINE_LAUNCHER.to_string()
}

fn default_plugin_api() -> u32 {
    2
}

fn default_view_name() -> String {
    "core:default".to_string()
}

fn default_dmenu_view_name() -> String {
    "core:dmenu".to_string()
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
            type = "launcher"
            sources = ["apps:main"]
            [plugins.apps.views.main]
            type = "launcher"
            "#,
        );
        assert_eq!(config.default_view, "core:default");
        assert_eq!(
            config.views["apps:main"].engine_type,
            ENGINE_LAUNCHER.to_string()
        );
        assert_eq!(config.views["core:default"].sources, vec!["apps:main"]);
    }

    #[test]
    fn view_type_selects_the_engine_directly() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            "#,
        );
        config.validate().unwrap();
        assert_eq!(config.engine("core:default").unwrap(), ENGINE_LAUNCHER);
        assert_eq!(
            config.view("core:default").unwrap().engine_type,
            ENGINE_LAUNCHER
        );
    }

    #[test]
    fn legacy_viewtypes_are_rejected() {
        let value: toml::Value = toml::from_str(
            r#"
            [viewtypes.launcher.engine]
            type = "launcher"
            [plugins.core.views.default]
            type = "launcher"
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
            type = "capture"
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
    fn aggregate_views_cannot_define_commands() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            sources = ["apps:main"]
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            run = ":"
            [plugins.apps.views.main]
            type = "launcher"
            "#,
        );
        let error = config
            .validate()
            .expect_err("aggregate commands should be rejected");
        assert!(error.to_string().contains("aggregate view"));
    }

    #[test]
    fn commands_cannot_combine_local_execution_and_navigation() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            [plugins.core.views.default.commands.open]
            key = "enter"
            label = "Open"
            run = ":"
            view = "shell:default"
            [plugins.shell.views.default]
            type = "embedded"
            command = ["sh"]
            "#,
        );
        let error = config
            .validate()
            .expect_err("run and view should be mutually exclusive");
        assert!(error.to_string().contains("cannot combine run and view"));
    }

    #[test]
    fn view_prefix_returns_the_display_prefix_and_query() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            sources = ["apps:main"]
            [plugins.apps.views.main]
            type = "launcher"
            display_prefix = "app"
            "#,
        );
        assert_eq!(
            config.resolve_view_prefix("core:default", "app term"),
            (Some("app".to_string()), "term".to_string())
        );
    }

    #[test]
    fn display_prefix_routes_to_a_view_outside_the_current_sources() {
        let messages = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            [plugins.core.views.messages]
            type = "launcher"
            display_prefix = "log"
            "#,
        );
        assert_eq!(
            messages.resolve_view_route("core:default", "log timeout"),
            Some(("core:messages".to_string(), "timeout".to_string()))
        );
        assert_eq!(
            messages.resolve_view_route("core:messages", "log timeout"),
            None
        );

        let embedded = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            [plugins.shell.views.default]
            type = "embedded"
            command = ["sh"]
            "#,
        );
        assert_eq!(
            embedded.resolve_view_route("core:default", "shell:default ls -la"),
            Some(("shell:default".to_string(), "ls -la".to_string()))
        );

        let aggregate = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            sources = ["apps:main"]
            [plugins.apps.views.main]
            type = "launcher"
            display_prefix = "app"
            "#,
        );
        assert_eq!(
            aggregate.resolve_view_route("core:default", "app term"),
            None
        );
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
    fn plugin_api_defaults_to_two() {
        let header: PluginHeader = toml::from_str("").unwrap();
        assert_eq!(header.api, 2);
        assert_eq!(PluginHeader::default().api, 2);
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
            [views.main]
            type = "launcher"
            items = '{{ script("scripts/items.sh") }}'

            [views.main.commands.run]
            key = "enter"
            label = "Run"
            run = { file = "scripts/run.sh" }
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
            type = "launcher"
            sources = ["filetest:main"]
            "#;
        fs::write(&config_path, default_source).unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.views["filetest:main"].items.as_deref(),
            Some("{{ script(\"scripts/items.sh\") }}")
        );
        assert_eq!(
            config.views["filetest:main"].commands["run"].run.as_deref(),
            Some("printf 'run\\n'\\n")
        );
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
            type = "launcher"

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
            type = "launcher"
            [plugins.base.views.main]
            type = "launcher"
            items = "{{ runtime:view.current.items }}"
            "#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main]
            items = "{{ config:items }}"
            "#,
        )
        .unwrap();
        merge_values(&mut base, overlay);
        let raw: RawConfig = base.try_into().unwrap();
        let config = Config::from_raw(raw, BTreeMap::new()).unwrap();
        assert_eq!(
            config.views["base:main"].items.as_deref(),
            Some("{{ config:items }}")
        );
    }
}
