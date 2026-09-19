use super::{
    CompiledConfig, RawConfig, RawSettings, RawSuiteManifest, View, Workflow, WorkflowHeader,
    WorkflowMount, normalize::normalize_view_keymaps, validate_settings_purity,
};
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
            ..
        } = self;
        let mut config = CompiledConfig::from_raw(raw, workflow_roots)
            .context("could not compile static configuration")?;
        config.log_file = log_file;
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

    let candidate = if let Some(path) = std::env::var_os("TLAUNCH_SETTINGS") {
        return resolve_settings(Some(Path::new(&path)));
    } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(path).join("tlaunch/settings.toml");
        if p.is_file() { Some(p) } else { None }
    } else if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".config/tlaunch/settings.toml");
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

pub(super) fn parse_atomic_workflow_package(
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
    normalize_view_keymaps(&mut views_value, workflow_id)?;

    let mut views: BTreeMap<String, View> = views_value
        .try_into()
        .with_context(|| format!("invalid [views] in workflow manifest {source_name}"))?;

    for (view_name, view) in &views {
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
    for view in views.values_mut() {
        for (id, command) in &commands {
            view.commands
                .entry(id.clone())
                .or_insert_with(|| command.clone());
        }
    }

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
    if manifest.suite.entrypoint.trim().is_empty() {
        bail!("suite manifest entrypoint cannot be empty");
    }

    Ok(manifest)
}

fn load_builtins(workflows: &mut BTreeMap<String, Workflow>) -> Result<()> {
    for &(id, source) in super::builtin::BUILTIN_WORKFLOWS {
        let (_, wf) = parse_atomic_workflow_package(source, &format!("built-in {id}"), id)?;
        workflows.insert(id.to_string(), wf);
    }
    Ok(())
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
        ensure_user_workflow_id_available(workflow_id)?;

        let (header, workflow) = parse_atomic_workflow_package(source, workflow_id, workflow_id)?;
        let mut workflows = BTreeMap::new();
        load_builtins(&mut workflows)?;
        workflows.insert(workflow_id.to_string(), workflow);

        let mut workflow_roots = BTreeMap::new();
        if let Some(root) = workflow_root {
            workflow_roots.insert(workflow_id.to_string(), root.to_path_buf());
        }

        let entrypoint_ref = format!("{workflow_id}:{}", header.entrypoint);
        let mut aliases = BTreeMap::new();
        aliases.insert(workflow_id.to_string(), entrypoint_ref.clone());
        aliases.insert(header.entrypoint.clone(), entrypoint_ref.clone());

        let (settings, settings_dir) = resolve_settings(settings_path)?;
        let log_file = settings.log_file.as_deref().map(|path| {
            if let Some(ref base) = settings_dir {
                resolve_config_path(base, path)
            } else {
                path.to_path_buf()
            }
        });

        let raw = RawConfig {
            default_view: Some(entrypoint_ref.clone()),
            entrypoint: Some(entrypoint_ref.clone()),
            image_protocol: settings.image_protocol,
            log_file: log_file.clone(),
            commands: Default::default(),
            aliases,
            view_aliases: [(entrypoint_ref.clone(), workflow_id.to_string())]
                .into_iter()
                .collect(),
            workflows,
            defaults: settings.defaults,
        };

        Ok(LoadedConfig {
            raw,
            workflow_roots,
            log_file,
            theme_selector: settings.theme,
            settings_file: settings_dir.map(|dir| dir.join("settings.toml")),
            suite_styles: BTreeMap::new(),
            settings_styles: settings.styles,
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
        load_builtins(&mut workflows)?;

        let mut workflow_roots = BTreeMap::new();
        let mut aliases = BTreeMap::new();

        for (member_id, mount) in &manifest.workflows {
            validate_workflow_id(member_id)?;
            ensure_user_workflow_id_available(member_id)?;

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
        let suite_entry = &manifest.suite.entrypoint;
        let resolved_entrypoint = if suite_entry.contains(':') {
            suite_entry.clone()
        } else if let Some(target) = aliases.get(suite_entry) {
            target.clone()
        } else {
            suite_entry.clone()
        };

        let (settings, settings_dir) = resolve_settings(settings_path)?;
        let log_file = settings.log_file.as_deref().map(|path| {
            settings_dir.as_ref().map_or_else(
                || path.to_path_buf(),
                |base| resolve_config_path(base, path),
            )
        });
        let image_protocol = settings.image_protocol;
        let defaults = settings.defaults;

        let commands = Default::default();

        let raw = RawConfig {
            default_view: Some(resolved_entrypoint.clone()),
            entrypoint: Some(resolved_entrypoint),
            image_protocol,
            log_file: log_file.clone(),
            commands,
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
        })
    }
}

fn ensure_user_workflow_id_available(workflow_id: &str) -> Result<()> {
    if workflow_id.starts_with("__") {
        bail!(
            "workflow ID {:?} is reserved for built-in workflows; user workflow IDs cannot start with '__'",
            workflow_id
        );
    }
    Ok(())
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
        let dir = if config_path.is_dir() {
            config_path
        } else {
            config_path.parent().unwrap_or_else(|| Path::new("."))
        };
        dir.join(path)
    }
}
