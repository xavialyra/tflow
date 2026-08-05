use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub type ViewRef = String;

#[derive(Debug, Clone)]
pub struct Config {
    pub default_view: ViewRef,
    pub dmenu_view: ViewRef,
    pub default_rule: String,
    pub rules: BTreeMap<String, Rule>,
    pub views: BTreeMap<ViewRef, View>,
    pub plugin_roots: BTreeMap<String, PathBuf>,
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

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewType {
    #[default]
    Launcher,
    Capture,
    Embedded,
}

#[derive(Debug, Clone, Deserialize)]
pub struct View {
    #[serde(rename = "type", default)]
    pub view_type: ViewType,
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
    #[serde(default = "default_rule_name")]
    default_rule: String,
    #[serde(default)]
    rules: BTreeMap<String, Rule>,
    #[serde(default)]
    plugins: BTreeMap<String, Plugin>,
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
    id: String,
    api: u32,
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

        let raw: RawConfig = merged
            .try_into()
            .context("merged configuration does not match the launcher schema")?;
        let mut config = Self::from_raw(raw, plugin_roots)?;
        if !config.rules.contains_key(&config.default_rule) {
            config
                .rules
                .insert(config.default_rule.clone(), Rule { filter: true });
        }
        config.validate()?;
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
            default_rule: raw.default_rule,
            rules: raw.rules,
            views,
            plugin_roots,
        })
    }

    pub fn validate(&self) -> Result<()> {
        let default_view = self
            .views
            .get(&self.default_view)
            .with_context(|| format!("default view {:?} is not defined", self.default_view))?;
        if default_view.view_type != ViewType::Launcher {
            bail!(
                "default view {:?} must be a launcher view",
                self.default_view
            );
        }
        if !self.rules.contains_key(&self.default_rule) {
            bail!(
                "default rule {:?} is not defined in [rules]",
                self.default_rule
            );
        }

        for (view_ref, view) in &self.views {
            validate_view_ref(view_ref)?;
            if view.view_type != ViewType::Launcher
                && (!view.sources.is_empty()
                    || view.discover.is_some()
                    || view.default_discover.is_some()
                    || view.query_discover.is_some())
            {
                bail!(
                    "view {:?} of type {:?} cannot provide discovery",
                    view_ref,
                    view.view_type
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
                if source.view_type != ViewType::Launcher {
                    bail!(
                        "view {:?} source {:?} is not a launcher view",
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

    pub fn view(&self, view_ref: &str) -> Option<&View> {
        self.views.get(view_ref)
    }

    pub fn dmenu_view(&self) -> Result<&View> {
        let view = self
            .views
            .get(&self.dmenu_view)
            .with_context(|| format!("dmenu view {:?} is not defined", self.dmenu_view))?;
        if view.view_type != ViewType::Launcher {
            bail!("dmenu view {:?} must be a launcher view", self.dmenu_view);
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
        let Some((prefix, query)) = input.split_once(' ') else {
            return (None, input.to_string());
        };
        if prefix.is_empty() {
            return (None, input.to_string());
        }

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
            (matches, query.trim_start().to_string())
        } else {
            (None, input.to_string())
        }
    }
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
        view_type: ViewType::Launcher,
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
        view_type: ViewType::Launcher,
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
            header.id,
            header.api
        );
    }
    let directory_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| {
            format!(
                "plugin directory for {} has no valid name",
                manifest.display()
            )
        })?;
    if header.id != directory_id {
        bail!(
            "plugin ID {:?} does not match directory {:?}",
            header.id,
            directory_id
        );
    }

    let table = value
        .as_table_mut()
        .context("plugin manifest root is not a table")?;
    table.remove("plugin");
    let mut views = table
        .remove("views")
        .with_context(|| format!("plugin {:?} is missing [views.*]", header.id))?;
    expand_script_refs(&mut views, root, &header.id)?;

    let mut plugin_table = toml::map::Map::new();
    plugin_table.insert("views".to_string(), views);
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(header.id.clone(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((header.id, toml::Value::Table(package_table)))
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

fn validate_script(script: &str, kind: &str, owner: &str) -> Result<()> {
    if script.trim().is_empty() {
        bail!("{} has an empty {} script", owner, kind);
    }
    Ok(())
}

fn default_view_name() -> String {
    "core:default".to_string()
}

fn default_dmenu_view_name() -> String {
    "core:dmenu".to_string()
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
        assert_eq!(config.views["apps:main"].view_type, ViewType::Launcher);
        assert_eq!(config.views["core:default"].sources, vec!["apps:main"]);
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
    fn command_keys_are_validated_and_normalized() {
        assert_eq!(normalize_key("Alt+C").unwrap(), "alt+c");
        assert_eq!(normalize_key("enter").unwrap(), "enter");
        assert!(normalize_key("c").is_err());
        assert!(normalize_key("ctrl+c").is_err());
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
        assert_eq!(view.view_type, ViewType::Launcher);
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
            [plugin]
            id = "filetest"
            api = 1

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
