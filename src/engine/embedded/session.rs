use super::pty::{self, EmbeddedOutcome};
use crate::engine::PreparedProcess;
use crate::terminal::Terminal;
use anyhow::Result;

pub(crate) struct EmbeddedSession {
    prepared: PreparedProcess,
    title: String,
}

impl EmbeddedSession {
    pub(crate) fn new(prepared: PreparedProcess, title: &str) -> Self {
        Self {
            prepared,
            title: title.to_string(),
        }
    }

    pub(crate) fn run(&self, terminal: &Terminal) -> Result<EmbeddedOutcome> {
        pty::run(
            &self.prepared.argv,
            &self.prepared.environment,
            self.prepared.current_dir.as_deref(),
            terminal,
            &self.title,
        )
    }
}
