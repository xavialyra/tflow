mod render;
mod session;

use self::session::CaptureSession;
use super::{
    Engine, EngineHost, InputFocus, ViewContext, ViewEffect, ViewInstance, ViewOutput, command,
    evaluate_field, evaluate_optional_string, require_field, validate_fields,
};
use crate::config::{ENGINE_CAPTURE, View};
use crate::engine::api::{LauncherAction, LauncherOutcome, ResolvedLauncherAction};
use crate::input::Key;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};

pub(crate) struct CaptureEngine;

impl Engine for CaptureEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_CAPTURE
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(name, view, &["output", "title"])?;
        require_field(name, view, "output")?;
        for field in ["output", "title"] {
            if let Some(value) = view.engine_field(field)
                && !value.is_str()
            {
                anyhow::bail!(
                    "view {:?} capture field {:?} must be a string expression",
                    name,
                    field
                );
            }
        }
        Ok(())
    }

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>> {
        let default_title = context.request.view_ref.clone();
        let evaluated = (|| {
            let title = evaluate_optional_string(&context, "title")?
                .unwrap_or_else(|| default_title.clone());
            let output = evaluate_field(&context, "output")?
                .context("capture engine requires an output field")?;
            let output = output
                .as_str()
                .context("capture output must evaluate to a string")?;
            Ok::<_, anyhow::Error>((title, output.to_string()))
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
            status,
            success,
            reported: false,
        }))
    }
}

struct CaptureView {
    view_ref: String,
    session: CaptureSession,
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

    fn resolve_launcher_action(
        &self,
        host: &EngineHost<'_>,
        key: Key,
    ) -> Option<ResolvedLauncherAction> {
        let chrome_footer = key.binding_name().is_some_and(|key| {
            host.config.chrome.footer.bindings.values().any(|binding| {
                crate::config::normalize_key(&binding.key).ok().as_deref() == Some(&key)
            })
        });
        if command::find_command_for_key(host.config, &self.view_ref, key).is_some()
            || chrome_footer
        {
            None
        } else {
            Some(ResolvedLauncherAction::View(LauncherAction::Back))
        }
    }

    fn handle_launcher_action(
        &mut self,
        _host: &mut EngineHost<'_>,
        action: LauncherAction,
        _input: crate::input::DecodedInput,
    ) -> Result<LauncherOutcome> {
        debug_assert_eq!(action, LauncherAction::Back);
        Ok(LauncherOutcome::Effect(Box::new(ViewEffect::Back(None))))
    }

    fn view_command_output(&self) -> Option<ViewOutput> {
        self.success.then(|| ViewOutput::Value {
            value: serde_json::Value::String(self.session.output().to_string()),
        })
    }

    fn chrome(&self, host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        let mut commands = host
            .config
            .view(&self.view_ref)
            .into_iter()
            .flat_map(|view| view.commands.values())
            .filter_map(|command| {
                crate::config::normalize_key(&command.key)
                    .ok()
                    .map(|key| (key, command.label.clone()))
            })
            .collect::<Vec<_>>();
        commands.push(("other key".to_string(), "Back".to_string()));
        crate::chrome::EngineChrome {
            title: Some(format!("capture: {}", self.session.title())),
            status: Some(self.session.status().to_string()),
            commands,
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn render(&mut self, _host: &EngineHost<'_>, frame: &mut Frame, area: Rect) {
        render::render_capture(frame, area, self.session.lines());
    }

    fn input_focus(&self) -> InputFocus {
        InputFocus::Unfocused
    }
}
