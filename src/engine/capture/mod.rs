mod render;
mod session;

use self::session::CaptureSession;
use super::{
    Engine, EngineHost, ViewContext, ViewEffect, ViewInstance, evaluate_field,
    evaluate_optional_string, require_field, validate_fields,
};
use crate::config::{ENGINE_CAPTURE, View};
use crate::terminal::Terminal;
use anyhow::{Context, Result};

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
        let default_title = context.location.view_ref.clone();
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
            view_ref: context.location.view_ref.clone(),
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
    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect> {
        if !self.reported {
            self.reported = true;
            host.record_view_status(&self.view_ref, &self.status, self.success);
        }
        if self.session.return_requested(terminal)? {
            Ok(ViewEffect::Back)
        } else {
            Ok(ViewEffect::Continue)
        }
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome {
            title: Some(format!("capture: {}", self.session.title())),
            status: Some(self.session.status().to_string()),
            commands: vec![("any key".to_string(), "Back".to_string())],
            ..crate::chrome::EngineChrome::default()
        }
    }

    fn content(
        &mut self,
        _host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<crate::chrome::ChromeContent> {
        render::capture_content(terminal, self.session.lines(), chrome)
    }
}
