//! Correlation contracts shared by the protocol host, task runtime, and Views.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ViewInstanceId(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TaskId(pub(crate) u64);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TaskOutcome {
    Completed(Value),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TaskEvent {
    pub(crate) instance: ViewInstanceId,
    pub(crate) task: TaskId,
    pub(crate) generation: u64,
    pub(crate) outcome: TaskOutcome,
}
