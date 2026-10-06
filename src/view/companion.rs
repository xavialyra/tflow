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

    /// Compatibility seed for targets whose query represents the source snapshot.
    /// The typed source payload itself is never flattened or parsed from this seed.
    pub(crate) fn query_seed(&self) -> Option<Value> {
        if !self.input.is_null() {
            let Some(mut obj) = self.input.as_object().cloned() else {
                return Some(self.input.clone());
            };
            if !self.parameters.is_null() {
                obj.insert("parameters".into(), self.parameters.clone());
            }
            Some(Value::Object(obj))
        } else if !self.parameters.is_null() {
            Some(self.parameters.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod request_tests;
