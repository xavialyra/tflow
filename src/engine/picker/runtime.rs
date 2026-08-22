use super::{Item, PickerView};
use crate::config::Config;
use crate::engine::{RuntimeStore, command};
use anyhow::Result;

impl PickerView {
    pub(crate) fn publish_runtime(
        &self,
        config: &Config,
        runtime: &mut RuntimeStore,
        input: &str,
    ) -> Result<()> {
        let frame = self.current();
        let selected_owner = self
            .results_current(input)
            .then(|| self.selected_item_owner())
            .flatten();
        let owner = selected_owner.or(Some(self.current_view_ref()));
        let page_view = self.current_view_ref();
        let commands = command::collect_page_owner_commands(config, page_view, selected_owner)?
            .into_values()
            .collect::<Vec<_>>();
        let items = frame
            .items
            .iter()
            .map(runtime_item_value)
            .collect::<Vec<_>>();
        let selected_item = self
            .results_current(input)
            .then(|| frame.items.get(frame.selected))
            .flatten()
            .map(runtime_item_value);
        let base = runtime
            .snapshot()
            .pointer("/view/current")
            .cloned()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "ref": frame.view,
                    "input": frame.query,
                    "raw_input": frame.query,
                    "query": frame.query,
                })
            });
        let mut current = serde_json::Map::new();
        current.insert(
            "ref".to_string(),
            base.get("ref")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.view)),
        );
        current.insert(
            "input".to_string(),
            base.get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
        );
        current.insert(
            "raw_input".to_string(),
            base.get("raw_input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
        );
        current.insert(
            "query".to_string(),
            base.get("query")
                .cloned()
                .unwrap_or_else(|| serde_json::json!(frame.query)),
        );
        if let Some(value) = base.get("state_revision").filter(|value| !value.is_null()) {
            current.insert("state_revision".to_string(), value.clone());
        }
        current.insert(
            "selected_index".to_string(),
            serde_json::json!(frame.selected),
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
        runtime.set_many([("/view/current", serde_json::Value::Object(current))])?;
        Ok(())
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
    fn public_item_provenance_excludes_internal_feed_context() {
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
