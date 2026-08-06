use crate::discovery::sanitize_text;
use crate::engine::{CommandInvocation, LauncherFocus, LauncherSession, PreparedCommand};
use crate::input::InputDecoder;
use crate::pty::{self, EmbeddedOutcome};
use crate::render;
use crate::runtime_log::{LogLevel, LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub(crate) const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);

pub(crate) struct EngineContext<'a> {
    session: &'a mut LauncherSession,
    decoder: &'a mut InputDecoder,
    runtime_log: &'a mut RuntimeLog,
    active_error: &'a mut Option<LogRecord>,
    active_error_deadline: &'a mut Option<Instant>,
}

impl<'a> EngineContext<'a> {
    pub(crate) fn new(
        session: &'a mut LauncherSession,
        decoder: &'a mut InputDecoder,
        runtime_log: &'a mut RuntimeLog,
        active_error: &'a mut Option<LogRecord>,
        active_error_deadline: &'a mut Option<Instant>,
    ) -> Self {
        Self {
            session,
            decoder,
            runtime_log,
            active_error,
            active_error_deadline,
        }
    }

    fn record_error(&mut self, invocation: &CommandInvocation, message: &str) {
        let record = self.runtime_log.record(
            LogLevel::Error,
            Some(&invocation.source_view),
            Some(&invocation.id),
            message,
        );
        *self.active_error = Some(record);
        *self.active_error_deadline = Some(Instant::now() + ERROR_DISPLAY_DURATION);
    }

    fn record_command_status(
        &mut self,
        invocation: &CommandInvocation,
        status: &str,
        success: bool,
    ) {
        if success {
            *self.active_error = None;
            *self.active_error_deadline = None;
            self.runtime_log.record(
                LogLevel::Info,
                Some(&invocation.source_view),
                Some(&invocation.id),
                status,
            );
        } else {
            self.record_error(invocation, status);
        }
    }

    pub(crate) fn execute_oneshot(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
    ) -> Result<()> {
        self.session.take_focus(LauncherFocus::Command)?;
        let result = (|| {
            terminal.leave()?;
            let result = prepared.process().status();
            terminal.reenter()?;
            Ok::<_, anyhow::Error>(result)
        })();
        self.session.restore_focus();
        let result = result?;
        match result {
            Ok(status) => {
                let message = status_message(&status);
                self.record_command_status(invocation, &message, status.success());
            }
            Err(error) => self.record_error(invocation, &error.to_string()),
        }
        Ok(())
    }

    pub(crate) fn execute_capture(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
        title: &str,
    ) -> Result<()> {
        self.session.take_focus(LauncherFocus::Capture)?;
        let process_result = (|| {
            terminal.leave()?;
            let result = prepared
                .process()
                .stdin(Stdio::null())
                .output()
                .with_context(|| format!("could not run command {}", invocation.id));
            terminal.reenter()?;
            Ok::<_, anyhow::Error>(result)
        })();
        let result = match process_result {
            Ok(result) => result,
            Err(error) => {
                self.session.restore_focus();
                return Err(error);
            }
        };
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
                self.record_error(invocation, &message);
                (message, "failed".to_string())
            }
        };
        let result = self.show_capture(terminal, title, &text, &status);
        self.session.restore_focus();
        result
    }

    pub(crate) fn execute_embedded(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
        title: &str,
    ) -> Result<()> {
        self.session.take_focus(LauncherFocus::Embedded)?;
        let outcome = pty::run(
            &prepared.argv,
            &prepared.environment,
            prepared.current_dir.as_deref(),
            terminal,
            title,
        );
        self.session.restore_focus();
        let outcome = outcome?;
        let message = embedded_status_message(outcome);
        let success = matches!(outcome, EmbeddedOutcome::ReturnedToLauncher)
            || matches!(outcome, EmbeddedOutcome::Exited(0));
        self.record_command_status(invocation, &message, success);
        Ok(())
    }

    pub(crate) fn execute_exit(
        &mut self,
        prepared: PreparedCommand,
        terminal: &mut Terminal,
        invocation: &CommandInvocation,
    ) -> Result<()> {
        self.session.take_focus(LauncherFocus::Command)?;
        terminal.leave()?;
        let status = prepared.process().status();
        match status {
            Ok(status) => {
                let message = status_message(&status);
                self.record_command_status(invocation, &message, status.success());
            }
            Err(error) => self.record_error(invocation, &error.to_string()),
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
            render::render_capture(terminal, title, &lines, status)?;
            let bytes = terminal.read_input(80)?;
            let mut keys = self.decoder.feed(&bytes);
            keys.extend(self.decoder.flush_due());
            if keys.iter().any(|key| {
                matches!(
                    key,
                    crate::engine::Key::CtrlC
                        | crate::engine::Key::Escape
                        | crate::engine::Key::Enter
                        | crate::engine::Key::Char(_)
                        | crate::engine::Key::Alt(_)
                )
            }) {
                return Ok(());
            }
        }
    }
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
