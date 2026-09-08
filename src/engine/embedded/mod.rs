mod protocol;
mod pty;
mod session;
mod terminal;

pub(crate) use self::protocol::{EmbeddedProtocolConfig, create_protocol_view};
pub(crate) use self::pty::EmbeddedOutcome;
use self::pty::EmbeddedPoll;
use self::session::{EmbeddedSession, EmbeddedStartPlan};
pub(crate) use self::terminal::{EmbeddedTerminal, EmbeddedTerminalSnapshot};

use super::{
    ActionId, EmbeddedResultConfig, EmbeddedResultFormat, EngineActionInput, EngineDecision,
    EngineEmission, EngineNotice, EngineRuntime, EngineValidationContext, ExternalTickResult,
    InputBindingFactoryContext, RawInputReceiver, RenderModel, RendererFactoryContext,
    RuntimeFactoryContext, require_field, validate_fields,
};
use crate::execution::PreparedProcess;
use crate::input::keymap::KeymapAction;
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};
use serde::Deserialize;
use serde_json::Value;

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
                    "view {:?} embedded escape-cancels must be a boolean",
                    view_ref
                )
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(true))
}

pub(super) fn definition() -> crate::engine::EngineDefinition {
    crate::engine::EngineDefinition::new()
        .with_factory_fields(crate::engine::FactoryFieldPlan {
            runtime: &["command", "result"],
            binding: &["escape-cancels"],
            binding_defaults: None,
        })
        .with_actions([crate::engine::ActionSpec::unit("embedded.cancel")])
}

fn reject_picker_sources(name: &str, view: &crate::workflow::config::View) -> Result<()> {
    if view.selected_items().is_some() || !view.selected_feeds().is_empty() {
        anyhow::bail!(
            "view {:?} using engine {:?} cannot provide picker items",
            name,
            view.selected_engine_type()
        );
    }
    Ok(())
}

pub(super) fn validate_config(context: EngineValidationContext<'_>) -> Result<()> {
    let name = context.view_ref;
    let view = context.view;
    reject_picker_sources(name, view)?;
    validate_fields(name, view, &["command", "result", "escape-cancels"])?;
    require_field(name, view, "command")?;
    if let Some(result) = view.engine_field("result") {
        parse_result_config(name, Some(result))?;
    }
    if let Some(escape_cancels) = view.engine_field("escape-cancels") {
        parse_escape_cancels_value(
            name,
            Some(crate::workflow::config::toml_to_json(escape_cancels)?),
        )?;
    }
    let command = view
        .engine_field("command")
        .expect("required embedded command was checked");
    let arguments = command
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("view {:?} embedded command must be an argv array", name))?;
    if arguments.is_empty() || arguments.iter().any(|argument| !argument.is_str()) {
        anyhow::bail!(
            "view {:?} embedded command must be a non-empty array of strings",
            name
        );
    }
    Ok(())
}

pub(super) fn create_view(
    context: RuntimeFactoryContext,
) -> Result<Box<dyn crate::engine::EngineRuntime>> {
    let view_ref = context.identity.view_ref.clone();
    let command = context
        .config
        .field("command")
        .context("embedded engine requires a command field")?;
    let command = command
        .as_array()
        .context("embedded command must be an argv array")?
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
    let command = if let Some(root) = context.config.workflow_root.as_ref() {
        command
            .into_iter()
            .map(|arg| {
                let path = std::path::Path::new(&arg);
                if !path.is_absolute() && root.join(path).is_file() {
                    root.join(path).to_string_lossy().into_owned()
                } else {
                    arg
                }
            })
            .collect()
    } else {
        command
    };
    let mut environment = vec![(
        "LAUNCHER_INPUT".to_string(),
        context.parameters.raw_input().to_string(),
    )];
    if let Some(root) = context.config.workflow_root.as_ref() {
        environment.push((
            "WORKFLOW_DIR".to_string(),
            root.to_string_lossy().into_owned(),
        ));
    }
    let result = parse_result_config_value(&view_ref, context.config.field("result").cloned())?;
    let session = EmbeddedSession::new(
        PreparedProcess {
            argv: command,
            environment,
            current_dir: None,
        },
        result,
        context.cancellation,
    );
    let start_plan = Some(session.prepare_start());
    Ok(Box::new(EmbeddedView {
        view_ref,
        session,
        start_plan,
        pending_outcome: None,
    }))
}

pub(super) fn create_renderer(
    _context: RendererFactoryContext,
) -> Result<Box<dyn crate::engine::ViewRenderer>> {
    Ok(Box::new(EmbeddedRenderer))
}

pub(crate) fn create_input_bindings(
    context: InputBindingFactoryContext,
) -> Result<Vec<crate::workflow::command::InputActionBinding>> {
    let escape_cancels = parse_escape_cancels_value(
        &context.identity.view_ref,
        context.bindings.engine_field("escape-cancels").cloned(),
    )?;
    Ok(if escape_cancels {
        vec![crate::workflow::command::InputActionBinding {
            key: crate::input::Key::Escape,
            action: crate::workflow::command::ResolvedInputAction::Engine(ActionId::new(
                "embedded.cancel",
            )),
            label: Some(EmbeddedAction::Cancel.label().to_string()),
            enabled: true,
        }]
    } else {
        Vec::new()
    })
}

struct EmbeddedView {
    view_ref: String,
    session: EmbeddedSession,
    start_plan: Option<EmbeddedStartPlan>,
    pending_outcome: Option<crate::engine::embedded::pty::EmbeddedRunResult>,
}

impl EmbeddedView {
    fn poll_runtime(&mut self, terminal_size: (u16, u16)) -> Result<()> {
        if self.pending_outcome.is_some() {
            return Ok(());
        }
        match self.session.poll(terminal_size) {
            Ok(EmbeddedPoll::Finished(result)) => self.pending_outcome = Some(result),
            Ok(EmbeddedPoll::Running) => {}
            Err(error) => self.pending_outcome = Some(failed_run_result(error)),
        }
        Ok(())
    }

    fn complete(&self) -> Result<ExternalTickResult> {
        let Some(result) = self.pending_outcome.as_ref() else {
            return Ok(ExternalTickResult::continue_without_notice());
        };
        let message = embedded_status_message(&result.outcome);
        let success = embedded_succeeded(&result.outcome);
        let notice = Some(if success {
            EngineNotice::Info {
                view_ref: self.view_ref.clone(),
                message: message.clone(),
            }
        } else {
            EngineNotice::Error {
                view_ref: self.view_ref.clone(),
                message: message.clone(),
            }
        });
        Ok(match &result.outcome {
            EmbeddedOutcome::Returned(output) => {
                ExternalTickResult::return_with(output.clone(), notice)
            }
            EmbeddedOutcome::Cancelled | EmbeddedOutcome::Exited(0) => {
                ExternalTickResult::close_with(notice)
            }
            EmbeddedOutcome::Exited(_) | EmbeddedOutcome::Signaled(_) => {
                ExternalTickResult::fail_with(message, notice)
            }
            EmbeddedOutcome::Failed(_) => ExternalTickResult::fail_with(message, notice),
        })
    }
}

fn failed_run_result(error: anyhow::Error) -> crate::engine::embedded::pty::EmbeddedRunResult {
    crate::engine::embedded::pty::EmbeddedRunResult {
        outcome: EmbeddedOutcome::Failed(error.to_string()),
    }
}

impl RawInputReceiver for EmbeddedView {
    fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.session.push_input(bytes)
    }
}

impl EngineRuntime for EmbeddedView {
    fn action(&mut self, input: EngineActionInput) -> Result<EngineEmission> {
        let decision = match input.invocation.id.as_str() {
            "embedded.cancel" => EngineDecision::Close,
            action => anyhow::bail!("unknown embedded action {:?}", action),
        };
        Ok(EngineEmission::decision(decision))
    }

    fn raw_receiver(&mut self) -> Option<&mut dyn RawInputReceiver> {
        Some(self)
    }

    fn drive_tick(&mut self, tick: crate::engine::EngineTick) -> Result<ExternalTickResult> {
        if self.pending_outcome.is_some() {
            return self.complete();
        }
        if let Some(plan) = self.start_plan.take() {
            match self.session.start_external(plan, tick.content_size) {
                Ok(runtime) => self.session.commit_start(runtime),
                Err(error) => {
                    self.pending_outcome = Some(failed_run_result(error));
                    return self.complete();
                }
            }
        }
        self.poll_runtime(tick.content_size)?;
        // Keep one render turn for the final PTY screen before consuming the
        // terminal outcome on the next drive tick.
        Ok(ExternalTickResult::continue_without_notice())
    }

    fn commit_external_tick(&mut self) {
        self.pending_outcome = None;
    }

    fn deactivate(&mut self) {
        self.pending_outcome = None;
        self.start_plan = None;
        self.session.deactivate();
    }

    fn drive_background_tick(&mut self) -> Result<()> {
        // Background polling never consumes the start plan: PTY creation is
        // reserved for the foreground external phase after Host commit.
        if !self.session.is_started() || self.pending_outcome.is_some() {
            return Ok(());
        }
        match self.session.poll_background() {
            Ok(EmbeddedPoll::Finished(result)) => self.pending_outcome = Some(result),
            Ok(EmbeddedPoll::Running) => {}
            Err(error) => self.pending_outcome = Some(failed_run_result(error)),
        }
        Ok(())
    }

    fn render_model(&self) -> RenderModel {
        RenderModel::new(
            "terminal",
            EmbeddedRenderModel {
                screen: self.session.screen().map(EmbeddedTerminal::snapshot),
            },
        )
    }
}

#[derive(Clone)]
struct EmbeddedRenderModel {
    screen: Option<EmbeddedTerminalSnapshot>,
}

pub(crate) struct EmbeddedRenderer;

impl crate::engine::ViewRenderer for EmbeddedRenderer {
    fn validate_model(&self, model: &RenderModel) -> anyhow::Result<()> {
        if model.kind() != "terminal" || model.downcast_ref::<EmbeddedRenderModel>().is_none() {
            anyhow::bail!(
                "embedded renderer/model pairing mismatch: renderer=terminal model={:?}",
                model
            );
        }
        Ok(())
    }

    fn chrome(&self, _model: &RenderModel) -> crate::ui::chrome::EngineChrome {
        crate::ui::chrome::EngineChrome::default()
    }

    fn render(
        &self,
        model: &RenderModel,
        _context: &crate::engine::RenderContext,
        frame: &mut Frame,
        area: Rect,
    ) {
        let Some(model) = model.downcast_ref::<EmbeddedRenderModel>() else {
            return;
        };
        let Some(screen) = &model.screen else {
            return;
        };
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
        EmbeddedOutcome::Failed(error) => format!("embedded view failed: {error}"),
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
    let value = crate::workflow::config::toml_to_json(value)?;
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
    use super::{EmbeddedSession, EngineRuntime, parse_escape_cancels_value};
    use crate::execution::PreparedProcess;
    use crate::lifecycle::CancellationToken;

    fn unstarted_view() -> super::EmbeddedView {
        let session = EmbeddedSession::new(
            PreparedProcess {
                argv: vec!["/bin/sh".to_string()],
                environment: Vec::new(),
                current_dir: None,
            },
            None,
            CancellationToken::new().observer(),
        );
        let start_plan = Some(session.prepare_start());
        super::EmbeddedView {
            view_ref: "test:embedded".to_string(),
            session,
            start_plan,
            pending_outcome: None,
        }
    }

    #[test]
    fn background_tick_does_not_consume_unstarted_start_plan() {
        let mut view = unstarted_view();
        view.drive_background_tick().unwrap();

        assert!(!view.session.is_started());
        assert!(view.start_plan.is_some());
    }

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
                .contains("escape-cancels must be a boolean")
        );
    }
}
