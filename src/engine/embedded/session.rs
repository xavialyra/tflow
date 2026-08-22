use crate::cancellation::CancellationToken;
use crate::embedded_terminal::EmbeddedTerminal;
use crate::engine::EmbeddedResultConfig;
use crate::engine::PreparedProcess;
use crate::engine::embedded::pty::{EmbeddedPoll, EmbeddedRuntime};
use anyhow::Result;

pub(crate) struct EmbeddedSession {
    prepared: PreparedProcess,
    result: Option<EmbeddedResultConfig>,
    cancellation: CancellationToken,
    runtime: Option<EmbeddedRuntime>,
}

impl EmbeddedSession {
    pub(crate) fn new(
        prepared: PreparedProcess,
        result: Option<EmbeddedResultConfig>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            prepared,
            result,
            cancellation,
            runtime: None,
        }
    }

    pub(crate) fn start(&mut self, size: (u16, u16), initial_input: &[u8]) -> Result<()> {
        if self.runtime.is_none() {
            self.runtime = Some(EmbeddedRuntime::start(
                &self.prepared,
                self.result,
                size,
                initial_input,
            )?);
        }
        Ok(())
    }

    pub(crate) fn poll(&mut self, size: (u16, u16)) -> Result<EmbeddedPoll> {
        self.runtime
            .as_mut()
            .map(|runtime| runtime.poll(size, &self.cancellation))
            .unwrap_or_else(|| anyhow::bail!("embedded session has not been started"))
    }

    pub(crate) fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(runtime) = &mut self.runtime {
            runtime.push_input(bytes)?;
        }
        Ok(())
    }

    pub(crate) fn request_resize(&mut self, size: (u16, u16)) {
        if let Some(runtime) = &mut self.runtime {
            runtime.request_resize(size);
        }
    }

    pub(crate) fn screen(&self) -> Option<&EmbeddedTerminal> {
        self.runtime.as_ref().map(EmbeddedRuntime::screen)
    }

    pub(crate) fn is_running(&self) -> bool {
        self.runtime.is_some()
    }
}
