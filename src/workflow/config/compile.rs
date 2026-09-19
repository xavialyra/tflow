use super::{
    CompiledConfig, Defaults, RawConfig, View, ViewRef, WorkflowMetadata,
};
use crate::workflow::parameter::ParameterRegistry;
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
};

impl CompiledConfig {
    #[allow(clippy::too_many_arguments)]
    fn build(
        views: BTreeMap<ViewRef, View>,
        workflows: BTreeMap<String, WorkflowMetadata>,
        defaults: Defaults,
        workflow_roots: BTreeMap<String, PathBuf>,
        parameter_registry: ParameterRegistry,
        entrypoint: ViewRef,
        aliases: BTreeMap<String, ViewRef>,
        view_aliases: BTreeMap<ViewRef, String>,
        all_commands: BTreeMap<String, super::Command>,
    ) -> Result<Self> {
        Ok(Self {
            entrypoint: entrypoint.clone(),
            default_view: Some(entrypoint),
            entrypoint_query: None,
            suite_file: None,
            image_protocol: super::ImageProtocol::default(),
            log_file: None,
            commands: super::CommandConfig::default(),
            aliases,
            view_aliases,
            all_commands,
            views,
            workflows,
            defaults,
            workflow_roots,
            parameter_registry: Arc::new(parameter_registry),
        })
    }
}

impl CompiledConfig {
    pub(super) fn from_raw(
        raw: RawConfig,
        workflow_roots: BTreeMap<ViewRef, PathBuf>,
    ) -> Result<Self> {
        let parameter_queries = raw
            .workflows
            .iter()
            .flat_map(|(workflow_id, workflow)| {
                workflow.views.iter().map(move |(view_name, view)| {
                    (format!("{workflow_id}:{view_name}"), view.query.clone())
                })
            })
            .collect::<Vec<_>>();
        let parameter_registry = ParameterRegistry::compile_view_queries(parameter_queries)?;
        let mut views = BTreeMap::new();
        let mut workflows = BTreeMap::new();
        let mut all_commands = BTreeMap::new();
        let user_wf_count = raw
            .workflows
            .iter()
            .filter(|(id, _)| !id.starts_with("__"))
            .count();

        for (package_id, workflow) in raw.workflows {
            let metadata = WorkflowMetadata {
                name: workflow.name.unwrap_or_else(|| package_id.clone()),
                entrypoint: workflow.entrypoint,
                styles: workflow.styles,
            };
            for (cmd_id, mut command) in workflow.commands {
                if command.label.is_empty() {
                    command.label = cmd_id.clone();
                }
                let fqid = format!("{package_id}:{cmd_id}");
                all_commands.insert(fqid.clone(), command.clone());
                if user_wf_count <= 1 && !package_id.starts_with("__") {
                    all_commands.insert(cmd_id.clone(), command.clone());
                }
            }
            for (view_name, view) in workflow.views {
                let view_ref = qualify_view_ref(&package_id, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
            workflows.insert(package_id, metadata);
        }

        let aliases = raw.aliases;
        let entrypoint = if let Some(ep) = raw.entrypoint.or(raw.default_view) {
            if views.contains_key(&ep) {
                ep
            } else if let Some(target) = aliases.get(&ep) {
                target.clone()
            } else if !ep.contains(':') {
                let matches: Vec<_> = views
                    .keys()
                    .filter(|k| k.split_once(':').is_some_and(|(_, v)| v == ep))
                    .cloned()
                    .collect();
                if matches.len() == 1 {
                    matches[0].clone()
                } else {
                    bail!("entrypoint {:?} cannot be resolved", ep);
                }
            } else {
                bail!("entrypoint {:?} does not exist in configured views", ep);
            }
        } else if let Some((wf_id, wf_meta)) =
            workflows.iter().find(|(id, _)| !id.starts_with("__"))
            && let Some(ep) = &wf_meta.entrypoint
        {
            let view_ref = format!("{wf_id}:{ep}");
            if views.contains_key(&view_ref) {
                view_ref
            } else {
                views.keys().next().cloned().unwrap_or_default()
            }
        } else {
            views
                .keys()
                .find(|k| !k.starts_with("__"))
                .cloned()
                .unwrap_or_else(|| views.keys().next().cloned().unwrap_or_default())
        };

        let compiled = CompiledConfig::build(
            views,
            workflows,
            raw.defaults,
            workflow_roots,
            parameter_registry,
            entrypoint,
            aliases,
            raw.view_aliases,
            all_commands,
        )?;
        Ok(Self {
            entrypoint: compiled.entrypoint.clone(),
            default_view: compiled.default_view.clone(),
            entrypoint_query: None,
            suite_file: None,
            image_protocol: raw.image_protocol,
            log_file: raw.log_file,
            commands: raw.commands,
            aliases: compiled.aliases,
            view_aliases: compiled.view_aliases,
            all_commands: compiled.all_commands,
            views: compiled.views,
            workflows: compiled.workflows,
            defaults: compiled.defaults,
            workflow_roots: compiled.workflow_roots,
            parameter_registry: compiled.parameter_registry,
        })
    }
}

fn qualify_view_ref(workflow: &str, view: &str) -> Result<ViewRef> {
    if workflow.trim().is_empty()
        || view.trim().is_empty()
        || workflow.contains(':')
        || view.contains(':')
    {
        bail!(
            "invalid view reference components {:?}:{:?}",
            workflow,
            view
        );
    }
    Ok(format!("{}:{}", workflow, view))
}
