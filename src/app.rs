use crate::config::{Command, Config, ViewType, normalize_key};
use crate::discovery::{DiscoveryResult, Item, discover, matches_query, sanitize_text};
use crate::pty::{self, EmbeddedOutcome};
use crate::runtime_log::{LogLevel, LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);
const INPUT_POLL_MS: i32 = 80;

struct DiscoveryRequest {
    id: u64,
    view: String,
    input: String,
    log_file: Option<PathBuf>,
}

struct DiscoveryResponse {
    id: u64,
    view: String,
    input: String,
    active_rule: String,
    query: String,
    result: std::result::Result<DiscoveryResult, String>,
}

struct LauncherFrame {
    view: String,
    input: String,
    items: Vec<Item>,
    selected: usize,
    active_rule: String,
    query: String,
    refresh_deadline: Option<Instant>,
    requested_input: String,
    results_input: String,
    discovery_pending: bool,
    pending_command: Option<Key>,
    command_owner: Option<String>,
}

impl LauncherFrame {
    fn new(view: &str, default_rule: &str) -> Self {
        Self {
            view: view.to_string(),
            input: String::new(),
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

#[derive(Clone)]
struct CommandInvocation {
    id: String,
    source_view: String,
    command: Command,
}

struct PreparedCommand {
    argv: Vec<String>,
    environment: Vec<(String, String)>,
    current_dir: Option<std::path::PathBuf>,
}

pub struct App<'a> {
    config: &'a Config,
    frames: Vec<LauncherFrame>,
    decoder: InputDecoder,
    discovery_tx: Sender<DiscoveryRequest>,
    discovery_rx: Receiver<DiscoveryResponse>,
    next_request_id: u64,
    latest_request_id: u64,
    requested_view: String,
    runtime_log: RuntimeLog,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
}

impl<'a> App<'a> {
    #[cfg(test)]
    pub fn new(config: &'a Config) -> Self {
        Self::with_runtime_log(config, RuntimeLog::disabled())
    }

    pub fn with_runtime_log(config: &'a Config, runtime_log: RuntimeLog) -> Self {
        let (discovery_tx, request_rx) = mpsc::channel();
        let (response_tx, discovery_rx) = mpsc::channel();
        let worker_config = config.clone();
        thread::spawn(move || discovery_worker(worker_config, request_rx, response_tx));

        Self {
            config,
            frames: vec![LauncherFrame::new(
                &config.default_view,
                &config.default_rule,
            )],
            decoder: InputDecoder::default(),
            discovery_tx,
            discovery_rx,
            next_request_id: 0,
            latest_request_id: 0,
            requested_view: String::new(),
            runtime_log,
            active_error: None,
            active_error_deadline: None,
        }
    }

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.request_discovery()?;

        loop {
            self.clear_expired_error();
            if self.collect_discoveries(terminal)? {
                return Ok(());
            }
            self.refresh_if_due()?;
            self.render(terminal)?;
            let timeout = self.input_timeout_ms();
            let mut keys = self.decoder.feed(&terminal.read_input(timeout)?);
            keys.extend(self.decoder.flush_due());

            let mut refresh = false;
            for key in keys {
                match self.handle_key(key, terminal)? {
                    CommandResult::Continue => {}
                    CommandResult::Refresh => refresh = true,
                    CommandResult::Exit => return Ok(()),
                }
            }
            if refresh {
                self.schedule_refresh();
            }
            self.refresh_if_due()?;
        }
    }

    fn current(&self) -> &LauncherFrame {
        self.frames.last().expect("launcher always has a root view")
    }

    fn current_mut(&mut self) -> &mut LauncherFrame {
        self.frames
            .last_mut()
            .expect("launcher always has a root view")
    }

    fn record_info(&mut self, source_view: Option<&str>, command: Option<&str>, message: &str) {
        self.runtime_log
            .record(LogLevel::Info, source_view, command, message);
    }

    fn record_error(&mut self, source_view: Option<&str>, command: Option<&str>, message: &str) {
        let record = self
            .runtime_log
            .record(LogLevel::Error, source_view, command, message);
        self.active_error = Some(record);
        self.active_error_deadline = Some(Instant::now() + ERROR_DISPLAY_DURATION);
    }

    fn clear_expired_error(&mut self) {
        if self
            .active_error_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_active_error();
        }
    }

    fn clear_active_error(&mut self) {
        self.active_error = None;
        self.active_error_deadline = None;
    }

    fn record_command_status(
        &mut self,
        invocation: &CommandInvocation,
        status: &str,
        success: bool,
    ) {
        if success {
            self.clear_active_error();
            self.record_info(Some(&invocation.source_view), Some(&invocation.id), status);
        } else {
            self.record_error(Some(&invocation.source_view), Some(&invocation.id), status);
        }
    }

    fn handle_key(&mut self, key: Key, terminal: &mut Terminal) -> Result<CommandResult> {
        match key {
            Key::CtrlC | Key::CtrlD => Ok(CommandResult::Exit),
            Key::CtrlK => {
                if self.current().view != self.config.command_view {
                    self.open_command_view()?;
                }
                Ok(CommandResult::Continue)
            }
            Key::Escape => {
                if !self.current().input.is_empty() {
                    self.current_mut().input.clear();
                    Ok(CommandResult::Refresh)
                } else if self.frames.len() > 1 {
                    self.pop_view()?;
                    Ok(CommandResult::Continue)
                } else {
                    Ok(CommandResult::Exit)
                }
            }
            Key::Enter | Key::Alt(_) => self.handle_command_key(key, terminal),
            Key::Up => {
                self.clear_active_error();
                if !self.current().items.is_empty() {
                    self.current_mut().selected = self.current().selected.saturating_sub(1);
                }
                Ok(CommandResult::Continue)
            }
            Key::Down => {
                self.clear_active_error();
                if !self.current().items.is_empty() {
                    let last = self.current().items.len() - 1;
                    self.current_mut().selected = (self.current().selected + 1).min(last);
                }
                Ok(CommandResult::Continue)
            }
            Key::Backspace => {
                if self.current_mut().input.pop().is_some() {
                    Ok(CommandResult::Refresh)
                } else {
                    Ok(CommandResult::Continue)
                }
            }
            Key::CtrlU => {
                if self.current().input.is_empty() {
                    Ok(CommandResult::Continue)
                } else {
                    self.current_mut().input.clear();
                    Ok(CommandResult::Refresh)
                }
            }
            Key::CtrlW => {
                let input = &mut self.current_mut().input;
                let previous_length = input.len();
                while input.chars().last().is_some_and(char::is_whitespace) {
                    input.pop();
                }
                while !input.chars().last().is_some_and(char::is_whitespace) && !input.is_empty() {
                    input.pop();
                }
                if input.len() != previous_length {
                    Ok(CommandResult::Refresh)
                } else {
                    Ok(CommandResult::Continue)
                }
            }
            Key::Char(character) if !character.is_control() => {
                self.current_mut().input.push(character);
                Ok(CommandResult::Refresh)
            }
            Key::Char(_) => Ok(CommandResult::Continue),
        }
    }

    fn handle_command_key(&mut self, key: Key, terminal: &mut Terminal) -> Result<CommandResult> {
        if self.current().command_owner.is_some() && !matches!(key, Key::Enter) {
            return Ok(CommandResult::Continue);
        }
        if !self.results_current() {
            if self.current().command_owner.is_some() {
                self.refresh_now()?;
            } else {
                self.current_mut().pending_command = Some(key);
                self.refresh_now()?;
                return Ok(CommandResult::Continue);
            }
        }

        if self.resolve_command(key).is_none() {
            let view = self.current().view.clone();
            let message = format!("no command for {}", key_display(key));
            self.record_error(Some(&view), None, &message);
            return Ok(CommandResult::Continue);
        }
        if self.execute_command(key, terminal)? {
            return Ok(CommandResult::Exit);
        }
        Ok(CommandResult::Continue)
    }

    fn schedule_refresh(&mut self) {
        self.clear_active_error();
        let frame = self.current_mut();
        frame.refresh_deadline = Some(Instant::now() + SEARCH_DEBOUNCE);
        frame.pending_command = None;
    }

    fn refresh_command_view(&mut self) {
        let owner = self.current().command_owner.clone();
        let input = self.current().input.clone();
        let default_rule = self.config.default_rule.clone();
        let owner_name = owner.clone().unwrap_or_default();
        let mut items = owner
            .as_deref()
            .and_then(|owner| self.config.view(owner))
            .map(|view| {
                view.commands
                    .values()
                    .filter_map(|command| {
                        let key = normalize_key(&command.key).ok()?;
                        let text = sanitize_text(&command.label);
                        (!text.is_empty()).then_some(Item {
                            prefix: "cmd".to_string(),
                            text,
                            value: Some(key),
                            metadata: Value::Null,
                            source_view: owner_name.clone(),
                        })
                    })
                    .filter(|item| matches_query(&item.text, &input))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        items.sort_by(|left, right| {
            compare_bindings(
                left.value.as_deref().unwrap_or_default(),
                right.value.as_deref().unwrap_or_default(),
            )
        });
        let frame = self.current_mut();
        frame.items = items;
        frame.selected = frame.selected.min(frame.items.len().saturating_sub(1));
        frame.active_rule = default_rule;
        frame.query = input.clone();
        frame.requested_input = input.clone();
        frame.results_input = input;
        frame.discovery_pending = false;
        frame.refresh_deadline = None;
    }

    fn request_discovery(&mut self) -> Result<()> {
        if self.current().command_owner.is_some() {
            self.refresh_command_view();
            return Ok(());
        }

        let current_view = self.current().view.clone();
        let current_input = self.current().input.clone();
        if let Some((target_view, query)) = self
            .config
            .resolve_view_route(&current_view, &current_input)
        {
            let parent = self.current_mut();
            parent.input.clear();
            parent.refresh_deadline = None;
            parent.discovery_pending = false;
            parent.pending_command = None;
            self.open_view_with_input(&target_view, &query)?;
            return Ok(());
        }

        let view = current_view;
        let input = current_input;
        self.current_mut().refresh_deadline = None;
        if self.current().discovery_pending
            && self.requested_view == view
            && self.current().requested_input == input
        {
            return Ok(());
        }

        self.next_request_id += 1;
        self.latest_request_id = self.next_request_id;
        self.requested_view = view.clone();
        self.current_mut().requested_input = input.clone();
        self.current_mut().discovery_pending = true;
        self.discovery_tx
            .send(DiscoveryRequest {
                id: self.latest_request_id,
                view,
                input,
                log_file: self.runtime_log.path().map(PathBuf::from),
            })
            .context("could not queue discovery request")
    }

    fn refresh_now(&mut self) -> Result<()> {
        self.request_discovery()
    }

    fn refresh_if_due(&mut self) -> Result<()> {
        if self
            .current()
            .refresh_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.refresh_now()?;
        }
        Ok(())
    }

    fn response_matches(&self, response: &DiscoveryResponse) -> bool {
        response.id == self.latest_request_id
            && response.view == self.current().view
            && response.input == self.current().input
    }

    fn collect_discoveries(&mut self, terminal: &mut Terminal) -> Result<bool> {
        while let Ok(response) = self.discovery_rx.try_recv() {
            if !self.response_matches(&response) {
                let response_view = response.view.clone();
                match &response.result {
                    Ok(result) => {
                        for error in &result.errors {
                            self.runtime_log.record(
                                LogLevel::Error,
                                Some(&response_view),
                                None,
                                error,
                            );
                        }
                    }
                    Err(error) => {
                        self.runtime_log
                            .record(LogLevel::Error, Some(&response_view), None, error);
                    }
                }
                continue;
            }

            let response_view = response.view.clone();
            let active_rule = response.active_rule.clone();
            let query = response.query.clone();
            let input = response.input.clone();
            let pending_command;
            match response.result {
                Ok(result) => {
                    let errors = result.errors;
                    {
                        let frame = self.current_mut();
                        frame.discovery_pending = false;
                        frame.active_rule = active_rule;
                        frame.query = query;
                        frame.items = result.items;
                        frame.results_input = input;
                        frame.selected = frame.selected.min(frame.items.len().saturating_sub(1));
                        pending_command = frame.pending_command.take();
                    }
                    if errors.is_empty() {
                        self.clear_active_error();
                    } else {
                        for error in errors {
                            self.record_error(Some(&response_view), None, &error);
                        }
                    }
                }
                Err(error) => {
                    {
                        let frame = self.current_mut();
                        frame.discovery_pending = false;
                        frame.active_rule = active_rule;
                        frame.query = query;
                        frame.items.clear();
                        frame.results_input = input;
                        frame.selected = 0;
                        pending_command = frame.pending_command.take();
                    }
                    self.record_error(Some(&response_view), None, &error);
                }
            }

            if let Some(key) = pending_command
                && self.execute_command(key, terminal)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn results_current(&self) -> bool {
        let frame = self.current();
        !frame.discovery_pending
            && frame.refresh_deadline.is_none()
            && frame.results_input == frame.input
    }

    fn input_timeout_ms(&self) -> i32 {
        let now = Instant::now();
        let refresh_remaining = self
            .current()
            .refresh_deadline
            .map(|deadline| deadline.saturating_duration_since(now));
        let error_remaining = self
            .active_error_deadline
            .map(|deadline| deadline.saturating_duration_since(now));
        let remaining = [refresh_remaining, error_remaining]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or_else(|| Duration::from_millis(INPUT_POLL_MS as u64));
        remaining.as_millis().min(INPUT_POLL_MS as u128).max(1) as i32
    }

    fn command_owner(&self) -> Option<&str> {
        let frame = self.current();
        frame.command_owner.as_deref().or_else(|| {
            frame
                .items
                .get(frame.selected)
                .map(|item| item.source_view.as_str())
        })
    }

    fn resolve_command(&self, key: Key) -> Option<CommandInvocation> {
        let frame = self.current();
        if frame.command_owner.is_some() {
            let binding = frame
                .items
                .get(frame.selected)
                .and_then(|item| item.value.as_deref())?;
            return self.find_command(frame.command_owner.as_deref()?, binding);
        }
        let key = command_key(key)?;
        self.find_command(self.command_owner()?, &key)
    }

    fn find_command(&self, view_ref: &str, key: &str) -> Option<CommandInvocation> {
        let view = self.config.view(view_ref)?;
        view.commands.iter().find_map(|(id, command)| {
            (normalize_key(&command.key).ok().as_deref() == Some(key)).then(|| CommandInvocation {
                id: id.clone(),
                source_view: view_ref.to_string(),
                command: command.clone(),
            })
        })
    }

    fn visible_commands(&self) -> Vec<(String, String)> {
        let Some(owner) = self.command_owner() else {
            return Vec::new();
        };
        let mut commands = BTreeMap::new();
        self.add_view_commands(&mut commands, owner);
        let mut commands = commands.into_iter().collect::<Vec<_>>();
        commands.sort_by(|left, right| compare_bindings(&left.0, &right.0));
        commands
    }

    fn add_view_commands(&self, commands: &mut BTreeMap<String, String>, view_ref: &str) {
        let Some(view) = self.config.view(view_ref) else {
            return;
        };
        for command in view.commands.values() {
            if let Ok(key) = normalize_key(&command.key) {
                commands.entry(key).or_insert_with(|| command.label.clone());
            }
        }
    }

    fn execute_command(&mut self, key: Key, terminal: &mut Terminal) -> Result<bool> {
        let Some(invocation) = self.resolve_command(key) else {
            return Ok(false);
        };
        let command_view = self.current().command_owner.is_some();
        let item = if command_view {
            let parent = self
                .frames
                .get(self.frames.len().saturating_sub(2))
                .context("command view has no parent frame")?;
            parent.items.get(parent.selected).cloned()
        } else {
            self.current().items.get(self.current().selected).cloned()
        };
        let command = invocation.command.clone();
        if command_view {
            self.frames.pop();
        }
        let current_view = self.current().view.clone();

        if command.run.is_none() {
            if let Some(target) = &command.view {
                self.open_view(target)?;
            } else {
                let message = format!("{} has no command", invocation.id);
                self.record_error(
                    Some(&invocation.source_view),
                    Some(&invocation.id),
                    &message,
                );
            }
            return Ok(false);
        }

        let prepared = self.prepare_command(&invocation, item.as_ref())?;
        if command.exit {
            self.execute_exit(prepared, terminal, &invocation)?;
            return Ok(true);
        }

        let target_type = command
            .view
            .as_deref()
            .and_then(|target| self.config.view(target))
            .map(|view| view.view_type);
        match target_type {
            Some(ViewType::Capture) => {
                self.execute_capture(prepared, terminal, &invocation, item.as_ref())?;
            }
            Some(ViewType::Embedded) => {
                self.execute_embedded(prepared, terminal, &invocation, item.as_ref())?
            }
            Some(ViewType::Launcher) | None => {
                self.execute_oneshot(prepared, terminal, &invocation)?;
                if let Some(target) = command.view.as_deref()
                    && target != current_view
                {
                    self.open_view(target)?;
                }
            }
        }
        Ok(false)
    }

    fn prepare_command(
        &self,
        invocation: &CommandInvocation,
        item: Option<&Item>,
    ) -> Result<PreparedCommand> {
        let view = self
            .config
            .view(&invocation.source_view)
            .with_context(|| format!("view {:?} disappeared", invocation.source_view))?;
        let script = invocation
            .command
            .run
            .as_ref()
            .context("command has no run script")?;
        let shell = invocation
            .command
            .shell
            .as_deref()
            .or(view.run_shell.as_deref())
            .unwrap_or("sh");
        let value = item
            .and_then(|item| item.value.as_deref())
            .or_else(|| item.map(|item| item.text.as_str()))
            .unwrap_or("");
        let metadata = item
            .map(|item| serde_json::to_string(&item.metadata))
            .transpose()
            .context("could not serialize selected item metadata")?
            .unwrap_or_else(|| Value::Null.to_string());
        let item_text = item.map(|item| item.text.clone()).unwrap_or_default();
        let source_view = item
            .map(|item| item.source_view.clone())
            .unwrap_or_else(|| invocation.source_view.clone());
        let frame = self.current();
        let plugin_root = self
            .config
            .plugin_root(&source_view)
            .map(|path| path.to_path_buf());
        let mut environment = vec![
            ("LAUNCHER_ITEM".to_string(), item_text),
            ("LAUNCHER_VALUE".to_string(), value.to_string()),
            ("LAUNCHER_METADATA".to_string(), metadata),
            (
                "LAUNCHER_PLUGIN".to_string(),
                plugin_name(&source_view).to_string(),
            ),
            ("LAUNCHER_VIEW".to_string(), frame.view.clone()),
            ("LAUNCHER_VIEW_REF".to_string(), source_view),
            ("LAUNCHER_COMMAND".to_string(), invocation.id.clone()),
            ("LAUNCHER_RULE".to_string(), frame.active_rule.clone()),
            ("LAUNCHER_QUERY".to_string(), frame.query.clone()),
            // Keep the old name available to existing scripts.
            (
                "LAUNCHER_PROVIDER".to_string(),
                plugin_name(&invocation.source_view).to_string(),
            ),
        ];
        if let Some(root) = &plugin_root {
            environment.push((
                "LAUNCHER_PLUGIN_DIR".to_string(),
                root.to_string_lossy().to_string(),
            ));
        }
        if let Some(path) = self.runtime_log.path() {
            environment.push((
                "LAUNCHER_LOG_FILE".to_string(),
                path.to_string_lossy().to_string(),
            ));
        }
        Ok(PreparedCommand {
            argv: vec![
                shell.to_string(),
                "-c".to_string(),
                script.clone(),
                "tui-launcher".to_string(),
            ],
            environment,
            current_dir: plugin_root,
        })
    }

    fn process(prepared: &PreparedCommand) -> ProcessCommand {
        let mut process = ProcessCommand::new(&prepared.argv[0]);
        process.args(&prepared.argv[1..]);
        if let Some(current_dir) = &prepared.current_dir {
            process.current_dir(current_dir);
        }
        for (key, value) in &prepared.environment {
            process.env(key, value);
        }
        process
    }

    fn execute_oneshot(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
    ) -> Result<()> {
        terminal.leave()?;
        let result = Self::process(&prepared).status();
        terminal.reenter()?;
        match result {
            Ok(status) => {
                let message = status_message(&status);
                self.record_command_status(invocation, &message, status.success());
            }
            Err(error) => self.record_error(
                Some(&invocation.source_view),
                Some(&invocation.id),
                &error.to_string(),
            ),
        }
        Ok(())
    }

    fn execute_capture(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
        item: Option<&Item>,
    ) -> Result<()> {
        terminal.leave()?;
        let result = Self::process(&prepared)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("could not run command {}", invocation.id));
        terminal.reenter()?;
        let (text, status) = match result {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                let status = status_message(&output.status);
                self.record_command_status(invocation, &status, output.status.success());
                (sanitize_text(&text), status)
            }
            Err(error) => {
                let message = error.to_string();
                self.record_error(
                    Some(&invocation.source_view),
                    Some(&invocation.id),
                    &message,
                );
                (message, "failed".to_string())
            }
        };
        let title = format!(
            "{} / {}",
            item.map(|item| item.text.as_str())
                .unwrap_or(&invocation.id),
            invocation.id
        );
        self.show_capture(terminal, &title, &text, &status)
    }

    fn execute_embedded(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
        item: Option<&Item>,
    ) -> Result<()> {
        let outcome = pty::run(
            &prepared.argv,
            &prepared.environment,
            prepared.current_dir.as_deref(),
            terminal,
            &format!(
                "{} / {}",
                item.map(|item| item.text.as_str())
                    .unwrap_or(&invocation.id),
                invocation.id
            ),
        )?;
        let message = embedded_status_message(outcome);
        let success = matches!(outcome, EmbeddedOutcome::ReturnedToLauncher)
            || matches!(outcome, EmbeddedOutcome::Exited(0));
        self.record_command_status(invocation, &message, success);
        Ok(())
    }

    fn execute_exit(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
    ) -> Result<()> {
        terminal.leave()?;
        let status = Self::process(&prepared).status();
        match status {
            Ok(status) => {
                let message = status_message(&status);
                self.record_command_status(invocation, &message, status.success());
            }
            Err(error) => self.record_error(
                Some(&invocation.source_view),
                Some(&invocation.id),
                &error.to_string(),
            ),
        }
        Ok(())
    }

    fn show_capture(
        &mut self,
        terminal: &mut Terminal,
        title: &str,
        output: &str,
        status: &str,
    ) -> Result<()> {
        let lines: Vec<String> = if output.is_empty() {
            vec!["(no output)".to_string()]
        } else {
            output.lines().map(sanitize_text).collect()
        };

        loop {
            render_capture(terminal, title, &lines, status)?;
            let bytes = terminal.read_input(80)?;
            let mut keys = self.decoder.feed(&bytes);
            keys.extend(self.decoder.flush_due());
            if keys.iter().any(|key| {
                matches!(
                    key,
                    Key::CtrlC | Key::Escape | Key::Enter | Key::Char(_) | Key::Alt(_)
                )
            }) {
                return Ok(());
            }
        }
    }

    fn open_command_view(&mut self) -> Result<()> {
        self.clear_active_error();
        let command_view_ref = self.config.command_view.clone();
        self.config.command_view()?;
        let command_owner = self
            .current()
            .items
            .get(self.current().selected)
            .map(|item| item.source_view.clone());
        let mut frame = LauncherFrame::new(&command_view_ref, &self.config.default_rule);
        frame.command_owner = command_owner;
        self.frames.push(frame);
        self.request_discovery()
    }

    fn open_view(&mut self, view_ref: &str) -> Result<()> {
        self.open_view_with_input(view_ref, "")
    }

    fn open_view_with_input(&mut self, view_ref: &str, input: &str) -> Result<()> {
        let view = self
            .config
            .view(view_ref)
            .with_context(|| format!("view {:?} is not configured", view_ref))?;
        if view.view_type != ViewType::Launcher {
            bail!("view {:?} cannot be opened as a launcher view", view_ref);
        }
        self.clear_active_error();
        let mut frame = LauncherFrame::new(view_ref, &self.config.default_rule);
        frame.input = input.to_string();
        self.frames.push(frame);
        self.request_discovery()
    }

    fn pop_view(&mut self) -> Result<()> {
        if self.frames.len() <= 1 {
            return Ok(());
        }
        self.clear_active_error();
        self.frames.pop();
        self.current_mut().discovery_pending = false;
        self.current_mut().pending_command = None;
        self.request_discovery()
    }

    fn render(&self, terminal: &Terminal) -> Result<()> {
        let (width, height) = terminal.size();
        let width = width as usize;
        let height = height as usize;
        let footer = self.footer_line(width);
        let list_height = height.saturating_sub(3);
        let frame = self.current();
        let mut lines = Vec::with_capacity(height);
        let prefix_width = prefix_column_width(&frame.items, width);
        let content_width = width.saturating_sub(prefix_width + 4);

        lines.push(format!(" TUI Launcher  [{}]", frame.view));
        lines.push(format!(" > {}", frame.input));

        let start = if frame.selected >= list_height && list_height > 0 {
            frame.selected + 1 - list_height
        } else {
            0
        };
        let selected_row = if frame.items.is_empty() || list_height == 0 {
            None
        } else {
            Some(2 + frame.selected.saturating_sub(start))
        };

        let searching = frame.refresh_deadline.is_some() || frame.discovery_pending;
        if list_height > 0 {
            if frame.items.is_empty() {
                lines.push(if searching {
                    "   (searching...)".to_string()
                } else {
                    "   (no matches)".to_string()
                });
            } else {
                for (offset, item) in frame.items.iter().skip(start).take(list_height).enumerate() {
                    let index = start + offset;
                    let marker = if index == frame.selected { "> " } else { "  " };
                    lines.push(format_item_line(
                        marker,
                        &item.prefix,
                        &item.text,
                        prefix_width,
                        content_width,
                    ));
                }
            }
        }

        while lines.len() < 2 + list_height {
            lines.push(String::new());
        }
        lines.push(footer);

        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[H")?;
        for row in 0..height {
            let line = lines.get(row).map(String::as_str).unwrap_or("");
            let clipped = clip(line, width);
            if selected_row == Some(row) && clipped.starts_with("> ") {
                write!(stdout, "\x1b[7m{}\x1b[0m", clipped)?;
            } else if row == 0 {
                write!(stdout, "\x1b[1;36m{}\x1b[0m", clipped)?;
            } else {
                stdout.write_all(clipped.as_bytes())?;
            }
            stdout.write_all(b"\x1b[K")?;
            if row + 1 < height {
                stdout.write_all(b"\r\n")?;
            }
        }
        stdout.flush().context("could not draw launcher")
    }

    fn footer_line(&self, width: usize) -> String {
        // Leave the terminal's last column unused so a full row cannot trigger autowrap.
        let width = width.saturating_sub(1);
        let frame = self.current();
        let left = self
            .active_error
            .as_ref()
            .map(|error| error.label.clone())
            .unwrap_or_else(|| frame.view.clone());
        let left_width = UnicodeWidthStr::width(left.as_str());
        let right_budget = if left_width + 2 < width {
            width - left_width - 2
        } else {
            (width * 3 / 5).max(1).min(width.saturating_sub(1))
        };
        let right = command_footer_text(&self.visible_commands(), right_budget);
        footer_row(&left, &right, width)
    }
}

fn discovery_worker(
    config: Config,
    requests: Receiver<DiscoveryRequest>,
    responses: Sender<DiscoveryResponse>,
) {
    while let Ok(mut request) = requests.recv() {
        while let Ok(next_request) = requests.try_recv() {
            request = next_request;
        }

        let (source_prefix, query) = config.resolve_view_prefix(&request.view, &request.input);
        let (rule_name, rule, rule_query) = config.resolve_rule(&query);
        let result = discover(
            &config,
            &request.view,
            rule_name,
            rule,
            &rule_query,
            source_prefix.as_deref(),
            request.log_file.as_deref(),
        )
        .map_err(|error| error.to_string());
        let response = DiscoveryResponse {
            id: request.id,
            view: request.view,
            input: request.input,
            active_rule: rule_name.to_string(),
            query: rule_query,
            result,
        };
        if responses.send(response).is_err() {
            break;
        }
    }
}

fn plugin_name(view_ref: &str) -> &str {
    view_ref
        .split_once(':')
        .map(|(plugin, _)| plugin)
        .unwrap_or(view_ref)
}

fn command_key(key: Key) -> Option<String> {
    match key {
        Key::Enter => Some("enter".to_string()),
        Key::Alt(character) if character.is_ascii_graphic() => {
            Some(format!("alt+{}", character.to_ascii_lowercase()))
        }
        _ => None,
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

fn display_binding(key: &str) -> String {
    if key == "enter" {
        return "Enter".to_string();
    }
    key.strip_prefix("alt+")
        .map(|character| format!("Alt-{}", character.to_ascii_uppercase()))
        .unwrap_or_else(|| key.to_string())
}

fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

fn command_footer_text(commands: &[(String, String)], width: usize) -> String {
    if commands.is_empty() || width == 0 {
        return String::new();
    }
    let formatted = commands
        .iter()
        .map(|(key, label)| format!("{} {}", display_binding(key), label))
        .collect::<Vec<_>>();
    let full = formatted.join(" | ");
    if UnicodeWidthStr::width(full.as_str()) <= width {
        return full;
    }

    let more = "Ctrl-K commands";
    let more_width = UnicodeWidthStr::width(more);
    let mut visible = Vec::new();
    let mut used = 0;
    for command in formatted {
        let command_width = UnicodeWidthStr::width(command.as_str());
        let separator = if visible.is_empty() { 0 } else { 3 };
        let required = used + separator + command_width + 3 + more_width;
        if required > width {
            break;
        }
        used += separator + command_width;
        visible.push(command);
    }
    visible.push(more.to_string());
    clip(visible.join(" | ").as_str(), width)
}

fn footer_row(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if right.is_empty() {
        return clip(left, width);
    }

    let gap = 2;
    if UnicodeWidthStr::width(left) + gap + UnicodeWidthStr::width(right) <= width {
        let padding = width - UnicodeWidthStr::width(left) - gap - UnicodeWidthStr::width(right);
        return format!("{}{}{}", left, " ".repeat(padding + gap), right);
    }

    let right_budget = (width * 3 / 5).max(1).min(width.saturating_sub(1));
    let right = clip(right, right_budget);
    let left_budget = width.saturating_sub(UnicodeWidthStr::width(right.as_str()) + 1);
    let left = clip(left, left_budget);
    let padding = width.saturating_sub(
        UnicodeWidthStr::width(left.as_str()) + UnicodeWidthStr::width(right.as_str()),
    );
    format!("{}{}{}", left, " ".repeat(padding), right)
}

fn prefix_column_width(items: &[Item], width: usize) -> usize {
    let longest = items
        .iter()
        .map(|item| UnicodeWidthStr::width(item.prefix.as_str()))
        .max()
        .unwrap_or(6)
        .clamp(6, 20);
    longest.min(width.saturating_sub(8).max(1))
}

fn format_item_line(
    marker: &str,
    prefix: &str,
    content: &str,
    prefix_width: usize,
    content_width: usize,
) -> String {
    let prefix = pad_right(&clip(prefix, prefix_width), prefix_width);
    format!("{}{}  {}", marker, prefix, clip(content, content_width))
}

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{}", text, " ".repeat(width.saturating_sub(used)))
}

fn render_capture(terminal: &Terminal, title: &str, lines: &[String], status: &str) -> Result<()> {
    let (width, height) = terminal.size();
    let width = width as usize;
    let height = height as usize;
    let inner_height = height.saturating_sub(4).max(1);
    let start = lines.len().saturating_sub(inner_height);

    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[?25l")?;
    write_capture_line(
        &mut stdout,
        1,
        &format!(" TUI Launcher  [capture: {}]", title),
        width,
        true,
    )?;
    write_capture_line(&mut stdout, 2, &format!(" > {}", title), width, false)?;
    for row in 0..inner_height {
        let content = lines.get(start + row).map(String::as_str).unwrap_or("");
        write_capture_line(&mut stdout, 3 + row, content, width, false)?;
    }
    write_capture_line(
        &mut stdout,
        height.saturating_sub(1).max(1),
        &format!(" status: {}", status),
        width,
        false,
    )?;
    write_capture_line(
        &mut stdout,
        height.max(1),
        " press any key to return | Esc",
        width,
        false,
    )?;
    stdout.flush().context("could not draw command output")
}

fn write_capture_line(
    stdout: &mut impl Write,
    row: usize,
    text: &str,
    width: usize,
    heading: bool,
) -> Result<()> {
    write!(stdout, "\x1b[{};1H\x1b[K", row)?;
    let text = clip(text, width);
    if heading {
        write!(stdout, "\x1b[1;36m{}\x1b[0m", text)?;
    } else {
        stdout.write_all(text.as_bytes())?;
    }
    Ok(())
}

fn embedded_status_message(outcome: EmbeddedOutcome) -> String {
    match outcome {
        EmbeddedOutcome::ReturnedToLauncher => "embedded command stopped".to_string(),
        EmbeddedOutcome::Exited(0) => "embedded command finished successfully".to_string(),
        EmbeddedOutcome::Exited(code) => format!("embedded command exited with code {}", code),
        EmbeddedOutcome::Signaled(signal) => {
            format!("embedded command terminated by signal {}", signal)
        }
    }
}

fn status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}

fn clip(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }

    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 3 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push_str("...");
    result
}

#[derive(Debug, Clone, Copy)]
enum CommandResult {
    Continue,
    Refresh,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Alt(char),
    Enter,
    Backspace,
    Up,
    Down,
    Escape,
    CtrlC,
    CtrlK,
    CtrlD,
    CtrlU,
    CtrlW,
}

#[derive(Default)]
pub(crate) struct InputDecoder {
    pending: Vec<u8>,
    escape_since: Option<Instant>,
}

impl InputDecoder {
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        self.parse()
    }

    pub(crate) fn flush_due(&mut self) -> Vec<Key> {
        if self
            .escape_since
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(35))
        {
            self.escape_since = None;
            if self.pending.first() == Some(&0x1b) {
                self.pending.remove(0);
                return vec![Key::Escape];
            }
        }
        Vec::new()
    }

    fn parse(&mut self) -> Vec<Key> {
        let mut keys = Vec::new();
        loop {
            let Some(&first) = self.pending.first() else {
                self.escape_since = None;
                break;
            };

            if first == 0x1b {
                if self.pending.len() == 1 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                if self.pending[1] != b'[' {
                    if self.pending[1].is_ascii_graphic() {
                        let character = self.pending[1] as char;
                        self.pending.drain(..2);
                        self.escape_since = None;
                        keys.push(Key::Alt(character));
                        continue;
                    }
                    self.pending.remove(0);
                    keys.push(Key::Escape);
                    self.escape_since = None;
                    continue;
                }
                if self.pending.len() < 3 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                let code = self.pending[2];
                let key = match code {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    _ => None,
                };
                if let Some(key) = key {
                    self.pending.drain(..3);
                    self.escape_since = None;
                    keys.push(key);
                    continue;
                }
                if self.pending.last() == Some(&b'~') {
                    self.pending.clear();
                    self.escape_since = None;
                    continue;
                }
                if self.pending.len() > 8 {
                    self.pending.remove(0);
                    self.escape_since = None;
                    keys.push(Key::Escape);
                    continue;
                }
                self.escape_since.get_or_insert_with(Instant::now);
                break;
            }

            if let Some(key) = control_key(first) {
                self.pending.remove(0);
                keys.push(key);
                continue;
            }

            if first < 0x80 {
                self.pending.remove(0);
                keys.push(Key::Char(first as char));
                continue;
            }

            let width = utf8_width(first);
            if self.pending.len() < width {
                break;
            }
            match std::str::from_utf8(&self.pending[..width]) {
                Ok(text) => {
                    if let Some(character) = text.chars().next() {
                        keys.push(Key::Char(character));
                    }
                    self.pending.drain(..width);
                }
                Err(_) => {
                    self.pending.remove(0);
                }
            }
        }
        keys
    }
}

fn control_key(byte: u8) -> Option<Key> {
    match byte {
        b'\r' | b'\n' => Some(Key::Enter),
        0x7f | 0x08 => Some(Key::Backspace),
        0x03 => Some(Key::CtrlC),
        0x04 => Some(Key::CtrlD),
        0x0b => Some(Key::CtrlK),
        0x15 => Some(Key::CtrlU),
        0x17 => Some(Key::CtrlW),
        _ => None,
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Command, DisplayType, Rule, View};
    use std::collections::BTreeMap;

    fn test_config() -> Config {
        let command = |key: &str, label: &str| Command {
            key: key.to_string(),
            label: label.to_string(),
            run: Some(":".to_string()),
            shell: None,
            view: None,
            exit: false,
        };
        let launcher = |commands| View {
            view_type: ViewType::Launcher,
            display: DisplayType::Text,
            sources: Vec::new(),
            display_prefix: None,
            discover: None,
            default_discover: None,
            query_discover: None,
            discover_shell: None,
            run_shell: None,
            filter: true,
            commands,
        };
        Config {
            default_view: "core:default".to_string(),
            dmenu_view: "core:dmenu".to_string(),
            command_view: "core:command".to_string(),
            default_rule: "default".to_string(),
            rules: BTreeMap::from([("default".to_string(), Rule { filter: true })]),
            views: BTreeMap::from([
                (
                    "core:default".to_string(),
                    launcher(BTreeMap::from([
                        ("run".to_string(), command("enter", "Run")),
                        ("apps".to_string(), command("alt+a", "Apps")),
                        ("shell".to_string(), command("alt+s", "Shell")),
                    ])),
                ),
                (
                    "apps:main".to_string(),
                    launcher(BTreeMap::from([(
                        "open".to_string(),
                        command("enter", "Open"),
                    )])),
                ),
                ("core:command".to_string(), launcher(BTreeMap::new())),
            ]),
            plugin_roots: BTreeMap::new(),
        }
    }

    #[test]
    fn launcher_view_stack_returns_to_parent() {
        let config = test_config();
        let mut app = App::new(&config);
        assert_eq!(app.current().view, "core:default");

        app.open_view("apps:main").unwrap();
        assert_eq!(app.frames.len(), 2);
        assert_eq!(app.current().view, "apps:main");

        app.pop_view().unwrap();
        assert_eq!(app.frames.len(), 1);
        assert_eq!(app.current().view, "core:default");
    }

    #[test]
    fn stale_discovery_responses_do_not_match_current_input() {
        let config = test_config();
        let mut app = App::new(&config);
        app.latest_request_id = 2;
        app.current_mut().input = "new".to_string();

        let response = |id: u64, input: &str| DiscoveryResponse {
            id,
            view: "core:default".to_string(),
            input: input.to_string(),
            active_rule: "default".to_string(),
            query: input.to_string(),
            result: Ok(DiscoveryResult::default()),
        };
        assert!(app.response_matches(&response(2, "new")));
        assert!(!app.response_matches(&response(1, "new")));
        assert!(!app.response_matches(&response(2, "old")));

        let mut wrong_view = response(2, "new");
        wrong_view.view = "apps:main".to_string();
        assert!(!app.response_matches(&wrong_view));
    }

    #[test]
    fn footer_displays_command_status() {
        let config = test_config();
        let mut app = App::new(&config);
        app.current_mut().items.push(Item {
            prefix: "core".to_string(),
            text: "item".to_string(),
            value: Some("value".to_string()),
            metadata: Value::Null,
            source_view: "core:default".to_string(),
        });
        app.record_error(Some("core:default"), None, "command finished");
        let footer = app.footer_line(120);
        assert!(!footer.contains('\n'));
        assert!(footer.contains("command finished"));
        assert!(footer.contains("Enter Run"));
        assert!(footer.contains("Alt-A Apps"));
        assert!(footer.contains("Alt-S Shell"));
        assert!(!footer.contains("Ctrl-K commands"));
        assert!(UnicodeWidthStr::width(app.footer_line(50).as_str()) < 50);

        app.clear_active_error();
        let narrow_footer = app.footer_line(40);
        assert!(narrow_footer.contains("Ctrl-K commands"));

        app.record_error(
            Some("very-long-source-view"),
            None,
            "a long discovery error",
        );
        let error_footer = app.footer_line(40);
        assert!(error_footer.contains("Ctrl-K commands"));

        app.active_error_deadline = Some(Instant::now() - Duration::from_secs(1));
        app.clear_expired_error();
        assert!(app.active_error.is_none());
        assert!(app.active_error_deadline.is_none());
    }

    #[test]
    fn command_view_uses_the_selected_item_view_as_owner() {
        let config = test_config();
        let mut app = App::new(&config);
        app.current_mut().items.push(Item {
            prefix: "app".to_string(),
            text: "Item".to_string(),
            value: Some("value".to_string()),
            metadata: Value::Null,
            source_view: "apps:main".to_string(),
        });
        app.open_command_view().unwrap();
        assert_eq!(app.current().view, "core:command");
        assert_eq!(app.current().command_owner.as_deref(), Some("apps:main"));
        assert_eq!(app.current().items[0].text, "Open");
        assert_eq!(app.current().items[0].value.as_deref(), Some("enter"));
    }

    #[test]
    fn decodes_control_and_navigation_keys() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\r\x1b[A\x1b[B\x7f\x03\x04\x0b\x15\x17"),
            vec![
                Key::Enter,
                Key::Up,
                Key::Down,
                Key::Backspace,
                Key::CtrlC,
                Key::CtrlD,
                Key::CtrlK,
                Key::CtrlU,
                Key::CtrlW,
            ]
        );
    }

    #[test]
    fn decodes_alt_and_utf8_input() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(b"\x1ba"), vec![Key::Alt('a')]);
        assert!(decoder.feed(&[0xe4]).is_empty());
        assert_eq!(decoder.feed(&[0xb8, 0xad]), vec![Key::Char('中')]);
    }

    #[test]
    fn keeps_incomplete_escape_sequences_pending() {
        let mut decoder = InputDecoder::default();
        assert!(decoder.feed(b"\x1b[").is_empty());
        assert_eq!(decoder.feed(b"A"), vec![Key::Up]);
    }
}
