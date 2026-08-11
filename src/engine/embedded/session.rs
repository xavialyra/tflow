use crate::engine::PreparedProcess;
use anyhow::{Context, Result};

pub(crate) struct EmbeddedSession {
    prepared: Option<PreparedProcess>,
}

impl EmbeddedSession {
    pub(crate) fn new(prepared: PreparedProcess) -> Self {
        Self {
            prepared: Some(prepared),
        }
    }

    pub(crate) fn take_prepared(&mut self) -> Result<PreparedProcess> {
        self.prepared
            .take()
            .context("embedded session was already started")
    }
}
