use super::{
    CompiledConfig, Defaults, ENGINE_PICKER, FeedSpec, RawConfig, View, ViewRef, WorkflowMetadata,
};
use crate::workflow::parameter::ParameterRegistry;
use anyhow::{Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
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
    ) -> Result<Self> {
        Ok(Self {
            entrypoint: entrypoint.clone(),
            default_view: Some(entrypoint),
            image_protocol: super::ImageProtocol::default(),
            log_file: None,
            commands: super::CommandConfig::default(),
            aliases,
            view_aliases,
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
        for (package_id, workflow) in raw.workflows {
            let metadata = WorkflowMetadata {
                name: workflow.name.unwrap_or_else(|| package_id.clone()),
                entrypoint: workflow.entrypoint,
                styles: workflow.styles,
            };
            for (view_name, mut view) in workflow.views {
                for (cmd_id, command) in &mut view.commands {
                    if command.label.is_empty() {
                        command.label = cmd_id.clone();
                    }
                }
                let view_ref = qualify_view_ref(&package_id, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
            workflows.insert(package_id, metadata);
        }
        expand_feed_patterns(&mut views)?;

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
        )?;
        Ok(Self {
            entrypoint: compiled.entrypoint.clone(),
            default_view: compiled.default_view.clone(),
            image_protocol: raw.image_protocol,
            log_file: raw.log_file,
            commands: raw.commands,
            aliases: compiled.aliases,
            view_aliases: compiled.view_aliases,
            views: compiled.views,
            workflows: compiled.workflows,
            defaults: compiled.defaults,
            workflow_roots: compiled.workflow_roots,
            parameter_registry: compiled.parameter_registry,
        })
    }
}

fn expand_feed_patterns(views: &mut BTreeMap<ViewRef, View>) -> Result<()> {
    let view_refs = views.keys().cloned().collect::<Vec<_>>();
    let picker_views = views
        .iter()
        .filter(|(_, view)| view.selected_engine_type() == ENGINE_PICKER)
        .map(|(view_ref, _)| view_ref.clone())
        .collect::<BTreeSet<_>>();
    for (owner_ref, owner) in views.iter_mut() {
        let mut expanded = Vec::new();
        for feed in &owner.engine.config.feeds {
            if let Some(view_name) = feed.view.strip_prefix("*:") {
                if view_name.is_empty() || view_name.contains(':') {
                    bail!(
                        "view {:?} has invalid feed pattern {:?}; expected *:view",
                        owner_ref,
                        feed.view
                    );
                }
                for candidate in &view_refs {
                    if candidate != owner_ref
                        && candidate
                            .split_once(':')
                            .is_some_and(|(_, name)| name == view_name)
                        && picker_views.contains(candidate)
                    {
                        expanded.push(FeedSpec {
                            view: candidate.clone(),
                        });
                    }
                }
            } else if feed.view.contains('*') {
                bail!(
                    "view {:?} has invalid feed pattern {:?}; only *:view is supported",
                    owner_ref,
                    feed.view
                );
            } else {
                expanded.push(feed.clone());
            }
        }
        owner.engine.config.feeds = expanded;
    }
    Ok(())
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
