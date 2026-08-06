use super::LauncherDriver;
use crate::config::Config;
use crate::engine::RuntimeStore;
use anyhow::Result;

impl LauncherDriver {
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
        let selected_item = frame
            .items
            .get(frame.selected)
            .map(super::command::runtime_item_value);
        runtime.set(
            "",
            serde_json::json!({
                "view": {
                    "current": {
                        "ref": frame.view,
                        "input": frame.input,
                        "query": frame.query,
                        "selected_index": frame.selected,
                        "selected_item": selected_item,
                        "items": items,
                        "command": commands,
                        "command_owner": owner,
                    }
                }
            }),
        )?;
        Ok(())
    }
}
