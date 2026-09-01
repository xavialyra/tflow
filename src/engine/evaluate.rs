use super::{EngineDefinition, EvaluatedBindingConfig, EvaluatedEngineConfig};
use crate::config::{Config, ConfigSource, EvaluationSnapshot};
use crate::expression::EvaluationStage;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) fn field(config: &EvaluatedEngineConfig, name: &str) -> Result<Option<Value>> {
    if let Some(error) = config.field_error(name) {
        return Err(anyhow!(error.to_string()));
    }
    Ok(config.field(name).cloned())
}

pub(crate) fn optional_string(
    config: &EvaluatedEngineConfig,
    name: &str,
) -> Result<Option<String>> {
    field(config, name)?
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .with_context(|| format!("engine field {:?} must evaluate to a string", name))
        })
        .transpose()
}

struct EvaluatedFields {
    values: BTreeMap<String, Value>,
    errors: BTreeMap<String, String>,
}

fn fields_for_factory(
    config: &Config,
    view_ref: &str,
    snapshot: &EvaluationSnapshot<'_>,
    names: &[&str],
    tolerated_errors: &[&str],
) -> Result<EvaluatedFields> {
    let view = config
        .view(view_ref)
        .with_context(|| format!("view {:?} disappeared during factory preparation", view_ref))?;
    let mut values = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for &name in names {
        if view.engine_field(name).is_none() {
            continue;
        }
        match config.get(
            ConfigSource::View(view_ref),
            snapshot,
            EvaluationStage::Operation,
            &[name],
        ) {
            Ok(Some(value)) => {
                values.insert(name.to_string(), value);
            }
            Ok(None) => {}
            Err(error) if tolerated_errors.contains(&name) => {
                errors.insert(name.to_string(), error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    // `items` is evaluated for each Picker request because its dynamic value
    // may depend on the current page/runtime snapshot.
    Ok(EvaluatedFields { values, errors })
}

pub(crate) fn engine_config(
    config: &Config,
    view_ref: &str,
    definition: &EngineDefinition,
    snapshot: &EvaluationSnapshot<'_>,
) -> Result<EvaluatedEngineConfig> {
    let fields = fields_for_factory(
        config,
        view_ref,
        snapshot,
        definition.factory_fields.runtime,
        definition.factory_fields.deferred_runtime_errors,
    )?;
    Ok(EvaluatedEngineConfig {
        fields: fields.values,
        field_errors: fields.errors,
        plugin_root: config.plugin_root(view_ref).map(|path| path.to_path_buf()),
    })
}

pub(crate) fn binding_config(
    config: &Config,
    view_ref: &str,
    definition: &EngineDefinition,
    snapshot: &EvaluationSnapshot<'_>,
) -> Result<EvaluatedBindingConfig> {
    let defaults = definition
        .factory_fields
        .binding_defaults
        .map(|path| {
            config.get(
                ConfigSource::Root,
                snapshot,
                EvaluationStage::Operation,
                path,
            )
        })
        .transpose()?
        .flatten();
    let view_keymap = config.get(
        ConfigSource::View(view_ref),
        snapshot,
        EvaluationStage::Operation,
        &["keymap"],
    )?;
    Ok(EvaluatedBindingConfig {
        defaults,
        view_keymap,
        engine_fields: fields_for_factory(
            config,
            view_ref,
            snapshot,
            definition.factory_fields.binding,
            &[],
        )?
        .values,
    })
}
