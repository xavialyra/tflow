mod command;
mod input;
mod items;
mod render;
mod runtime;
mod session;

use super::{Engine, RuntimeStore, ViewContext, ViewInstance, validate_fields};
use crate::config::{Config, ENGINE_LAUNCHER, EngineDefinition};
use crate::expression::ExpressionMethods;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) use command::CommandInvocation;
pub(crate) use items::{Item, spawn_items_scheduler};
pub(crate) use session::LauncherView;

pub(crate) struct LauncherEngine;

impl Engine for LauncherEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_LAUNCHER
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["items", "commands"])
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let scheduler = spawn_items_scheduler(context.config.clone(), context.runtime.clone());
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
        Ok(Box::new(LauncherView::new(
            &context.location.view_ref,
            &context.location.input,
            scheduler,
            context.log_file.map(PathBuf::from),
            command_owner,
            parent_item,
        )))
    }
}

pub(crate) struct ViewEvaluator<'a> {
    config: &'a Config,
    view_ref: &'a str,
    runtime: &'a RuntimeStore,
}

impl<'a> ViewEvaluator<'a> {
    pub(crate) fn new(config: &'a Config, view_ref: &'a str, runtime: &'a RuntimeStore) -> Self {
        Self {
            config,
            view_ref,
            runtime,
        }
    }

    pub(crate) fn evaluate_field(&self, field: &str) -> Result<Option<Value>> {
        let script_root = self
            .config
            .plugin_root(self.view_ref)
            .unwrap_or_else(|| Path::new("."));
        let mut methods = ExpressionMethods::new(script_root);
        self.config.evaluate_engine_field(
            self.view_ref,
            field,
            self.runtime.snapshot(),
            &mut methods,
        )
    }
}
