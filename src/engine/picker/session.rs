use super::input::{PickerInputAction, ViewCompletion};
use super::items::{Item, ItemsEvent, ItemsRequest, ItemsTaskHandle, submit_items_task};
use super::keymap::PickerKeymap;
use super::render;
use crate::chrome::ShellInput;
use crate::config::Config;
use crate::engine::{
    EngineHost, NavigationMode, TaskCompletion, TaskScheduler, ViewEffect, ViewInstance,
    ViewLocation,
};
use crate::input::{InputDecoder, Key};
use crate::router::Router;
use crate::terminal::Terminal;
use crate::text::{matches_query, sanitize_text};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

pub(crate) struct PickerFrame {
    pub(crate) view: String,
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) query: String,
    pub(crate) refresh_deadline: Option<Instant>,
    pub(crate) requested_input: String,
    pub(crate) results_input: String,
    pub(crate) items_pending: bool,
    pub(crate) pending_command: Option<Key>,
    pub(crate) command_owner: Option<String>,
}

impl PickerFrame {
    pub(crate) fn new(view: &str) -> Self {
        Self {
            view: view.to_string(),
            items: Vec::new(),
            selected: 0,
            query: String::new(),
            refresh_deadline: None,
            requested_input: String::new(),
            results_input: String::new(),
            items_pending: false,
            pending_command: None,
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
    log_file: Option<PathBuf>,
    decoder: InputDecoder,
    started: bool,
    route_child: bool,
    parent_item: Option<Item>,
    router: Router,
    pub(super) completion: Option<ViewCompletion>,
    pub(super) keymap: PickerKeymap,
}

impl PickerView {
    pub(super) fn new(
        view: &str,
        tasks: TaskScheduler,
        config: Arc<Config>,
        route_child: bool,
        keymap: PickerKeymap,
    ) -> Self {
        Self {
            frame: PickerFrame::new(view),
            tasks,
            config: Arc::clone(&config),
            items_task: None,
            requested_view: String::new(),
            log_file: None,
            decoder: InputDecoder::default(),
            started: false,
            route_child,
            parent_item: None,
            router: Router::new(&config),
            completion: None,
            keymap,
        }
    }

    pub(super) fn with_context(
        mut self,
        log_file: Option<PathBuf>,
        command_owner: Option<String>,
        parent_item: Option<Item>,
    ) -> Self {
        self.log_file = log_file;
        self.frame.command_owner = command_owner;
        self.parent_item = parent_item;
        self
    }

    pub(crate) fn current(&self) -> &PickerFrame {
        &self.frame
    }

    pub(crate) fn current_mut(&mut self) -> &mut PickerFrame {
        &mut self.frame
    }

    pub(crate) fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    pub(crate) fn schedule_refresh(&mut self) {
        self.frame.refresh_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.frame.pending_command = None;
    }

    pub(crate) fn refresh_due(&self) -> bool {
        self.frame
            .refresh_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn results_current(&self, input: &str) -> bool {
        !self.frame.items_pending
            && self.frame.refresh_deadline.is_none()
            && self.frame.results_input == input
    }

    pub(crate) fn command_view_active(&self) -> bool {
        self.frame.command_owner.is_some()
    }

    pub(crate) fn current_view_ref(&self) -> &str {
        &self.frame.view
    }

    pub(crate) fn queue_pending_command(&mut self, key: Key) {
        self.frame.pending_command = Some(key);
    }

    pub(crate) fn command_owner(&self) -> Option<&str> {
        self.frame.command_owner.as_deref().or_else(|| {
            self.frame
                .items
                .get(self.frame.selected)
                .map(|item| item.source_view.as_str())
        })
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

    pub(super) fn open_completion(&mut self, input: &ShellInput) {
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

    pub(super) fn accept_completion(&mut self, input: &mut ShellInput) -> bool {
        let Some(completion) = self.completion.take() else {
            return false;
        };
        apply_view_completion(input, &completion)
    }

    fn request_current(&mut self, host: &mut EngineHost<'_>) -> Result<Option<ViewEffect>> {
        if self.frame.command_owner.is_some() {
            self.refresh_command_view(host)?;
            return Ok(None);
        }

        let current_view = self.frame.view.clone();
        let current_input = host.input.raw.clone();
        let query = host.input.params.clone();
        self.publish_runtime(host.config, host.runtime, &current_input, &query)?;
        self.request_items(&current_view, &current_input, &query);
        Ok(None)
    }

    fn refresh_command_view(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let input = host.input.raw.clone();
        let owner_name = self.frame.command_owner.clone().unwrap_or_default();
        self.publish_runtime(host.config, host.runtime, &input, &input)?;
        let commands = host
            .runtime
            .snapshot()
            .pointer("/view/current/command")
            .and_then(serde_json::Value::as_array)
            .context("runtime command list must be an array")?;
        let mut items = commands
            .iter()
            .filter_map(|command| {
                let key = command.get("key")?.as_str()?.to_string();
                let text = sanitize_text(command.get("label")?.as_str()?);
                (!text.is_empty()).then_some(Item {
                    prefix: "cmd".to_string(),
                    text,
                    value: Some(key),
                    metadata: command.clone(),
                    source_view: owner_name.clone(),
                })
            })
            .filter(|item| matches_query(&item.text, &input))
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            super::command::compare_bindings(
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
        self.frame.items_pending = false;
        self.frame.refresh_deadline = None;
        Ok(())
    }

    fn request_items(&mut self, view: &str, input: &str, query: &str) {
        if self.frame.items_pending
            && self.requested_view == view
            && self.frame.requested_input == input
        {
            return;
        }
        self.requested_view = view.to_string();
        self.frame.requested_input = input.to_string();
        self.frame.items_pending = true;
        self.frame.refresh_deadline = None;
        self.items_task = Some(submit_items_task(
            &self.tasks,
            &self.config,
            ItemsRequest {
                view: view.to_string(),
                input: input.to_string(),
                query: query.to_string(),
            },
        ));
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
                self.frame.pending_command = None;
                events.push(ItemsEvent {
                    current: true,
                    view: self.requested_view.clone(),
                    errors: Vec::new(),
                    failure: Some("items task stopped before producing a result".to_string()),
                    pending_command: None,
                });
                return events;
            }
        };
        if !task_response.is_current() {
            self.frame.items_pending = false;
            self.schedule_refresh();
            return events;
        }
        let response = match task_response.into_completion() {
            TaskCompletion::Completed(response) => response,
            TaskCompletion::Cancelled => {
                self.frame.items_pending = false;
                self.schedule_refresh();
                return events;
            }
        };
        if response.view != self.frame.view || response.input != input {
            self.frame.items_pending = false;
            self.schedule_refresh();
            return events;
        }
        let view = response.view;
        let pending_command;
        let (errors, failure) = match response.result {
            Ok(result) => {
                let errors = result.errors;
                self.frame.items_pending = false;
                self.frame.query = response.query;
                self.frame.items = result.items;
                self.frame.results_input = response.input;
                self.frame.selected = self
                    .frame
                    .selected
                    .min(self.frame.items.len().saturating_sub(1));
                pending_command = self.frame.pending_command.take();
                (errors, None)
            }
            Err(error) => {
                self.frame.items_pending = false;
                self.frame.query = response.query;
                self.frame.items.clear();
                self.frame.results_input = response.input;
                self.frame.selected = 0;
                pending_command = self.frame.pending_command.take();
                (Vec::new(), Some(error))
            }
        };
        events.push(ItemsEvent {
            current: true,
            view,
            errors,
            failure,
            pending_command,
        });
        events
    }

    fn open_command_view(&self, host: &EngineHost<'_>) -> Result<ViewEffect> {
        let command_view_ref = host.config.command_view.clone();
        host.config.command_view()?;
        let parent_item = self.frame.items.get(self.frame.selected).cloned();
        let command_owner = parent_item.as_ref().map(|item| item.source_view.clone());
        let context = serde_json::json!({
            "command_owner": command_owner,
            "parent_item": parent_item,
        });
        Ok(ViewEffect::Navigate {
            location: ViewLocation::new(command_view_ref, "").with_context(context),
            mode: NavigationMode::Push,
        })
    }

    fn handle_command_key(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
        key: Key,
    ) -> Result<ViewEffect> {
        if self.command_view_active() && !matches!(key, Key::Enter) {
            return Ok(ViewEffect::Continue);
        }
        if host.input.rejected {
            return Ok(ViewEffect::Continue);
        }
        let current_input = host.input.raw.clone();
        if !self.results_current(&current_input) {
            if host.input.changed {
                self.queue_pending_command(key);
                return Ok(ViewEffect::Continue);
            }
            if self.command_view_active() {
                self.request_current(host)?;
            } else {
                self.queue_pending_command(key);
                self.request_current(host)?;
                return Ok(ViewEffect::Continue);
            }
        }

        let query = self.frame.query.clone();
        self.publish_runtime(host.config, host.runtime, &current_input, &query)?;
        let Some(action) = self.prepare_command_action(
            host.config,
            host.runtime.snapshot(),
            key,
            host.log_file(),
        )?
        else {
            let view = self.current_view_ref().to_string();
            let message = format!("no command for {}", key_display(key));
            host.record_error_message(Some(&view), None, &message);
            return Ok(ViewEffect::Continue);
        };
        let command_view = self.command_view_active();
        match action {
            super::command::CommandAction::Report {
                invocation,
                message,
            } => {
                host.record_error(&invocation, &message);
                Ok(ViewEffect::Continue)
            }
            super::command::CommandAction::Navigate { target, input } => Ok(ViewEffect::Navigate {
                location: ViewLocation::new(target, input),
                mode: if command_view {
                    NavigationMode::Replace
                } else {
                    NavigationMode::Push
                },
            }),
            super::command::CommandAction::Execute {
                invocation,
                prepared,
                exit,
            } => {
                super::command::execute_local(host, prepared, terminal, &invocation, exit)?;
                if exit {
                    Ok(ViewEffect::Exit)
                } else if command_view {
                    Ok(ViewEffect::Back)
                } else {
                    Ok(ViewEffect::Continue)
                }
            }
        }
    }

    fn input_timeout(&self, host: &EngineHost<'_>) -> i32 {
        let now = Instant::now();
        let refresh_remaining = self
            .frame
            .refresh_deadline
            .map(|deadline| deadline.saturating_duration_since(now));
        let error_remaining =
            (*host.active_error_deadline).map(|deadline| deadline.saturating_duration_since(now));
        [refresh_remaining, error_remaining]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or_else(|| Duration::from_millis(INPUT_POLL_MS as u64))
            .as_millis()
            .min(INPUT_POLL_MS as u128)
            .max(1) as i32
    }

    fn handle_events(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<Option<ViewEffect>> {
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

            if let Some(error) = event.failure {
                host.record_error_message(Some(&event.view), None, &error);
            } else if event.errors.is_empty() {
                host.clear_error();
            } else {
                for error in event.errors {
                    host.record_error_message(Some(&event.view), None, &error);
                }
            }

            if let Some(key) = event.pending_command {
                return self.handle_command_key(host, terminal, key).map(Some);
            }
        }
        Ok(None)
    }
}

fn apply_view_completion(input: &mut ShellInput, completion: &ViewCompletion) -> bool {
    let Some(candidate) = completion.candidates.get(completion.selected) else {
        return false;
    };
    let old_cursor = input.cursor;
    let old_length = completion.selector_end - completion.selector_start;
    let replacement = candidate.view_ref.clone();
    input.replace_range(
        completion.selector_start,
        completion.selector_end,
        &replacement,
    );
    if old_cursor > completion.selector_end {
        let new_cursor = if replacement.len() >= old_length {
            old_cursor + replacement.len() - old_length
        } else {
            old_cursor.saturating_sub(old_length - replacement.len())
        };
        input.set_cursor(new_cursor);
    }
    true
}

impl ViewInstance for PickerView {
    fn activate(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let query = self.frame.query.clone();
        self.publish_runtime(host.config, host.runtime, &host.input.raw, &query)
    }

    fn restore_input(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        let input = host.input.raw.clone();
        self.items_task.take();
        self.frame.refresh_deadline = None;
        self.frame.items_pending = false;
        self.frame.pending_command = None;
        self.frame.requested_input = input.clone();
        self.frame.results_input = input;
        Ok(())
    }

    fn input_changed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        self.schedule_refresh();
        Ok(())
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        self.items_task.take();
        self.frame.refresh_deadline = None;
        self.frame.items_pending = false;
        self.frame.pending_command = None;
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        self.close_completion();
        if self.items_task.take().is_some() {
            self.frame.items_pending = false;
            self.schedule_refresh();
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

        if let Some(effect) = self.handle_events(host, terminal)? {
            return Ok(effect);
        }
        if self.refresh_due()
            && let Some(effect) = self.request_current(host)?
        {
            return Ok(effect);
        }

        let bytes = terminal.read_input(self.input_timeout(host))?;
        let mut keys = self.decoder.feed(&bytes);
        keys.extend(self.decoder.flush_due());
        let mut refresh = false;
        for key in keys {
            let command_available = self.resolve_command(host.config, key).is_some();
            let nested_input = self.route_child || self.command_view_active();
            match self.handle_input(
                key,
                &host.config.command_view,
                command_available,
                host.input,
                nested_input,
            ) {
                PickerInputAction::Continue => {}
                PickerInputAction::Refresh => refresh = true,
                PickerInputAction::ClearError => host.clear_error(),
                PickerInputAction::OpenCompletion => self.open_completion(host.input),
                PickerInputAction::CycleCompletion(direction) => self.cycle_completion(direction),
                PickerInputAction::AcceptCompletion => {
                    if self.accept_completion(host.input) {
                        refresh = true;
                    }
                }
                PickerInputAction::CloseCompletion => self.close_completion(),
                PickerInputAction::Activate(key) => {
                    return self.handle_command_key(host, terminal, key);
                }
                PickerInputAction::OpenCommandView => return self.open_command_view(host),
                PickerInputAction::Back => return Ok(ViewEffect::Back),
                PickerInputAction::Exit => return Ok(ViewEffect::Exit),
            }
        }
        if refresh {
            host.input.changed = true;
            host.input.rejected = false;
            host.clear_error();
            self.schedule_refresh();
        }
        Ok(ViewEffect::Continue)
    }

    fn chrome(&self, host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        let searching = self.frame.refresh_deadline.is_some() || self.frame.items_pending;
        let (status, commands) = if let Some(completion) = &self.completion {
            (
                format!("{} views", completion.candidates.len()),
                vec![
                    ("tab".to_string(), "Next".to_string()),
                    ("enter".to_string(), "Open".to_string()),
                    ("escape".to_string(), "Close".to_string()),
                ],
            )
        } else {
            (
                if searching {
                    "searching...".to_string()
                } else {
                    format!("{} results", self.frame.items.len())
                },
                self.visible_commands(host.config),
            )
        };
        crate::chrome::EngineChrome {
            title: None,
            status: Some(status),
            commands,
        }
    }

    fn render(
        &mut self,
        _host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<()> {
        let state = self.render_state();
        render::render_picker(terminal, &state, chrome)
    }
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
    use crate::router::ViewCandidate;

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
        let mut input = ShellInput::new("app query");
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 3,
        };

        assert!(apply_view_completion(&mut input, &completion));
        assert_eq!(input.raw, "apps:main query");
        assert_eq!(input.cursor, input.raw.len());
    }

    #[test]
    fn completion_places_cursor_after_a_selector_edited_in_place() {
        let mut input = ShellInput::new("ap query");
        input.set_cursor(2);
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 2,
        };

        assert!(apply_view_completion(&mut input, &completion));
        assert_eq!(input.raw, "apps:main query");
        assert_eq!(input.cursor, "apps:main".len());
    }
}
