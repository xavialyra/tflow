use crate::command::CommandInvocation;
use crate::config::Config;
use crate::diagnostics::{LogLevel, LogRecord, RuntimeLog};
use crate::input::InputBuffer;
use crate::runtime::RuntimeStore;
use crate::state::StateInstance;
use crate::theme::ResolvedTheme;
use std::time::{Duration, Instant};

pub(crate) const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);

pub(crate) struct EngineHost<'a> {
    pub(crate) config: &'a Config,
    pub(crate) theme: ResolvedTheme,
    pub(crate) input: &'a InputBuffer,
    pub(crate) state: &'a StateInstance,
    pub(crate) runtime: &'a mut RuntimeStore,
    pub(crate) runtime_log: &'a mut RuntimeLog,
    pub(crate) active_error: &'a mut Option<LogRecord>,
    pub(crate) active_error_deadline: &'a mut Option<Instant>,
}

impl<'a> EngineHost<'a> {
    pub(crate) fn record_error(&mut self, invocation: &CommandInvocation, message: &str) {
        self.record_error_message(
            Some(invocation.source_view()),
            Some(invocation.id()),
            message,
        );
    }

    pub(crate) fn record_error_message(
        &mut self,
        source_view: Option<&str>,
        command: Option<&str>,
        message: &str,
    ) {
        let record = self
            .runtime_log
            .record(LogLevel::Error, source_view, command, message);
        *self.active_error = Some(record);
        *self.active_error_deadline = Some(Instant::now() + ERROR_DISPLAY_DURATION);
    }

    pub(crate) fn record_command_status(
        &mut self,
        invocation: &CommandInvocation,
        status: &str,
        success: bool,
    ) {
        if success {
            self.clear_error();
            self.runtime_log.record(
                LogLevel::Info,
                Some(invocation.source_view()),
                Some(invocation.id()),
                status,
            );
        } else {
            self.record_error(invocation, status);
        }
    }

    pub(crate) fn record_view_status(&mut self, view_ref: &str, status: &str, success: bool) {
        if success {
            self.clear_error();
            self.runtime_log
                .record(LogLevel::Info, Some(view_ref), None, status);
        } else {
            self.record_error_message(Some(view_ref), None, status);
        }
    }

    pub(crate) fn clear_error(&mut self) {
        *self.active_error = None;
        *self.active_error_deadline = None;
    }
}
