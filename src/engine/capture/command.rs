use super::session::CaptureSession;
use crate::engine::{CommandExecution, EngineDriver, EngineHost, SessionEffect};
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct CaptureCommandDriver {
    execution: Option<CommandExecution>,
}

impl CaptureCommandDriver {
    pub(crate) fn new(execution: CommandExecution) -> Self {
        Self {
            execution: Some(execution),
        }
    }
}

impl EngineDriver for CaptureCommandDriver {
    fn step(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<SessionEffect> {
        let execution = self
            .execution
            .take()
            .ok_or_else(|| anyhow::anyhow!("capture command driver was already completed"))?;
        let CommandExecution {
            invocation,
            prepared,
            title,
            ..
        } = execution;
        let mut capture = CaptureSession::new(&title);
        let outcome = capture.execute(prepared, terminal, &invocation.id)?;
        host.record_command_status(&invocation, &outcome.status, outcome.success);
        capture.wait_for_return(terminal)?;
        Ok(SessionEffect::Back)
    }

    fn render(&self, _host: &EngineHost<'_>, _terminal: &Terminal) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn unsupported_view() -> anyhow::Error {
    anyhow::anyhow!("capture engine views require a command")
}
