use super::{Item, PickerView};
use crate::config::Config;
use crate::engine::CommandInvocation;
use crate::engine::command::{self, CommandAction, CommandContext, CommandItem};
use crate::input::Key;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

impl PickerView {
    pub(crate) fn resolve_command(&self, config: &Config, key: Key) -> Option<CommandInvocation> {
        let frame = self.current();
        if self.command_view_active() {
            let item = frame.items.get(frame.selected)?;
            let binding = item.value.as_deref()?;
            return command::find_command(config, &item.source_view, binding);
        }
        // Owner commands win on key conflicts; page-level commands remain available.
        let owner = self.selected_item_owner();
        let page = self.current_view_ref();
        match (owner, page) {
            (Some(owner), page) if owner != page => {
                command::find_command_for_key_on_views(config, [owner, page], key)
            }
            (Some(owner), _) => command::find_command_for_key(config, owner, key),
            (None, page) => command::find_command_for_key(config, page, key),
        }
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
        let page_view = self
            .command_page_view
            .as_deref()
            .unwrap_or_else(|| self.current_view_ref());
        let page_state = self.command_page_state.as_ref().unwrap_or(active_state);
        let page_binding_raw = self
            .command_page_binding_raw
            .as_deref()
            .unwrap_or(&self.current().query);
        let (state, binding_raw) = if invocation.source_view == page_view {
            (page_state, page_binding_raw)
        } else {
            let context = item
                .and_then(|item| self.feed_contexts.get(&item.feed_id))
                .filter(|context| context.owner_view == invocation.source_view)
                .with_context(|| {
                    format!(
                        "command owner {:?} has no matching feed context",
                        invocation.source_view
                    )
                })?;
            (&context.state, context.binding_raw.as_str())
        };
        let evaluation_runtime = self.command_evaluation_runtime(runtime);
        command::prepare_command_action(
            config,
            invocation,
            CommandContext {
                active_view: page_view,
                state,
                binding_raw,
                runtime: &evaluation_runtime,
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

    pub(crate) fn command_evaluation_runtime(&self, runtime: &Value) -> Value {
        let Some(parent_runtime) = self.command_page_runtime() else {
            return runtime.clone();
        };
        let mut overlay = runtime.clone();
        if let (Some(current), Some(view)) = (
            parent_runtime.pointer("/view/current"),
            overlay.get_mut("view").and_then(Value::as_object_mut),
        ) {
            view.insert("current".to_string(), current.clone());
        }
        if let (Some(input), Some(session)) = (
            parent_runtime.pointer("/session/input"),
            overlay.get_mut("session").and_then(Value::as_object_mut),
        ) {
            session.insert("input".to_string(), input.clone());
        }
        overlay
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
