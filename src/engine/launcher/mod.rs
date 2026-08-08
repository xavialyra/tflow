mod command;
mod input;
mod items;
mod keymap;
mod render;
mod runtime;
mod session;

use self::keymap::LauncherKeymap;
use super::{Engine, ViewContext, ViewInstance, validate_fields};
use crate::config::{ENGINE_LAUNCHER, View};
use crate::expression::ExpressionMethods;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) use command::CommandInvocation;
pub(crate) use items::Item;
pub(crate) use session::LauncherView;

pub(crate) struct LauncherEngine;

impl Engine for LauncherEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_LAUNCHER
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(name, view, &["bindings"])?;
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
            .evaluate_default_launcher_bindings(&runtime, &mut methods)?;
        let view_bindings = context.config.evaluate_view_field(
            &context.location.view_ref,
            "bindings",
            &runtime,
            &mut methods,
        )?;
        drop(runtime);
        let keymap = LauncherKeymap::from_values(default_bindings, view_bindings)?;
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
        Ok(Box::new(
            LauncherView::new(
                &context.location.view_ref,
                context.tasks.clone(),
                Arc::new(context.config.clone()),
                context.location.shell_input.is_some(),
                keymap,
            )
            .with_context(
                context.log_file.map(PathBuf::from),
                command_owner,
                parent_item,
            ),
        ))
    }
}

pub(crate) fn validate_bindings(defaults: Option<&Value>, view: Option<&Value>) -> Result<()> {
    LauncherKeymap::validate_values(defaults, view)
}
