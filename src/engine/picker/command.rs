use super::{Item, PickerView};
use crate::config::{CommandRequirement, CommandScope, Config};
use crate::engine::command;
use crate::engine::{
    CommandContext, CommandExecution, CommandInvocation, CommandOwnerContext,
    CommandSelectionContext, EngineHost, ViewOutput, ViewOutputItem,
};
use crate::input::Key;
use anyhow::{Context, Result};

impl PickerView {
    pub(crate) fn resolve_command(
        &self,
        config: &Config,
        key: Key,
        input: &str,
    ) -> Option<CommandInvocation> {
        let page = self.current_view_ref();
        let page_command = command::find_command_for_key(config, page, key);
        let owner = self
            .results_current(input)
            .then(|| self.selected_item_owner())
            .flatten()
            .filter(|owner| *owner != page);
        let owner_command = owner
            .and_then(|owner| command::find_command_for_key(config, owner, key))
            .filter(|invocation| invocation.command.scope == CommandScope::Selection);
        owner_command.or(page_command)
    }

    pub(crate) fn command_requires_items(&self, config: &Config, key: Key, input: &str) -> bool {
        if let Some(invocation) = self.resolve_command(config, key, input) {
            return invocation.command.requires == CommandRequirement::Items;
        }
        if self.results_current(input) {
            return false;
        }
        let requires_items = |owner: &str| {
            command::find_command_for_key(config, owner, key).is_some_and(|invocation| {
                invocation.command.scope == CommandScope::Selection
                    && invocation.command.requires == CommandRequirement::Items
            })
        };
        if self
            .selected_item_owner()
            .filter(|owner| *owner != self.current_view_ref())
            .is_some_and(requires_items)
        {
            return true;
        }
        config
            .feed_views(self.current_view_ref())
            .unwrap_or_default()
            .into_iter()
            .any(|(owner, _)| requires_items(&owner))
    }

    pub(crate) fn prepare_command_execution(
        &self,
        host: &EngineHost<'_>,
        key: Key,
    ) -> Result<Option<CommandExecution>> {
        let Some(invocation) = self.resolve_command(host.config, key, &host.input.raw) else {
            return Ok(None);
        };
        let context = self.command_context(host)?;
        command::resolve_visible_command(
            host.config,
            &context,
            invocation
                .view_reference()
                .expect("picker command invocation has a footer origin"),
        )?;
        Ok(Some(CommandExecution {
            invocation,
            context,
        }))
    }

    pub(crate) fn command_context(&self, host: &EngineHost<'_>) -> Result<CommandContext> {
        let page = CommandOwnerContext {
            view_ref: self.current_view_ref().to_string(),
            state: host.state.clone(),
            binding_raw: host.input.params.clone(),
        };
        let selection = self
            .results_current(&host.input.raw)
            .then(|| self.current().items.get(self.current().selected))
            .flatten()
            .map(|item| self.selection_context(item, &page))
            .transpose()?;
        let output_item = selection.as_ref().map(|selection| selection.item.clone());
        let output =
            (output_item.is_some() || !page.binding_raw.is_empty()).then(|| ViewOutput::Selected {
                item: output_item,
                input: page.binding_raw.clone(),
            });
        Ok(CommandContext {
            page,
            selection,
            runtime: host.runtime.snapshot().clone(),
            request: host.request.clone(),
            output,
            log_file: host.runtime_log.path().map(std::path::Path::to_path_buf),
        })
    }

    fn selection_context(
        &self,
        item: &Item,
        page: &CommandOwnerContext,
    ) -> Result<CommandSelectionContext> {
        let owner = if item.source_view == page.view_ref {
            page.clone()
        } else {
            let context = self
                .feed_contexts
                .get(&item.feed_id)
                .filter(|context| context.owner_view == item.source_view)
                .with_context(|| {
                    format!(
                        "selected item owner {:?} has no matching feed context",
                        item.source_view
                    )
                })?;
            CommandOwnerContext {
                view_ref: context.owner_view.clone(),
                state: context.state.clone(),
                binding_raw: context.binding_raw.clone(),
            }
        };
        Ok(CommandSelectionContext {
            owner,
            item: ViewOutputItem {
                text: item.text.clone(),
                value: item.value.clone(),
                metadata: item.metadata.clone(),
                source_view: item.source_view.clone(),
            },
        })
    }
}
