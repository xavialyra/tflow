use super::{
    CommandBinding, CompiledConfig, Config, Defaults, ENGINE_PICKER, FeedSpec, PluginMetadata,
    RawConfig, StateInstance, View, ViewRef,
};
use crate::expression::{EvaluationStage, TemplateRegistry, is_dynamic_string};
use crate::state::StateRegistry;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

impl CompiledConfig {
    fn build(
        views: BTreeMap<ViewRef, View>,
        plugins: BTreeMap<String, PluginMetadata>,
        defaults: Defaults,
        plugin_roots: BTreeMap<String, PathBuf>,
        config_value: Value,
    ) -> Result<Self> {
        let template_value = if config_value
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            views_config_value(&views, &plugins)?
        } else {
            config_value.clone()
        };
        let template_registry = TemplateRegistry::compile_json_tree(&template_value)?;
        validate_view_bootstrap_requirements(&views, &template_registry)?;
        let state_registry = if config_value.get("plugins").is_some() {
            StateRegistry::compile_with_templates(&config_value, &template_registry)?
        } else {
            StateRegistry::default()
        };
        Ok(Self {
            views,
            plugins,
            defaults,
            plugin_roots,
            config_value,
            template_registry,
            state_registry,
        })
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
            super::validation::validate_string_requirements(
                templates,
                &command.key,
                EvaluationStage::Bootstrap,
                &format!("view {view_ref:?} command {command_id:?} key"),
            )?;
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

impl Config {
    pub(super) fn from_raw(
        mut raw: RawConfig,
        plugin_roots: BTreeMap<ViewRef, PathBuf>,
        mut config_value: Value,
    ) -> Result<Self> {
        let default_view = raw.default_view;
        let mut views = BTreeMap::new();
        let mut plugins = BTreeMap::new();
        for (package_id, plugin) in raw.plugins {
            let metadata = PluginMetadata {
                name: plugin.name.unwrap_or_else(|| package_id.clone()),
            };
            for (view_name, view) in plugin.views {
                let view_ref = qualify_view_ref(&package_id, &view_name)?;
                if views.insert(view_ref.clone(), view).is_some() {
                    bail!("duplicate view {:?}", view_ref);
                }
            }
            plugins.insert(package_id, metadata);
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
            CompiledConfig::build(views, plugins, raw.defaults, plugin_roots, config_value)?;
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
            input_value: Value::Null,
            invocation_state: StateInstance::default(),
            compiled,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        default_view: Option<ViewRef>,
        views: BTreeMap<ViewRef, View>,
        plugins: BTreeMap<String, PluginMetadata>,
        plugin_roots: BTreeMap<String, PathBuf>,
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
            plugins,
            Defaults::default(),
            plugin_roots,
            config_value,
        )?;
        Ok(Self {
            default_view,
            image_protocol: super::ImageProtocol::default(),
            log_file: None,
            commands,
            input_value: Value::Null,
            invocation_state: StateInstance::default(),
            compiled,
        })
    }

    #[cfg(test)]
    pub(crate) fn rebuild_template_registry(&mut self) -> Result<()> {
        self.test_rebuild_compiled()
    }

    #[cfg(test)]
    pub(crate) fn test_rebuild_compiled(&mut self) -> Result<()> {
        self.compiled = CompiledConfig::build(
            self.compiled.views.clone(),
            self.compiled.plugins.clone(),
            self.compiled.defaults.clone(),
            self.compiled.plugin_roots.clone(),
            self.compiled.config_value.clone(),
        )?;
        Ok(())
    }
}

fn views_config_value(
    views: &BTreeMap<ViewRef, View>,
    plugins: &BTreeMap<String, PluginMetadata>,
) -> Result<Value> {
    let mut plugin_values = serde_json::Map::new();
    for (package, metadata) in plugins {
        plugin_values.insert(
            package.clone(),
            serde_json::json!({"name": metadata.name, "views": {}}),
        );
    }
    for (view_ref, view) in views {
        let (package, name) = view_ref
            .split_once(':')
            .with_context(|| format!("test view {:?} is not namespaced", view_ref))?;
        let plugin = plugin_values
            .entry(package.to_string())
            .or_insert_with(|| serde_json::json!({"name": package, "views": {}}));
        let views = plugin
            .get_mut("views")
            .and_then(Value::as_object_mut)
            .context("test plugin views must be an object")?;
        views.insert(name.to_string(), serde_json::to_value(view)?);
    }
    let mut value = serde_json::json!({"plugins": plugin_values});
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
