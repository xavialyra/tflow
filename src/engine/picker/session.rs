use super::input::{PickerInputAction, ViewCompletion};
use super::items::{Item, ItemsEvent, ItemsRequest, ItemsTaskHandle, submit_items_task};
use super::keymap::PickerKeymap;
use super::render;
use crate::chrome::ShellInput;
use crate::config::Config;
use crate::engine::{
    CompletionRequest, EngineHost, NavigationMode, TaskCompletion, TaskScheduler, ViewEffect,
    ViewInstance, ViewLocation, ViewOutput, ViewOutputItem,
};
use crate::input::{InputDecoder, Key};
use crate::router::Router;
use crate::terminal::Terminal;
use crate::text::{matches_query, sanitize_text};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

pub(super) struct PickerOptions {
    pub(super) show_prefix: bool,
    pub(super) input_prefix: Option<String>,
}

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
            refresh_deadline: None,
            requested_input: String::new(),
            results_input: String::new(),
            items_pending: false,
            pending_command: None,
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
    pub(super) source_states: BTreeMap<String, crate::state::StateInstance>,
    options: PickerOptions,
    feedback: Option<String>,
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
        options: PickerOptions,
    ) -> Self {
        Self {
            frame: PickerFrame::new(view),
            tasks,
            config: Arc::clone(&config),
            items_task: None,
            requested_view: String::new(),
            source_states: BTreeMap::new(),
            options,
            feedback: None,
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

    pub(crate) fn log_file(&self) -> Option<&Path> {
        self.log_file.as_deref()
    }

    pub(super) fn list_presentation(&self) -> (bool, String) {
        (self.options.show_prefix, "(no matches)".to_string())
    }

    pub(crate) fn schedule_refresh(&mut self) {
        self.frame.refresh_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        self.frame.pending_command = None;
        self.frame.pending_selection = 0;
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
                .or(Some(self.frame.view.as_str()))
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
        let source_states = self.source_states(host)?;
        self.request_items(&current_view, &current_input, &query, source_states);
        Ok(None)
    }

    fn refresh_command_view(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let input = host.input.raw.clone();
        let owner_name = self.frame.command_owner.clone().unwrap_or_default();
        self.publish_runtime(host.config, host.runtime, &input, &input)?;
        let commands = host
            .runtime
            .snapshot()
            .pointer("/view/active/command")
            .and_then(serde_json::Value::as_array)
            .context("runtime command list must be an array")?;
        let mut items = commands
            .iter()
            .filter_map(|command| {
                let key = command.get("key")?.as_str()?.to_string();
                let text = sanitize_text(command.get("label")?.as_str()?);
                (!text.is_empty()).then(|| Item {
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

    fn source_states(
        &mut self,
        host: &EngineHost<'_>,
    ) -> Result<BTreeMap<String, crate::state::StateInstance>> {
        let mut states = BTreeMap::new();
        for (source_ref, _) in host.config.source_views(self.current_view_ref())? {
            if source_ref == host.state.view_ref() {
                states.insert(source_ref, host.state.clone());
                continue;
            }
            let state = self
                .source_states
                .entry(source_ref.clone())
                .or_insert(host.config.instantiate_state(&source_ref)?);
            states.insert(source_ref, state.clone());
        }
        Ok(states)
    }

    fn request_items(
        &mut self,
        view: &str,
        input: &str,
        query: &str,
        source_states: BTreeMap<String, crate::state::StateInstance>,
    ) {
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
                source_states,
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
                self.frame.pending_selection = 0;
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
                let pending_selection = std::mem::take(&mut self.frame.pending_selection);
                self.move_selection(pending_selection);
                pending_command = self.frame.pending_command.take();
                (errors, None)
            }
            Err(error) => {
                self.frame.items_pending = false;
                self.frame.query = response.query;
                self.frame.items.clear();
                self.frame.results_input = response.input;
                self.frame.selected = 0;
                self.frame.pending_selection = 0;
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
        let owner_state = command_owner
            .as_deref()
            .and_then(|owner| self.source_states.get(owner))
            .unwrap_or(host.state)
            .clone();
        let context = serde_json::json!({
            "command_owner": command_owner,
            "parent_item": parent_item,
        });
        Ok(ViewEffect::Navigate {
            location: ViewLocation::new(command_view_ref, "")
                .with_context(context)
                .with_owner_state(owner_state),
            mode: NavigationMode::Push,
        })
    }

    fn handle_command_key(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
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
            if host.input.changed {
                self.queue_pending_command(key);
                return Ok(Some(ViewEffect::Continue));
            }
            if self.command_view_active() {
                self.request_current(host)?;
            } else {
                self.queue_pending_command(key);
                self.request_current(host)?;
                return Ok(None);
            }
        }

        let query = self.frame.query.clone();
        self.publish_runtime(host.config, host.runtime, &current_input, &query)?;
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
            super::command::CommandAction::Navigate { target, input } => {
                Ok(Some(ViewEffect::Navigate {
                    location: ViewLocation::new(target, input),
                    mode: if command_view {
                        NavigationMode::Replace
                    } else {
                        NavigationMode::Push
                    },
                }))
            }
            super::command::CommandAction::Complete { invocation, state } => Ok(self
                .complete_selection(
                    &host.input.raw.clone(),
                    invocation,
                    state,
                    host.runtime.snapshot().clone(),
                )),
            super::command::CommandAction::Execute {
                invocation,
                prepared,
                exit,
            } => {
                super::command::execute_local(host, prepared, terminal, &invocation, exit)?;
                if exit {
                    Ok(Some(ViewEffect::Exit))
                } else if command_view {
                    Ok(Some(ViewEffect::Back))
                } else {
                    Ok(Some(ViewEffect::Continue))
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

            if let Some(key) = event.pending_command
                && let Some(effect) = self.activate_item(host, terminal, key)?
            {
                return Ok(Some(effect));
            }
        }
        Ok(None)
    }

    fn clear_feedback(&mut self) {
        self.feedback = None;
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
        self.clear_feedback();
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
        input: &str,
        invocation: super::command::CommandInvocation,
        state: crate::state::StateInstance,
        runtime: serde_json::Value,
    ) -> Option<ViewEffect> {
        let item = self
            .frame
            .items
            .get(self.frame.selected)
            .map(|item| ViewOutputItem {
                text: item.text.clone(),
                value: item.value.clone(),
                metadata: item.metadata.clone(),
                source_view: item.source_view.clone(),
            });
        if item.is_some() || !input.is_empty() {
            return Some(ViewEffect::Complete(CompletionRequest {
                source_view: invocation.source_view,
                command_id: invocation.id,
                state,
                runtime,
                output: ViewOutput::Selected {
                    item,
                    input: input.to_string(),
                },
            }));
        }
        self.feedback = Some("no matching item".to_string());
        None
    }

    fn input_edited(&mut self, host: &mut EngineHost<'_>, refresh: &mut bool) -> Result<()> {
        self.clear_feedback();
        host.clear_error();
        host.input.rejected = false;
        self.frame.pending_command = None;
        self.frame.pending_selection = 0;
        if !self.route_child && host.config.has_query(self.current_view_ref()) {
            host.input.params = host.input.raw.clone();
            let view_ref = self.current_view_ref().to_string();
            host.sync_query_state(&view_ref)?;
        }
        *refresh = !host.input.rejected;
        Ok(())
    }

    fn activate_item(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
        key: Key,
    ) -> Result<Option<ViewEffect>> {
        self.handle_command_key(host, terminal, key)
    }
}

fn apply_view_completion(input: &mut ShellInput, completion: &ViewCompletion) -> bool {
    let Some(candidate) = completion.candidates.get(completion.selected) else {
        return false;
    };
    let old_cursor = input.cursor;
    let old_length = completion.selector_end - completion.selector_start;
    let mut replacement = candidate.view_ref.clone();
    if completion.selector_end == input.raw.len() {
        replacement.push(' ');
    }
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
        self.clear_feedback();
        let input = host.input.raw.clone();
        self.items_task.take();
        self.frame.refresh_deadline = None;
        self.frame.items_pending = false;
        self.frame.pending_command = None;
        self.frame.pending_selection = 0;
        self.frame.requested_input = input.clone();
        self.frame.results_input = input;
        Ok(())
    }

    fn input_changed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        self.clear_feedback();
        self.schedule_refresh();
        Ok(())
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        self.close_completion();
        self.items_task.take();
        self.frame.refresh_deadline = None;
        self.frame.items_pending = false;
        self.frame.pending_command = None;
        self.frame.pending_selection = 0;
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
            let action = self.handle_input(
                key,
                &host.config.command_view,
                command_available,
                host.input,
                nested_input,
            );
            match action {
                PickerInputAction::Continue => {}
                PickerInputAction::Refresh => self.input_edited(host, &mut refresh)?,
                PickerInputAction::Select(direction) => self.select_item(host, direction)?,
                PickerInputAction::OpenCompletion => self.open_completion(host.input),
                PickerInputAction::CycleCompletion(direction) => self.cycle_completion(direction),
                PickerInputAction::AcceptCompletion => {
                    if self.accept_completion(host.input) {
                        self.input_edited(host, &mut refresh)?;
                    }
                }
                PickerInputAction::CloseCompletion => self.close_completion(),
                PickerInputAction::Activate(key) => {
                    if let Some(effect) = self.activate_item(host, terminal, key)? {
                        return Ok(effect);
                    }
                }
                PickerInputAction::OpenCommandView => {
                    return self.open_command_view(host);
                }
                PickerInputAction::Back => return Ok(ViewEffect::Back),
                PickerInputAction::Exit => return Ok(ViewEffect::Exit),
            }
        }
        if refresh {
            host.input.changed = true;
            host.input.rejected = false;
            host.clear_error();
            if self.frame.pending_command.is_none() && self.frame.pending_selection == 0 {
                self.schedule_refresh();
            }
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
                self.feedback.clone().unwrap_or_else(|| {
                    if searching {
                        "searching...".to_string()
                    } else {
                        format!("{} results", self.frame.items.len())
                    }
                }),
                self.visible_commands(host.config),
            )
        };
        crate::chrome::EngineChrome {
            title: None,
            status: Some(status),
            commands,
            presentation: self
                .options
                .input_prefix
                .as_deref()
                .map(crate::chrome::ChromePresentation::with_input_prefix)
                .unwrap_or_default(),
        }
    }

    fn content(
        &mut self,
        _host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<crate::chrome::ChromeContent> {
        let state = self.render_state();
        render::picker_content(terminal, &state, chrome)
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
    fn completion_appends_a_route_separator_at_the_end_of_input() {
        let mut input = ShellInput::new("app");
        let completion = ViewCompletion {
            candidates: vec![candidate()],
            selected: 0,
            selector_start: 0,
            selector_end: 3,
        };

        assert!(apply_view_completion(&mut input, &completion));
        assert_eq!(input.raw, "apps:main ");
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
