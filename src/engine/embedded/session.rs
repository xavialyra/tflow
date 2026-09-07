use crate::engine::EmbeddedResultConfig;
use crate::engine::EmbeddedTerminal;
use crate::engine::embedded::pty::{EmbeddedPoll, EmbeddedRuntime};
use crate::execution::PreparedProcess;
use crate::lifecycle::CancellationObserver;
use anyhow::Result;

#[derive(Debug, Clone, Copy)]
pub(super) struct EmbeddedStartPlan;

pub(crate) struct EmbeddedSession {
    prepared: PreparedProcess,
    result: Option<EmbeddedResultConfig>,
    cancellation: CancellationObserver,
    runtime: Option<EmbeddedRuntime>,
    pending_input: Vec<u8>,
}

impl EmbeddedSession {
    pub(crate) fn new(
        prepared: PreparedProcess,
        result: Option<EmbeddedResultConfig>,
        cancellation: CancellationObserver,
    ) -> Self {
        Self {
            prepared,
            result,
            cancellation,
            runtime: None,
            pending_input: Vec::new(),
        }
    }

    /// Build the inert start description before the external boundary.
    pub(super) fn prepare_start(&self) -> EmbeddedStartPlan {
        EmbeddedStartPlan
    }

    /// Create the PTY and child for an already prepared start. The returned
    /// runtime is not visible to the session until `commit_start` succeeds.
    pub(super) fn start_external(
        &mut self,
        _plan: EmbeddedStartPlan,
        size: (u16, u16),
    ) -> Result<EmbeddedRuntime> {
        anyhow::ensure!(
            self.runtime.is_none(),
            "embedded session start was committed more than once"
        );
        let pending_input = std::mem::take(&mut self.pending_input);
        let mut runtime = EmbeddedRuntime::start(&self.prepared, self.result, size, &[])?;
        if let Err(error) = runtime.push_input(&pending_input) {
            drop(runtime);
            return Err(error);
        }
        Ok(runtime)
    }

    /// Publish a successfully created external runtime after its start stage.
    pub(super) fn commit_start(&mut self, runtime: EmbeddedRuntime) {
        assert!(
            self.runtime.is_none(),
            "embedded session start was committed more than once"
        );
        self.runtime = Some(runtime);
    }

    pub(crate) fn poll(&mut self, size: (u16, u16)) -> Result<EmbeddedPoll> {
        self.runtime
            .as_mut()
            .map(|runtime| runtime.poll(size, &self.cancellation))
            .unwrap_or_else(|| anyhow::bail!("embedded session has not been started"))
    }

    pub(crate) fn poll_background(&mut self) -> Result<EmbeddedPoll> {
        self.runtime
            .as_mut()
            .map(|runtime| runtime.poll_background(&self.cancellation))
            .unwrap_or_else(|| anyhow::bail!("embedded session has not been started"))
    }

    pub(crate) fn is_started(&self) -> bool {
        self.runtime.is_some()
    }

    pub(crate) fn deactivate(&mut self) {
        // Dropping EmbeddedRuntime closes the PTY and terminates the process
        // group. Taking it here makes lifecycle cleanup explicit and idempotent.
        self.runtime.take();
        self.pending_input.clear();
    }

    pub(crate) fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(runtime) = &mut self.runtime {
            runtime.push_input(bytes)?;
        } else {
            self.pending_input.extend_from_slice(bytes);
        }
        Ok(())
    }

    pub(crate) fn screen(&self) -> Option<&EmbeddedTerminal> {
        self.runtime.as_ref().map(EmbeddedRuntime::screen)
    }
}
