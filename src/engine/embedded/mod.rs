mod pty;
mod session;

pub(crate) use self::pty::EmbeddedOutcome;
use self::pty::EmbeddedPoll;
use self::session::EmbeddedSession;

use super::api::{LauncherOutcome, ResolvedInputAction, ViewAction};
use super::keymap::{ActionBindings, KeymapAction};
use super::{
    EmbeddedResultConfig, EmbeddedResultFormat, Engine, EngineHost, InputActionBinding, InputFocus,
    PreparedProcess, ViewContext, ViewEffect, ViewInputMode, ViewInstance, ViewReturn,
    evaluate_field, evaluate_optional_string, require_field, validate_fields,
};
use crate::config::{ENGINE_EMBEDDED, View};
use crate::expression::{Template, is_dynamic_string};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};
use serde::Deserialize;
use serde_json::Value;

pub(crate) struct EmbeddedEngine;

const DEFAULT_RESULT_LIMIT: usize = 1024 * 1024;
const MAX_RESULT_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResultFormatConfig {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum EmbeddedAction {
    Cancel,
}

impl EmbeddedAction {
    fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
        }
    }
}

impl KeymapAction for EmbeddedAction {
    const LABEL: &'static str = "embedded";

    fn name(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        (name == "cancel").then_some(Self::Cancel)
    }

    fn default_bindings() -> &'static [(crate::input::Key, Self)] {
        &[(crate::input::Key::Escape, Self::Cancel)]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultConfig {
    format: ResultFormatConfig,
    #[serde(default = "required_result")]
    required: bool,
    #[serde(default = "default_result_limit")]
    max_bytes: usize,
}

fn required_result() -> bool {
    true
}

fn default_result_limit() -> usize {
    DEFAULT_RESULT_LIMIT
}

fn parse_escape_cancels_value(view_ref: &str, value: Option<Value>) -> Result<bool> {
    value
        .map(|value| {
            value.as_bool().with_context(|| {
                format!(
                    "view {:?} embedded escape-cancels must evaluate to a boolean",
                    view_ref
                )
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(true))
}

fn contains_dynamic(value: &toml::Value) -> bool {
    match value {
        toml::Value::String(source) => is_dynamic_string(source),
        toml::Value::Array(values) => values.iter().any(contains_dynamic),
        toml::Value::Table(values) => values.values().any(contains_dynamic),
        toml::Value::Boolean(_)
        | toml::Value::Datetime(_)
        | toml::Value::Float(_)
        | toml::Value::Integer(_) => false,
    }
}

impl Engine for EmbeddedEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_EMBEDDED
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(
            name,
            view,
            &["command", "title", "result", "escape-cancels"],
        )?;
        require_field(name, view, "command")?;
        if let Some(title) = view.engine_field("title")
            && !title.is_str()
        {
            anyhow::bail!(
                "view {:?} embedded title must be a string or template",
                name
            );
        }
        if let Some(result) = view.engine_field("result")
            && !contains_dynamic(result)
        {
            parse_result_config(name, Some(result))?;
        }
        if let Some(escape_cancels) = view.engine_field("escape-cancels")
            && !contains_dynamic(escape_cancels)
        {
            parse_escape_cancels_value(name, Some(crate::config::toml_to_json(escape_cancels)?))?;
        }
        let command = view
            .engine_field("command")
            .expect("required embedded command was checked");
        if let Some(source) = command.as_str() {
            if Template::parse(source)?.is_complete_path() {
                return Ok(());
            }
            anyhow::bail!(
                "view {:?} embedded command must be an argv array or complete dynamic path",
                name
            );
        }
        let arguments = command.as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "view {:?} embedded command must be an argv array or dynamic path",
                name
            )
        })?;
        if arguments.is_empty() || arguments.iter().any(|argument| !argument.is_str()) {
            anyhow::bail!(
                "view {:?} embedded command must be a non-empty array of strings",
                name
            );
        }
        Ok(())
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let command = evaluate_field(&context, "command")?
            .context("embedded engine requires a command field")?;
        let command = command
            .as_array()
            .context("embedded command must evaluate to an array")?
            .iter()
            .map(|argument| {
                argument
                    .as_str()
                    .map(str::to_string)
                    .context("embedded command arguments must be strings")
            })
            .collect::<Result<Vec<_>>>()?;
        if command.is_empty() {
            anyhow::bail!("embedded command must not be empty");
        }
        let title = evaluate_optional_string(&context, "title")?
            .unwrap_or_else(|| context.request.view_ref.clone());
        let plugin = context
            .request
            .view_ref
            .split_once(':')
            .map(|(package, _)| package)
            .unwrap_or(&context.request.view_ref)
            .to_string();
        let plugin_root = context
            .config
            .plugin_root(&context.request.view_ref)
            .map(|path| path.to_path_buf());
        let mut environment = vec![
            (
                "LAUNCHER_VIEW_REF".to_string(),
                context.request.view_ref.clone(),
            ),
            ("LAUNCHER_INPUT".to_string(), context.input.params.clone()),
            ("LAUNCHER_PLUGIN".to_string(), plugin),
        ];
        if let Some(root) = &plugin_root {
            environment.push((
                "LAUNCHER_PLUGIN_DIR".to_string(),
                root.to_string_lossy().to_string(),
            ));
        }
        context
            .config
            .view(&context.request.view_ref)
            .context("embedded View disappeared during creation")?;
        let result = parse_result_config_value(
            &context.request.view_ref,
            evaluate_field(&context, "result")?,
        )?;
        let escape_cancels = parse_escape_cancels_value(
            &context.request.view_ref,
            evaluate_field(&context, "escape-cancels")?,
        )?;
        Ok(Box::new(EmbeddedView {
            view_ref: context.request.view_ref.clone(),
            title: title.clone(),
            keymap: escape_cancels
                .then(|| ActionBindings::from_values(None, None))
                .transpose()?,
            session: EmbeddedSession::new(
                PreparedProcess {
                    argv: command,
                    environment,
                    current_dir: plugin_root,
                },
                result,
                context.cancellation,
            ),
            pending_outcome: None,
        }))
    }
}

struct EmbeddedView {
    view_ref: String,
    title: String,
    keymap: Option<ActionBindings<EmbeddedAction>>,
    session: EmbeddedSession,
    pending_outcome: Option<crate::engine::embedded::pty::EmbeddedRunResult>,
}

impl EmbeddedView {
    fn poll_runtime(&mut self, terminal: &Terminal) -> Result<()> {
        if self.pending_outcome.is_some() {
            return Ok(());
        }
        self.session.start(terminal.size(), &[])?;
        if let EmbeddedPoll::Finished(result) = self.session.poll(terminal.size())? {
            self.pending_outcome = Some(result);
        }
        Ok(())
    }

    fn complete(&mut self, host: &mut EngineHost<'_>) -> Result<ViewEffect> {
        let Some(result) = self.pending_outcome.take() else {
            return Ok(ViewEffect::Continue);
        };
        let message = embedded_status_message(&result.outcome);
        let success = embedded_succeeded(&result.outcome);
        host.record_view_status(&self.view_ref, &message, success);
        match result.outcome {
            EmbeddedOutcome::Returned(output) => Ok(ViewEffect::Return(ViewReturn {
                source_view: self.view_ref.clone(),
                output,
                adapter: None,
            })),
            _ => Ok(ViewEffect::Back(None)),
        }
    }
}

impl ViewInstance for EmbeddedView {
    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect> {
        if self.pending_outcome.is_some() {
            return self.complete(host);
        }
        self.poll_runtime(terminal)?;
        Ok(ViewEffect::Continue)
    }

    fn background_step(
        &mut self,
        _host: &mut EngineHost<'_>,
        terminal: &mut Terminal,
    ) -> Result<ViewEffect> {
        self.poll_runtime(terminal).map(|_| ViewEffect::Continue)
    }

    fn input_mode(&self) -> ViewInputMode {
        if self.pending_outcome.is_none() && self.session.is_running() {
            ViewInputMode::Passthrough
        } else {
            ViewInputMode::Keymap
        }
    }

    fn handle_unbound_input(
        &mut self,
        _host: &mut EngineHost<'_>,
        bytes: &[u8],
    ) -> Result<LauncherOutcome> {
        self.session.push_input(bytes)?;
        Ok(LauncherOutcome::Continue)
    }

    fn handle_terminal_eof(&mut self, host: &mut EngineHost<'_>) -> Result<LauncherOutcome> {
        host.record_view_status(&self.view_ref, "embedded view cancelled", true);
        Ok(LauncherOutcome::Effect(Box::new(ViewEffect::Back(None))))
    }

    fn input_action_bindings(&self, _host: &EngineHost<'_>) -> Vec<InputActionBinding> {
        self.keymap
            .iter()
            .flat_map(|keymap| keymap.bindings())
            .map(|(key, action)| InputActionBinding {
                key,
                action: ResolvedInputAction::View(ViewAction::new(action.name())),
                label: Some(action.label().to_string()),
                mode: ViewInputMode::Passthrough,
                enabled: true,
            })
            .collect()
    }

    fn handle_view_action(
        &mut self,
        host: &mut EngineHost<'_>,
        action: ViewAction,
        _input: crate::input::DecodedInput,
    ) -> Result<LauncherOutcome> {
        match action.name() {
            "cancel" => self.handle_terminal_eof(host),
            _ => unreachable!("embedded received an unsupported View action"),
        }
    }

    fn launcher_input_timeout(&self, _host: &EngineHost<'_>) -> Option<i32> {
        // Keep passthrough input responsive while the embedded process redraws.
        (self.input_mode() == ViewInputMode::Passthrough).then_some(10)
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome {
            title: Some(format!("embedded: {}", self.title)),
            status: Some("keys pass through".to_string()),
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn render(&mut self, _host: &EngineHost<'_>, frame: &mut Frame, area: Rect) {
        self.session
            .request_resize((area.width.max(1), area.height.max(1)));
        if let Some(screen) = self.session.screen() {
            frame.render_widget(screen.widget(), area);
            if let Some((column, row)) = screen.cursor()
                && column < area.width as usize
                && row < area.height as usize
            {
                frame.set_cursor_position((
                    area.x.saturating_add(column as u16),
                    area.y.saturating_add(row as u16),
                ));
            }
        }
    }

    fn input_focus(&self) -> InputFocus {
        InputFocus::Unfocused
    }
}

pub(crate) fn embedded_succeeded(outcome: &EmbeddedOutcome) -> bool {
    matches!(
        outcome,
        EmbeddedOutcome::Cancelled | EmbeddedOutcome::Exited(0) | EmbeddedOutcome::Returned(_)
    )
}

pub(crate) fn embedded_status_message(outcome: &EmbeddedOutcome) -> String {
    match outcome {
        EmbeddedOutcome::Cancelled => "embedded view cancelled".to_string(),
        EmbeddedOutcome::Exited(0) => "embedded view finished successfully".to_string(),
        EmbeddedOutcome::Exited(code) => format!("embedded view exited with code {}", code),
        EmbeddedOutcome::Signaled(signal) => {
            format!("embedded view terminated by signal {}", signal)
        }
        EmbeddedOutcome::Returned(_) => "embedded view returned a result".to_string(),
    }
}

fn parse_result_config(
    view_ref: &str,
    value: Option<&toml::Value>,
) -> Result<Option<EmbeddedResultConfig>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = crate::config::toml_to_json(value)?;
    parse_result_config_value(view_ref, Some(value))
}

fn parse_result_config_value(
    view_ref: &str,
    value: Option<Value>,
) -> Result<Option<EmbeddedResultConfig>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value: ResultConfig = serde_json::from_value(value)
        .with_context(|| format!("view {:?} has invalid embedded result config", view_ref))?;
    anyhow::ensure!(
        value.max_bytes > 0 && value.max_bytes <= MAX_RESULT_LIMIT,
        "view {:?} embedded result max_bytes must be between 1 and {}",
        view_ref,
        MAX_RESULT_LIMIT
    );
    Ok(Some(EmbeddedResultConfig {
        format: match value.format {
            ResultFormatConfig::Text => EmbeddedResultFormat::Text,
            ResultFormatConfig::Json => EmbeddedResultFormat::Json,
        },
        required: value.required,
        max_bytes: value.max_bytes,
    }))
}

#[cfg(test)]
mod tests {
    use super::parse_escape_cancels_value;

    #[test]
    fn escape_cancellation_defaults_on_and_requires_a_boolean() {
        assert!(parse_escape_cancels_value("core:default", None).unwrap());
        assert!(
            !parse_escape_cancels_value("core:default", Some(serde_json::Value::Bool(false)))
                .unwrap()
        );
        let error = parse_escape_cancels_value(
            "core:default",
            Some(serde_json::Value::String("false".to_string())),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("escape-cancels must evaluate to a boolean")
        );
    }
}
