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
        let selected_owner = results_current
            .then(|| self.selected_item_owner())
            .flatten();
        let owner = selected_owner.or(Some(self.current_view_ref()));
        let page_view = self.current_view_ref();
        let commands = self
            .services
            .page_commands(page_view, selected_owner)?
            .into_values()
            .filter(|value| {
                if results_current {
                    return true;
                }
                let Some(view_ref) = value
                    .pointer("/ref/view")
                    .and_then(serde_json::Value::as_str)
                else {
                    return false;
                };
                let Some(command_id) = value.pointer("/ref/id").and_then(serde_json::Value::as_str)
                else {
                    return false;
                };
                self.services.is_non_selection_command(view_ref, command_id)
            })
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
        // Keep null so templates like selected_item resolve instead of missing.
        current.insert(
            "selected_item".to_string(),
            selected_item.unwrap_or(serde_json::Value::Null),
        );
        current.insert("items".to_string(), serde_json::json!(items));
        current.insert("command".to_string(), serde_json::json!(commands));
        current.insert(
            "command_owner".to_string(),
            match owner {
                Some(owner) => serde_json::Value::String(owner.to_string()),
                None => serde_json::Value::Null,
            },
        );
        Ok(RuntimeUpdate {
            path: "/view/current".to_string(),
            value: serde_json::Value::Object(current),
        })
    }
}

fn runtime_item_value(item: &Item) -> serde_json::Value {
    serde_json::json!({
        "prefix": item.prefix,
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
        "owner_view": item.source_view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::picker::items::FeedId;

    #[test]
    fn public_item_provenance_excludes_internal_feed_state() {
        let value = runtime_item_value(&Item {
            prefix: "app".to_string(),
            text: "Terminal".to_string(),
            value: Some("terminal".to_string()),
            metadata: serde_json::json!({"kind": "app"}),
            source_view: "apps:default".to_string(),
            feed_id: FeedId("apps:default".to_string()),
        });
        assert_eq!(value["owner_view"], "apps:default");
        assert!(value.get("feed_id").is_none());
        assert!(value.get("state").is_none());
        assert!(value.get("binding_raw").is_none());
        assert!(value.get("query").is_none());
        assert!(value.get("owner_query").is_none());
        assert!(value.get("source_view").is_none());
    }
}
