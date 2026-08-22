mod keymap;
mod render;
mod session;

use self::keymap::{CaptureAction, CaptureKeymap};
use self::session::CaptureSession;
use super::{
    Engine, EngineHost, InputActionBinding, InputFocus, ViewContext, ViewEffect, ViewInputMode,
    ViewInstance, ViewOutput, evaluate_field, evaluate_optional_string, require_field,
    validate_fields,
};
use crate::command::{LauncherOutcome, ResolvedInputAction, ViewAction};
use crate::config::{
    ConfigSource, Defaults, ENGINE_CAPTURE, ResolvedScriptSource, ScriptSourceSpec, View,
    toml_to_json,
};
use crate::execution::{ensure_script_success, run_script};
use crate::expression::EvaluationStage;
use crate::terminal::Terminal;
use anyhow::{Context, Result, bail};
use ratatui::{Frame, layout::Rect};

pub(crate) struct CaptureEngine;

impl Engine for CaptureEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_CAPTURE
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(name, view, &["output", "title"])?;
        require_field(name, view, "output")?;
        if let Some(title) = view.engine_field("title")
            && !title.is_str()
        {
            anyhow::bail!("view {:?} capture title must be a string or template", name);
        }
        let output = view
            .engine_field("output")
            .expect("required capture output was checked");
        if !output.is_str() {
            let source = ScriptSourceSpec::parse(output)
                .with_context(|| format!("view {:?} capture output", name))?;
            source
                .validate_capture_source()
                .with_context(|| format!("view {:?} capture output", name))?;
        }
        Ok(())
    }

    fn validate_defaults(&self, defaults: &Defaults) -> Result<()> {
        let bindings = defaults
            .capture
            .bindings
            .as_ref()
            .map(toml_to_json)
            .transpose()?;
        CaptureKeymap::validate_values(bindings.as_ref(), None).context("capture bindings")
    }

    fn validate_keymap(&self, name: &str, view: &View) -> Result<()> {
        let keymap = view.keymap.as_ref().map(toml_to_json).transpose()?;
        CaptureKeymap::validate_values(None, keymap.as_ref())
            .with_context(|| format!("view {:?} capture keymap", name))
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let default_bindings = context.config.get(
            ConfigSource::Root,
            &context.evaluation,
            EvaluationStage::Operation,
            &["defaults", "capture", "bindings"],
        )?;
        let view_keymap = context.config.get(
            ConfigSource::View(context.state.view_ref()),
            &context.evaluation,
            EvaluationStage::Operation,
            &["keymap"],
        )?;
        let keymap = CaptureKeymap::from_values(default_bindings, view_keymap)?;

        let default_title = context.request.view_ref.clone();
        let evaluated = (|| {
            let title = evaluate_optional_string(&context, "title")?
                .unwrap_or_else(|| default_title.clone());
            let output = evaluate_output(&context)?;
            Ok::<_, anyhow::Error>((title, output))
        })();
        let (title, output, status, success) = match evaluated {
            Ok((title, output)) => (title, output, "finished successfully".to_string(), true),
            Err(error) => (
                default_title,
                error.to_string(),
                "failed".to_string(),
                false,
            ),
        };
        Ok(Box::new(CaptureView {
            view_ref: context.request.view_ref.clone(),
            session: CaptureSession::new(&title, &output, &status),
            keymap,
            status,
            success,
            reported: false,
        }))
    }
}

fn evaluate_output(context: &ViewContext<'_>) -> Result<String> {
    let output =
        evaluate_field(context, "output")?.context("capture engine requires an output field")?;
    if let Some(output) = output.as_str() {
        return Ok(output.to_string());
    }

    let source = ResolvedScriptSource::parse(&output)
        .context("capture output must evaluate to a string or script source")?;
    let root = context
        .config
        .plugin_root(&context.request.view_ref)
        .with_context(|| {
            format!(
                "capture source {:?} has no plugin root",
                context.request.view_ref
            )
        })?;
    let args = source.script_args("capture script args")?;
    let output = run_script(
        root,
        &source.file,
        &args,
        source.max_output_bytes,
        &context.cancellation,
    )?;
    ensure_script_success(&output)?;
    if output.stdout.is_empty() {
        bail!("script produced no JSON output");
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("script {} did not produce valid JSON", source.file))?;
    value
        .as_str()
        .map(str::to_string)
        .context("capture script source must produce a JSON string")
}

struct CaptureView {
    view_ref: String,
    session: CaptureSession,
    keymap: CaptureKeymap,
    status: String,
    success: bool,
    reported: bool,
}

impl ViewInstance for CaptureView {
    fn step(&mut self, host: &mut EngineHost<'_>, _terminal: &mut Terminal) -> Result<ViewEffect> {
        if !self.reported {
            self.reported = true;
            host.record_view_status(&self.view_ref, &self.status, self.success);
        }
        Ok(ViewEffect::Continue)
    }

    fn launcher_input_timeout(&self, _host: &EngineHost<'_>) -> Option<i32> {
        Some(80)
    }

    fn captures_editor_input(&self) -> bool {
        true
    }

    fn input_action_bindings(&self, _host: &EngineHost<'_>) -> Vec<InputActionBinding> {
        self.keymap
            .bindings()
            .map(|(key, action)| InputActionBinding {
                key,
                action: ResolvedInputAction::View(ViewAction::new(match action {
                    CaptureAction::Copy => "copy",
                    CaptureAction::Back => "back",
                })),
                label: Some(action.label().to_string()),
                mode: ViewInputMode::Keymap,
                enabled: action != CaptureAction::Copy || self.success,
            })
            .collect()
    }

    fn handle_view_action(
        &mut self,
        _host: &mut EngineHost<'_>,
        action: ViewAction,
        _input: crate::input::DecodedInput,
    ) -> Result<LauncherOutcome> {
        let effect = match action.name() {
            "copy" => ViewEffect::CopyToClipboard(self.session.output().to_string()),
            "back" => ViewEffect::Back(None),
            _ => unreachable!("capture received an unsupported View action"),
        };
        Ok(LauncherOutcome::Effect(Box::new(effect)))
    }

    fn view_command_output(&self) -> Option<ViewOutput> {
        self.success.then(|| ViewOutput::Value {
            value: serde_json::Value::String(self.session.output().to_string()),
        })
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome {
            title: Some(format!("capture: {}", self.session.title())),
            status: Some(self.session.status().to_string()),
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn render(&mut self, host: &EngineHost<'_>, frame: &mut Frame, area: Rect) {
        render::render_capture(frame, area, self.session.lines(), &host.theme);
    }

    fn input_focus(&self) -> InputFocus {
        InputFocus::Unfocused
    }
}
