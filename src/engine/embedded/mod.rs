mod pty;
mod session;

use self::pty::EmbeddedOutcome;
use self::session::EmbeddedSession;
use super::{
    Engine, EngineHost, PreparedProcess, ViewContext, ViewEffect, ViewInstance, evaluate_field,
    evaluate_optional_string, require_field, validate_fields,
};
use crate::config::{ENGINE_EMBEDDED, View};
use crate::expression::Template;
use crate::terminal::Terminal;
use anyhow::{Context, Result};

pub(crate) struct EmbeddedEngine;

impl Engine for EmbeddedEngine {
    fn engine_type(&self) -> &'static str {
        ENGINE_EMBEDDED
    }

    fn validate_config(&self, name: &str, view: &View) -> Result<()> {
        validate_fields(name, view, &["command", "title"])?;
        require_field(name, view, "command")?;
        if let Some(title) = view.engine_field("title")
            && !title.is_str()
        {
            anyhow::bail!("view {:?} embedded title must be a string expression", name);
        }
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
            .unwrap_or_else(|| context.location.view_ref.clone());
        let plugin = context
            .location
            .view_ref
            .split_once(':')
            .map(|(package, _)| package)
            .unwrap_or(&context.location.view_ref)
            .to_string();
        let plugin_root = context
            .config
            .plugin_root(&context.location.view_ref)
            .map(|path| path.to_path_buf());
        let mut environment = vec![
            (
                "LAUNCHER_VIEW_REF".to_string(),
                context.location.view_ref.clone(),
            ),
            ("LAUNCHER_INPUT".to_string(), context.location.input.clone()),
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
        Ok(Box::new(EmbeddedView {
            view_ref: context.location.view_ref.clone(),
            title: title.clone(),
            chrome: None,
            session: Some(EmbeddedSession::new(PreparedProcess {
                argv: command,
                environment,
                current_dir: plugin_root,
            })),
        }))
    }
}

struct EmbeddedView {
    view_ref: String,
    title: String,
    chrome: Option<crate::chrome::ChromeFrame>,
    session: Option<EmbeddedSession>,
}

impl ViewInstance for EmbeddedView {
    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect> {
        let session = self
            .session
            .take()
            .context("embedded view was already completed")?;
        let chrome = self
            .chrome
            .as_ref()
            .context("embedded view was not rendered before it started")?;
        let outcome = session.run(terminal, chrome)?;
        let message = embedded_status_message(outcome);
        let success = matches!(outcome, EmbeddedOutcome::ReturnedToLauncher)
            || matches!(outcome, EmbeddedOutcome::Exited(0));
        host.record_view_status(&self.view_ref, &message, success);
        Ok(ViewEffect::Back)
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome {
            title: Some(format!("embedded: {}", self.title)),
            status: Some("keys pass through".to_string()),
            commands: vec![
                ("Esc".to_string(), "Return".to_string()),
                ("Ctrl-C".to_string(), "Interrupt".to_string()),
            ],
        }
    }

    fn render(
        &mut self,
        _host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<()> {
        self.chrome = Some(chrome.clone());
        terminal.write_output(
            format!(
                "\x1b[2J\x1b[H\x1b[1;36m{}\x1b[0m\x1b[K\x1b[2;1H{}\x1b[K\x1b[{};1H{}\x1b[K",
                chrome.header,
                chrome.input_line(),
                terminal.size().1,
                chrome.footer
            )
            .as_bytes(),
        )
    }
}

fn embedded_status_message(outcome: EmbeddedOutcome) -> String {
    match outcome {
        EmbeddedOutcome::ReturnedToLauncher => "embedded view stopped".to_string(),
        EmbeddedOutcome::Exited(0) => "embedded view finished successfully".to_string(),
        EmbeddedOutcome::Exited(code) => format!("embedded view exited with code {}", code),
        EmbeddedOutcome::Signaled(signal) => {
            format!("embedded view terminated by signal {}", signal)
        }
    }
}
