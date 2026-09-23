use super::{EngineDefinition, ProjectedBindingConfig, ProjectedEngineConfig};
use crate::workflow::config::{CompiledConfig, toml_to_json};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

fn fields_for_view(
    config: &CompiledConfig,
    view_ref: &str,
    names: &[&str],
) -> Result<BTreeMap<String, Value>> {
    let view = config
        .view(view_ref)
        .with_context(|| format!("view {:?} disappeared during factory preparation", view_ref))?;
    names
        .iter()
        .filter_map(|name| {
            view.engine_field(name)
                .map(|value| toml_to_json(value).map(|value| ((*name).to_string(), value)))
        })
        .collect()
}

pub(crate) fn project_engine_config(
    config: &CompiledConfig,
    view_ref: &str,
    definition: &EngineDefinition,
    launch_input: Value,
) -> Result<ProjectedEngineConfig> {
    Ok(ProjectedEngineConfig {
        fields: fields_for_view(config, view_ref, definition.factory_fields.runtime)?,
        workflow_root: config
            .workflow_root(view_ref)
            .map(std::path::Path::to_path_buf),
        launch_input,
    })
}

pub(crate) fn project_binding_config(
    config: &CompiledConfig,
    view_ref: &str,
    definition: &EngineDefinition,
) -> Result<ProjectedBindingConfig> {
    let defaults = match definition.factory_fields.binding_defaults {
        Some(path) if path == ["defaults", "picker", "bindings"] => config
            .picker_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["defaults", "capture", "bindings"] => config
            .capture_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["defaults", "embedded", "bindings"] => config
            .embedded_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["defaults", "form", "bindings"] => config
            .form_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) => anyhow::bail!("unsupported engine binding defaults path {:?}", path),
        None => None,
    };
    let view_keymap = config
        .view(view_ref)
        .and_then(|view| view.keymap.as_ref())
        .map(serde_json::to_value)
        .transpose()?;
    Ok(ProjectedBindingConfig {
        defaults,
        view_keymap,
        engine_fields: fields_for_view(config, view_ref, definition.factory_fields.binding)?,
    })
}
