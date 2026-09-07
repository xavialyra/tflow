use super::{
    CompiledConfig, RawConfig, WorkflowHeader,
    normalize::{
        normalize_engine_configs, normalize_keymap_tables, normalize_view_keymaps,
        remove_disabled_workflows,
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
    workflow_roots: BTreeMap<String, PathBuf>,
    log_file: Option<PathBuf>,
    theme_selector: Option<String>,
}

impl LoadedConfig {
    pub(crate) fn theme_selector(&self) -> Option<&str> {
        self.theme_selector.as_deref()
    }

    pub(crate) fn compile(self) -> Result<CompiledConfig> {
        let LoadedConfig {
            raw,
            mut merged,
            workflow_roots,
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
        let mut config = CompiledConfig::from_raw(raw, workflow_roots, config_value)
            .context("could not compile dynamic configuration values")?;
        config.log_file = log_file;
        Ok(config)
    }
}

impl CompiledConfig {
    pub(crate) fn load_unvalidated(user_path: &Path) -> Result<LoadedConfig> {
        let user_source = fs::read_to_string(user_path)
            .with_context(|| format!("could not read config {}", user_path.display()))?;
        let mut user_config: toml::Value = toml::from_str(&user_source)
            .with_context(|| format!("cannot parse config {}", user_path.display()))?;
        reject_root_workflows(&user_config)?;
        let disabled_workflows = disabled_workflows(Some(&user_config))?;
        if let Some(table) = user_config.as_table_mut() {
            table.remove("disabled_workflows");
        }
        remove_disabled_workflows(&mut user_config, &disabled_workflows);
        normalize_keymap_tables(&mut user_config)
            .with_context(|| format!("invalid keymap in {}", user_path.display()))?;
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let workflow_directory = user_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("workflows");
        let workflow_roots =
            load_workflow_packages(&mut merged, &workflow_directory, &disabled_workflows)?;
        merge_values(&mut merged, user_config);
        remove_disabled_workflows(&mut merged, &disabled_workflows);

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
            workflow_roots,
            log_file,
            theme_selector,
        })
    }
}

pub(super) fn reject_root_workflows(user_config: &toml::Value) -> Result<()> {
    if user_config.get("workflows").is_some() || user_config.get("plugins").is_some() {
        bail!(
            "root configuration cannot define plugins or workflows; use the sibling workflows/ directory with single-file workflows (<id>.toml) or directory packages (<id>/workflow.toml)"
        );
    }
    Ok(())
}

pub(super) fn disabled_workflows(user_config: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let mut disabled = BTreeSet::new();
    let Some(table) = user_config.and_then(toml::Value::as_table) else {
        return Ok(disabled);
    };

    if let Some(value) = table.get("disabled_workflows") {
        let entries = value
            .as_array()
            .with_context(|| "disabled_workflows must be an array of workflow IDs")?;
        for entry in entries {
            let entry = entry
                .as_str()
                .with_context(|| "disabled_workflows entries must be strings")?;
            disabled.insert(entry.to_string());
        }
    }

    Ok(disabled)
}

pub(super) fn load_workflow_packages(
    merged: &mut toml::Value,
    directory: &Path,
    disabled: &BTreeSet<String>,
) -> Result<BTreeMap<String, PathBuf>> {
    if !directory.exists() {
        return Ok(BTreeMap::new());
    }
    if !directory.is_dir() {
        bail!("workflow path {:?} is not a directory", directory);
    }

    enum WorkflowCandidate {
        SingleFile {
            id: String,
            path: PathBuf,
        },
        Directory {
            id: String,
            manifest: PathBuf,
            root: PathBuf,
        },
    }

    let mut candidates = Vec::new();
    let mut sources_by_id: BTreeMap<String, PathBuf> = BTreeMap::new();

    let mut entries: Vec<_> = fs::read_dir(directory)
        .with_context(|| format!("could not read workflow directory {}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("could not read workflow directory {}", directory.display()))?;
    entries.sort_by_key(|e| e.path());

    for entry in entries {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .with_context(|| format!("workflow file {:?} has no valid name", path.display()))?
                .to_string();
            validate_workflow_id(&id)?;
            if let Some(prev) = sources_by_id.insert(id.clone(), path.clone()) {
                bail!(
                    "duplicate workflow {:?} detected: {:?} conflicts with {:?}",
                    id,
                    path.display(),
                    prev.display()
                );
            }
            candidates.push(WorkflowCandidate::SingleFile { id, path });
        } else if path.is_dir() {
            let manifest = path.join("workflow.toml");
            if manifest.is_file() {
                let id = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .with_context(|| {
                        format!("workflow directory {:?} has no valid name", path.display())
                    })?
                    .to_string();
                validate_workflow_id(&id)?;
                if let Some(prev) = sources_by_id.insert(id.clone(), manifest.clone()) {
                    bail!(
                        "duplicate workflow {:?} detected: {:?} conflicts with {:?}",
                        id,
                        manifest.display(),
                        prev.display()
                    );
                }
                candidates.push(WorkflowCandidate::Directory {
                    id,
                    manifest,
                    root: path,
                });
            }
        }
    }

    let mut roots = BTreeMap::new();
    let mut view_aliases: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();

    for candidate in candidates {
        let (id, manifest, root_dir) = match candidate {
            WorkflowCandidate::SingleFile { id, path } => (id, path, None),
            WorkflowCandidate::Directory { id, manifest, root } => (id, manifest, Some(root)),
        };
        if disabled.contains(&id) {
            continue;
        }
        let (wf_id, package) = read_workflow_package(&manifest, &id)?;

        // Validate view alias conflicts
        if let Some(workflows) = package.get("workflows").and_then(toml::Value::as_table) {
            if let Some(wf) = workflows.get(&wf_id).and_then(toml::Value::as_table) {
                if let Some(views) = wf.get("views").and_then(toml::Value::as_table) {
                    for (view_name, view) in views {
                        let view_ref = format!("{wf_id}:{view_name}");
                        if let Some(alias) = view.get("alias").and_then(toml::Value::as_str) {
                            if let Some((prev_view, prev_src)) = view_aliases
                                .insert(alias.to_string(), (view_ref.clone(), manifest.clone()))
                            {
                                bail!(
                                    "conflicting view alias {:?} defined for {:?} in {:?} conflicts with {:?} in {:?}",
                                    alias,
                                    view_ref,
                                    manifest.display(),
                                    prev_view,
                                    prev_src.display()
                                );
                            }
                        }
                    }
                }
            }
        }

        if let Some(root) = root_dir {
            roots.insert(id, root);
        }
        merge_values(merged, package);
    }
    Ok(roots)
}

pub(super) fn read_workflow_package(
    manifest: &Path,
    workflow_id: &str,
) -> Result<(String, toml::Value)> {
    validate_workflow_id(workflow_id)?;

    let source = fs::read_to_string(manifest)
        .with_context(|| format!("could not read workflow manifest {}", manifest.display()))?;
    let mut value: toml::Value = toml::from_str(&source)
        .with_context(|| format!("could not parse workflow manifest {}", manifest.display()))?;

    if value.get("plugin").is_some() {
        bail!(
            "workflow manifest {} uses legacy [plugin]; workflows must declare [workflow] as of ADR 0001",
            manifest.display()
        );
    }

    let header_value = value.get("workflow").cloned().with_context(|| {
        format!(
            "workflow manifest {} is missing [workflow]",
            manifest.display()
        )
    })?;
    let header: WorkflowHeader = header_value
        .try_into()
        .with_context(|| format!("invalid [workflow] in {}", manifest.display()))?;
    if header.api != 1 {
        bail!(
            "workflow {:?} uses unsupported API version {}",
            workflow_id,
            header.api
        );
    }

    let table = value
        .as_table_mut()
        .context("workflow manifest root is not a table")?;
    table.remove("workflow");
    let mut views = table
        .remove("views")
        .with_context(|| format!("workflow {:?} is missing [views.*]", workflow_id))?;
    normalize_view_keymaps(&mut views, workflow_id)?;

    let mut workflow_table = toml::map::Map::new();
    workflow_table.insert("name".to_string(), toml::Value::String(header.name));
    workflow_table.insert("views".to_string(), views);
    if let Some(styles) = table.remove("styles") {
        workflow_table.insert("styles".to_string(), styles);
    }
    let mut workflows_table = toml::map::Map::new();
    workflows_table.insert(workflow_id.to_string(), toml::Value::Table(workflow_table));
    let mut package_table = toml::map::Map::new();
    package_table.insert("workflows".to_string(), toml::Value::Table(workflows_table));
    Ok((workflow_id.to_string(), toml::Value::Table(package_table)))
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

pub(super) fn validate_workflow_id(workflow_id: &str) -> Result<()> {
    if workflow_id.is_empty()
        || workflow_id.contains(':')
        || workflow_id.chars().any(char::is_whitespace)
    {
        bail!("workflow ID {:?} is not valid", workflow_id);
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
