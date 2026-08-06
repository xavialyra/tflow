use crate::expression::{
    EvalContext, MethodResolver, Template, TreeReferences, evaluate_json_value,
};
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
    pub default_rule: String,
    pub rules: BTreeMap<String, Rule>,
    pub views: BTreeMap<ViewRef, View>,
    pub viewtypes: BTreeMap<String, ViewTypeDefinition>,
    pub plugin_roots: BTreeMap<String, PathBuf>,
    pub(crate) config_value: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    #[serde(default = "default_true")]
    pub filter: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayType {
    #[default]
    Text,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewTypeDefinition {
    pub engine: EngineDefinition,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EngineDefinition {
    #[serde(rename = "type")]
    pub engine_type: String,
    #[serde(default)]
    pub config: toml::Table,
}

#[derive(Debug, Clone, Deserialize)]
pub struct View {
    #[serde(rename = "type", default = "default_viewtype_name")]
    pub view_type: String,
    #[serde(default)]
    pub display: DisplayType,
    #[serde(default)]
    pub sources: Vec<ViewRef>,
    #[serde(default)]
    pub display_prefix: Option<String>,
    #[serde(default)]
    pub discover: Option<String>,
    #[serde(default)]
    pub default_discover: Option<String>,
    #[serde(default)]
    pub query_discover: Option<String>,
    #[serde(default)]
    pub discover_shell: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default = "default_true")]
    pub filter: bool,
    #[serde(default)]
    pub commands: BTreeMap<String, Command>,
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
    #[serde(default = "default_rule_name")]
    default_rule: String,
    #[serde(default)]
    rules: BTreeMap<String, Rule>,
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
    #[serde(default)]
    viewtypes: BTreeMap<String, ViewTypeDefinition>,
    #[serde(default)]
    providers: BTreeMap<String, LegacyProvider>,
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

#[derive(Debug, Clone, Deserialize)]
struct LegacyProvider {
    pub discover: Option<String>,
    pub default_discover: Option<String>,
    pub query_discover: Option<String>,
    pub run: Option<String>,
    #[serde(default)]
    pub display_prefix: Option<String>,
    #[serde(default)]
    pub discover_shell: Option<String>,
    #[serde(default)]
    pub run_shell: Option<String>,
    #[serde(default = "default_true")]
    pub filter: bool,
    #[serde(default)]
    pub mode: LegacyMode,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LegacyMode {
    #[default]
    Oneshot,
    Capture,
    Embedded,
    Takeover,
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
        if !config.rules.contains_key(&config.default_rule) {
            config
                .rules
                .insert(config.default_rule.clone(), Rule { filter: true });
        }
        config.validate_with_engines(engines)?;
        Ok(config)
    }

    fn from_raw(raw: RawConfig, plugin_roots: BTreeMap<String, PathBuf>) -> Result<Self> {
        let mut views = BTreeMap::new();
        for (plugin_name, plugin) in raw.plugins {
            for (view_name, view) in plugin.views {
                let view_ref = qualify_view_ref(&plugin_name, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
        }

        let mut legacy_refs = Vec::new();
        for (provider_name, provider) in raw.providers {
            let view_ref = qualify_view_ref(&provider_name, "default")?;
            let include_in_default = provider.discover.is_some()
                || provider.default_discover.is_some()
                || provider.query_discover.is_some();
            let view = legacy_provider_view(provider_name.as_str(), provider);
            if views.insert(view_ref.clone(), view).is_some() {
                bail!("legacy provider conflicts with view {:?}", view_ref);
            }
            if include_in_default {
                legacy_refs.push(view_ref);
            }
        }

        if !views.contains_key(&raw.default_view) && raw.default_view == "default" {
            let default_view = default_aggregate_view(legacy_refs.clone());
            views.insert("core:default".to_string(), default_view);
        }
        if let Some(default_view) = views.get_mut(&raw.default_view) {
            for legacy_ref in legacy_refs {
                if !default_view.sources.contains(&legacy_ref) {
                    default_view.sources.push(legacy_ref);
                }
            }
        }

        Ok(Self {
            default_view: raw.default_view,
            dmenu_view: raw.dmenu_view,
            command_view: raw.command_view,
            default_rule: raw.default_rule,
            rules: raw.rules,
            views,
            viewtypes: raw.viewtypes,
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
        let default_engine = self.engine(&self.default_view)?;
        if default_engine != ENGINE_LAUNCHER {
            bail!(
                "default view {:?} must use the launcher engine",
                self.default_view
            );
        }
        self.validate_viewtypes(engines)?;
        if !self.rules.contains_key(&self.default_rule) {
            bail!(
                "default rule {:?} is not defined in [rules]",
                self.default_rule
            );
        }

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
            let engine = self.engine(view_ref)?;
            if engine != ENGINE_LAUNCHER
                && (!view.sources.is_empty()
                    || view.discover.is_some()
                    || view.default_discover.is_some()
                    || view.query_discover.is_some())
            {
                bail!(
                    "view {:?} using engine {:?} cannot provide discovery",
                    view_ref,
                    engine
                );
            }
            if let Some(command) = &view.discover {
                validate_script(command, "discover", view_ref)?;
            }
            if let Some(command) = &view.default_discover {
                validate_script(command, "default_discover", view_ref)?;
            }
            if let Some(command) = &view.query_discover {
                validate_script(command, "query_discover", view_ref)?;
            }
            if let Some(shell) = &view.discover_shell {
                validate_script(shell, "discover_shell", view_ref)?;
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
                if command.exit && command.run.is_none() {
                    bail!(
                        "view {:?} command {:?} exits without a run script",
                        view_ref,
                        command_id
                    );
                }
                if command.exit && command.view.is_some() {
                    bail!(
                        "view {:?} command {:?} cannot exit and target another view",
                        view_ref,
                        command_id
                    );
                }
            }
        }

        Ok(())
    }

    fn validate_viewtypes(&self, engines: &crate::engine::EngineRegistry) -> Result<()> {
        if self.viewtypes.is_empty() {
            bail!("no viewtypes are configured");
        }
        for (name, viewtype) in &self.viewtypes {
            validate_viewtype_name(name)?;
            validate_engine(&viewtype.engine.engine_type, engines)
                .with_context(|| format!("viewtype {:?}", name))?;
            engines.validate_config(name, &viewtype.engine)?;
        }
        Ok(())
    }

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub fn viewtype(&self, name: &str) -> Option<&ViewTypeDefinition> {
        self.viewtypes.get(name)
    }

    pub fn engine(&self, view_ref: &str) -> Result<&str> {
        let view = self
            .views
            .get(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        let viewtype = self.viewtypes.get(&view.view_type).with_context(|| {
            format!(
                "view {:?} references missing viewtype {:?}",
                view_ref, view.view_type
            )
        })?;
        Ok(viewtype.engine.engine_type.as_str())
    }

    pub fn evaluate_engine_field(
        &self,
        view_ref: &str,
        field: &str,
        runtime: &Value,
        methods: &mut dyn MethodResolver,
    ) -> Result<Option<Value>> {
        let viewtype_name = self
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?
            .view_type
            .clone();
        let Some(engine) = self
            .viewtype(&viewtype_name)
            .map(|viewtype| &viewtype.engine)
        else {
            return Ok(None);
        };
        let Some(raw) = engine.config.get(field) else {
            return Ok(None);
        };
        let raw = toml_to_json(&toml::Value::Table(
            [(field.to_string(), raw.clone())].into_iter().collect(),
        ))?;
        let references = TreeReferences {
            config: &self.config_value,
            runtime,
        };
        let mut context = EvalContext {
            references: &references,
            methods,
        };
        let raw_field = raw
            .get(field)
            .expect("engine field was inserted into the temporary table");
        let value = if let Some(source) = raw_field.as_str() {
            Template::parse(source)?.evaluate_value(&mut context)?
        } else {
            evaluate_json_value(raw_field, &mut context)?
        };
        Ok(Some(value))
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

    pub fn resolve_rule<'a>(&'a self, _input: &str) -> (&'a str, &'a Rule, String) {
        let rule_name = self.default_rule.as_str();
        (rule_name, &self.rules[rule_name], _input.to_string())
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
                *view_ref != current_view_ref
                    && self.engine(view_ref).ok() == Some(ENGINE_LAUNCHER)
                    && view.display_prefix.as_deref() == Some(prefix)
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

fn legacy_provider_view(_provider_name: &str, provider: LegacyProvider) -> View {
    let mut commands = BTreeMap::new();
    if let Some(run) = provider.run {
        let (view, exit) = match provider.mode {
            LegacyMode::Oneshot => (None, false),
            LegacyMode::Capture => (Some("core:capture".to_string()), false),
            LegacyMode::Embedded => (Some("core:embedded".to_string()), false),
            LegacyMode::Takeover => (None, true),
        };
        commands.insert(
            "default".to_string(),
            Command {
                key: "enter".to_string(),
                label: "Run".to_string(),
                run: Some(run),
                shell: provider.run_shell,
                view,
                exit,
            },
        );
    }
    View {
        view_type: ENGINE_LAUNCHER.to_string(),
        display: DisplayType::Text,
        sources: Vec::new(),
        display_prefix: provider.display_prefix,
        discover: provider.discover,
        default_discover: provider.default_discover,
        query_discover: provider.query_discover,
        discover_shell: provider.discover_shell,
        run_shell: None,
        filter: provider.filter,
        commands,
    }
}

fn default_aggregate_view(source_refs: Vec<ViewRef>) -> View {
    View {
        view_type: ENGINE_LAUNCHER.to_string(),
        display: DisplayType::Text,
        sources: source_refs,
        display_prefix: None,
        discover: None,
        default_discover: None,
        query_discover: None,
        discover_shell: None,
        run_shell: None,
        filter: true,
        commands: BTreeMap::new(),
    }
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
    let key = key.trim().to_ascii_lowercase();
    if key == "enter" {
        return Ok(key);
    }
    if let Some(character) = key.strip_prefix("alt+")
        && character.chars().count() == 1
    {
        let character = character.chars().next().unwrap();
        if character.is_ascii_graphic() {
            return Ok(format!("alt+{}", character));
        }
    }
    bail!(
        "unsupported command key {:?}; use enter or alt+<character>",
        key
    )
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
    plugin_table.insert("views".to_string(), views);
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(plugin_id.to_string(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((plugin_id.to_string(), toml::Value::Table(package_table)))
}

fn expand_script_refs(value: &mut toml::Value, root: &Path, owner: &str) -> Result<()> {
    const SCRIPT_KEYS: [&str; 4] = ["discover", "default_discover", "query_discover", "run"];
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

fn validate_viewtype_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
        bail!("invalid viewtype name {:?}", name);
    }
    Ok(())
}

fn validate_engine(engine: &str, registry: &crate::engine::EngineRegistry) -> Result<()> {
    if registry.contains(engine) {
        Ok(())
    } else {
        bail!("unsupported viewtype engine {:?}", engine)
    }
}

fn default_viewtype_name() -> String {
    ENGINE_LAUNCHER.to_string()
}

fn default_plugin_api() -> u32 {
    1
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

fn default_rule_name() -> String {
    "default".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs};

    fn config(source: &str) -> Config {
        let value: toml::Value = toml::from_str(source).unwrap();
        let mut raw: RawConfig = value.try_into().unwrap();
        if raw.viewtypes.is_empty() {
            raw.viewtypes = test_viewtypes();
        }
        Config::from_raw(raw, BTreeMap::new()).unwrap()
    }

    fn test_viewtypes() -> BTreeMap<String, ViewTypeDefinition> {
        BTreeMap::from([
            (
                ENGINE_LAUNCHER.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_LAUNCHER.to_string(),
                        config: toml::Table::new(),
                    },
                },
            ),
            (
                ENGINE_CAPTURE.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_CAPTURE.to_string(),
                        config: toml::Table::new(),
                    },
                },
            ),
            (
                ENGINE_EMBEDDED.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_EMBEDDED.to_string(),
                        config: toml::Table::new(),
                    },
                },
            ),
        ])
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
            config.views["apps:main"].view_type,
            ENGINE_LAUNCHER.to_string()
        );
        assert_eq!(config.views["core:default"].sources, vec!["apps:main"]);
    }

    #[test]
    fn viewtype_profiles_supply_engines_and_expressions() {
        let config = config(
            r#"
            default_view = "core:default"
            [rules.default]
            filter = true
            [viewtypes.rows.engine]
            type = "launcher"
            [viewtypes.rows.engine.config]
            commands = "{{ config:commands }}"
            [plugins.core.views.default]
            type = "rows"
            "#,
        );
        config.validate().unwrap();
        assert_eq!(config.engine("core:default").unwrap(), ENGINE_LAUNCHER);
        assert_eq!(config.view("core:default").unwrap().view_type, "rows");
        assert_eq!(
            config.viewtype("rows").unwrap().engine.engine_type,
            ENGINE_LAUNCHER
        );
    }

    #[test]
    fn aggregate_views_cannot_define_commands() {
        let config = config(
            r#"
            default_view = "core:default"
            [rules.default]
            filter = true
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
    fn display_prefix_routes_to_a_launcher_view_outside_the_current_sources() {
        let messages = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            [plugins.core.views.messages]
            type = "launcher"
            display_prefix = "log"
            discover = "cat log.jsonl"
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
        assert!(normalize_key("c").is_err());
        assert!(normalize_key("ctrl+c").is_err());
    }

    #[test]
    fn plugin_api_defaults_to_one() {
        let header: PluginHeader = toml::from_str("").unwrap();
        assert_eq!(header.api, 1);
        assert_eq!(PluginHeader::default().api, 1);
    }

    #[test]
    fn plugin_directory_names_are_valid_view_namespace_components() {
        assert!(validate_plugin_id("apps").is_ok());
        assert!(validate_plugin_id("my-app").is_ok());
        assert!(validate_plugin_id("bad:name").is_err());
        assert!(validate_plugin_id("bad name").is_err());
    }

    #[test]
    fn legacy_provider_becomes_namespaced_launcher_view() {
        let config = config(
            r#"
            default_view = "core:default"
            [plugins.core.views.default]
            type = "launcher"
            [providers.apps]
            display_prefix = "app"
            discover = "printf '{}\\n'"
            run = "echo run"
            mode = "capture"
            "#,
        );
        let view = &config.views["apps:default"];
        assert_eq!(view.view_type, ENGINE_LAUNCHER.to_string());
        assert_eq!(
            view.commands["default"].view.as_deref(),
            Some("core:capture")
        );
        assert!(
            config.views["core:default"]
                .sources
                .contains(&"apps:default".to_string())
        );
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
            discover = { file = "scripts/discover.sh" }

            [views.main.commands.run]
            key = "enter"
            label = "Run"
            run = { file = "scripts/run.sh" }
            "#,
        )
        .unwrap();
        fs::write(
            plugin_root.join("scripts/discover.sh"),
            "printf '%s\\n' '{\"label\":\"from file\"}'\\n",
        )
        .unwrap();
        fs::write(plugin_root.join("scripts/run.sh"), "printf 'run\\n'\\n").unwrap();
        let config_path = root.join("config.toml");
        let default_source = r#"
            default_view = "core:default"
            [viewtypes.launcher.engine]
            type = "launcher"
            [viewtypes.capture.engine]
            type = "capture"
            [viewtypes.embedded.engine]
            type = "embedded"
            [plugins.core.views.default]
            type = "launcher"
            sources = ["filetest:main"]
            "#;
        fs::write(&config_path, default_source).unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.views["filetest:main"].discover.as_deref(),
            Some("printf '%s\\n' '{\"label\":\"from file\"}'\\n")
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
            [viewtypes.launcher.engine]
            type = "launcher"
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
            crate::projection::apply_path(config.config_value.clone(), "$.aa.*.bb").unwrap(),
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
            discover = "base-discover"
            "#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
            [plugins.base.views.main]
            query_discover = "user-query"
            "#,
        )
        .unwrap();
        merge_values(&mut base, overlay);
        let raw: RawConfig = base.try_into().unwrap();
        let config = Config::from_raw(raw, BTreeMap::new()).unwrap();
        assert_eq!(
            config.views["base:main"].discover.as_deref(),
            Some("base-discover")
        );
        assert_eq!(
            config.views["base:main"].query_discover.as_deref(),
            Some("user-query")
        );
    }
}
