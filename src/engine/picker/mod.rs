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
use crate::config::{ConfigReadContext, ConfigScope, ENGINE_PICKER, View};
use crate::text::sanitize_terminal_text;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) use items::Item;

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAction {
    Activate(crate::input::Key),
    OpenCommandView,
}

pub(crate) struct PickerEngine;

impl Engine for PickerEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_PICKER
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(
            name,
            view,
            &["bindings", "prompt", "show_prefix", "layout", "preview"],
        )?;
        validate_static_string(view.engine_field("prompt"), "prompt")?;
        validate_static_bool(view.engine_field("show_prefix"), "show_prefix")?;
        validate_static_table(view.engine_field("layout"), "layout")?;
        validate_static_table(view.engine_field("preview"), "preview")?;
        Ok(())
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let runtime = context.runtime.read();
        let default_bindings = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::Root,
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["defaults", "picker", "bindings"],
        )?;
        let view_bindings = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::View(context.state),
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["bindings"],
        )?;
        let prompt = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::View(context.state),
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["prompt"],
        )?;
        let show_prefix = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::View(context.state),
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["show_prefix"],
        )?;
        let layout = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::View(context.state),
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["layout"],
        )?;
        let preview = context.config.get(
            ConfigReadContext {
                scope: ConfigScope::View(context.state),
                runtime: &runtime,
                input: &context.config.input_value,
                cancellation: None,
                binding_raw: None,
            },
            &["preview"],
        )?;
        drop(runtime);
        let keymap = PickerKeymap::from_values(default_bindings, view_bindings)?;
        let options = PickerOptions {
            show_prefix: parse_bool(show_prefix, "show_prefix", false)?,
            input_prefix: parse_optional_string(prompt, "prompt")?
                .map(|prompt| sanitize_terminal_text(&prompt)),
            preview: self::preview::parse(layout, preview)?,
        };
        let command_context = context
            .request
            .command_picker
            .clone()
            .map(|context| *context);
        let picker = PickerView::new(
            &context.request.view_ref,
            context.tasks.clone(),
            Arc::new(context.config.clone()),
            context.request.route_child,
            keymap,
            options,
        )
        .with_context(context.log_file.map(PathBuf::from), command_context);
        Ok(Box::new(picker))
    }
}

pub(crate) fn validate_bindings(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
    PickerKeymap::validate_values(defaults, view)
}

fn is_dynamic(value: &toml::Value) -> bool {
    value.as_str().is_some_and(|source| source.contains("{{"))
}

fn validate_static_string(value: Option<&toml::Value>, name: &str) -> Result<()> {
    match value {
        None | Some(toml::Value::String(_)) => Ok(()),
        Some(_) => bail!("picker field {:?} must be a string or expression", name),
    }
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

fn parse_optional_string(value: Option<Value>, name: &str) -> Result<Option<String>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("picker field {:?} must evaluate to a string or null", name))
        .map(Some)
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
