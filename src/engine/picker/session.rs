use super::PendingAction;
use super::items::{FeedContext, FeedId, Item, ItemsEvent, ItemsRequest, ItemsTaskHandle};
use super::keymap::PickerKeymap;
use super::preview::{PickerPreview, PickerPreviewConfig};
use super::render;
use crate::config::Config;
use crate::engine::api::{EditorAction, LauncherAction, LauncherOutcome, ResolvedLauncherAction};
use crate::engine::{
    EngineHost, InputRefreshPolicy, TaskCompletion, TaskScheduler, ViewEffect, ViewInstance,
};
use crate::input::{DecodedInput, Key};
use crate::router::Router;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const INPUT_DEBOUNCE: Duration = Duration::from_millis(120);

pub(super) struct PickerOptions {
    pub(super) show_prefix: bool,
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
    request: Option<serde_json::Value>,
    options: PickerOptions,
    started: bool,
    route_child: bool,
    router: Router,
    pub(super) keymap: PickerKeymap,
    preview: Option<PickerPreview>,
}

impl PickerView {
    pub(super) fn new(
        view: &str,
        tasks: TaskScheduler,
        config: Arc<Config>,
        route_child: bool,
        request: Option<serde_json::Value>,
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
            request,
            options,
            started: false,
            route_child,
            router: Router::new(&config),
            keymap,
            preview,
        }
    }

    pub(crate) fn current(&self) -> &PickerFrame {
        &self.frame
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

    pub(crate) fn current_view_ref(&self) -> &str {
        &self.frame.view
    }

    pub(crate) fn queue_pending_action(&mut self, action: PendingAction) {
        self.frame.pending_action = Some(action);
    }

    /// Owner of the selected list row, if any (feed owner or single-source page).
    pub(crate) fn selected_item_owner(&self) -> Option<&str> {
        self.frame
            .items
            .get(self.frame.selected)
            .map(|item| item.source_view.as_str())
    }

    fn request_current(&mut self, host: &mut EngineHost<'_>) -> Result<Option<ViewEffect>> {
        if host.input.rejected {
            return Ok(None);
        }
        self.frame.input_pending = false;

        let current_view = self.frame.view.clone();
        let current_input = host.input.raw.clone();
        // Committed params are the feed default binding raw (successful parse).
        let binding_raw = host.input.params.clone();
        self.publish_runtime(host.config, host.runtime, &host.input.raw)?;
        self.request_items(
            &current_view,
            &current_input,
            &binding_raw,
            host.state.clone(),
        );
        Ok(None)
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
        self.items_task = Some(self.tasks.submit_items(
            &self.config,
            ItemsRequest {
                view: view.to_string(),
                generation: self.request_generation,
                input: input.to_string(),
                binding_raw: binding_raw.to_string(),
                page_state,
                request: self.request.clone(),
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

    fn collect_items(&mut self) -> Vec<ItemsEvent> {
        let mut events = Vec::new();
        let Some(mut task) = self.items_task.take() else {
            return events;
        };
        let response = match task.try_recv() {
            Ok(TaskCompletion::Completed(response)) => response,
            Ok(TaskCompletion::Cancelled) => {
                self.frame.items_pending = false;
                self.schedule_retry();
                return events;
            }
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
                    view: self.requested_view.clone(),
                    errors: Vec::new(),
                    failure: Some("items task stopped before producing a result".to_string()),
                    pending_action: None,
                });
                return events;
            }
        };
        if response.generation != self.request_generation {
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
            view,
            errors,
            failure,
            pending_action,
        });
        events
    }

    fn handle_command_key(
        &mut self,
        host: &mut EngineHost<'_>,
        key: Key,
    ) -> Result<Option<ViewEffect>> {
        if host.input.rejected {
            return Ok(Some(ViewEffect::Continue));
        }
        let current_input = host.input.raw.clone();
        if self.command_requires_items(host.config, key, &current_input)
            && !self.results_current(&current_input)
        {
            self.queue_pending_action(PendingAction::Activate(key));
            self.request_current(host)?;
            return Ok(None);
        }

        self.publish_runtime(host.config, host.runtime, &host.input.raw)?;
        let Some(execution) = self.prepare_command_execution(host, key)? else {
            let view = self.current_view_ref().to_string();
            let message = format!("no command for {}", key_display(key));
            host.record_error_message(Some(&view), None, &message);
            return Ok(Some(ViewEffect::Continue));
        };
        if execution.context.output.is_none()
            && matches!(
                &execution.invocation.command.action,
                crate::config::CommandAction::Return { payload } if payload.value.is_none()
            )
        {
            return Ok(Some(ViewEffect::Continue));
        }
        Ok(Some(ViewEffect::DispatchCommand(execution)))
    }

    fn update_preview(&mut self, config: &Config, terminal: &mut Terminal) {
        let item = self.frame.items.get(self.frame.selected).cloned();
        if let Some(preview) = &mut self.preview {
            preview.update(item.as_ref(), config, terminal);
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
        for event in self.collect_items() {
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

    fn activate_item(&mut self, host: &mut EngineHost<'_>, key: Key) -> Result<Option<ViewEffect>> {
        self.handle_command_key(host, key)
    }
}

impl ViewInstance for PickerView {
    fn activate(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        self.publish_runtime(host.config, host.runtime, &host.input.raw)
    }

    fn restore_input(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
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
        self.items_task.take();
        self.frame.input_pending = false;
        self.frame.retry_requested = false;
        self.frame.items_pending = false;
        self.frame.pending_action = None;
        self.frame.pending_selection = 0;
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        if self.items_task.take().is_some() {
            self.frame.items_pending = false;
            self.schedule_retry();
        }
        if let Some(preview) = &mut self.preview {
            preview.deactivate();
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

    fn resolve_launcher_action(
        &self,
        host: &EngineHost<'_>,
        key: Key,
    ) -> Option<ResolvedLauncherAction> {
        let action = match self.keymap.action(key) {
            Some(super::keymap::PickerAction::Exit) => LauncherAction::Exit,
            Some(super::keymap::PickerAction::Back)
                if self.route_child || host.input.raw.is_empty() =>
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
                if self
                    .resolve_command(host.config, key, &host.input.raw)
                    .is_some()
                    || self.command_requires_items(host.config, key, &host.input.raw) =>
            {
                LauncherAction::Activate
            }
            Some(super::keymap::PickerAction::TogglePreview) => return None,
            None if self
                .resolve_command(host.config, key, &host.input.raw)
                .is_some()
                || self.command_requires_items(host.config, key, &host.input.raw) =>
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
        input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        let key = input
            .key
            .context("picker launcher action has no decoded key")?;
        match action {
            LauncherAction::MoveNext => self.select_item(host, 1)?,
            LauncherAction::MovePrevious => self.select_item(host, -1)?,
            LauncherAction::Activate => {
                if let Some(effect) = self.activate_item(host, key)? {
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

    fn view_command_context(
        &self,
        host: &mut EngineHost<'_>,
    ) -> Result<crate::engine::CommandContext> {
        self.publish_runtime(host.config, host.runtime, &host.input.raw)?;
        self.command_context(host)
    }

    fn chrome(&self, host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        let status = selection_count(self.frame.selected, self.frame.items.len());
        let commands = self.visible_commands(host.config, &host.input.raw);
        let mut presentation = crate::chrome::ChromePresentation::default();
        if let Some(end) = self
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
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn render(&mut self, host: &EngineHost<'_>, frame: &mut Frame, area: Rect) {
        let state = self.render_state();
        let theme = host.theme;
        if let Some(preview) = &mut self.preview {
            let (items_area, preview_area) = preview.areas(area);
            render::render_picker(frame, items_area, &state, &theme);
            if let Some(preview_area) = preview_area {
                preview.render(frame, preview_area, &theme);
                preview.render_separator(frame, items_area, preview_area, &theme);
            }
        } else {
            render::render_picker(frame, area, &state, &theme);
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
    use super::*;
    use crate::chrome::InputBuffer;
    use crate::engine::RuntimeStore;
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
            None,
            PickerKeymap::from_values(None, None).unwrap(),
            PickerOptions {
                show_prefix: false,
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
            None,
            PickerKeymap::from_values(None, None).unwrap(),
            PickerOptions {
                show_prefix: false,
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
        let request = None;
        let theme = crate::theme::Theme::terminal();

        config.update_query_input(&mut state, "AB").unwrap();
        let mut input = InputBuffer::new("AB");
        {
            let mut host = EngineHost {
                config: &config,
                theme,
                input: &input,
                state: &state,
                request: &request,
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
                theme,
                input: &input,
                state: &state,
                request: &request,
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
                theme,
                input: &input,
                state: &state,
                request: &request,
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
}
