use super::{
    CommandBinding, CompiledConfig, Defaults, ENGINE_PICKER, FeedSpec, RawConfig, View, ViewRef,
    WorkflowMetadata,
};
use crate::workflow::expression::{EvaluationStage, TemplateRegistry, is_dynamic_string};
use crate::workflow::parameter::ParameterRegistry;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

impl CompiledConfig {
    fn build(
        views: BTreeMap<ViewRef, View>,
        workflows: BTreeMap<String, WorkflowMetadata>,
        defaults: Defaults,
        workflow_roots: BTreeMap<String, PathBuf>,
        config_value: Value,
    ) -> Result<Self> {
        let template_value = if config_value
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            views_config_value(&views, &workflows)?
        } else {
            config_value.clone()
        };
        let template_registry =
            TemplateRegistry::compile_json_tree(&template_registry_value(template_value))?;
        validate_view_bootstrap_requirements(&views, &template_registry)?;
        let parameter_registry = if config_value.get("workflows").is_some() {
            ParameterRegistry::compile_with_templates(&config_value, &template_registry)?
        } else {
            ParameterRegistry::default()
        };
        Ok(Self {
            default_view: None,
            image_protocol: super::ImageProtocol::default(),
            log_file: None,
            commands: super::CommandConfig::default(),
            views,
            workflows,
            defaults,
            workflow_roots,
            config_value,
            template_registry,
            parameter_registry: Arc::new(parameter_registry),
        })
    }
}

/// Inline run scripts are executable source, not dynamic configuration. Their
/// contents are deliberately omitted from the template registry because command
/// preparation passes them to the script runner verbatim.
fn template_registry_value(mut value: Value) -> Value {
    exclude_run_script_bodies(&mut value);
    value
}

fn exclude_run_script_bodies(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                exclude_run_script_bodies(value);
            }
        }
        Value::Object(values) => {
            if values.get("type").and_then(Value::as_str) == Some("run") {
                values.remove("script");
                if let Some(payload) = values.get_mut("payload").and_then(Value::as_object_mut) {
                    payload.remove("script");
                    // A run handler's `script` field is also executable source
                    // when its source resolves to `inline`.
                    if let Some(handler) = payload.get_mut("handler") {
                        exclude_run_handler_script_body(handler);
                    }
                }
                if let Some(handler) = values.get_mut("handler") {
                    exclude_run_handler_script_body(handler);
                }
            }
            for value in values.values_mut() {
                exclude_run_script_bodies(value);
            }
        }
        _ => {}
    }
}

fn exclude_run_handler_script_body(handler: &mut Value) {
    if let Some(handler) = handler.as_object_mut() {
        handler.remove("script");
    }
}

fn validate_view_bootstrap_requirements(
    views: &BTreeMap<ViewRef, View>,
    templates: &TemplateRegistry,
) -> Result<()> {
    for (view_ref, view) in views {
        super::validation::validate_string_requirements(
            templates,
            view.selected_engine_type(),
            EvaluationStage::Bootstrap,
            &format!("view {view_ref:?} engine type"),
        )?;
        super::validation::validate_optional_string_requirements(
            templates,
            view.alias.as_deref(),
            EvaluationStage::Bootstrap,
            &format!("view {view_ref:?} alias"),
        )?;
        if let Some(query) = &view.query {
            super::validation::validate_toml_requirements(
                templates,
                &toml::Value::Table(query.clone()),
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} query schema"),
            )?;
        }
        for feed in view.selected_feeds() {
            super::validation::validate_string_requirements(
                templates,
                &feed.view,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} feed target"),
            )?;
        }
        for (command_id, command) in &view.commands {
            if let Some(key) = &command.key {
                super::validation::validate_string_requirements(
                    templates,
                    key,
                    EvaluationStage::Bootstrap,
                    &format!("view {view_ref:?} command {command_id:?} key"),
                )?;
            }
            super::validation::validate_string_requirements(
                templates,
                &command.label,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} command {command_id:?} label"),
            )?;
        }
    }
    Ok(())
}

impl CompiledConfig {
    pub(super) fn from_raw(
        mut raw: RawConfig,
        workflow_roots: BTreeMap<ViewRef, PathBuf>,
        mut config_value: Value,
    ) -> Result<Self> {
        let default_view = raw.default_view;
        let mut views = BTreeMap::new();
        let mut workflows = BTreeMap::new();
        for (package_id, workflow) in raw.workflows {
            let metadata = WorkflowMetadata {
                name: workflow.name.unwrap_or_else(|| package_id.clone()),
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
        if views.contains_key("selectors:commands") {
            raw.commands
                .bindings
                .entry("commands".to_string())
                .or_insert_with(CommandBinding::builtin_commands);
            super::normalize::inject_builtin_commands_value(&mut config_value);
        }
        let compiled =
            CompiledConfig::build(views, workflows, raw.defaults, workflow_roots, config_value)?;
        super::validation::validate_optional_string_requirements(
            &compiled.template_registry,
            default_view.as_deref(),
            EvaluationStage::Bootstrap,
            "default_view",
        )?;
        Ok(Self {
            default_view,
            image_protocol: raw.image_protocol,
            log_file: raw.log_file,
            commands: raw.commands,
            views: compiled.views,
            workflows: compiled.workflows,
            defaults: compiled.defaults,
            workflow_roots: compiled.workflow_roots,
            config_value: compiled.config_value,
            template_registry: compiled.template_registry,
            parameter_registry: compiled.parameter_registry,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        default_view: Option<ViewRef>,
        views: BTreeMap<ViewRef, View>,
        workflows: BTreeMap<String, WorkflowMetadata>,
        workflow_roots: BTreeMap<String, PathBuf>,
        mut config_value: Value,
    ) -> Result<Self> {
        let mut commands = super::CommandConfig::default();
        if views.contains_key("selectors:commands") {
            commands
                .bindings
                .insert("commands".to_string(), CommandBinding::builtin_commands());
            super::normalize::inject_builtin_commands_value(&mut config_value);
        }
        let compiled = CompiledConfig::build(
            views,
            workflows,
            Defaults::default(),
            workflow_roots,
            config_value,
        )?;
        Ok(Self {
            default_view,
            image_protocol: super::ImageProtocol::default(),
            log_file: None,
            commands,
            views: compiled.views,
            workflows: compiled.workflows,
            defaults: compiled.defaults,
            workflow_roots: compiled.workflow_roots,
            config_value: compiled.config_value,
            template_registry: compiled.template_registry,
            parameter_registry: compiled.parameter_registry,
        })
    }

    #[cfg(test)]
    pub(crate) fn rebuild_template_registry(&mut self) -> Result<()> {
        self.test_rebuild_compiled()
    }

    #[cfg(test)]
    pub(crate) fn test_rebuild_compiled(&mut self) -> Result<()> {
        let rebuilt = CompiledConfig::build(
            self.views.clone(),
            self.workflows.clone(),
            self.defaults.clone(),
            self.workflow_roots.clone(),
            self.config_value.clone(),
        )?;
        self.views = rebuilt.views;
        self.workflows = rebuilt.workflows;
        self.defaults = rebuilt.defaults;
        self.workflow_roots = rebuilt.workflow_roots;
        self.config_value = rebuilt.config_value;
        self.template_registry = rebuilt.template_registry;
        self.parameter_registry = rebuilt.parameter_registry;
        Ok(())
    }
}

fn views_config_value(
    views: &BTreeMap<ViewRef, View>,
    workflows: &BTreeMap<String, WorkflowMetadata>,
) -> Result<Value> {
    let mut workflow_values = serde_json::Map::new();
    for (package, metadata) in workflows {
        workflow_values.insert(
            package.clone(),
            serde_json::json!({"name": metadata.name, "views": {}}),
        );
    }
    for (view_ref, view) in views {
        let (package, name) = view_ref
            .split_once(':')
            .with_context(|| format!("test view {:?} is not namespaced", view_ref))?;
        let workflow = workflow_values
            .entry(package.to_string())
            .or_insert_with(|| serde_json::json!({"name": package, "views": {}}));
        let views = workflow
            .get_mut("views")
            .and_then(Value::as_object_mut)
            .context("test workflow views must be an object")?;
        views.insert(name.to_string(), serde_json::to_value(view)?);
    }
    let mut value = serde_json::json!({"workflows": workflow_values});
    remove_null_fields(&mut value);
    super::normalize::normalize_engine_configs(&mut value);
    Ok(value)
}

fn remove_null_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_null_fields(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                remove_null_fields(value);
            }
            values.retain(|_, value| !value.is_null());
        }
        _ => {}
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
            if is_dynamic_string(&feed.view) {
                let value = Value::String(feed.view.clone());
                let templates = TemplateRegistry::compile_json_tree(&value)?;
                super::validation::validate_json_requirements(
                    &templates,
                    &value,
                    EvaluationStage::Bootstrap,
                    &format!("view {owner_ref:?} feed target"),
                )?;
            }
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
