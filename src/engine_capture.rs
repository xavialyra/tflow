use super::{
    CommandExecution, Engine, EngineContext, EngineDefinition, EngineResult, validate_fields,
};
use crate::config::ENGINE_CAPTURE;
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct CaptureEngine;

impl Engine for CaptureEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_CAPTURE
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["output", "title"])
    }

    fn execute(
        &self,
        execution: CommandExecution,
        context: &mut EngineContext<'_>,
        terminal: &mut Terminal,
    ) -> Result<EngineResult> {
        let CommandExecution {
            invocation,
            prepared,
            title,
            ..
        } = execution;
        context.execute_capture(prepared, terminal, &invocation, &title)?;
        Ok(EngineResult {
            exit: false,
            next_view: None,
        })
    }
}
