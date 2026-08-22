use super::state::{ReturnTransition, prepared_action_effect};
use super::{AppSession, SessionOutcome};
use crate::command::{CommandInvocation, ViewEffect};
use crate::engine::{EngineHost, InputEdit};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Instant;

impl AppSession<'_> {
    pub(super) fn process_effect(
        &mut self,
        mut effect: ViewEffect,
        terminal: &mut Terminal,
    ) -> Result<Option<SessionOutcome>> {
        for _ in 0..64 {
            effect = match effect {
                ViewEffect::Continue => return Ok(None),
                ViewEffect::CopyToClipboard(value) => {
                    terminal.copy_to_clipboard(&value)?;
                    ViewEffect::Continue
                }
                ViewEffect::Exit => return Ok(Some(SessionOutcome::Exited)),
                ViewEffect::DispatchCommand(execution) => {
                    prepared_action_effect(crate::command::prepare_command_action(
                        self.config,
                        execution,
                        &self.cancellation,
                    )?)
                }
                ViewEffect::RunCommand {
                    invocation,
                    prepared,
                    exit,
                } => {
                    terminal.leave()?;
                    let status = prepared.status(&self.cancellation);
                    if !exit {
                        terminal.reenter()?;
                    }
                    self.record_command_result(&invocation, status);
                    if exit {
                        ViewEffect::Exit
                    } else {
                        ViewEffect::Continue
                    }
                }
                ViewEffect::EditInput(edit) => {
                    self.apply_input_edit(edit)?;
                    self.reconcile_input()?.unwrap_or(ViewEffect::Continue)
                }
                ViewEffect::Navigate {
                    request,
                    mode,
                    parent_edit,
                } => {
                    self.apply_navigation(request, mode, None, parent_edit)?;
                    ViewEffect::Continue
                }
                ViewEffect::Call(call) => {
                    self.apply_call(call)?;
                    ViewEffect::Continue
                }
                ViewEffect::Return(returned) => match self.apply_return(returned)? {
                    ReturnTransition::Effect(effect) => *effect,
                    ReturnTransition::Outcome(outcome) => return Ok(Some(outcome)),
                },
                ViewEffect::Back(edit) => {
                    if let Some(outcome) = self.pop_current(edit)? {
                        return Ok(Some(outcome));
                    }
                    ViewEffect::Continue
                }
            };
        }
        anyhow::bail!("command action recursion exceeded 64 effects")
    }

    fn record_command_result(
        &mut self,
        invocation: &CommandInvocation,
        status: std::io::Result<std::process::ExitStatus>,
    ) {
        let entry = self
            .views
            .last_mut()
            .expect("session has no active view while recording a command result");
        let mut host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        match status {
            Ok(status) => {
                let message = command_status_message(&status);
                host.record_command_status(invocation, &message, status.success());
            }
            Err(error) => host.record_error(invocation, &error.to_string()),
        }
    }

    pub(super) fn delete_backward(&mut self) -> Result<bool> {
        let returns_to_parent = self.views.len() > 1
            && self
                .views
                .last()
                .is_some_and(|entry| entry.input.raw.is_empty() && entry.input.cursor == 0);
        if returns_to_parent {
            self.pop_current(None)?;
            return Ok(false);
        }
        Ok(self
            .views
            .last_mut()
            .context("session has no active view")?
            .input
            .delete_backward())
    }

    pub(super) fn mark_input_changed(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        entry.input_dirty = true;
        entry.input.rejected = false;
        self.active_error = None;
        self.active_error_deadline = None;
        Ok(())
    }

    pub(super) fn dispatch_input_ready(&mut self) -> Result<ViewEffect> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if entry
            .input_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        {
            return Ok(ViewEffect::Continue);
        }
        entry.input_deadline = None;
        let mut host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        entry.instance.input_ready(&mut host)
    }

    pub(super) fn apply_input_edit(&mut self, edit: InputEdit) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        match edit {
            InputEdit::SetBuffer { raw, cursor } => {
                entry.input.raw = raw;
                entry.input.set_cursor(cursor);
            }
        }
        entry.input_dirty = true;
        entry.input.rejected = false;
        self.active_error = None;
        self.active_error_deadline = None;
        Ok(())
    }

    pub(super) fn apply_inactive_parent_edit(
        &mut self,
        edit: InputEdit,
        runtime_snapshot: &Value,
    ) -> Result<()> {
        let (input, input_dirty, input_deadline, state) = {
            let entry = self.views.last().context("session has no parent view")?;
            (
                entry.input.clone(),
                entry.input_dirty,
                entry.input_deadline,
                entry.state.clone(),
            )
        };
        let active_error = self.active_error.clone();
        let active_error_deadline = self.active_error_deadline;
        self.runtime.replace(runtime_snapshot.clone());

        let transaction = (|| {
            self.apply_input_edit(edit)?;
            anyhow::ensure!(
                self.reconcile_input()?.is_none(),
                "a parent input edit produced another navigation"
            );
            let entry = self
                .views
                .last_mut()
                .context("session has no parent view")?;
            anyhow::ensure!(!entry.input.rejected, "the parent input edit was rejected");
            entry.input_deadline = None;
            Ok(())
        })();
        if let Err(error) = transaction {
            let entry = self
                .views
                .last_mut()
                .context("session has no parent view during rollback")?;
            entry.input = input;
            entry.input_dirty = input_dirty;
            entry.input_deadline = input_deadline;
            entry.state = state;
            self.active_error = active_error;
            self.active_error_deadline = active_error_deadline;
            self.runtime.replace(runtime_snapshot.clone());
            let activation = self.activate_current();
            let restoration = self.restore_current_input();
            let mut rollback_errors = Vec::new();
            if let Err(activation_error) = activation {
                rollback_errors.push(format!("activate failed: {activation_error:#}"));
            }
            if let Err(restoration_error) = restoration {
                rollback_errors.push(format!("input restore failed: {restoration_error:#}"));
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(error.context(format!(
                    "could not fully restore the parent View after its input edit failed: {}",
                    rollback_errors.join("; ")
                )))
            };
        }
        Ok(())
    }
}

fn command_status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}
