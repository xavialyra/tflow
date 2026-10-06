use super::{
    CompiledConfig, HostConfig, RawConfig, RawSettings, RawSuiteManifest, View, Workflow,
    WorkflowHeader, WorkflowMount, normalize::normalize_view_bindings, validate_settings_purity,
};
use crate::identity::{ENV_SETTINGS, ENV_SUITE, PRODUCT};
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct LoadedConfig {
    pub(super) raw: RawConfig,
    pub(crate) workflow_roots: BTreeMap<String, PathBuf>,
    pub(crate) log_file: Option<PathBuf>,
    pub(crate) theme_selector: Option<String>,
    pub(crate) settings_file: Option<PathBuf>,
    pub(crate) suite_styles: BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
    pub(crate) settings_styles:
        BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>>,
    pub(crate) entrypoint_query: Option<serde_json::Value>,
    pub(crate) suite_file: Option<PathBuf>,
}

impl LoadedConfig {
    pub(crate) fn theme_selector(&self) -> Option<&str> {
        self.theme_selector.as_deref()
    }

    pub(crate) fn suite_styles(
        &self,
    ) -> &BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>> {
        &self.suite_styles
    }

    pub(crate) fn settings_styles(
        &self,
    ) -> &BTreeMap<String, BTreeMap<String, crate::ui::theme::RawStyleBinding>> {
        &self.settings_styles
    }

    pub(crate) fn compile(self) -> Result<CompiledConfig> {
        let LoadedConfig {
            raw,
            workflow_roots,
            log_file,
            entrypoint_query,
            suite_file,
            ..
        } = self;
        let mut config = CompiledConfig::from_raw(raw, workflow_roots)
            .context("could not compile static configuration")?;
        config.log_file = log_file;
        config.entrypoint_query = entrypoint_query;
        config.suite_file = suite_file;
        Ok(config)
    }
}

pub(super) fn resolve_settings(
    settings_path: Option<&Path>,
) -> Result<(RawSettings, Option<PathBuf>)> {
    if let Some(path) = settings_path {
        let source = fs::read_to_string(path)
            .with_context(|| format!("could not read settings file {}", path.display()))?;
        let table: toml::Table = toml::from_str(&source)
            .with_context(|| format!("cannot parse settings file {}", path.display()))?;
        validate_settings_purity(&table, path)?;
        let settings: RawSettings = toml::Value::Table(table)
            .try_into()
            .with_context(|| format!("invalid settings schema in {}", path.display()))?;
        let base_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        return Ok((settings, Some(base_dir)));
    }

    let candidate = if let Some(path) = std::env::var_os(ENV_SETTINGS) {
        return resolve_settings(Some(Path::new(&path)));
    } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(path).join(PRODUCT).join("settings.toml");
        if p.is_file() { Some(p) } else { None }
    } else if let Some(home) = std::env::var_os("HOME") {
        let config_home = PathBuf::from(home).join(".config").join(PRODUCT);
        let p = config_home.join("settings.toml");
        if p.is_file() { Some(p) } else { None }
    } else {
        None
    };

    if let Some(path) = candidate.filter(|p| p.is_file()) {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("could not read settings file {}", path.display()))?;
        let table: toml::Table = toml::from_str(&source)
            .with_context(|| format!("cannot parse settings file {}", path.display()))?;
        validate_settings_purity(&table, &path)?;
        let settings: RawSettings = toml::Value::Table(table)
            .try_into()
            .with_context(|| format!("invalid settings schema in {}", path.display()))?;
        let base_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok((settings, Some(base_dir)))
    } else {
        Ok((RawSettings::default(), None))
    }
}

pub(crate) fn parse_atomic_workflow_package(
    source: &str,
    source_name: &str,
    workflow_id: &str,
) -> Result<(WorkflowHeader, Workflow)> {
    validate_workflow_id(workflow_id)?;
    let mut value: toml::Value = toml::from_str(source)
        .with_context(|| format!("could not parse workflow manifest {source_name}"))?;
    let table = value
        .as_table_mut()
        .context("workflow manifest root is not a table")?;

    if table.contains_key("suite") {
        bail!(
            "target {:?} is a suite manifest ([suite]), but -w/--workflow expects an atomic workflow ([workflow]). Tip: use -s/--suite instead.",
            source_name
        );
    }

    let header_value = table
        .remove("workflow")
        .with_context(|| format!("workflow manifest {source_name} is missing [workflow]"))?;
    let header: WorkflowHeader = header_value
        .try_into()
        .with_context(|| format!("invalid [workflow] in {source_name}"))?;
    if header.api != 1 {
        bail!(
            "workflow {:?} uses unsupported API {}",
            workflow_id,
            header.api
        );
    }
    if header.name.trim().is_empty() {
        bail!("workflow {:?} name cannot be empty", workflow_id);
    }
    if header.entrypoint.trim().is_empty() {
        bail!("workflow {:?} entrypoint cannot be empty", workflow_id);
    }

    let mut views_value = table
        .remove("views")
        .with_context(|| format!("workflow {:?} is missing [views.*]", workflow_id))?;
    normalize_view_bindings(&mut views_value, workflow_id)?;

    if let Some(views_table) = views_value.as_table() {
        for (view_name, view_table) in views_table {
            if view_table
                .as_table()
                .is_some_and(|t| t.contains_key("commands"))
            {
                bail!(
                    "view {:?} in workflow {:?} cannot define [commands]; ADR 0006 promotes business commands to workflow root [commands]",
                    view_name,
                    workflow_id
                );
            }
        }
    }

    let views: BTreeMap<String, View> = views_value
        .try_into()
        .with_context(|| format!("invalid [views] in workflow manifest {source_name}"))?;

    for (view_name, view) in &views {
        view.validate_engine_shape(view_name)?;
        if let Some(alias) = &view.alias {
            bail!(
                "view {:?} in workflow {:?} cannot declare alias {:?}; ADR 0005 centralizes aliases in suite manifests ([aliases])",
                view_name,
                workflow_id,
                alias
            );
        }
    }

    if !views.contains_key(&header.entrypoint) {
        bail!(
            "workflow {:?} entrypoint {:?} does not match any declared view in [views]",
            workflow_id,
            header.entrypoint
        );
    }

    let commands: BTreeMap<String, super::Command> = match table.remove("commands") {
        Some(value) => value
            .try_into()
            .with_context(|| format!("invalid [commands] in {source_name}"))?,
        None => BTreeMap::new(),
    };

    let styles: BTreeMap<String, crate::ui::theme::RawStyleBinding> = match table.remove("styles") {
        Some(val) => val
            .try_into()
            .with_context(|| format!("invalid [styles] in {source_name}"))?,
        None => BTreeMap::new(),
    };

    if !table.is_empty() {
        let fields = table.keys().cloned().collect::<Vec<_>>();
        bail!(
            "workflow manifest {} has unsupported fields {:?}",
            source_name,
            fields
        );
    }

    let workflow = Workflow {
        name: Some(header.name.clone()),
        entrypoint: Some(header.entrypoint.clone()),
        views,
        commands,
        styles,
    };
    Ok((header, workflow))
}

pub(super) fn parse_suite_manifest(source: &str, source_name: &str) -> Result<RawSuiteManifest> {
    let value: toml::Value = toml::from_str(source)
        .with_context(|| format!("could not parse suite manifest {source_name}"))?;
    let table = value
        .as_table()
        .context("suite manifest root is not a table")?;

    if table.contains_key("workflow") {
        bail!(
            "target {:?} is an atomic workflow ([workflow]), but -s/--suite expects a suite manifest ([suite]). Tip: use -w/--workflow instead.",
            source_name
        );
    }

    if !table.contains_key("suite") {
        bail!("suite manifest {:?} is missing [suite] header", source_name);
    }

    if table.contains_key("views") {
        bail!(
            "suite manifest {} cannot define [views]; views belong to atomic workflows (ADR 0005)",
            source_name
        );
    }

    let manifest: RawSuiteManifest = value
        .try_into()
        .with_context(|| format!("invalid suite manifest schema in {source_name}"))?;

    if manifest.suite.api != 1 {
        bail!(
            "suite manifest {:?} uses unsupported API {}",
            manifest.suite.name,
            manifest.suite.api
        );
    }
    if manifest.suite.name.trim().is_empty() {
        bail!("suite manifest name cannot be empty");
    }
    let entrypoint = manifest
        .suite
        .entrypoint
        .as_ref()
        .with_context(|| format!("suite manifest {:?} is missing entrypoint", source_name))?;
    if entrypoint.target().trim().is_empty() {
        bail!("suite manifest entrypoint cannot be empty");
    }

    Ok(manifest)
}

impl CompiledConfig {
    pub(crate) fn load_workflow_unvalidated(
        workflow_path: &Path,
        settings_path: Option<&Path>,
    ) -> Result<LoadedConfig> {
        let (source, workflow_id, root_dir) = if workflow_path.is_file() {
            let id = workflow_path
                .file_stem()
                .and_then(|s| s.to_str())
                .with_context(|| {
                    format!(
                        "workflow file {:?} has no valid name",
                        workflow_path.display()
                    )
                })?
                .to_string();
            let s = fs::read_to_string(workflow_path).with_context(|| {
                format!("could not read workflow file {}", workflow_path.display())
            })?;
            let parent = workflow_path.parent().map(Path::to_path_buf);
            (s, id, parent)
        } else if workflow_path.is_dir() {
            let suite_file = workflow_path.join("suite.toml");
            let manifest = workflow_path.join("workflow.toml");
            if suite_file.is_file() && !manifest.is_file() {
                bail!(
                    "target directory {:?} contains suite.toml, but -w/--workflow expects an atomic workflow. Tip: use -s/--suite instead.",
                    workflow_path.display()
                );
            }
            if !manifest.is_file() {
                bail!(
                    "workflow directory {:?} is missing workflow.toml",
                    workflow_path.display()
                );
            }
            let id = workflow_path
                .file_name()
                .and_then(|s| s.to_str())
                .with_context(|| {
                    format!(
                        "workflow directory {:?} has no valid name",
                        workflow_path.display()
                    )
                })?
                .to_string();
            let s = fs::read_to_string(&manifest).with_context(|| {
                format!("could not read workflow manifest {}", manifest.display())
            })?;
            (s, id, Some(workflow_path.to_path_buf()))
        } else {
            bail!("workflow path {:?} does not exist", workflow_path);
        };

        Self::load_workflow_source_unvalidated(
            &source,
            &workflow_id,
            root_dir.as_deref(),
            settings_path,
        )
    }

    pub(crate) fn load_workflow_from_str_unvalidated(
        source: &str,
        workflow_id: &str,
        settings_path: Option<&Path>,
    ) -> Result<LoadedConfig> {
        Self::load_workflow_source_unvalidated(source, workflow_id, None, settings_path)
    }

    fn load_workflow_source_unvalidated(
        source: &str,
        workflow_id: &str,
        workflow_root: Option<&Path>,
        settings_path: Option<&Path>,
    ) -> Result<LoadedConfig> {
        validate_workflow_id(workflow_id)?;

        let (header, workflow) = parse_atomic_workflow_package(source, workflow_id, workflow_id)?;
        let mut workflows = BTreeMap::new();
        workflows.insert(workflow_id.to_string(), workflow);

        inject_builtin_workflows(&mut workflows);

        let mut workflow_roots = BTreeMap::new();
        if let Some(root) = workflow_root {
            workflow_roots.insert(workflow_id.to_string(), root.to_path_buf());
        }

        let entrypoint_ref = format!("{workflow_id}:{}", header.entrypoint);
        let mut aliases = BTreeMap::new();
        aliases.insert(workflow_id.to_string(), entrypoint_ref.clone());
        aliases.insert(header.entrypoint.clone(), entrypoint_ref.clone());

        let (settings, settings_dir) = resolve_settings(settings_path)?;
        let log_file = resolve_log_file(&settings, settings_dir.as_deref());

        let defaults = settings.resolve_defaults();
        let resolved_host_bindings = resolve_host_bindings(
            merge_host_binding_targets([settings.host.as_ref()])?,
            &workflows,
            HostBindingPolicy::Lenient,
        )?;

        let raw = RawConfig {
            entrypoint: Some(entrypoint_ref.clone()),
            chrome_commands_show: settings.chrome_commands_show.clone(),
            image_protocol: settings.image_protocol,
            log_file: log_file.clone(),
            host_bindings: resolved_host_bindings,
            aliases,
            view_aliases: [(entrypoint_ref.clone(), workflow_id.to_string())]
                .into_iter()
                .collect(),
            workflows,
            defaults,
        };

        Ok(LoadedConfig {
            raw,
            workflow_roots,
            log_file,
            theme_selector: settings.theme,
            settings_file: settings_dir.map(|dir| dir.join("settings.toml")),
            suite_styles: BTreeMap::new(),
            settings_styles: settings.styles,
            entrypoint_query: None,
            suite_file: None,
        })
    }

    pub(crate) fn load_suite_unvalidated(
        suite_path: &Path,
        settings_path: Option<&Path>,
    ) -> Result<LoadedConfig> {
        let (suite_file, base_dir) = if suite_path.is_file() {
            (
                suite_path.to_path_buf(),
                suite_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
            )
        } else if suite_path.is_dir() {
            let wf_file = suite_path.join("workflow.toml");
            let s_file = suite_path.join("suite.toml");
            let d_file = suite_path.join("default.toml");
            if wf_file.is_file() && !s_file.is_file() && !d_file.is_file() {
                bail!(
                    "target directory {:?} contains workflow.toml, but -s/--suite expects a suite manifest. Tip: use -w/--workflow instead.",
                    suite_path.display()
                );
            }
            if s_file.is_file() {
                (s_file, suite_path.to_path_buf())
            } else if d_file.is_file() {
                (d_file, suite_path.to_path_buf())
            } else {
                bail!(
                    "suite directory {:?} has no suite.toml or default.toml",
                    suite_path.display()
                );
            }
        } else {
            bail!("suite path {:?} does not exist", suite_path);
        };

        let source = fs::read_to_string(&suite_file)
            .with_context(|| format!("could not read suite manifest {}", suite_file.display()))?;
        let manifest = parse_suite_manifest(&source, &suite_file.display().to_string())?;

        let mut workflows = BTreeMap::new();

        let mut workflow_roots = BTreeMap::new();
        let mut aliases = BTreeMap::new();

        for (member_id, mount) in &manifest.workflows {
            validate_workflow_id(member_id)?;

            let (manifest_path, root_dir) = match mount {
                WorkflowMount::Table(spec) => match (&spec.file, &spec.dir) {
                    (Some(file), None) => {
                        let path = resolve_config_path(&base_dir, file);
                        if !path.is_file() {
                            bail!(
                                "workflow mount {:?} file {:?} does not exist",
                                member_id,
                                path.display()
                            );
                        }
                        let parent = path
                            .parent()
                            .unwrap_or_else(|| Path::new("."))
                            .to_path_buf();
                        (path, parent)
                    }
                    (None, Some(dir)) => {
                        let dir_path = resolve_config_path(&base_dir, dir);
                        if !dir_path.is_dir() {
                            bail!(
                                "workflow mount {:?} dir {:?} does not exist",
                                member_id,
                                dir_path.display()
                            );
                        }
                        let wf = dir_path.join("workflow.toml");
                        if !wf.is_file() {
                            bail!(
                                "workflow mount {:?} directory {:?} is missing workflow.toml",
                                member_id,
                                dir_path.display()
                            );
                        }
                        (wf, dir_path)
                    }
                    (Some(_), Some(_)) => bail!(
                        "workflow mount {:?} must specify either file or dir, not both",
                        member_id
                    ),
                    (None, None) => {
                        bail!("workflow mount {:?} must specify file or dir", member_id)
                    }
                },
                WorkflowMount::String(path) => {
                    let p = resolve_config_path(&base_dir, path);
                    if p.is_file() {
                        let parent = p.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
                        (p, parent)
                    } else if p.is_dir() {
                        let wf = p.join("workflow.toml");
                        if !wf.is_file() {
                            bail!(
                                "workflow mount {:?} directory {:?} is missing workflow.toml",
                                member_id,
                                p.display()
                            );
                        }
                        (wf, p)
                    } else {
                        bail!(
                            "workflow mount {:?} path {:?} does not exist",
                            member_id,
                            p.display()
                        );
                    }
                }
            };

            let wf_source = fs::read_to_string(&manifest_path).with_context(|| {
                format!(
                    "could not read mounted workflow {}",
                    manifest_path.display()
                )
            })?;
            let toml_val: toml::Value = toml::from_str(&wf_source).with_context(|| {
                format!("cannot parse mounted workflow {}", manifest_path.display())
            })?;
            if toml_val.as_table().is_some_and(|t| t.contains_key("suite")) {
                bail!(
                    "suite manifest {:?} cannot mount suite manifest {:?}; suites cannot be nested (strict 2-tier architecture)",
                    suite_file.display(),
                    manifest_path.display()
                );
            }

            let (header, workflow) = parse_atomic_workflow_package(
                &wf_source,
                &manifest_path.display().to_string(),
                member_id,
            )?;

            // Member key shorthand alias: member_id -> member_id:entrypoint
            let member_entry = format!("{member_id}:{}", header.entrypoint);
            aliases.insert(member_id.clone(), member_entry);

            workflow_roots.insert(member_id.clone(), root_dir);
            workflows.insert(member_id.clone(), workflow);
        }

        let mut view_aliases = BTreeMap::new();
        for (alias, target) in &aliases {
            view_aliases.insert(target.clone(), alias.clone());
        }
        // Resolve shorthand targets against members, never against aliases already
        // visited in map order. Renaming an alias must not change its target.
        let member_aliases = aliases.clone();
        // Suite explicit aliases
        for (alias, target) in &manifest.aliases {
            if alias.trim().is_empty()
                || alias.contains(':')
                || alias.chars().any(char::is_whitespace)
            {
                bail!("suite alias {:?} is invalid", alias);
            }
            let target_ref = if target.contains(':') {
                target.clone()
            } else if let Some(shorthand) = member_aliases.get(target) {
                shorthand.clone()
            } else {
                target.clone()
            };
            if let Some(member_target) = member_aliases.get(alias)
                && member_target != &target_ref
            {
                bail!(
                    "suite alias {:?} conflicts with member shorthand targeting {:?}; use a different alias name",
                    alias,
                    member_target
                );
            }
            aliases.insert(alias.clone(), target_ref.clone());
            view_aliases.insert(target_ref, alias.clone());
        }

        // Resolve suite entrypoint
        let suite_entry_spec = manifest
            .suite
            .entrypoint
            .as_ref()
            .context("missing suite entrypoint")?;
        let suite_entry = suite_entry_spec.target();
        let resolved_entrypoint = if suite_entry.contains(':') {
            suite_entry.to_string()
        } else if let Some(target) = aliases.get(suite_entry) {
            target.clone()
        } else {
            suite_entry.to_string()
        };
        let entrypoint_query = suite_entry_spec
            .query()
            .map(|tbl| super::toml_to_json(&toml::Value::Table(tbl.clone())))
            .transpose()?;

        let canonical_suite = suite_file
            .canonicalize()
            .unwrap_or_else(|_| suite_file.clone());
        unsafe {
            std::env::set_var(ENV_SUITE, &canonical_suite);
        }

        let (settings, settings_dir) = resolve_settings(settings_path)?;
        let log_file = resolve_log_file(&settings, settings_dir.as_deref());
        let image_protocol = settings.image_protocol;
        inject_builtin_workflows(&mut workflows);

        let defaults = settings.resolve_defaults();
        let resolved_host_bindings = resolve_host_bindings(
            merge_host_binding_targets([manifest.host.as_ref(), settings.host.as_ref()])?,
            &workflows,
            HostBindingPolicy::Strict,
        )?;

        let raw = RawConfig {
            entrypoint: Some(resolved_entrypoint),
            chrome_commands_show: settings
                .chrome_commands_show
                .clone()
                .or_else(|| manifest.chrome_commands_show.clone()),
            image_protocol,
            log_file: log_file.clone(),
            host_bindings: resolved_host_bindings,
            aliases,
            view_aliases,
            workflows,
            defaults,
        };

        Ok(LoadedConfig {
            raw,
            workflow_roots,
            log_file,
            theme_selector: settings.theme,
            settings_file: settings_dir.map(|dir| dir.join("settings.toml")),
            suite_styles: manifest.styles,
            settings_styles: settings.styles,
            entrypoint_query,
            suite_file: Some(canonical_suite),
        })
    }
}

/// Reads one `[host.bindings]` entry.
///
/// A string names the command this layer binds to the key; `false` declares that
/// this layer has no binding for it. Those stay two different mechanisms: `false`
/// settles at compile time, so the key is never claimed by the host layer and
/// the entry does not exist, whereas a View's `unbind` releases a binding whose
/// entry still exists and stays reachable by identity. Every other value is a
/// configuration error rather than a silent removal.
fn parse_host_binding_target(key: &str, val: &toml::Value) -> Result<Option<String>> {
    match val {
        toml::Value::String(address) => {
            let address = address.trim();
            if address.is_empty() {
                bail!("[host.bindings] {key:?} must be a command id or false");
            }
            Ok(Some(address.to_string()))
        }
        toml::Value::Boolean(false) => Ok(None),
        other => bail!(
            "[host.bindings] {key:?} must be a command id or false, got {}",
            other.type_str()
        ),
    }
}

/// Default host-layer bindings, present unless a `[host.bindings]` entry
/// explicitly disables them.
const DEFAULT_HOST_BINDING_TARGETS: [(&str, &str); 3] = [
    ("ctrl+k", "__commands.palette"),
    ("ctrl+g", "__parameters.edit"),
    ("ctrl+l", "@host:open_companion"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostBindingPolicy {
    /// Drop unresolvable bindings (standalone workflow: no workspace context).
    Lenient,
    /// Reject unresolvable bindings (suite manifest: explicit workspace).
    Strict,
}

/// Injects the builtin `__commands` / `__parameters` workflows unless a user
/// workflow shadows them by convention.
fn inject_builtin_workflows(workflows: &mut BTreeMap<String, Workflow>) {
    if !workflows.contains_key("__commands")
        && let Ok(workflow) = crate::workflow::builtin::builtin_commands_workflow()
    {
        workflows.insert("__commands".to_string(), workflow);
    }
    if !workflows.contains_key("__parameters")
        && let Ok(workflow) = crate::workflow::builtin::builtin_parameters_workflow()
    {
        workflows.insert("__parameters".to_string(), workflow);
    }
}

fn resolve_log_file(settings: &RawSettings, settings_dir: Option<&Path>) -> Option<PathBuf> {
    settings.log_file.as_deref().map(|path| {
        settings_dir.map_or_else(
            || path.to_path_buf(),
            |base| resolve_config_path(base, path),
        )
    })
}

/// Merges default, suite, and settings `[host.bindings]` targets in increasing
/// precedence, so a later file's entry replaces an earlier one for the same key.
/// `false` entries stay as `None`: the host layer declares no binding for that
/// key at all.
fn merge_host_binding_targets<'a>(
    overrides: impl IntoIterator<Item = Option<&'a HostConfig>>,
) -> Result<BTreeMap<String, Option<String>>> {
    let mut targets = DEFAULT_HOST_BINDING_TARGETS
        .iter()
        .map(|(key, target)| (key.to_string(), Some(target.to_string())))
        .collect::<BTreeMap<_, _>>();
    for host in overrides.into_iter().flatten() {
        if let Some(bindings) = &host.bindings {
            for (key, value) in bindings {
                targets.insert(key.clone(), parse_host_binding_target(key, value)?);
            }
        }
    }
    Ok(targets)
}

/// Resolves raw `[host.bindings]` entries into references into the command
/// definition map: canonical physical key -> command FQID.
fn resolve_host_bindings(
    targets: BTreeMap<String, Option<String>>,
    workflows: &BTreeMap<String, Workflow>,
    policy: HostBindingPolicy,
) -> Result<BTreeMap<String, String>> {
    let mut resolved = BTreeMap::new();
    for (raw_key, target) in targets {
        let Some(target_cmd) = target else {
            continue;
        };
        if target_cmd.starts_with("host.") {
            bail!(
                "host binding {:?} is invalid: host actions must use '@host:<action>' syntax (e.g. '@host:{}')",
                target_cmd,
                target_cmd.strip_prefix("host.").unwrap_or(&target_cmd)
            );
        }
        let (canonical_target, is_host_action) =
            if let Some(name) = target_cmd.strip_prefix("@host:") {
                let fqid = format!("host.{name}");
                let is_host = crate::command::host_action_label(&fqid).is_some();
                if !is_host {
                    bail!("unknown host action {:?}", target_cmd);
                }
                (fqid, true)
            } else {
                (target_cmd.clone(), false)
            };
        let resolvable = is_host_action
            || target_cmd
                .split_once('.')
                .is_some_and(|(workflow, command)| {
                    workflows
                        .get(workflow)
                        .is_some_and(|wf| wf.commands.contains_key(command))
                });
        if !resolvable {
            if policy == HostBindingPolicy::Lenient {
                continue;
            }
            bail!(
                "host binding {:?} must target a known workflow.command or @host:<action>",
                target_cmd
            );
        }
        let canonical_key = match crate::input::Key::canonical_binding_name(&raw_key) {
            Ok(key) => key,
            Err(error) => {
                if policy == HostBindingPolicy::Lenient {
                    continue;
                }
                return Err(error)
                    .with_context(|| format!("invalid host binding key {:?}", raw_key));
            }
        };
        resolved.insert(canonical_key, canonical_target);
    }
    Ok(resolved)
}

pub(super) fn validate_workflow_id(workflow_id: &str) -> Result<()> {
    if workflow_id.is_empty()
        || workflow_id.contains(':')
        || workflow_id.contains('.')
        || workflow_id.contains('@')
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
        let dir = if config_path.is_dir() {
            config_path
        } else {
            config_path.parent().unwrap_or_else(|| Path::new("."))
        };
        dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_workflows() -> BTreeMap<String, Workflow> {
        let core: Workflow = toml::from_str(
            r#"
            [commands.open]
            label = "Open"
            type = "return"
            value = "open"
            "#,
        )
        .unwrap();
        let commands: Workflow = toml::from_str(
            r#"
            [commands.palette]
            label = "Commands"
            type = "return"
            value = "palette"
            "#,
        )
        .unwrap();
        BTreeMap::from([
            ("core".to_string(), core),
            ("__commands".to_string(), commands),
        ])
    }

    #[test]
    fn host_binding_merge_applies_settings_over_suite_over_defaults() {
        let suite: HostConfig = toml::from_str(
            r#"
            [bindings]
            "ctrl+k" = "core.open"
            "#,
        )
        .unwrap();
        let settings: HostConfig = toml::from_str(
            r#"
            [bindings]
            "ctrl+k" = false
            "ctrl+j" = "core.open"
            "#,
        )
        .unwrap();

        let targets = merge_host_binding_targets([Some(&suite), Some(&settings)]).unwrap();
        // The settings removal beats both the suite override and the default.
        assert_eq!(targets["ctrl+k"], None);
        assert_eq!(targets["ctrl+j"], Some("core.open".to_string()));
        // An untouched default survives.
        assert_eq!(targets["ctrl+g"], Some("__parameters.edit".to_string()));
    }

    /// `false` is the one way to declare "the host layer has no binding here";
    /// every other non-command value is rejected instead of silently removing
    /// the binding.
    #[test]
    fn host_binding_values_are_command_ids_or_false() {
        for value in ["\"\"", "\" \"", "true", "42", "[\"a\"]"] {
            let host: HostConfig = toml::from_str(&format!(
                r#"
                [bindings]
                "ctrl+k" = {value}
                "#
            ))
            .unwrap();
            let error = merge_host_binding_targets([Some(&host)])
                .expect_err("only a command id or false may appear in [host.bindings]");
            assert!(
                error.to_string().contains("[host.bindings]")
                    && error.to_string().contains("must be a command id or false"),
                "unexpected error for {value}: {error}"
            );
        }

        // The old `"noop"` alias is gone: it now reads as a command id and fails
        // to resolve, instead of quietly removing the binding.
        let host: HostConfig = toml::from_str(
            r#"
            [bindings]
            "ctrl+k" = "noop"
            "#,
        )
        .unwrap();
        let targets = merge_host_binding_targets([Some(&host)]).unwrap();
        assert_eq!(targets["ctrl+k"], Some("noop".to_string()));
        let error = resolve_host_bindings(targets, &sample_workflows(), HostBindingPolicy::Strict)
            .expect_err("`noop` is not a command");
        assert!(
            error.to_string().contains("known workflow.command"),
            "{error}"
        );
    }

    #[test]
    fn host_binding_rejects_bare_host_prefix_and_requires_host_sigil() {
        let mut workflows = sample_workflows();
        inject_builtin_workflows(&mut workflows);

        let host: HostConfig = toml::from_str(
            r#"
            [bindings]
            "ctrl+o" = "host.open_companion"
            "#,
        )
        .unwrap();
        let targets = merge_host_binding_targets([Some(&host)]).unwrap();
        let error = resolve_host_bindings(targets, &workflows, HostBindingPolicy::Strict)
            .expect_err("bare host.open_companion must be rejected");
        assert!(
            error
                .to_string()
                .contains("host actions must use '@host:<action>' syntax"),
            "{error}"
        );

        let host: HostConfig = toml::from_str(
            r#"
            [bindings]
            "ctrl+o" = "@host:open_companion"
            "#,
        )
        .unwrap();
        let targets = merge_host_binding_targets([Some(&host)]).unwrap();
        let resolved = resolve_host_bindings(targets, &workflows, HostBindingPolicy::Strict)
            .expect("@host:open_companion must resolve");
        assert_eq!(resolved["ctrl+o"], crate::command::OPEN_COMPANION);
    }

    #[test]
    fn default_host_bindings_drop_when_their_builtin_is_absent() {
        let resolved = resolve_host_bindings(
            merge_host_binding_targets([]).unwrap(),
            &sample_workflows(),
            HostBindingPolicy::Lenient,
        )
        .unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved["ctrl+k"], "__commands.palette");
        assert_eq!(resolved["ctrl+l"], crate::command::OPEN_COMPANION);
        // `__parameters` is absent from the sample, so its default is dropped.
        assert!(!resolved.contains_key("ctrl+g"));
    }

    #[test]
    fn unresolved_host_bindings_are_lenient_standalone_and_strict_in_a_suite() {
        let mut targets = BTreeMap::new();
        targets.insert("ctrl+x".to_string(), Some("core.missing".to_string()));

        assert!(
            resolve_host_bindings(
                targets.clone(),
                &sample_workflows(),
                HostBindingPolicy::Lenient,
            )
            .unwrap()
            .is_empty()
        );

        let error = resolve_host_bindings(targets, &sample_workflows(), HostBindingPolicy::Strict)
            .expect_err("a suite must reject an unknown host binding target");
        assert!(error.to_string().contains("known workflow.command"));
    }

    #[test]
    fn entrypoint_defaults_to_main_when_omitted() {
        let toml_src = r#"
            [workflow]
            api = 1
            name = "demo"

            [views.main]
            engine = "picker"

            [views.main.picker]
        "#;
        let (header, workflow) = parse_atomic_workflow_package(toml_src, "test", "demo").unwrap();
        assert_eq!(header.entrypoint, "main");
        assert_eq!(workflow.entrypoint.as_deref(), Some("main"));
        assert!(workflow.views.contains_key("main"));
    }

    #[test]
    fn entrypoint_can_be_explicitly_configured() {
        let toml_src = r#"
            [workflow]
            api = 1
            name = "demo"
            entrypoint = "search"

            [views.search]
            engine = "picker"

            [views.search.picker]
        "#;
        let (header, workflow) = parse_atomic_workflow_package(toml_src, "test", "demo").unwrap();
        assert_eq!(header.entrypoint, "search");
        assert_eq!(workflow.entrypoint.as_deref(), Some("search"));
        assert!(workflow.views.contains_key("search"));
    }

    #[test]
    fn entrypoint_defaulting_to_main_fails_if_main_view_missing() {
        let toml_src = r#"
            [workflow]
            api = 1
            name = "demo"

            [views.custom]
            engine = "picker"

            [views.custom.picker]
        "#;
        let err = parse_atomic_workflow_package(toml_src, "test", "demo")
            .expect_err("omitted entrypoint requires [views.main]");
        assert!(
            err.to_string()
                .contains("entrypoint \"main\" does not match any declared view"),
            "unexpected error message: {err}"
        );
    }
}
