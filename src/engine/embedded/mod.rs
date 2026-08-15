mod pty;
mod session;

pub(crate) use self::pty::{EmbeddedOutcome, EmbeddedRunResult};
use self::session::EmbeddedSession;
use super::{
    EmbeddedResultConfig, EmbeddedResultFormat, Engine, EngineHost, InputFocus, PreparedProcess,
    ViewContext, ViewEffect, ViewInstance, evaluate_field, evaluate_optional_string, require_field,
    validate_fields,
};
use crate::config::{ENGINE_EMBEDDED, View};
use crate::expression::Template;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};
use serde::Deserialize;

pub(crate) struct EmbeddedEngine;

const DEFAULT_RESULT_LIMIT: usize = 1024 * 1024;
const MAX_RESULT_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResultFormatConfig {
    Text,
    Json,
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

fn parse_escape_cancels(view_ref: &str, value: Option<&toml::Value>) -> Result<bool> {
    value
        .map(|value| {
            value.as_bool().with_context(|| {
                format!(
                    "view {:?} embedded escape-cancels must be a boolean",
                    view_ref
                )
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(true))
}

fn chrome_commands(escape_cancels: bool) -> Vec<(String, String)> {
    if escape_cancels {
        vec![("escape".to_string(), "Cancel".to_string())]
    } else {
        Vec::new()
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
        if !view.commands.is_empty() {
            anyhow::bail!(
                "view {:?} using the embedded engine cannot define View commands",
                name
            );
        }
        if let Some(title) = view.engine_field("title")
            && !title.is_str()
        {
            anyhow::bail!("view {:?} embedded title must be a string expression", name);
        }
        parse_result_config(name, view.engine_field("result"))?;
        parse_escape_cancels(name, view.engine_field("escape-cancels"))?;
        let command = view
            .engine_field("command")
            .expect("required embedded command was checked");
        if let Some(source) = command.as_str() {
            if Template::parse(source)?.is_complete_expression() {
                return Ok(());
            }
            anyhow::bail!(
                "view {:?} embedded command must be an argv array or complete expression",
                name
            );
        }
        let arguments = command.as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "view {:?} embedded command must be an argv array or expression",
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
        if let Some(path) = context.log_file {
            environment.push((
                "LAUNCHER_LOG_FILE".to_string(),
                path.to_string_lossy().to_string(),
            ));
        }
        let view = context
            .config
            .view(&context.request.view_ref)
            .context("embedded View disappeared during creation")?;
        let result = parse_result_config(&context.request.view_ref, view.engine_field("result"))?;
        let escape_cancels = parse_escape_cancels(
            &context.request.view_ref,
            view.engine_field("escape-cancels"),
        )?;
        Ok(Box::new(EmbeddedView {
            title: title.clone(),
            result,
            escape_cancels,
            session: EmbeddedSession::new(PreparedProcess {
                argv: command,
                environment,
                current_dir: plugin_root,
            }),
        }))
    }
}

struct EmbeddedView {
    title: String,
    result: Option<EmbeddedResultConfig>,
    escape_cancels: bool,
    session: EmbeddedSession,
}

impl ViewInstance for EmbeddedView {
    fn step(&mut self, _host: &mut EngineHost<'_>, _terminal: &mut Terminal) -> Result<ViewEffect> {
        Ok(ViewEffect::RunEmbedded {
            prepared: self.session.prepared(),
            result: self.result,
            escape_cancels: self.escape_cancels,
        })
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome {
            title: Some(format!("embedded: {}", self.title)),
            status: Some("keys pass through".to_string()),
            commands: chrome_commands(self.escape_cancels),
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn render(&mut self, _host: &EngineHost<'_>, _frame: &mut Frame, _area: Rect) {}

    fn input_focus(&self) -> InputFocus {
        InputFocus::Unfocused
    }

    fn chrome_footer_enabled(&self) -> bool {
        false
    }
}

pub(crate) fn run(
    prepared: &PreparedProcess,
    result: Option<EmbeddedResultConfig>,
    escape_cancels: bool,
    initial_input: Vec<u8>,
    terminal: &mut Terminal,
    content_size: &dyn Fn(u16, u16) -> (u16, u16),
    render: &mut dyn FnMut(
        &mut Terminal,
        &crate::embedded_terminal::EmbeddedTerminal,
    ) -> Result<()>,
) -> Result<EmbeddedRunResult> {
    pty::run(
        prepared,
        result,
        escape_cancels,
        initial_input,
        terminal,
        content_size,
        render,
    )
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
    let value: ResultConfig = value
        .clone()
        .try_into()
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
    use super::{chrome_commands, parse_escape_cancels};

    #[test]
    fn escape_cancellation_defaults_on_and_requires_a_boolean() {
        assert!(parse_escape_cancels("core:default", None).unwrap());
        assert!(!parse_escape_cancels("core:default", Some(&toml::Value::Boolean(false))).unwrap());
        let error = parse_escape_cancels(
            "core:default",
            Some(&toml::Value::String("false".to_string())),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("escape-cancels must be a boolean")
        );
    }

    #[test]
    fn chrome_only_advertises_enabled_launcher_controls() {
        assert_eq!(
            chrome_commands(true),
            vec![("escape".to_string(), "Cancel".to_string())]
        );
        assert!(chrome_commands(false).is_empty());
    }
}
