mod command;
mod items;
mod keymap;
mod preview;
mod render;
mod runtime;
mod session;

use self::keymap::PickerKeymap;
use self::session::PickerOptions;
pub(crate) use self::session::PickerView;
use super::{Engine, ViewContext, ViewInstance, validate_fields};
use crate::config::{ConfigReadContext, ConfigScope, Defaults, ENGINE_PICKER, View, toml_to_json};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::sync::Arc;

pub(crate) use items::{Item, ItemsRequest, ItemsResponse, load_items_for_page};

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
        validate_static_bool(view.engine_field("show_prefix"), "show_prefix")?;
        validate_static_table(view.engine_field("layout"), "layout")?;
        validate_static_table(view.engine_field("preview"), "preview")?;
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
        let runtime = context.runtime.read();
        let default_bindings = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::Root,
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: Some(context.cancellation.clone()),
                binding_raw: None,
            },
            &["defaults", "picker", "bindings"],
        )?;
        let request = context.request.reference_value();
        let get_view_field = |path: &[&str]| {
            context.config.get_with_references(
                ConfigReadContext {
                    scope: ConfigScope::View(context.state),
                    runtime: &runtime,
                    input: &context.config.input_value,
                    cancellation: Some(context.cancellation.clone()),
                    binding_raw: None,
                },
                path,
                request.as_ref(),
                None,
            )
        };
        let view_keymap = get_view_field(&["keymap"])?;
        let show_prefix = get_view_field(&["show_prefix"])?;
        let layout = get_view_field(&["layout"])?;
        let preview = get_view_field(&["preview"])?;
        drop(runtime);
        let keymap = PickerKeymap::from_values(default_bindings, view_keymap)?;
        let options = PickerOptions {
            show_prefix: parse_bool(show_prefix, "show_prefix", false)?,
            preview: self::preview::parse(layout, preview)?,
        };
        let picker = PickerView::new(
            &context.request.view_ref,
            context.tasks.clone(),
            Arc::new(context.config.clone()),
            context.request.route_child,
            context.request.reference_value(),
            keymap,
            options,
        );
        Ok(Box::new(picker))
    }
}

fn is_dynamic(value: &toml::Value) -> bool {
    value.as_str().is_some_and(|source| source.contains("{{"))
}

fn validate_static_table(value: Option<&toml::Value>, name: &str) -> Result<()> {
    match value {
        None | Some(toml::Value::Table(_)) => Ok(()),
        Some(_) => bail!("picker field {:?} must be a table", name),
    }
}

fn validate_static_bool(value: Option<&toml::Value>, name: &str) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if is_dynamic(value) || value.is_bool() {
        Ok(())
    } else {
        bail!("picker field {:?} must be a boolean or expression", name)
    }
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
    fn static_picker_fields_are_validated_before_view_creation() {
        assert!(
            validate_static_bool(
                Some(&toml::Value::String("false".to_string())),
                "show_prefix"
            )
            .is_err()
        );
        assert!(validate_static_bool(Some(&toml::Value::Boolean(false)), "show_prefix").is_ok());
    }
}
