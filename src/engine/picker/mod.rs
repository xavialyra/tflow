mod command;
mod items;
mod keymap;
mod preview;
mod render;
mod runtime;
mod session;
mod tasks;

use self::keymap::PickerKeymap;
use self::session::PickerOptions;
pub(crate) use self::session::PickerView;
use super::{Engine, ViewContext, ViewInstance, validate_fields};
use crate::config::{ConfigSource, Defaults, ENGINE_PICKER, View, toml_to_json};
use crate::expression::{EvaluationStage, Template};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::sync::Arc;

pub(crate) use items::Item;
pub(crate) use tasks::TaskScheduler;

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAction {
    Activate(crate::input::Key),
}

pub(crate) struct PickerEngine;

impl Engine for PickerEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_PICKER
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(name, view, &["show_prefix", "layout", "preview"])?;
        validate_picker_bool(view.engine_field("show_prefix"), "show_prefix")?;
        validate_picker_table(view.engine_field("layout"), "layout")?;
        validate_picker_table(view.engine_field("preview"), "preview")?;
        Ok(())
    }

    fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        let bindings = defaults
            .picker
            .bindings
            .as_ref()
            .map(toml_to_json)
            .transpose()?;
        PickerKeymap::validate_values(bindings.as_ref(), None).context("picker bindings")
    }

    fn validate_keymap(&self, name: &str, view: &View) -> Result<()> {
        let keymap = view.keymap.as_ref().map(toml_to_json).transpose()?;
        PickerKeymap::validate_values(None, keymap.as_ref())
            .with_context(|| format!("view {:?} picker keymap", name))
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let default_bindings = context.config.get(
            ConfigSource::Root,
            &context.evaluation,
            EvaluationStage::Operation,
            &["defaults", "picker", "bindings"],
        )?;
        let source = ConfigSource::View(context.state.view_ref());
        let get_view_field = |path: &[&str]| {
            context.config.get(
                source,
                &context.evaluation,
                EvaluationStage::Operation,
                path,
            )
        };
        let view_keymap = get_view_field(&["keymap"])?;
        let show_prefix = get_view_field(&["show_prefix"])?;
        let layout = get_view_field(&["layout"])?;
        let preview = get_view_field(&["preview"])?;
        let keymap = PickerKeymap::from_values(default_bindings, view_keymap)?;
        let options = PickerOptions {
            show_prefix: parse_bool(show_prefix, "show_prefix", false)?,
            preview: self::preview::parse(layout, preview)?,
        };
        let picker = PickerView::new(
            &context.request.view_ref,
            context.tasks.clone(),
            Arc::new(context.config.clone()),
            keymap,
            options,
        );
        Ok(Box::new(picker))
    }
}

fn validate_picker_bool(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_bool() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a boolean or complete dynamic path",
        name
    )
}

fn validate_picker_table(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_table() || is_complete_dynamic_path(value)? {
        return Ok(());
    }
    bail!(
        "picker field {:?} must be a table or complete dynamic path",
        name
    )
}

fn is_complete_dynamic_path(value: &toml::Value) -> Result<bool> {
    let Some(source) = value.as_str() else {
        return Ok(false);
    };
    Ok(Template::parse(source)?.is_complete_path())
}

fn parse_bool(value: Option<Value>, name: &str, default: bool) -> Result<bool> {
    value
        .map(|value| {
            value
                .as_bool()
                .with_context(|| format!("picker field {:?} must evaluate to a boolean", name))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_runtime_fields_allow_dynamic_values_for_post_resolution_validation() {
        let view: View = toml::from_str(
            r#"
            [engine]
            type = "picker"
            [engine.config]
            show_prefix = "{{ page.query.show_prefix }}"
            layout = "{{ page.query.layout }}"
            preview = "{{ page.query.preview }}"
            "#,
        )
        .unwrap();
        PickerEngine.validate_config("core:dynamic", &view).unwrap();
    }
}
