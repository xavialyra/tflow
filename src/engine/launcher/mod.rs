mod command;
mod input;
mod items;
mod render;
mod runtime;
mod session;

use super::RuntimeStore;
use crate::config::Config;
use crate::expression::ExpressionMethods;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

pub(crate) use command::{
    CommandExecution, CommandInvocation, LauncherCommandEngine, PreparedCommand,
};
pub(crate) use items::{Item, ItemsTaskScheduler, spawn_items_scheduler};
pub(crate) use session::LauncherDriver;

pub(crate) struct LauncherEngine<'a> {
    config: &'a Config,
    view_ref: String,
    runtime: &'a RuntimeStore,
}

impl<'a> LauncherEngine<'a> {
    pub(crate) fn new(config: &'a Config, view_ref: &str, runtime: &'a RuntimeStore) -> Self {
        Self {
            config,
            view_ref: view_ref.to_string(),
            runtime,
        }
    }

    pub(crate) fn evaluate_field(&self, field: &str) -> Result<Option<Value>> {
        let script_root = self
            .config
            .plugin_root(&self.view_ref)
            .unwrap_or_else(|| Path::new("."));
        let mut methods = ExpressionMethods::new(script_root);
        self.config.evaluate_engine_field(
            &self.view_ref,
            field,
            self.runtime.snapshot(),
            &mut methods,
        )
    }
}
