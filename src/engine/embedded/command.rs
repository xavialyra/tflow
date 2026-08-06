use super::pty::EmbeddedOutcome;
use super::session::EmbeddedSession;
use crate::engine::{CommandExecution, EngineDriver, EngineHost, SessionEffect};
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct EmbeddedCommandDriver {
    execution: Option<CommandExecution>,
}

impl EmbeddedCommandDriver {
    pub(crate) fn new(execution: CommandExecution) -> Self {
        Self {
            execution: Some(execution),
        }
    }
}

impl EngineDriver for EmbeddedCommandDriver {
    fn step(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<SessionEffect> {
        let execution = self
            .execution
            .take()
            .ok_or_else(|| anyhow::anyhow!("embedded command driver was already completed"))?;
        let CommandExecution {
            invocation,
            prepared,
            title,
            ..
        } = execution;
        let session = EmbeddedSession::new(prepared, &title);
        let outcome = session.run(terminal)?;
        let message = embedded_status_message(outcome);
        let success = matches!(outcome, EmbeddedOutcome::ReturnedToLauncher)
            || matches!(outcome, EmbeddedOutcome::Exited(0));
        host.record_command_status(&invocation, &message, success);
        Ok(SessionEffect::Back)
    }

    fn render(&self, _host: &EngineHost<'_>, _terminal: &Terminal) -> Result<()> {
        Ok(())
    }
}

fn embedded_status_message(outcome: EmbeddedOutcome) -> String {
    match outcome {
        EmbeddedOutcome::ReturnedToLauncher => "embedded command stopped".to_string(),
        EmbeddedOutcome::Exited(0) => "embedded command finished successfully".to_string(),
        EmbeddedOutcome::Exited(code) => format!("embedded command exited with code {}", code),
        EmbeddedOutcome::Signaled(signal) => {
            format!("embedded command terminated by signal {}", signal)
        }
    }
}
