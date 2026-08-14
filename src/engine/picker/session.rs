use super::PendingAction;
use super::items::{
    FeedContext, FeedId, Item, ItemsEvent, ItemsRequest, ItemsTaskHandle, submit_items_task,
};
use super::keymap::PickerKeymap;
use super::preview::{PickerPreview, PickerPreviewConfig};
use super::render;
use crate::chrome::InputBuffer;
use crate::config::Config;
use crate::engine::api::{
    EditorAction, InputEdit, LauncherAction, LauncherOutcome, ResolvedLauncherAction,
};
use crate::engine::command::{self, CommandAction};
use crate::engine::{
    CommandInvocation, CommandPickerContext, CommandPickerItem, CommandPickerOwnerContext,
    CompletionRequest, EngineHost, InputRefreshPolicy, NavigationMode, NavigationRequest,
    TaskCompletion, TaskScheduler, ViewEffect, ViewInstance, ViewOutput, ViewOutputItem,
};
use crate::input::Key;
use crate::router::{Router, ViewCandidate};
use crate::terminal::Terminal;
use crate::text::{matches_query, sanitize_text};
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const INPUT_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone)]
pub(crate) struct ViewCompletion {
    pub(crate) candidates: Vec<ViewCandidate>,
    pub(crate) selected: usize,
    pub(crate) selector_start: usize,
    pub(crate) selector_end: usize,
}

pub(super) struct PickerOptions {
    pub(super) show_prefix: bool,
    pub(super) input_prefix: Option<String>,
    pub(super) preview: Option<PickerPreviewConfig>,
}

pub(crate) struct PickerFrame {
    pub(crate) view: String,
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) query: String,
    pub(crate) input_pending: bool,
    pub(crate) retry_requested: bool,
    pub(crate) requested_input: String,
    pub(crate) results_input: String,
    pub(crate) results_valid: bool,
    pub(crate) items_pending: bool,
    pub(crate) pending_action: Option<PendingAction>,
    pub(crate) pending_selection: isize,
    pub(crate) command_owner: Option<String>,
}

impl PickerFrame {
    pub(crate) fn new(view: &str) -> Self {
        Self {
            view: view.to_string(),
            items: Vec::new(),
            selected: 0,
            query: String::new(),
            input_pending: false,
            retry_requested: false,
            requested_input: String::new(),
            results_input: String::new(),
            results_valid: false,
            items_pending: false,
            pending_action: None,
            pending_selection: 0,
            command_owner: None,
        }
    }
}

pub(crate) struct PickerView {
    frame: PickerFrame,
    tasks: TaskScheduler,
    config: Arc<Config>,
    items_task: Option<ItemsTaskHandle>,
    requested_view: String,
    requested_binding_raw: String,
    requested_state_revision: u64,
    requested_page_state: Option<crate::state::StateInstance>,
    request_generation: u64,
    pub(super) feed_contexts: BTreeMap<FeedId, FeedContext>,
    pub(super) command_page_view: Option<String>,
    pub(super) command_page_state: Option<crate::state::StateInstance>,
    pub(super) command_page_binding_raw: Option<String>,
    command_page_runtime: Option<serde_json::Value>,
    options: PickerOptions,
    log_file: Option<PathBuf>,
    started: bool,
    route_child: bool,
    parent_item: Option<Item>,
    router: Router,
    pub(super) completion: Option<ViewCompletion>,
    pub(super) keymap: PickerKeymap,
    preview: Option<PickerPreview>,
}

impl PickerView {
    pub(super) fn new(
        view: &str,
        tasks: TaskScheduler,
        config: Arc<Config>,
        route_child: bool,
        keymap: PickerKeymap,
        options: PickerOptions,
    ) -> Self {
        let preview = options.preview.clone().map(PickerPreview::new);
        Self {
            frame: PickerFrame::new(view),
            tasks,
            config: Arc::clone(&config),
            items_task: None,
            requested_view: String::new(),
            requested_binding_raw: String::new(),
            requested_state_revision: 0,
            requested_page_state: None,
            request_generation: 0,
            feed_contexts: BTreeMap::new(),
            command_page_view: None,
            command_page_state: None,
            command_page_binding_raw: None,
            command_page_runtime: None,
            options,
            log_file: None,
            started: false,
            route_child,
            parent_item: None,
            router: Router::new(&config),
            completion: None,
            keymap,
            preview,
        }
    }

    pub(super) fn with_context(
        mut self,
        log_file: Option<PathBuf>,
        command_context: Option<CommandPickerContext>,
    ) -> Self {
        self.log_file = log_file;
        let Some(context) = command_context else {
            return self;
        };
        self.command_page_view = Some(context.page_view);
        self.command_page_state = Some(context.page_state);
        self.command_page_binding_raw = Some(context.page_binding_raw);
        self.command_page_runtime = Some(context.page_runtime);
        if let Some(owner) = context.owner {
            self.frame.command_owner = Some(owner.view_ref.clone());
            self.feed_contexts.insert(
                FeedId(owner.view_ref.clone()),
                FeedContext {
                    owner_view: owner.view_ref,
                    state: owner.state,
                    binding_raw: owner.binding_raw,
                },
            );
        }
        self.parent_item = context.parent_item.map(|item| Item {
            prefix: item.prefix,
            text: item.text,
            value: item.value,
            metadata: item.metadata,
            feed_id: FeedId(item.source_view.clone()),
            source_view: item.source_view,
        });
        self
    }

    pub(crate) fn current(&self) -> &PickerFrame {
        &self.frame
    }

    pub(crate) fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    pub(super) fn list_presentation(&self) -> (bool, String) {
        (self.options.show_prefix, "(no matches)".to_string())
    }

    pub(crate) fn schedule_retry(&mut self) {
        self.frame.results_valid = false;
        if self.frame.input_pending {
            self.frame.retry_requested = false;
            return;
        }
        self.frame.retry_requested = true;
        self.frame.pending_action = None;
        self.frame.pending_selection = 0;
    }

    pub(crate) fn results_current(&self, input: &str) -> bool {
        self.frame.results_valid
            && !self.frame.input_pending
            && !self.frame.retry_requested
            && !self.frame.items_pending
            && self.frame.results_input == input
    }

    pub(crate) fn command_view_active(&self) -> bool {
        self.command_page_view.is_some()
    }

    pub(crate) fn current_view_ref(&self) -> &str {
        &self.frame.view
    }

    pub(crate) fn queue_pending_action(&mut self, action: PendingAction) {
        self.frame.pending_action = Some(action);
    }

    pub(crate) fn command_owner(&self) -> Option<&str> {
        if self.command_view_active() {
            return self
                .frame
                .command_owner
                .as_deref()
                .or(self.command_page_view.as_deref());
        }
        self.selected_item_owner()
            .or(Some(self.frame.view.as_str()))
    }

    pub(super) fn command_view_owner(&self) -> Option<&str> {
        self.frame.command_owner.as_deref()
    }

    pub(super) fn command_page_runtime(&self) -> Option<&serde_json::Value> {
        self.command_page_runtime.as_ref()
    }

    /// Owner of the selected list row, if any (feed owner or single-source page).
    pub(crate) fn selected_item_owner(&self) -> Option<&str> {
        self.frame
            .items
            .get(self.frame.selected)
            .map(|item| item.source_view.as_str())
    }

    pub(crate) fn command_parent_item(&self) -> Option<&Item> {
        self.parent_item.as_ref()
    }

    pub(super) fn completion_active(&self) -> bool {
        self.completion.is_some()
    }

    pub(super) fn close_completion(&mut self) {
        self.completion = None;
    }

    pub(super) fn open_completion(&mut self, input: &InputBuffer) {
        let selector_end = input
            .raw
            .find(char::is_whitespace)
            .unwrap_or(input.raw.len());
        let query_end = input.cursor.min(selector_end);
        let selector = &input.raw[..query_end];
        self.completion = Some(ViewCompletion {
            candidates: self.router.complete_views(selector),
            selected: 0,
            selector_start: 0,
            selector_end,
        });
    }

    pub(super) fn cycle_completion(&mut self, direction: isize) {
        let Some(completion) = self.completion.as_mut() else {
            return;
        };
        if completion.candidates.is_empty() {
            return;
        }
        let count = completion.candidates.len() as isize;
        completion.selected =
            ((completion.selected as isize + direction).rem_euclid(count)) as usize;
    }

    pub(super) fn accept_completion(&mut self, input: &InputBuffer) -> Option<InputEdit> {
        let completion = self.completion.take()?;
        view_completion_edit(input, &completion)
    }

    fn request_current(&mut self, host: &mut EngineHost<'_>) -> Result<Option<ViewEffect>> {
        if host.input.rejected {
            return Ok(None);
        }
        self.frame.input_pending = false;
        if self.command_view_active() {
            self.refresh_command_view(host)?;
            return Ok(None);
        }

        let current_view = self.frame.view.clone();
        let current_input = host.input.raw.clone();
        // Committed params are the feed default binding raw (successful parse).
        let binding_raw = host.input.params.clone();
        self.publish_runtime(host.config, host.runtime)?;
        self.request_items(
            &current_view,
            &current_input,
            &binding_raw,
            host.state.clone(),
        );
        Ok(None)
    }

    fn refresh_command_view(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let input = host.input.raw.clone();
        let page_view = self
            .command_page_view
            .as_deref()
            .context("command picker has no page")?;
        let commands = command::collect_page_owner_commands(
            host.config,
            page_view,
            self.frame.command_owner.as_deref(),
        )?;
        let mut items = commands
            .into_iter()
            .filter_map(|(key, command)| {
                let source_view = command.get("owner")?.as_str()?.to_string();
                let text = sanitize_text(command.get("label")?.as_str()?);
                (!text.is_empty()).then(|| Item {
                    prefix: "cmd".to_string(),
                    text,
                    value: Some(key),
                    metadata: command,
                    feed_id: FeedId(source_view.clone()),
                    source_view,
                })
            })
            .filter(|item| matches_query(&item.text, &input))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            command::compare_bindings(
                left.value.as_deref().unwrap_or_default(),
                right.value.as_deref().unwrap_or_default(),
            )
        });
        self.frame.items = items;
        self.frame.selected = self
            .frame
            .selected
            .min(self.frame.items.len().saturating_sub(1));
        self.frame.query = input.clone();
        self.frame.requested_input = input.clone();
        self.frame.results_input = input;
        self.frame.results_valid = true;
        self.frame.items_pending = false;
        self.frame.input_pending = false;
        self.frame.retry_requested = false;
        Ok(())
    }

    fn request_items(
        &mut self,
        view: &str,
        input: &str,
        binding_raw: &str,
        page_state: crate::state::StateInstance,
    ) {
        let state_revision = page_state.revision();
        if self.frame.items_pending
            && self.requested_view == view
            && self.frame.requested_input == input
            && self.requested_binding_raw == binding_raw
            && self.requested_state_revision == state_revision
            && self.requested_page_state.as_ref() == Some(&page_state)
        {
            return;
        }
        self.request_generation = self.request_generation.wrapping_add(1);
        self.requested_view = view.to_string();
        self.requested_binding_raw = binding_raw.to_string();
        self.requested_state_revision = state_revision;
        self.requested_page_state = Some(page_state.clone());
        self.frame.requested_input = input.to_string();
        self.frame.items_pending = true;
        self.frame.retry_requested = false;
        self.items_task = Some(submit_items_task(
            &self.tasks,
            &self.config,
            ItemsRequest {
                view: view.to_string(),
                generation: self.request_generation,
                input: input.to_string(),
                binding_raw: binding_raw.to_string(),
                page_state,
            },
        ));
    }

    fn invalidate_items_for_committed_input(&mut self) {
        self.items_task.take();
        self.frame.input_pending = true;
        self.frame.retry_requested = false;
        self.frame.results_valid = false;
        self.frame.items_pending = false;
        self.frame.pending_action = None;
        self.frame.pending_selection = 0;
    }

    fn collect_items(&mut self, input: &str) -> Vec<ItemsEvent> {
        let mut events = Vec::new();
        let Some(mut task) = self.items_task.take() else {
            return events;
        };
        let task_response = match task.try_recv() {
            Ok(response) => response,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.items_task = Some(task);
                return events;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.frame.items_pending = false;
                self.frame.results_valid = false;
                self.frame.pending_action = None;
                self.frame.pending_selection = 0;
                events.push(ItemsEvent {
                    current: true,
                    view: self.requested_view.clone(),
                    errors: Vec::new(),
                    failure: Some("items task stopped before producing a result".to_string()),
                    pending_action: None,
                });
                return events;
            }
        };
        if !task_response.is_current() {
            self.frame.items_pending = false;
            self.schedule_retry();
            return events;
        }
        let response = match task_response.into_completion() {
            TaskCompletion::Completed(response) => response,
            TaskCompletion::Cancelled => {
                self.frame.items_pending = false;
                self.schedule_retry();
                return events;
            }
        };
        if response.view != self.frame.view
            || response.input != input
            || response.generation != self.request_generation
            || response.binding_raw != self.requested_binding_raw
            || response.state_revision != self.requested_state_revision
        {
            self.frame.items_pending = false;
            self.schedule_retry();
            return events;
        }
        let view = response.view;
        let pending_action;
        let (errors, failure) = match response.result {
            Ok(result) => {
                let errors = result.errors;
                self.frame.items_pending = false;
                self.frame.query = response.query;
                self.feed_contexts = result.contexts;
                self.frame.items = result.items;
                self.frame.results_input = response.input;
                self.frame.results_valid = true;
                self.frame.selected = self
                    .frame
                    .selected
                    .min(self.frame.items.len().saturating_sub(1));
                let pending_selection = std::mem::take(&mut self.frame.pending_selection);
                self.move_selection(pending_selection);
                pending_action = self.frame.pending_action.take();
                (errors, None)
            }
            Err(error) => {
                self.frame.items_pending = false;
                self.frame.query = response.query;
                self.feed_contexts.clear();
                self.frame.items.clear();
                self.frame.results_input = response.input;
                self.frame.results_valid = true;
                self.frame.selected = 0;
                self.frame.pending_selection = 0;
                self.frame.pending_action = None;
                pending_action = None;
                (Vec::new(), Some(error))
            }
        };
        events.push(ItemsEvent {
            current: true,
            view,
            errors,
            failure,
            pending_action,
        });
        events
    }

    fn open_command_view(&self, host: &EngineHost<'_>) -> Result<ViewEffect> {
        let command_view_ref = host.config.command_view.clone();
        host.config.command_view()?;
        let parent_item = self.frame.items.get(self.frame.selected);
        let owner = parent_item
            .and_then(|item| self.feed_contexts.get(&item.feed_id))
            .filter(|context| context.owner_view != self.frame.view)
            .map(|context| CommandPickerOwnerContext {
                view_ref: context.owner_view.clone(),
                state: context.state.clone(),
                binding_raw: context.binding_raw.clone(),
            });
        let parent_item = parent_item.map(|item| CommandPickerItem {
            prefix: item.prefix.clone(),
            text: item.text.clone(),
            value: item.value.clone(),
            metadata: item.metadata.clone(),
            source_view: item.source_view.clone(),
        });
        let context = CommandPickerContext {
            page_view: self.frame.view.clone(),
            page_state: host.state.clone(),
            page_binding_raw: self.frame.query.clone(),
            page_runtime: host.runtime.snapshot().clone(),
            owner,
            parent_item,
        };
        Ok(ViewEffect::Navigate {
            request: NavigationRequest::new(command_view_ref, "").with_command_picker(context),
            mode: NavigationMode::Push,
        })
    }

    fn handle_open_command_view(
        &mut self,
        host: &mut EngineHost<'_>,
    ) -> Result<Option<ViewEffect>> {
        if host.input.rejected {
            return Ok(Some(ViewEffect::Continue));
        }
        if !self.results_current(&host.input.raw) {
            self.queue_pending_action(PendingAction::OpenCommandView);
            self.request_current(host)?;
            return Ok(None);
        }
        self.publish_runtime(host.config, host.runtime)?;
        self.open_command_view(host).map(Some)
    }

    fn handle_command_key(
        &mut self,
        host: &mut EngineHost<'_>,
        key: Key,
    ) -> Result<Option<ViewEffect>> {
        if self.command_view_active() && !matches!(key, Key::Enter) {
            return Ok(Some(ViewEffect::Continue));
        }
        if host.input.rejected {
            return Ok(Some(ViewEffect::Continue));
        }
        let current_input = host.input.raw.clone();
        if !self.results_current(&current_input) {
            if self.command_view_active() {
                self.request_current(host)?;
            } else {
                self.queue_pending_action(PendingAction::Activate(key));
                self.request_current(host)?;
                return Ok(None);
            }
        }

        self.publish_runtime(host.config, host.runtime)?;
        let Some(action) = self.prepare_command_action(
            host.config,
            host.state,
            host.runtime.snapshot(),
            key,
            host.log_file(),
        )?
        else {
            let view = self.current_view_ref().to_string();
            let message = format!("no command for {}", key_display(key));
            host.record_error_message(Some(&view), None, &message);
            return Ok(Some(ViewEffect::Continue));
        };
        let command_view = self.command_view_active();
        match action {
            CommandAction::Navigate { target, query } => {
                let request = match query {
                    Some(query) => NavigationRequest::new(target, "").with_query(query),
                    None => NavigationRequest::with_defaults(target),
                };
                Ok(Some(ViewEffect::Navigate {
                    request,
                    mode: if command_view {
                        NavigationMode::Replace
                    } else {
                        NavigationMode::Push
                    },
                }))
            }
            CommandAction::Complete {
                invocation,
                state,
                binding_raw,
            } => {
                let runtime = self.command_evaluation_runtime(host.runtime.snapshot());
                Ok(self.complete_selection(invocation, state, binding_raw, runtime))
            }
            CommandAction::Execute {
                invocation,
                prepared,
                exit,
            } => Ok(Some(ViewEffect::RunCommand {
                invocation,
                prepared,
                exit,
                return_to_parent: command_view,
            })),
        }
    }

    fn update_preview(&mut self, config: &Config, terminal: &mut Terminal) {
        let item = (!self.completion_active())
            .then(|| self.frame.items.get(self.frame.selected))
            .flatten()
            .cloned();
        if let Some(preview) = &mut self.preview {
            preview.update(item.as_ref(), config, &self.tasks, terminal);
        }
    }

    fn input_timeout(&self, host: &EngineHost<'_>) -> i32 {
        (*host.active_error_deadline)
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| Duration::from_millis(INPUT_POLL_MS as u64))
            .as_millis()
            .min(INPUT_POLL_MS as u128)
            .max(1) as i32
    }

    fn handle_events(&mut self, host: &mut EngineHost<'_>) -> Result<Option<ViewEffect>> {
        let input = host.input.raw.clone();
        for event in self.collect_items(&input) {
            if !event.current {
                for error in event.errors {
                    host.runtime_log.record(
                        crate::runtime_log::LogLevel::Error,
                        Some(&event.view),
                        None,
                        &error,
                    );
                }
                if let Some(error) = event.failure {
                    host.runtime_log.record(
                        crate::runtime_log::LogLevel::Error,
                        Some(&event.view),
                        None,
                        &error,
                    );
                }
                continue;
            }

            let failed = event.failure.is_some();
            if let Some(error) = event.failure {
                host.record_error_message(Some(&event.view), None, &error);
            } else if event.errors.is_empty() {
                host.clear_error();
            } else {
                for error in event.errors {
                    host.record_error_message(Some(&event.view), None, &error);
                }
            }

            if !failed && let Some(action) = event.pending_action {
                let effect = match action {
                    PendingAction::Activate(key) => self.activate_item(host, key)?,
                    PendingAction::OpenCommandView => self.handle_open_command_view(host)?,
                };
                if effect.is_some() {
                    return Ok(effect);
                }
            }
        }
        Ok(None)
    }

    fn move_selection(&mut self, direction: isize) {
        if self.frame.items.is_empty() {
            self.frame.selected = 0;
            return;
        }
        let last = self.frame.items.len() - 1;
        self.frame.selected = self
            .frame
            .selected
            .saturating_add_signed(direction)
            .min(last);
    }

    fn select_item(&mut self, host: &mut EngineHost<'_>, direction: isize) -> Result<()> {
        if host.input.rejected {
            self.move_selection(direction);
            return Ok(());
        }
        host.clear_error();
        let input = host.input.raw.clone();
        if !self.results_current(&input) {
            self.frame.pending_selection = self.frame.pending_selection.saturating_add(direction);
            self.request_current(host)?;
        } else {
            self.move_selection(direction);
        }
        Ok(())
    }

    fn complete_selection(
        &mut self,
        invocation: CommandInvocation,
        state: crate::state::StateInstance,
        binding_raw: String,
        runtime: serde_json::Value,
    ) -> Option<ViewEffect> {
        let selected = if self.command_view_active() {
            self.command_parent_item()
        } else {
            self.frame.items.get(self.frame.selected)
        };
        let item = selected.map(|item| ViewOutputItem {
            text: item.text.clone(),
            value: item.value.clone(),
            metadata: item.metadata.clone(),
            source_view: item.source_view.clone(),
        });
        if item.is_some() || !binding_raw.is_empty() {
            return Some(ViewEffect::Complete(CompletionRequest {
                source_view: invocation.source_view,
                command_id: invocation.id,
                state,
                binding_raw: binding_raw.clone(),
                runtime,
                output: ViewOutput::Selected {
                    item,
                    input: binding_raw,
                },
            }));
        }
        None
    }

    fn activate_item(&mut self, host: &mut EngineHost<'_>, key: Key) -> Result<Option<ViewEffect>> {
        self.handle_command_key(host, key)
    }
}

fn view_completion_edit(input: &InputBuffer, completion: &ViewCompletion) -> Option<InputEdit> {
    let candidate = completion.candidates.get(completion.selected)?;
    let old_length = completion.selector_end - completion.selector_start;
    let mut replacement = candidate.view_ref.clone();
    if completion.selector_end == input.raw.len() {
        replacement.push(' ');
    }
    let cursor = if input.cursor > completion.selector_end {
        if replacement.len() >= old_length {
            input.cursor + replacement.len() - old_length
        } else {
            input.cursor.saturating_sub(old_length - replacement.len())
        }
    } else {
        completion.selector_start + replacement.len()
    };
    Some(InputEdit::ReplaceRange {
        start: completion.selector_start,
        end: completion.selector_end,
        replacement,
        cursor,
    })
}

impl ViewInstance for PickerView {
    fn activate(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        self.publish_runtime(host.config, host.runtime)
    }

    fn restore_input(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        let input = host.input.raw.clone();
        self.items_task.take();
        self.frame.input_pending = false;
        self.frame.retry_requested = false;
        self.frame.items_pending = false;
        self.frame.pending_action = None;
        self.frame.pending_selection = 0;
        if !self.results_current(&input) {
            self.request_current(host)?;
        }
        Ok(())
    }

    fn input_committed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.invalidate_items_for_committed_input();
        Ok(())
    }

    fn input_refresh_policy(&self) -> InputRefreshPolicy {
        InputRefreshPolicy::Debounced(INPUT_DEBOUNCE)
    }

    fn input_ready(&mut self, host: &mut EngineHost<'_>) -> Result<ViewEffect> {
        self.frame.input_pending = false;
        if !self.results_current(&host.input.raw)
            && let Some(effect) = self.request_current(host)?
        {
            return Ok(effect);
        }
        Ok(ViewEffect::Continue)
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        self.items_task.take();
        self.frame.input_pending = false;
        self.frame.retry_requested = false;
        self.frame.items_pending = false;
        self.frame.pending_action = None;
        self.frame.pending_selection = 0;
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        self.close_completion();
        if self.items_task.take().is_some() {
            self.frame.items_pending = false;
            self.schedule_retry();
        }
        Ok(())
    }

    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect> {
        if !self.started {
            self.started = true;
            if let Some(effect) = self.request_current(host)? {
                return Ok(effect);
            }
        }

        if let Some(effect) = self.handle_events(host)? {
            return Ok(effect);
        }
        self.update_preview(host.config, terminal);
        if self.frame.retry_requested
            && let Some(effect) = self.request_current(host)?
        {
            return Ok(effect);
        }
        Ok(ViewEffect::Continue)
    }

    fn launcher_input_timeout(&self, host: &EngineHost<'_>) -> Option<i32> {
        Some(self.input_timeout(host))
    }

    fn captures_editor_input(&self) -> bool {
        self.completion_active()
    }

    fn resolve_launcher_action(
        &self,
        host: &EngineHost<'_>,
        key: Key,
    ) -> Option<ResolvedLauncherAction> {
        if self.completion_active() {
            let action = match key {
                Key::Escape => LauncherAction::CloseCompletion,
                Key::Enter => LauncherAction::AcceptCompletion,
                Key::Tab | Key::Down => LauncherAction::CycleCompletionNext,
                Key::BackTab | Key::Up => LauncherAction::CycleCompletionPrevious,
                _ => LauncherAction::DismissCompletion,
            };
            return Some(ResolvedLauncherAction::View(action));
        }
        let action = match self.keymap.action(key) {
            Some(super::keymap::PickerAction::Exit) => LauncherAction::Exit,
            Some(super::keymap::PickerAction::OpenCommands)
                if self.current().view != host.config.command_view =>
            {
                LauncherAction::OpenCommandView
            }
            Some(super::keymap::PickerAction::OpenCompletion) => LauncherAction::OpenCompletion,
            Some(super::keymap::PickerAction::Back)
                if self.route_child || self.command_view_active() || host.input.raw.is_empty() =>
            {
                LauncherAction::Back
            }
            Some(super::keymap::PickerAction::Back) => {
                return Some(ResolvedLauncherAction::Edit(EditorAction::ClearInput));
            }
            Some(super::keymap::PickerAction::DeleteBackward) => {
                return Some(ResolvedLauncherAction::Edit(EditorAction::DeleteBackward));
            }
            Some(super::keymap::PickerAction::ClearInput) => {
                return Some(ResolvedLauncherAction::Edit(EditorAction::ClearInput));
            }
            Some(super::keymap::PickerAction::DeleteWord) => {
                return Some(ResolvedLauncherAction::Edit(EditorAction::DeleteWord));
            }
            Some(super::keymap::PickerAction::SelectPrevious) => LauncherAction::MovePrevious,
            Some(super::keymap::PickerAction::SelectNext) => LauncherAction::MoveNext,
            Some(super::keymap::PickerAction::Activate) => LauncherAction::Activate,
            Some(super::keymap::PickerAction::TogglePreview) if self.preview.is_some() => {
                LauncherAction::TogglePreview
            }
            Some(super::keymap::PickerAction::TogglePreview)
                if self.resolve_command(host.config, key).is_some()
                    && !self.command_view_active() =>
            {
                LauncherAction::Activate
            }
            Some(super::keymap::PickerAction::TogglePreview) => return None,
            None if self.resolve_command(host.config, key).is_some()
                && !self.command_view_active() =>
            {
                LauncherAction::Activate
            }
            _ => return None,
        };
        Some(ResolvedLauncherAction::View(action))
    }

    fn handle_launcher_action(
        &mut self,
        host: &mut EngineHost<'_>,
        action: LauncherAction,
        key: Key,
    ) -> Result<LauncherOutcome> {
        match action {
            LauncherAction::MoveNext => self.select_item(host, 1)?,
            LauncherAction::MovePrevious => self.select_item(host, -1)?,
            LauncherAction::OpenCompletion => self.open_completion(host.input),
            LauncherAction::CycleCompletionNext => self.cycle_completion(1),
            LauncherAction::CycleCompletionPrevious => self.cycle_completion(-1),
            LauncherAction::AcceptCompletion => {
                if let Some(edit) = self.accept_completion(host.input) {
                    return Ok(LauncherOutcome::EditInput(edit));
                }
            }
            LauncherAction::CloseCompletion => self.close_completion(),
            LauncherAction::DismissCompletion => {
                self.close_completion();
                return Ok(LauncherOutcome::ReplayKey(key));
            }
            LauncherAction::Activate => {
                if let Some(effect) = self.activate_item(host, key)? {
                    return Ok(LauncherOutcome::Effect(Box::new(effect)));
                }
            }
            LauncherAction::OpenCommandView => {
                if let Some(effect) = self.handle_open_command_view(host)? {
                    return Ok(LauncherOutcome::Effect(Box::new(effect)));
                }
            }
            LauncherAction::TogglePreview => {
                if let Some(preview) = &mut self.preview {
                    preview.toggle_visibility();
                }
            }
            LauncherAction::Back => {
                return Ok(LauncherOutcome::Effect(Box::new(ViewEffect::Back(None))));
            }
            LauncherAction::Exit => {
                return Ok(LauncherOutcome::Effect(Box::new(ViewEffect::Exit)));
            }
        }
        Ok(LauncherOutcome::Continue)
    }

    fn chrome(&self, host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        let (status, commands) = if let Some(completion) = &self.completion {
            (
                selection_count(completion.selected, completion.candidates.len()),
                vec![
                    ("tab".to_string(), "Next".to_string()),
                    ("enter".to_string(), "Open".to_string()),
                    ("escape".to_string(), "Close".to_string()),
                ],
            )
        } else {
            (
                selection_count(self.frame.selected, self.frame.items.len()),
                self.visible_commands(host.config),
            )
        };
        let mut presentation = self
            .options
            .input_prefix
            .as_deref()
            .map(crate::chrome::ChromePresentation::with_input_prefix)
            .unwrap_or_default();
        if !self.command_view_active()
            && self.frame.view != host.config.command_view
            && let Some(end) = self
                .router
                .recognized_prefix_end(&self.frame.view, &host.input.raw)
        {
            presentation = presentation.with_recognized_input_prefix(end);
        }
        crate::chrome::EngineChrome {
            title: None,
            status: Some(status),
            commands,
            presentation,
        }
    }

    fn render(&mut self, _host: &EngineHost<'_>, frame: &mut Frame, area: Rect) {
        let state = self.render_state();
        if let Some(preview) = &mut self.preview {
            let (items_area, preview_area) = preview.areas(area);
            render::render_picker(frame, items_area, &state);
            if let Some(preview_area) = preview_area {
                preview.render(frame, preview_area);
                preview.render_separator(frame, items_area, preview_area);
            }
        } else {
            render::render_picker(frame, area, &state);
        }
    }
}

fn selection_count(selected: usize, total: usize) -> String {
    let current = if total == 0 {
        0
    } else {
        selected.saturating_add(1)
    };
    format!("{current} of {total}")
}

fn key_display(key: Key) -> String {
    match key {
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::BackTab => "Shift-Tab".to_string(),
        Key::Alt(character) => format!("Alt-{}", character.to_ascii_uppercase()),
        Key::Escape => "Esc".to_string(),
        Key::Left => "Left".to_string(),
        Key::Right => "Right".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Delete => "Delete".to_string(),
        Key::Ctrl(character) => format!("Ctrl-{}", character.to_ascii_uppercase()),
        Key::Char(character) => character.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::items::ItemsResponse;
    use super::*;
    use crate::engine::RuntimeStore;
    use crate::router::ViewCandidate;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn failed_item_refresh_discards_pending_actions() {
        let config = Arc::new(crate::config::load_test_fixture().unwrap());
        let runtime = RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let mut picker = PickerView::new(
            "core:default",
            tasks,
            config,
            false,
            PickerKeymap::from_values(None, None).unwrap(),
            PickerOptions {
                show_prefix: false,
                input_prefix: None,
                preview: None,
            },
        );
        picker.frame.items_pending = true;
        picker.frame.pending_action = Some(PendingAction::OpenCommandView);
        picker.items_task = Some(picker.tasks.submit_keyed(
            (),
            "picker-items".to_string(),
            |_, _, _| ItemsResponse {
                view: "core:default".to_string(),
                generation: 0,
                input: String::new(),
                binding_raw: String::new(),
                state_revision: 0,
                query: String::new(),
                result: Err("items provider failed".to_string()),
            },
        ));

        let mut events = Vec::new();
        for _ in 0..10 {
            events = picker.collect_items("");
            if !events.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].failure.as_deref(), Some("items provider failed"));
        assert!(events[0].pending_action.is_none());
        assert!(picker.frame.pending_action.is_none());
    }

    #[test]
    fn state_identity_advances_generation_when_view_and_raw_input_match() {
        let config = Arc::new(crate::config::load_test_fixture().unwrap());
        let runtime = RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let mut picker = PickerView::new(
            "core:default",
            tasks,
            Arc::clone(&config),
            false,
            PickerKeymap::from_values(None, None).unwrap(),
            PickerOptions {
                show_prefix: false,
                input_prefix: None,
                preview: None,
            },
        );
        let mut first = config.instantiate_state("core:default").unwrap();
        let mut second = config.instantiate_state("core:default").unwrap();
        config.update_query_input(&mut first, "first").unwrap();
        config.update_query_input(&mut second, "second").unwrap();
        assert_eq!(first.revision(), second.revision());

        picker.request_items("core:default", "same", "same", first);
        let first_generation = picker.request_generation;
        picker.request_items("core:default", "same", "same", second);

        assert_eq!(picker.request_generation, first_generation + 1);
    }

    #[test]
    fn input_round_trip_waits_for_ready_and_requests_the_latest_snapshot() {
        let config = Arc::new(crate::config::load_test_fixture().unwrap());
        let mut runtime = RuntimeStore::new();
        runtime.replace(serde_json::json!({
            "view": {"current": {"ref": "core:default"}},
            "session": {"input": {"raw": "A", "params": "A"}}
        }));
        let tasks = TaskScheduler::new(runtime.handle());
        let mut picker = PickerView::new(
            "core:default",
            tasks,
            Arc::clone(&config),
            false,
            PickerKeymap::from_values(None, None).unwrap(),
            PickerOptions {
                show_prefix: false,
                input_prefix: None,
                preview: None,
            },
        );
        let mut state = config.instantiate_state("core:default").unwrap();
        config.update_query_input(&mut state, "A").unwrap();
        picker.frame.results_input = "A".to_string();
        picker.frame.results_valid = true;
        picker.request_items("core:default", "A", "A", state.clone());
        let first_generation = picker.request_generation;
        let mut runtime_log = crate::runtime_log::RuntimeLog::disabled();
        let mut active_error = None;
        let mut active_error_deadline = None;

        config.update_query_input(&mut state, "AB").unwrap();
        let mut input = InputBuffer::new("AB");
        {
            let mut host = EngineHost {
                config: &config,
                input: &input,
                state: &state,
                runtime: &mut runtime,
                runtime_log: &mut runtime_log,
                active_error: &mut active_error,
                active_error_deadline: &mut active_error_deadline,
            };
            picker.input_committed(&mut host).unwrap();
        }

        config.update_query_input(&mut state, "A").unwrap();
        input = InputBuffer::new("A");
        {
            let mut host = EngineHost {
                config: &config,
                input: &input,
                state: &state,
                runtime: &mut runtime,
                runtime_log: &mut runtime_log,
                active_error: &mut active_error,
                active_error_deadline: &mut active_error_deadline,
            };
            picker.input_committed(&mut host).unwrap();
        }

        assert_eq!(state.revision(), 3);
        assert_eq!(picker.request_generation, first_generation);
        assert!(picker.items_task.is_none());
        assert!(picker.frame.input_pending);
        assert!(!picker.frame.retry_requested);
        assert!(!picker.frame.results_valid);

        let effect = {
            let mut host = EngineHost {
                config: &config,
                input: &input,
                state: &state,
                runtime: &mut runtime,
                runtime_log: &mut runtime_log,
                active_error: &mut active_error,
                active_error_deadline: &mut active_error_deadline,
            };
            picker.input_ready(&mut host).unwrap()
        };

        assert!(matches!(effect, ViewEffect::Continue));
        assert_eq!(picker.request_generation, first_generation + 1);
        assert_eq!(picker.requested_state_revision, state.revision());
        assert_eq!(picker.requested_page_state.as_ref(), Some(&state));
    }

    #[test]
    fn selection_count_uses_zero_for_an_empty_list() {
        assert_eq!(selection_count(0, 0), "0 of 0");
        assert_eq!(selection_count(1, 3), "2 of 3");
    }

    fn candidate() -> ViewCandidate {
        ViewCandidate {
            view_ref: "apps:main".to_string(),
            alias: Some("app".to_string()),
            plugin_name: "Applications".to_string(),
            engine_type: "picker".to_string(),
        }
    }

    #[test]
    fn completion_preserves_cursor_in_the_query() {
        let input = InputBuffer::new("app query");
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 3,
        };

        assert_eq!(
            view_completion_edit(&input, &completion),
            Some(InputEdit::ReplaceRange {
                start: 0,
                end: 3,
                replacement: "apps:main".to_string(),
                cursor: "apps:main query".len(),
            })
        );
    }

    #[test]
    fn completion_appends_a_route_separator_at_the_end_of_input() {
        let input = InputBuffer::new("app");
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 3,
        };

        assert_eq!(
            view_completion_edit(&input, &completion),
            Some(InputEdit::ReplaceRange {
                start: 0,
                end: 3,
                replacement: "apps:main ".to_string(),
                cursor: "apps:main ".len(),
            })
        );
    }

    #[test]
    fn completion_places_cursor_after_a_selector_edited_in_place() {
        let mut input = InputBuffer::new("ap query");
        input.set_cursor(2);
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 2,
        };

        assert_eq!(
            view_completion_edit(&input, &completion),
            Some(InputEdit::ReplaceRange {
                start: 0,
                end: 2,
                replacement: "apps:main".to_string(),
                cursor: "apps:main".len(),
            })
        );
    }
}
