use super::{
    Config, PluginHeader, RawConfig,
    normalize::{
        normalize_engine_configs, normalize_keymap_tables, normalize_view_keymaps,
        remove_disabled_plugins,
    },
};
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct LoadedConfig {
    raw: RawConfig,
    merged: toml::Value,
    plugin_roots: BTreeMap<String, PathBuf>,
    log_file: Option<PathBuf>,
    theme_selector: Option<String>,
}

impl LoadedConfig {
    pub(crate) fn theme_selector(&self) -> Option<&str> {
        self.theme_selector.as_deref()
    }

    pub(crate) fn compile(self) -> Result<Config> {
        let LoadedConfig {
            raw,
            mut merged,
            plugin_roots,
            log_file,
            ..
        } = self;
        if let Some(table) = merged.as_table_mut() {
            table.remove("theme");
            table.remove("log_file");
        }
        let mut config_value = super::toml_to_json(&merged)
            .context("merged configuration cannot be represented as JSON")?;
        normalize_engine_configs(&mut config_value);
        let mut config = Config::from_raw(raw, plugin_roots, config_value)
            .context("could not compile dynamic configuration values")?;
        config.log_file = log_file;
        Ok(config)
    }
}

impl Config {
    pub(crate) fn load_unvalidated(user_path: &Path) -> Result<LoadedConfig> {
        let user_source = fs::read_to_string(user_path)
            .with_context(|| format!("could not read config {}", user_path.display()))?;
        let mut user_config: toml::Value = toml::from_str(&user_source)
            .with_context(|| format!("cannot parse config {}", user_path.display()))?;
        reject_root_plugins(&user_config)?;
        let disabled_plugins = disabled_plugins(Some(&user_config))?;
        if let Some(table) = user_config.as_table_mut() {
            table.remove("disabled_plugins");
        }
        remove_disabled_plugins(&mut user_config, &disabled_plugins);
        normalize_keymap_tables(&mut user_config)
            .with_context(|| format!("invalid keymap in {}", user_path.display()))?;
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let plugin_directory = user_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("plugins");
        let plugin_roots = load_plugin_packages(&mut merged, &plugin_directory, &disabled_plugins)?;
        merge_values(&mut merged, user_config);
        remove_disabled_plugins(&mut merged, &disabled_plugins);

        let raw: RawConfig = merged.clone().try_into().with_context(|| {
            format!(
                "merged configuration from {} does not match the launcher schema",
                user_path.display()
            )
        })?;
        let theme_selector = raw.theme.clone();
        let log_file = raw
            .log_file
            .as_deref()
            .map(|path| resolve_config_path(user_path, path));
        Ok(LoadedConfig {
            raw,
            merged,
            plugin_roots,
            log_file,
            theme_selector,
        })
    }
}

pub(super) fn reject_root_plugins(user_config: &toml::Value) -> Result<()> {
    if user_config.get("plugins").is_some() {
        bail!(
            "root configuration cannot define plugins; use the sibling plugins/<id>/plugin.toml files"
        );
    }
    Ok(())
}

pub(super) fn disabled_plugins(user_config: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let mut disabled = BTreeSet::new();
    let Some(table) = user_config.and_then(toml::Value::as_table) else {
        return Ok(disabled);
    };

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

pub(super) fn load_plugin_packages(
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
        let plugin_id = manifest
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .with_context(|| {
                format!(
                    "plugin directory for {} has no valid name",
                    manifest.display()
                )
            })?;
        if disabled.contains(plugin_id) {
            continue;
        }
        let (plugin_id, package) = read_plugin_package(&manifest)?;
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

pub(super) fn read_plugin_package(manifest: &Path) -> Result<(String, toml::Value)> {
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
    normalize_view_keymaps(&mut views, plugin_id)?;

    let mut plugin_table = toml::map::Map::new();
    plugin_table.insert("name".to_string(), toml::Value::String(header.name));
    plugin_table.insert("views".to_string(), views);
    if let Some(styles) = table.remove("styles") {
        plugin_table.insert("styles".to_string(), styles);
    }
    let mut plugins_table = toml::map::Map::new();
    plugins_table.insert(plugin_id.to_string(), toml::Value::Table(plugin_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("plugins".to_string(), toml::Value::Table(plugins_table));
    Ok((plugin_id.to_string(), toml::Value::Table(package_table)))
}

pub(super) fn merge_values(base: &mut toml::Value, overlay: toml::Value) {
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

pub(super) fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.is_empty() || plugin_id.contains(':') || plugin_id.chars().any(char::is_whitespace)
    {
        bail!("plugin directory {:?} is not a valid package ID", plugin_id);
    }
    Ok(())
}

pub(super) fn resolve_config_path(config_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    }
}
