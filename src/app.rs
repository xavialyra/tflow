use crate::command::{ERROR_DISPLAY_DURATION, EngineContext};
use crate::config::{Config, ENGINE_LAUNCHER};
use crate::discovery::{Item, matches_query, sanitize_text};
use crate::engine::Key;
use crate::engine::{
    CommandAction, CommandExecution, EngineKeyAction, EngineRegistry, EngineSession,
    LauncherEngine, LauncherFrame, LauncherSession, RuntimeStore,
};
use crate::input::InputDecoder;
#[cfg(test)]
use crate::render;
use crate::runtime_log::{LogLevel, LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const INPUT_POLL_MS: i32 = 80;

pub struct App<'a> {
    config: &'a Config,
    session: LauncherSession,
    runtime: RuntimeStore,
    engines: EngineRegistry,
    decoder: InputDecoder,
    runtime_log: RuntimeLog,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
}

impl<'a> App<'a> {
    #[cfg(test)]
    pub fn new(config: &'a Config) -> Self {
        Self::with_runtime_log(config, RuntimeLog::disabled())
    }

    #[cfg(test)]
    pub fn with_runtime_log(config: &'a Config, runtime_log: RuntimeLog) -> Self {
        Self::with_runtime_log_and_engines(config, runtime_log, EngineRegistry::new())
    }

    pub(crate) fn with_runtime_log_and_engines(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
    ) -> Self {
        let log_file = runtime_log.path().map(PathBuf::from);
        Self {
            config,
            session: LauncherSession::new(
                &config.default_view,
                &config.default_rule,
                config.clone(),
                log_file,
            ),
            runtime: RuntimeStore::new(),
            decoder: InputDecoder::default(),
            runtime_log,
            engines,
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
        self.session.current()
    }

    fn current_mut(&mut self) -> &mut LauncherFrame {
        self.session.current_mut()
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

    fn handle_key(&mut self, key: Key, terminal: &mut Terminal) -> Result<CommandResult> {
        match self.session.handle_key(key, &self.config.command_view) {
            EngineKeyAction::Continue => Ok(CommandResult::Continue),
            EngineKeyAction::Refresh => Ok(CommandResult::Refresh),
            EngineKeyAction::ClearError => {
                self.clear_active_error();
                Ok(CommandResult::Continue)
            }
            EngineKeyAction::Command(key) => self.handle_command_key(key, terminal),
            EngineKeyAction::OpenCommandView => {
                self.open_command_view()?;
                Ok(CommandResult::Continue)
            }
            EngineKeyAction::PopView => {
                self.pop_view()?;
                Ok(CommandResult::Continue)
            }
            EngineKeyAction::Exit => Ok(CommandResult::Exit),
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

        let Some(action) = LauncherEngine::prepare_command_action(
            self.config,
            &mut self.session,
            key,
            self.runtime_log.path(),
        )?
        else {
            let view = self.current().view.clone();
            let message = format!("no command for {}", key_display(key));
            self.record_error(Some(&view), None, &message);
            return Ok(CommandResult::Continue);
        };
        match action {
            CommandAction::Report {
                invocation,
                message,
            } => self.record_error(
                Some(&invocation.source_view),
                Some(&invocation.id),
                &message,
            ),
            CommandAction::OpenView { target } => self.open_view(&target)?,
            CommandAction::Execute(execution) => {
                if self.execute_command(execution, terminal)? {
                    return Ok(CommandResult::Exit);
                }
            }
        }
        Ok(CommandResult::Continue)
    }

    fn schedule_refresh(&mut self) {
        self.clear_active_error();
        self.session.schedule_refresh();
    }

    fn refresh_command_view(&mut self) -> Result<()> {
        let input = self.current().input.clone();
        let default_rule = self.config.default_rule.clone();
        let owner_name = self.current().command_owner.clone().unwrap_or_default();
        let current_view = self.current().view.clone();
        self.publish_runtime_snapshot()?;
        let engine = LauncherEngine::new(self.config, &current_view, &self.runtime);
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
        Ok(())
    }

    fn request_discovery(&mut self) -> Result<()> {
        if self.current().command_owner.is_some() {
            self.refresh_command_view()?;
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

        self.session
            .request_discovery(&current_view, &current_input)
    }

    fn refresh_now(&mut self) -> Result<()> {
        self.request_discovery()
    }

    fn refresh_if_due(&mut self) -> Result<()> {
        if self.session.refresh_due() {
            self.refresh_now()?;
        }
        Ok(())
    }

    fn collect_discoveries(&mut self, terminal: &mut Terminal) -> Result<bool> {
        for event in self.session.collect_events() {
            if !event.current {
                for error in event.errors {
                    self.runtime_log
                        .record(LogLevel::Error, Some(&event.view), None, &error);
                }
                if let Some(error) = event.failure {
                    self.runtime_log
                        .record(LogLevel::Error, Some(&event.view), None, &error);
                }
                continue;
            }

            if let Some(error) = event.failure {
                self.record_error(Some(&event.view), None, &error);
            } else if event.errors.is_empty() {
                self.clear_active_error();
            } else {
                for error in event.errors {
                    self.record_error(Some(&event.view), None, &error);
                }
            }

            if let Some(key) = event.pending_command
                && matches!(self.handle_command_key(key, terminal)?, CommandResult::Exit)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn results_current(&self) -> bool {
        self.session.results_current()
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

    fn publish_runtime_snapshot(&mut self) -> Result<()> {
        LauncherEngine::publish_runtime(self.config, &self.session, &mut self.runtime)
    }

    fn execute_command(
        &mut self,
        execution: CommandExecution,
        terminal: &mut Terminal,
    ) -> Result<bool> {
        let result = {
            let mut context = EngineContext::new(
                &mut self.session,
                &mut self.decoder,
                &mut self.runtime_log,
                &mut self.active_error,
                &mut self.active_error_deadline,
            );
            let engine_type = execution.engine_type.clone();
            self.engines
                .execute(&engine_type, execution, &mut context, terminal)?
        };
        if let Some(target) = result.next_view {
            self.open_view(&target)?;
        }
        Ok(result.exit)
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
        self.session.push(frame);
        self.request_discovery()
    }

    fn open_view(&mut self, view_ref: &str) -> Result<()> {
        self.open_view_with_input(view_ref, "")
    }

    fn open_view_with_input(&mut self, view_ref: &str, input: &str) -> Result<()> {
        if self.config.engine(view_ref)? != ENGINE_LAUNCHER {
            bail!(
                "view {:?} cannot be opened with the launcher engine",
                view_ref
            );
        }
        self.clear_active_error();
        let mut frame = LauncherFrame::new(view_ref, &self.config.default_rule);
        frame.input = input.to_string();
        self.session.push(frame);
        self.request_discovery()
    }

    fn pop_view(&mut self) -> Result<()> {
        if self.session.len() <= 1 {
            return Ok(());
        }
        self.clear_active_error();
        self.session.pop();
        self.current_mut().discovery_pending = false;
        self.current_mut().pending_command = None;
        self.request_discovery()
    }

    fn render(&self, terminal: &Terminal) -> Result<()> {
        let status = self.active_error.as_ref().map(|error| error.label.as_str());
        self.session.render(self.config, terminal, status)
    }

    #[cfg(test)]
    fn footer_line(&self, width: usize, state: &crate::engine::LauncherRenderState) -> String {
        let left = self
            .active_error
            .as_ref()
            .map(|error| error.label.as_str())
            .unwrap_or(state.view.as_str());
        render::footer_line(width, left, &state.commands)
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

fn compare_bindings(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "enter", right == "enter") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

#[derive(Debug, Clone, Copy)]
enum CommandResult {
    Continue,
    Refresh,
    Exit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Command, DisplayType, ENGINE_LAUNCHER, EngineDefinition, Rule, View, ViewTypeDefinition,
    };
    use crate::engine::LauncherFocus;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use unicode_width::UnicodeWidthStr;

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
            view_type: ENGINE_LAUNCHER.to_string(),
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
            viewtypes: BTreeMap::from([(
                ENGINE_LAUNCHER.to_string(),
                ViewTypeDefinition {
                    engine: EngineDefinition {
                        engine_type: ENGINE_LAUNCHER.to_string(),
                        config: toml::from_str(
                            r#"
                            commands = "{{ runtime:view.current.command }}"
                            "#,
                        )
                        .unwrap(),
                    },
                },
            )]),
            plugin_roots: BTreeMap::new(),
            config_value: Value::Object(serde_json::Map::new()),
        }
    }

    #[test]
    fn launcher_view_stack_returns_to_parent() {
        let config = test_config();
        let mut app = App::new(&config);
        assert_eq!(app.current().view, "core:default");

        app.open_view("apps:main").unwrap();
        assert_eq!(app.session.len(), 2);
        assert_eq!(app.current().view, "apps:main");

        app.pop_view().unwrap();
        assert_eq!(app.session.len(), 1);
        assert_eq!(app.current().view, "core:default");
    }

    #[test]
    fn session_focus_is_exclusive_and_restorable() {
        let config = test_config();
        let default_rule = config.default_rule.clone();
        let mut session = LauncherSession::new("core:default", &default_rule, config, None);
        assert_eq!(session.focus(), LauncherFocus::Launcher);
        session.take_focus(LauncherFocus::Capture).unwrap();
        assert_eq!(session.focus(), LauncherFocus::Capture);
        assert!(session.take_focus(LauncherFocus::Embedded).is_err());
        session.restore_focus();
        assert_eq!(session.focus(), LauncherFocus::Launcher);
    }

    #[test]
    fn engine_builds_command_actions_from_session_state() {
        let config = test_config();
        let default_rule = config.default_rule.clone();
        let mut session = LauncherSession::new("core:default", &default_rule, config.clone(), None);
        session.current_mut().items.push(Item {
            prefix: "core".to_string(),
            text: "item".to_string(),
            value: Some("value".to_string()),
            metadata: Value::Null,
            source_view: "core:default".to_string(),
        });

        let action =
            LauncherEngine::prepare_command_action(&config, &mut session, Key::Enter, None)
                .unwrap()
                .expect("selected command should produce an action");
        match action {
            CommandAction::Execute(execution) => {
                assert_eq!(execution.engine_type, ENGINE_LAUNCHER);
                assert!(!execution.exit);
                assert_eq!(execution.invocation.id, "run");
                assert_eq!(execution.title, "item / run");
                assert_eq!(execution.prepared.argv[0], "sh");
            }
            CommandAction::OpenView { .. } | CommandAction::Report { .. } => {
                panic!("expected a runnable command action")
            }
        }
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
        let state = LauncherEngine::render_state(app.config, &app.session);
        let footer = app.footer_line(120, &state);
        assert!(!footer.contains('\n'));
        assert!(footer.contains("command finished"));
        assert!(footer.contains("Enter Run"));
        assert!(footer.contains("Alt-A Apps"));
        assert!(footer.contains("Alt-S Shell"));
        assert!(!footer.contains("Ctrl-K commands"));
        assert!(UnicodeWidthStr::width(app.footer_line(50, &state).as_str()) < 50);

        app.clear_active_error();
        let narrow_footer = app.footer_line(40, &state);
        assert!(narrow_footer.contains("Ctrl-K commands"));

        app.record_error(
            Some("very-long-source-view"),
            None,
            "a long discovery error",
        );
        let error_footer = app.footer_line(40, &state);
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
