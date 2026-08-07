use super::discovery::{
    DiscoveryEvent, DiscoveryRequest, DiscoveryResponse, discovery_result_error, discovery_worker,
};
use super::render;
use crate::config::Config;
use crate::discovery::{Item, matches_query, sanitize_text};
use crate::engine::{
    EngineDriver, EngineHost, EngineKeyAction, Key, LauncherEngine, RuntimeHandle, SessionEffect,
};
use crate::input::InputDecoder;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

pub(crate) struct LauncherFrame {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) active_rule: String,
    pub(crate) query: String,
    pub(crate) refresh_deadline: Option<Instant>,
    pub(crate) requested_input: String,
    pub(crate) results_input: String,
    pub(crate) discovery_pending: bool,
    pub(crate) pending_command: Option<Key>,
    pub(crate) command_owner: Option<String>,
}

impl LauncherFrame {
    pub(crate) fn new(view: &str, default_rule: &str, input: &str) -> Self {
        Self {
            view: view.to_string(),
            input: input.to_string(),
            items: Vec::new(),
            selected: 0,
            active_rule: default_rule.to_string(),
            query: String::new(),
            refresh_deadline: None,
            requested_input: String::new(),
            results_input: String::new(),
            discovery_pending: false,
            pending_command: None,
            command_owner: None,
        }
    }
}

pub(crate) struct LauncherDriver {
    frame: LauncherFrame,
    discovery_tx: Sender<DiscoveryRequest>,
    discovery_rx: Receiver<DiscoveryResponse>,
    next_request_id: u64,
    latest_request_id: u64,
    requested_view: String,
    log_file: Option<PathBuf>,
    decoder: InputDecoder,
    started: bool,
    parent_item: Option<Item>,
}

impl LauncherDriver {
    pub(crate) fn new(
        view: &str,
        default_rule: &str,
        input: &str,
        config: Config,
        runtime: RuntimeHandle,
        log_file: Option<PathBuf>,
        command_owner: Option<String>,
        parent_item: Option<Item>,
    ) -> Self {
        let (discovery_tx, request_rx) = mpsc::channel();
        let (response_tx, discovery_rx) = mpsc::channel();
        thread::spawn(move || discovery_worker(config, runtime, request_rx, response_tx));
        let mut frame = LauncherFrame::new(view, default_rule, input);
        frame.command_owner = command_owner;
        Self {
            frame,
            discovery_tx,
            discovery_rx,
            next_request_id: 0,
            latest_request_id: 0,
            requested_view: String::new(),
            log_file,
            decoder: InputDecoder::default(),
            started: false,
            parent_item,
        }
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
        !self.frame.discovery_pending
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

    fn request_current(&mut self, host: &mut EngineHost<'_>) -> Result<Option<SessionEffect>> {
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
            self.frame.discovery_pending = false;
            self.frame.pending_command = None;
            return Ok(Some(SessionEffect::OpenView {
                view_ref: target_view,
                input: query,
                replace_current: false,
            }));
        }

        let (_, query) = host
            .config
            .resolve_view_prefix(&current_view, &current_input);
        let (_, _, request_query) = host.config.resolve_rule(&query);
        self.publish_runtime(host.config, host.runtime, &request_query)?;
        self.request_discovery(&current_view, &current_input)?;
        Ok(None)
    }

    fn refresh_command_view(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
        let input = self.frame.input.clone();
        let default_rule = host.config.default_rule.clone();
        let owner_name = self.frame.command_owner.clone().unwrap_or_default();
        let current_view = self.frame.view.clone();
        self.publish_runtime(host.config, host.runtime, &input)?;
        let engine = LauncherEngine::new(host.config, &current_view, host.runtime);
        let projected = engine
            .evaluate_field("commands")?
            .context("launcher viewtype has no commands field")?;
        let commands = projected
            .as_array()
            .context("runtime command projection must return an array")?;
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
        self.frame.active_rule = default_rule;
        self.frame.query = input.clone();
        self.frame.requested_input = input.clone();
        self.frame.results_input = input;
        self.frame.discovery_pending = false;
        self.frame.refresh_deadline = None;
        Ok(())
    }

    fn request_discovery(&mut self, view: &str, input: &str) -> Result<()> {
        if self.frame.discovery_pending
            && self.requested_view == view
            && self.frame.requested_input == input
        {
            return Ok(());
        }
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.latest_request_id = self.next_request_id;
        self.requested_view = view.to_string();
        self.frame.requested_input = input.to_string();
        self.frame.discovery_pending = true;
        self.frame.refresh_deadline = None;
        self.discovery_tx
            .send(DiscoveryRequest {
                id: self.latest_request_id,
                view: view.to_string(),
                input: input.to_string(),
                log_file: self.log_file.clone(),
            })
            .context("could not queue discovery request")
    }

    fn collect_discoveries(&mut self) -> Vec<DiscoveryEvent> {
        let mut events = Vec::new();
        while let Ok(response) = self.discovery_rx.try_recv() {
            if !self.response_matches(&response) {
                let (errors, failure) = discovery_result_error(&response.result);
                events.push(DiscoveryEvent {
                    current: false,
                    view: response.view,
                    errors,
                    failure,
                    pending_command: None,
                });
                continue;
            }

            let view = response.view;
            let pending_command;
            let (errors, failure) = match response.result {
                Ok(result) => {
                    let errors = result.errors;
                    self.frame.discovery_pending = false;
                    self.frame.active_rule = response.active_rule;
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
                    self.frame.discovery_pending = false;
                    self.frame.active_rule = response.active_rule;
                    self.frame.query = response.query;
                    self.frame.items.clear();
                    self.frame.results_input = response.input;
                    self.frame.selected = 0;
                    pending_command = self.frame.pending_command.take();
                    (Vec::new(), Some(error))
                }
            };
            events.push(DiscoveryEvent {
                current: true,
                view,
                errors,
                failure,
                pending_command,
            });
        }
        events
    }

    fn response_matches(&self, response: &DiscoveryResponse) -> bool {
        response.id == self.latest_request_id
            && response.view == self.frame.view
            && response.input == self.frame.input
    }

    fn open_command_driver(&self, host: &EngineHost<'_>) -> Result<SessionEffect> {
        let command_view_ref = host.config.command_view.clone();
        host.config.command_view()?;
        let parent_item = self.frame.items.get(self.frame.selected).cloned();
        let command_owner = parent_item.as_ref().map(|item| item.source_view.clone());
        let driver = Self::new(
            &command_view_ref,
            &host.config.default_rule,
            "",
            host.config.clone(),
            host.runtime.handle(),
            host.log_file().map(PathBuf::from),
            command_owner,
            parent_item,
        );
        Ok(SessionEffect::Push(Box::new(driver)))
    }

    fn handle_command_key(&mut self, host: &mut EngineHost<'_>, key: Key) -> Result<SessionEffect> {
        if self.command_view_active() && !matches!(key, Key::Enter) {
            return Ok(SessionEffect::Continue);
        }
        if !self.results_current() {
            if self.command_view_active() {
                self.request_current(host)?;
            } else {
                self.queue_pending_command(key);
                self.request_current(host)?;
                return Ok(SessionEffect::Continue);
            }
        }

        let Some(action) = self.prepare_command_action(host.config, key, host.log_file())? else {
            let view = self.current_view_ref().to_string();
            let message = format!("no command for {}", key_display(key));
            host.record_error_message(Some(&view), None, &message);
            return Ok(SessionEffect::Continue);
        };
        let replace_current = self.command_view_active();
        match action {
            super::command::CommandAction::Report {
                invocation,
                message,
            } => {
                host.record_error(&invocation, &message);
                Ok(SessionEffect::Continue)
            }
            super::command::CommandAction::OpenView { target } => Ok(SessionEffect::OpenView {
                view_ref: target,
                input: String::new(),
                replace_current,
            }),
            super::command::CommandAction::Execute(execution) => Ok(SessionEffect::RunCommand {
                execution: Box::new(execution),
                replace_current,
            }),
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

    fn handle_events(&mut self, host: &mut EngineHost<'_>) -> Result<Option<SessionEffect>> {
        for event in self.collect_discoveries() {
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
                return self.handle_command_key(host, key).map(Some);
            }
        }
        Ok(None)
    }
}

impl EngineDriver for LauncherDriver {
    fn step(
        &mut self,
        host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<SessionEffect> {
        if !self.started {
            self.started = true;
            if let Some(effect) = self.request_current(host)? {
                return Ok(effect);
            }
        }

        if let Some(effect) = self.handle_events(host)? {
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
            match self.handle_input(key, &host.config.command_view) {
                EngineKeyAction::Continue => {}
                EngineKeyAction::Refresh => refresh = true,
                EngineKeyAction::ClearError => host.clear_error(),
                EngineKeyAction::Command(key) => {
                    return self.handle_command_key(host, key);
                }
                EngineKeyAction::OpenCommandView => return self.open_command_driver(host),
                EngineKeyAction::PopView => return Ok(SessionEffect::Back),
                EngineKeyAction::Exit => return Ok(SessionEffect::Exit),
            }
        }
        if refresh {
            host.clear_error();
            self.schedule_refresh();
        }
        Ok(SessionEffect::Continue)
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
        Key::CtrlC => "Ctrl-C".to_string(),
        Key::CtrlD => "Ctrl-D".to_string(),
        Key::CtrlK => "Ctrl-K".to_string(),
        Key::CtrlU => "Ctrl-U".to_string(),
        Key::CtrlW => "Ctrl-W".to_string(),
        Key::Char(character) => character.to_string(),
    }
}
