use super::{Item, PickerView};
use crate::config::Config;
use crate::engine::CommandInvocation;
use crate::engine::command::{self, CommandAction, CommandContext, CommandItem};
use crate::input::Key;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

impl PickerView {
    pub(crate) fn resolve_command(&self, config: &Config, key: Key) -> Option<CommandInvocation> {
        let frame = self.current();
        if frame.command_owner.is_some() {
            let binding = frame
                .items
                .get(frame.selected)
                .and_then(|item| item.value.as_deref())?;
            return command::find_command(config, frame.command_owner.as_deref()?, binding);
        }
        command::find_command_for_key(config, self.command_owner()?, key)
    }

    pub(crate) fn prepare_command_action(
        &self,
        config: &Config,
        active_state: &crate::state::StateInstance,
        runtime: &Value,
        key: Key,
        log_file: Option<&Path>,
    ) -> Result<Option<CommandAction>> {
        let Some(invocation) = self.resolve_command(config, key) else {
            return Ok(None);
        };
        let item = self.command_item();
        let state = if invocation.source_view == active_state.view_ref() {
            active_state
        } else {
            self.source_states
                .get(&invocation.source_view)
                .unwrap_or(active_state)
        };
        command::prepare_command_action(
            config,
            invocation,
            CommandContext {
                active_view: self.current_view_ref(),
                query: &self.current().query,
                state,
                runtime,
                item: item.map(command_item),
                log_file,
            },
        )
        .map(Some)
    }

    fn command_item(&self) -> Option<&Item> {
        if self.command_view_active() {
            self.command_parent_item()
        } else {
            self.current().items.get(self.current().selected)
        }
    }
}

fn command_item(item: &Item) -> CommandItem<'_> {
    CommandItem {
        text: &item.text,
        value: item.value.as_deref(),
        metadata: &item.metadata,
        source_view: &item.source_view,
    }
}
