use super::pty::{self, EmbeddedOutcome};
use crate::engine::PreparedProcess;
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct EmbeddedSession {
    prepared: PreparedProcess,
}

impl EmbeddedSession {
    pub(crate) fn new(prepared: PreparedProcess) -> Self {
        Self { prepared }
    }

    pub(crate) fn run(
        &self,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<EmbeddedOutcome> {
        pty::run(
            &self.prepared.argv,
            &self.prepared.environment,
            self.prepared.current_dir.as_deref(),
            terminal,
            chrome,
        )
    }
}
