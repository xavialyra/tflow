use super::input::LauncherInputAction;
use super::items::{Item, ItemsEvent, ItemsRequest, ItemsTaskHandle, submit_items_task};
use super::keymap::LauncherKeymap;
use super::render;
use crate::config::Config;
use crate::engine::{
    EngineHost, NavigationMode, TaskCompletion, TaskScheduler, ViewEffect, ViewInstance,
    ViewLocation,
};
use crate::input::{InputDecoder, Key};
use crate::terminal::Terminal;
use crate::text::{matches_query, sanitize_text};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

pub(crate) struct LauncherFrame {
    pub(crate) view: String,
    pub(crate) input: String,
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

impl LauncherFrame {
    pub(crate) fn new(view: &str, input: &str) -> Self {
        Self {
            view: view.to_string(),
            input: input.to_string(),
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

pub(crate) struct LauncherView {
    frame: LauncherFrame,
    tasks: TaskScheduler,
    config: Arc<Config>,
    items_task: Option<ItemsTaskHandle>,
    requested_view: String,
    log_file: Option<PathBuf>,
    decoder: InputDecoder,
    started: bool,
    parent_item: Option<Item>,
    pub(super) keymap: LauncherKeymap,
}

impl LauncherView {
    pub(super) fn new(
        view: &str,
        input: &str,
        tasks: TaskScheduler,
        config: Arc<Config>,
        keymap: LauncherKeymap,
    ) -> Self {
        Self {
            frame: LauncherFrame::new(view, input),
            tasks,
            config,
            items_task: None,
            requested_view: String::new(),
            log_file: None,
            decoder: InputDecoder::default(),
            started: false,
            parent_item: None,
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

    pub(crate) fn current(&self) -> &LauncherFrame {
        &self.frame
    }

    pub(crate) fn current_mut(&mut self) -> &mut LauncherFrame {
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

    pub(crate) fn results_current(&self) -> bool {
        !self.frame.items_pending
            && self.frame.refresh_deadline.is_none()
            && self.frame.results_input == self.frame.input
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

    fn request_current(&mut self, host: &mut EngineHost<'_>) -> Result<Option<ViewEffect>> {
        if self.frame.command_owner.is_some() {
            self.refresh_command_view(host)?;
            return Ok(None);
        }

        let current_view = self.frame.view.clone();
        let current_input = self.frame.input.clone();
        if let Some((target_view, query)) = host
            .config
            .resolve_view_route(&current_view, &current_input)
        {
            self.frame.input.clear();
            self.frame.refresh_deadline = None;
            self.frame.items_pending = false;
            self.frame.pending_command = None;
            return Ok(Some(ViewEffect::Navigate {
                location: ViewLocation::new(target_view, query),
                mode: NavigationMode::Push,
            }));
        }

        let (_, query) = host
            .config
            .resolve_view_prefix(&current_view, &current_input);
        self.publish_runtime(host.config, host.runtime, &query)?;
        self.request_items(&current_view, &current_input);
        Ok(None)
    }

    fn refresh_command_view(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let input = self.frame.input.clone();
        let owner_name = self.frame.command_owner.clone().unwrap_or_default();
        self.publish_runtime(host.config, host.runtime, &input)?;
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

    fn request_items(&mut self, view: &str, input: &str) {
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
            },
        ));
    }

    fn collect_items(&mut self) -> Vec<ItemsEvent> {
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
        if response.view != self.frame.view || response.input != self.frame.input {
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
        if !self.results_current() {
            if self.command_view_active() {
                self.request_current(host)?;
            } else {
                self.queue_pending_command(key);
                self.request_current(host)?;
                return Ok(ViewEffect::Continue);
            }
        }

        let query = self.frame.query.clone();
        self.publish_runtime(host.config, host.runtime, &query)?;
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
        for event in self.collect_items() {
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

impl ViewInstance for LauncherView {
    fn activate(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let query = self.frame.query.clone();
        self.publish_runtime(host.config, host.runtime, &query)
    }

    fn deactivate(&mut self) -> Result<()> {
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
            match self.handle_input(key, &host.config.command_view, command_available) {
                LauncherInputAction::Continue => {}
                LauncherInputAction::Refresh => refresh = true,
                LauncherInputAction::ClearError => host.clear_error(),
                LauncherInputAction::Activate(key) => {
                    return self.handle_command_key(host, terminal, key);
                }
                LauncherInputAction::OpenCommandView => return self.open_command_view(host),
                LauncherInputAction::Back => return Ok(ViewEffect::Back),
                LauncherInputAction::Exit => return Ok(ViewEffect::Exit),
            }
        }
        if refresh {
            host.clear_error();
            self.schedule_refresh();
        }
        Ok(ViewEffect::Continue)
    }

    fn render(&self, host: &EngineHost<'_>, terminal: &Terminal) -> Result<()> {
        let state = self.render_state(host.config);
        let left = host
            .active_error
            .as_ref()
            .map(|error| error.label.as_str())
            .unwrap_or(state.view.as_str());
        render::render_launcher(terminal, &state, left)
    }
}

fn key_display(key: Key) -> String {
    match key {
        Key::Enter => "Enter".to_string(),
        Key::Alt(character) => format!("Alt-{}", character.to_ascii_uppercase()),
        Key::Escape => "Esc".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Backspace => "Backspace".to_string(),
        Key::Ctrl(character) => format!("Ctrl-{}", character.to_ascii_uppercase()),
        Key::Char(character) => character.to_string(),
    }
}
