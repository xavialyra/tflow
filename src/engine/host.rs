use crate::chrome::ShellInput;
use crate::config::Config;
use crate::engine::picker::CommandInvocation;
use crate::engine::runtime::RuntimeStore;
use crate::runtime_log::{LogLevel, LogRecord, RuntimeLog};
use crate::state::StateInstance;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);

pub(crate) struct EngineHost<'a> {
    pub(crate) config: &'a Config,
    pub(crate) input: &'a mut ShellInput,
    pub(crate) state: &'a mut StateInstance,
    pub(crate) runtime: &'a mut RuntimeStore,
    pub(crate) runtime_log: &'a mut RuntimeLog,
    pub(crate) active_error: &'a mut Option<LogRecord>,
    pub(crate) active_error_deadline: &'a mut Option<Instant>,
}

impl<'a> EngineHost<'a> {
    pub(crate) fn log_file(&self) -> Option<&Path> {
        self.runtime_log.path()
    }

    pub(crate) fn sync_query_state(&mut self, view_ref: &str) -> anyhow::Result<bool> {
        let source = self.input.params.clone();
        if self.state.view_ref() != view_ref {
            anyhow::bail!(
                "active state {:?} does not belong to view {:?}",
                self.state.view_ref(),
                view_ref
            );
        }
        match self.config.update_query_input(self.state, &source) {
            Ok(false) => Ok(false),
            Ok(true) => {
                self.input.rejected = false;
                Ok(true)
            }
            Err(error) => {
                self.input.rejected = true;
                self.record_error_message(Some(view_ref), None, &error.to_string());
                Ok(false)
            }
        }
    }

    pub(crate) fn record_error(&mut self, invocation: &CommandInvocation, message: &str) {
        self.record_error_message(Some(&invocation.source_view), Some(&invocation.id), message);
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
                Some(&invocation.source_view),
                Some(&invocation.id),
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
