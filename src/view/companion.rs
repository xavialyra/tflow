//! Explicit source data delivered to companion Views, separately from target query and editor input.

use super::ViewCommandSnapshot;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompanionData {
    pub(crate) parameters: Value,
    pub(crate) input: Value,
    pub(crate) engine_state: Value,
}

impl CompanionData {
    pub(crate) fn from_snapshot(snapshot: &ViewCommandSnapshot) -> Self {
        let state = snapshot
            .publication
            .as_ref()
            .map(|p| p.current.clone())
            .unwrap_or(Value::Null);
        Self {
            parameters: snapshot.parameters.clone(),
            input: state.clone(),
            engine_state: state,
        }
    }
}

#[cfg(test)]
mod request_tests;
