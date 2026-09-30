use super::{
    CaptureConfig, EmbeddedConfig, EngineDefinition, FormConfig, PickerConfig,
    ProjectedBindingConfig, ProjectedEngineConfig, TypedEngineConfig,
};
use crate::workflow::config::{CompiledConfig, toml_to_json};
use anyhow::{Context, Result};
use serde_json::Value;

pub(crate) fn project_engine_config(
    config: &CompiledConfig,
    view_ref: &str,
    _definition: &EngineDefinition,
    launch_input: Value,
) -> Result<ProjectedEngineConfig> {
    let view = config
        .view(view_ref)
        .with_context(|| format!("view {:?} disappeared during factory preparation", view_ref))?;

    let typed = match view.selected_engine_type() {
        "picker" => {
            let table = view.picker.as_ref().cloned().unwrap_or_default();
            let parsed: Option<PickerConfig> = toml::Value::Table(table).try_into().ok();
            parsed.map(TypedEngineConfig::Picker)
        }
        "capture" => {
            let table = view.capture.as_ref().cloned().unwrap_or_default();
            let parsed: Option<CaptureConfig> = toml::Value::Table(table).try_into().ok();
            parsed.map(TypedEngineConfig::Capture)
        }
        "form" => {
            let table = view.form.as_ref().cloned().unwrap_or_default();
            let parsed: Option<FormConfig> = toml::Value::Table(table).try_into().ok();
            parsed.map(TypedEngineConfig::Form)
        }
        "embedded" => {
            let table = view.embedded.as_ref().cloned().unwrap_or_default();
            let parsed: Option<EmbeddedConfig> = toml::Value::Table(table).try_into().ok();
            parsed.map(TypedEngineConfig::Embedded)
        }
        _ => None,
    };

    Ok(ProjectedEngineConfig {
        typed,
        workflow_root: config
            .workflow_root(view_ref)
            .map(std::path::Path::to_path_buf),
        launch_input,
    })
}

pub(crate) fn project_binding_config(
    config: &CompiledConfig,
    definition: &EngineDefinition,
) -> Result<ProjectedBindingConfig> {
    let defaults = match definition.binding_defaults {
        Some(path) if path == ["picker", "bindings"] => config
            .picker_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["capture", "bindings"] => config
            .capture_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["embedded", "bindings"] => config
            .embedded_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) if path == ["form", "bindings"] => config
            .form_default_bindings()
            .map(toml_to_json)
            .transpose()?,
        Some(path) => anyhow::bail!("unsupported engine binding defaults path {:?}", path),
        None => None,
    };
    Ok(ProjectedBindingConfig { defaults })
}
