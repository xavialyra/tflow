mod command;
mod pty;
mod session;

use super::{CommandExecution, Engine, EngineDriver, ItemsTaskScheduler, validate_fields};
use crate::config::{Config, ENGINE_EMBEDDED, EngineDefinition};
use crate::engine::RuntimeHandle;
use anyhow::Result;
use std::path::Path;

pub(crate) struct EmbeddedEngine;

impl Engine for EmbeddedEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_EMBEDDED
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["command"])
    }

    fn create_view(
        &self,
        _config: &Config,
        _view_ref: &str,
        _input: &str,
        _log_file: Option<&Path>,
        _runtime: RuntimeHandle,
        _items_scheduler: ItemsTaskScheduler,
    ) -> Result<Box<dyn EngineDriver>> {
        Err(anyhow::anyhow!("embedded engine views require a command"))
    }

    fn create_command(&self, execution: CommandExecution) -> Result<Box<dyn EngineDriver>> {
        Ok(Box::new(command::EmbeddedCommandDriver::new(execution)))
    }
}
