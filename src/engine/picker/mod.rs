mod command;
mod input;
mod items;
mod keymap;
mod render;
mod runtime;
mod session;

use self::keymap::PickerKeymap;
use self::session::{PickerOptions, PickerView};
use super::{Engine, ViewContext, ViewInstance, validate_fields};
use crate::config::{ENGINE_PICKER, View};
use crate::expression::ExpressionMethods;
use crate::text::sanitize_terminal_text;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) use command::CommandInvocation;
pub(crate) use items::Item;

pub(crate) struct PickerEngine;

impl Engine for PickerEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_PICKER
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(
            name,
            view,
            &["bindings", "max_rows", "prompt", "show_prefix"],
        )?;
        validate_static_max_rows(view.engine_field("max_rows"))?;
        validate_static_string(view.engine_field("prompt"), "prompt")?;
        validate_static_bool(view.engine_field("show_prefix"), "show_prefix")?;
        Ok(())
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let script_root = context
            .config
            .plugin_root(&context.location.view_ref)
            .unwrap_or_else(|| std::path::Path::new("."));
        let runtime = context.runtime.read();
        let mut methods = ExpressionMethods::new(script_root);
        let default_bindings = context
            .config
            .evaluate_default_picker_bindings(&runtime, &mut methods)?;
        let view_bindings = context.config.evaluate_view_field(
            &context.location.view_ref,
            context.state,
            "bindings",
            &runtime,
            &mut methods,
        )?;
        let max_rows = context.config.evaluate_view_field(
            &context.location.view_ref,
            context.state,
            "max_rows",
            &runtime,
            &mut methods,
        )?;
        let prompt = context.config.evaluate_view_field(
            &context.location.view_ref,
            context.state,
            "prompt",
            &runtime,
            &mut methods,
        )?;
        let show_prefix = context.config.evaluate_view_field(
            &context.location.view_ref,
            context.state,
            "show_prefix",
            &runtime,
            &mut methods,
        )?;
        drop(runtime);
        let keymap = PickerKeymap::from_values(default_bindings, view_bindings)?;
        let options = PickerOptions {
            show_prefix: parse_bool(show_prefix, "show_prefix", true)?,
            max_rows: parse_max_rows(max_rows)?,
            input_prefix: parse_optional_string(prompt, "prompt")?
                .map(|prompt| sanitize_terminal_text(&prompt)),
        };
        let command_owner = context
            .location
            .context
            .get("command_owner")
            .and_then(Value::as_str)
            .map(str::to_string);
        let parent_item = context
            .location
            .context
            .get("parent_item")
            .cloned()
            .filter(|value| !value.is_null())
            .map(serde_json::from_value)
            .transpose()?;
        let mut picker = PickerView::new(
            &context.location.view_ref,
            context.tasks.clone(),
            Arc::new(context.config.clone()),
            context.location.shell_input.is_some(),
            keymap,
            options,
        )
        .with_context(
            context.log_file.map(PathBuf::from),
            command_owner.clone(),
            parent_item,
        );
        if let (Some(owner), Some(state)) = (command_owner, context.location.owner_state.clone()) {
            picker.source_states.insert(owner, state);
        }
        Ok(Box::new(picker))
    }
}

pub(crate) fn validate_bindings(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
    PickerKeymap::validate_values(defaults, view)
}

fn is_dynamic(value: &toml::Value) -> bool {
    value.as_str().is_some_and(|source| source.contains("{{"))
}

fn validate_static_max_rows(value: Option<&toml::Value>) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if is_dynamic(value) {
        return Ok(());
    }
    match value.as_integer() {
        Some(rows) if rows > 0 => Ok(()),
        Some(_) => bail!("picker field \"max_rows\" must be greater than zero"),
        None => bail!("picker field \"max_rows\" must be a positive integer or expression"),
    }
}

fn validate_static_string(value: Option<&toml::Value>, name: &str) -> Result<()> {
    match value {
        None | Some(toml::Value::String(_)) => Ok(()),
        Some(_) => bail!("picker field {:?} must be a string or expression", name),
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

fn parse_max_rows(value: Option<Value>) -> Result<Option<usize>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let rows = value
        .as_u64()
        .context("picker field \"max_rows\" must evaluate to a positive integer or null")?;
    if rows == 0 {
        bail!("picker field \"max_rows\" must be greater than zero");
    }
    usize::try_from(rows)
        .context("picker field \"max_rows\" does not fit this platform")
        .map(Some)
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
        assert!(validate_static_max_rows(Some(&toml::Value::Integer(0))).is_err());
        assert!(validate_static_max_rows(Some(&toml::Value::Integer(4))).is_ok());
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
