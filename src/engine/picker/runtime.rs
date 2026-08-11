use super::PickerView;
use crate::config::Config;
use crate::engine::{RuntimeStore, command};
use anyhow::Result;

impl PickerView {
    pub(crate) fn publish_runtime(
        &self,
        config: &Config,
        runtime: &mut RuntimeStore,
    ) -> Result<()> {
        let frame = self.current();
        let owner = self.command_owner().map(str::to_string);
        let commands = owner
            .as_deref()
            .and_then(|owner| config.view(owner).map(|view| (owner, view)))
            .map(|(owner, view)| {
                view.commands
                    .iter()
                    .map(|(id, command)| command::runtime_command_value(owner, id, command))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let items = frame
            .items
            .iter()
            .map(command::runtime_item_value)
            .collect::<Vec<_>>();
        let selected_item = if frame.command_owner.is_some() {
            self.command_parent_item().map(command::runtime_item_value)
        } else {
            frame
                .items
                .get(frame.selected)
                .map(command::runtime_item_value)
        };
        let log_file = self
            .log_file()
            .map(|path| path.to_string_lossy().to_string());
        runtime.set_many([
            ("/view/active/log_file", serde_json::json!(log_file)),
            (
                "/view/active/selected_index",
                serde_json::json!(frame.selected),
            ),
            (
                "/view/active/selected_item",
                serde_json::json!(selected_item),
            ),
            ("/view/active/items", serde_json::json!(items)),
            ("/view/active/command", serde_json::json!(commands)),
            ("/view/active/command_owner", serde_json::json!(owner)),
        ])?;
        Ok(())
    }
}
