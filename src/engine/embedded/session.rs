use crate::engine::PreparedProcess;
pub(crate) struct EmbeddedSession {
    prepared: PreparedProcess,
}

impl EmbeddedSession {
    pub(crate) fn new(prepared: PreparedProcess) -> Self {
        Self { prepared }
    }

    pub(crate) fn prepared(&self) -> PreparedProcess {
        self.prepared.clone()
    }
}
