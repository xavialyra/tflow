use super::{Item, PickerView};
use crate::engine::{EngineRuntimeSnapshot, RuntimeUpdate};
use anyhow::Result;
use serde_json::Value;

struct RuntimeProjection {
    input: String,
    reference: Value,
    current_input: Value,
    raw_input: Value,
    query: Value,
    state_revision: Option<Value>,
    cursor: Option<Value>,
    buffer_revision: Option<Value>,
}

impl PickerView {
    pub(crate) fn runtime_update(
        &self,
        runtime: &EngineRuntimeSnapshot,
        input: &str,
    ) -> Result<RuntimeUpdate> {
        let frame = self.current();
        let base = runtime.current();
        self.runtime_update_value(RuntimeProjection {
            input: input.to_string(),
            reference: base
                .get("ref")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.view)),
            current_input: base
                .get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
            raw_input: base
                .get("raw_input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
            query: base
                .get("query")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
            state_revision: base
                .get("state_revision")
                .filter(|value| !value.is_null())
                .cloned(),
            cursor: base.get("cursor").cloned(),
            buffer_revision: base.get("buffer_revision").cloned(),
        })
    }

    fn runtime_update_value(&self, projection: RuntimeProjection) -> Result<RuntimeUpdate> {
        let RuntimeProjection {
            input,
            reference,
            current_input,
            raw_input,
            query,
            state_revision,
            cursor,
            buffer_revision,
        } = projection;
        let frame = self.current();
        let results_current = self.results_current(&input);
        let page_view = self.current_view_ref();
        let commands = self
            .services
            .page_commands(page_view)?
            .into_values()
            .collect::<Vec<_>>();
        let items = if results_current {
            frame
                .selection
                .items
                .iter()
                .map(runtime_item_value)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let selected_item = results_current
            .then(|| frame.selection.items.get(frame.selection.selected))
            .flatten()
            .map(runtime_item_value);
        let mut current = serde_json::Map::new();
        current.insert("ref".to_string(), reference);
        current.insert("input".to_string(), current_input);
        current.insert("raw_input".to_string(), raw_input);
        current.insert("query".to_string(), query);
        if let Some(value) = state_revision.filter(|value| !value.is_null()) {
            current.insert("state_revision".to_string(), value);
        }
        if let Some(value) = cursor {
            current.insert("cursor".to_string(), value);
        }
        if let Some(value) = buffer_revision {
            current.insert("buffer_revision".to_string(), value);
        }
        current.insert(
            "selected_index".to_string(),
            serde_json::json!(frame.selection.selected),
        );
        // Keep an explicit null when no item is selected for a stable runtime shape.
        current.insert(
            "selected_item".to_string(),
            selected_item.unwrap_or(serde_json::Value::Null),
        );
        current.insert("items".to_string(), serde_json::json!(items));
        current.insert("command".to_string(), serde_json::json!(commands));
        Ok(RuntimeUpdate {
            path: "/view/current".to_string(),
            value: serde_json::Value::Object(current),
        })
    }
}

fn runtime_item_value(item: &Item) -> serde_json::Value {
    serde_json::json!({
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_item_provenance_excludes_internal_feed_state() {
        let value = runtime_item_value(&Item {
            text: "Terminal".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("Terminal".to_string()).into(),
            value: Some("terminal".to_string()),
            metadata: serde_json::json!({"kind": "app"}),
            bindings: std::collections::BTreeMap::new(),
            source_view: "apps:default".to_string(),
        });
        assert!(value.get("owner_view").is_none());
        assert!(value.get("feed_id").is_none());
        assert!(value.get("state").is_none());
        assert!(value.get("binding_raw").is_none());
        assert!(value.get("query").is_none());
        assert!(value.get("owner_query").is_none());
        assert!(value.get("source_view").is_none());
    }
}
