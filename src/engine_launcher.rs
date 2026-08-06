use super::{
    CommandExecution, Engine, EngineContext, EngineDefinition, EngineResult, validate_fields,
};
use crate::config::ENGINE_LAUNCHER;
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct LauncherCommandEngine;

impl Engine for LauncherCommandEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_LAUNCHER
    }

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()> {
        validate_fields(name, definition, &["items", "commands"])
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
            exit,
            next_view,
            ..
        } = execution;
        if exit {
            context.execute_exit(prepared, terminal, &invocation)?;
            return Ok(EngineResult {
                exit: true,
                next_view: None,
            });
        }
        context.execute_oneshot(prepared, terminal, &invocation)?;
        Ok(EngineResult {
            exit: false,
            next_view,
        })
    }
}
