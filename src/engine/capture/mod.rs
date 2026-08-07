mod command;
mod render;
mod session;

use super::{CommandExecution, Engine, EngineDriver, ItemsTaskScheduler, validate_fields};
use crate::config::{Config, ENGINE_CAPTURE, EngineDefinition};
use crate::engine::RuntimeHandle;
use anyhow::Result;
use std::path::Path;

pub(crate) struct CaptureEngine;

impl Engine for CaptureEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_CAPTURE
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["output", "title"])
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
        Err(command::unsupported_view())
    }

    fn create_command(&self, execution: CommandExecution) -> Result<Box<dyn EngineDriver>> {
        Ok(Box::new(command::CaptureCommandDriver::new(execution)))
    }
}
