use super::PickerView;
use crate::config::Config;
use crate::engine::RuntimeStore;
use anyhow::Result;

impl PickerView {
    pub(crate) fn publish_runtime(
        &self,
        config: &Config,
        runtime: &mut RuntimeStore,
        raw_input: &str,
        query: &str,
    ) -> Result<()> {
        let frame = self.current();
        let owner = self.command_owner().map(str::to_string);
        let commands = owner
            .as_deref()
            .and_then(|owner| config.view(owner).map(|view| (owner, view)))
            .map(|(owner, view)| {
                view.commands
                    .iter()
                    .map(|(id, command)| super::command::runtime_command_value(owner, id, command))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let items = frame
            .items
            .iter()
            .map(super::command::runtime_item_value)
            .collect::<Vec<_>>();
        let selected_item = if frame.command_owner.is_some() {
            self.command_parent_item()
                .map(super::command::runtime_item_value)
        } else {
            frame
                .items
                .get(frame.selected)
                .map(super::command::runtime_item_value)
        };
        let log_file = self
            .log_file()
            .map(|path| path.to_string_lossy().to_string());
        runtime.set(
            "/view",
            serde_json::json!({
                "active": {
                    "ref": frame.view,
                    "input": query,
                    "raw_input": raw_input,
                    "query": query,
                    "request": {
                        "input": query,
                        "raw_input": raw_input,
                        "query": query,
                    },
                    "log_file": log_file,
                    "selected_index": frame.selected,
                    "selected_item": selected_item,
                    "items": items,
                    "command": commands,
                    "command_owner": owner,
                }
            }),
        )?;
        runtime.set(
            "/session",
            serde_json::json!({"input": {"raw": raw_input, "params": query}}),
        )?;
        Ok(())
    }
}
