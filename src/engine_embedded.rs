use super::{
    CommandExecution, Engine, EngineContext, EngineDefinition, EngineResult, validate_fields,
};
use crate::config::ENGINE_EMBEDDED;
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct EmbeddedEngine;

impl Engine for EmbeddedEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_EMBEDDED
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["command"])
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
        context.execute_embedded(prepared, terminal, &invocation, &title)?;
        Ok(EngineResult {
            exit: false,
            next_view: None,
        })
    }
}
