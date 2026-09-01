use super::decision::staged_runtime;
use super::state::{ReturnTransition, prepared_action_effect};
use super::{AppSession, SessionOutcome};
use crate::command::{CommandInvocation, LauncherOutcome, ViewEffect};
use crate::engine::InputEdit;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::time::Instant;

pub(super) struct PreparedInputReady {
    prepared: crate::session::navigation::PreparedActiveCommit,
}

struct PreparedParameterPatch {
    index: usize,
    mount_id: crate::input::ViewMountId,
    prepared: crate::session::navigation::PreparedActiveCommit,
}

impl AppSession<'_> {
    pub(super) fn process_effect(
        &mut self,
        effect: ViewEffect,
        terminal: &mut Terminal,
    ) -> Result<Option<SessionOutcome>> {
        let mut consumed_transition = false;
        match self.process_effect_inner(effect, terminal, &mut consumed_transition) {
            Err(error) if consumed_transition && !self.cancellation.is_cancelled() => {
                self.report_consumed_transition_error(error);
                Ok(None)
            }
            result => result,
        }
    }

    fn process_effect_inner(
        &mut self,
        mut effect: ViewEffect,
        terminal: &mut Terminal,
        consumed_transition: &mut bool,
    ) -> Result<Option<SessionOutcome>> {
        for _ in 0..64 {
            if let Some(outcome) = self.committed_outcome.take() {
                return Ok(Some(outcome));
            }
            effect = match effect {
                ViewEffect::Continue => match self.deferred_effect.take() {
                    Some(effect) => effect,
                    None => return Ok(None),
                },
                ViewEffect::CopyToClipboard(value) => {
                    terminal.copy_to_clipboard(&value)?;
                    ViewEffect::Continue
                }
                ViewEffect::Exit => {
                    self.exit_session();
                    return Ok(Some(SessionOutcome::Exited));
                }
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
                ViewEffect::Return(returned) => {
                    let consumes_boundary =
                        self.views.iter().any(|entry| entry.call_boundary.is_some());
                    let transition = self.apply_return(returned)?;
                    *consumed_transition |= consumes_boundary;
                    match transition {
                        ReturnTransition::Effect(effect) => *effect,
                        ReturnTransition::Outcome(outcome) => return Ok(Some(outcome)),
                    }
                }
                ViewEffect::Back(edit) => {
                    let consumes_child = self.views.len() > 1;
                    if let Some(outcome) = self.pop_current(edit)? {
                        return Ok(Some(outcome));
                    }
                    *consumed_transition |= consumes_child;
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
        let view_ref = self
            .views
            .last()
            .expect("session has no active view while recording a command result")
            .view_ref
            .clone();
        match status {
            Ok(status) => {
                let message = command_status_message(&status);
                if status.success() {
                    self.active_error = None;
                    self.active_error_deadline = None;
                    self.runtime_log.record(
                        crate::diagnostics::LogLevel::Info,
                        Some(invocation.source_view()),
                        Some(invocation.id()),
                        &message,
                    );
                } else {
                    self.apply_engine_notice(crate::engine::EngineNotice::Error {
                        view_ref: invocation.source_view().to_string(),
                        message,
                    });
                }
            }
            Err(error) => self.apply_engine_notice(crate::engine::EngineNotice::Error {
                view_ref,
                message: error.to_string(),
            }),
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
        self.views
            .last_mut()
            .context("session has no active view")?
            .mark_input_changed();
        self.clear_input_error();
        Ok(())
    }

    pub(super) fn clear_input_error(&mut self) {
        self.active_error = None;
        self.active_error_deadline = None;
    }

    pub(super) fn dispatch_input_ready(&mut self) -> Result<Option<PreparedInputReady>> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        {
            let entry = &self.views[index];
            if entry
                .input_deadline
                .is_none_or(|deadline| Instant::now() < deadline)
            {
                return Ok(None);
            }
        }
        let context = self.current_view_context()?;
        let mut staged_runtime = staged_runtime(&self.runtime);
        let prepared = self.dispatch_engine_normal_at(index, &mut staged_runtime, |runtime| {
            runtime.input_ready(context)
        })?;
        let mut source = self.prepared_parent_from_frame(index, false)?;
        source.input_deadline = None;
        source.publication = prepared.publication;
        source.reports = prepared.reports;
        source.work_pending = true;
        let prepared =
            self.prepare_active_commit_at(index, source, prepared.effect, &mut staged_runtime)?;
        Ok(Some(PreparedInputReady { prepared }))
    }

    pub(super) fn commit_input_ready(
        &mut self,
        prepared: PreparedInputReady,
    ) -> Result<LauncherOutcome> {
        Ok(self.commit_active(prepared.prepared))
    }

    pub(super) fn apply_input_edit(&mut self, edit: InputEdit) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        match edit {
            InputEdit::SetBuffer { raw, cursor } => entry.replace_input(raw, cursor)?,
        }
        self.clear_input_error();
        Ok(())
    }

    pub(super) fn apply_parameter_patch_request(
        &mut self,
        request: crate::parameter::ParameterPatchRequest,
    ) -> Result<LauncherOutcome> {
        let prepared = self.prepare_parameter_patch_request(request)?;
        self.commit_parameter_patch(prepared)
    }

    fn prepare_parameter_patch_request(
        &mut self,
        request: crate::parameter::ParameterPatchRequest,
    ) -> Result<PreparedParameterPatch> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let entry = &self.views[index];
        let target = entry.mount_id;
        anyhow::ensure!(
            request.target == target,
            "parameter patch targets mount {:?}, active mount is {:?}",
            request.target,
            target
        );

        let mut state = entry.state.clone();
        let mut input = entry.input.clone();
        let mut committed_buffer_projection = entry.committed_buffer_projection.clone();
        let mut buffer_generation = entry.buffer_generation;
        let mut input_dirty = entry.input_dirty;
        let input_deadline = entry.input_deadline;
        let policy = request.input_policy;
        self.config.apply_parameter_patch(&mut state, &request)?;
        let rendered = (policy == crate::parameter::ParameterInputPolicy::Rerender)
            .then(|| self.config.render_parameter_input(&state))
            .transpose()?;

        let published_input = match policy {
            crate::parameter::ParameterInputPolicy::Preserve => {
                // Preserve publishes the last committed buffer while applying the typed patch.
                committed_buffer_projection.clone()
            }
            crate::parameter::ParameterInputPolicy::Rerender => {
                let raw = rendered.expect("Rerender must render parameter input");
                let cursor = input.cursor.min(raw.len());
                input.replace_all(raw, cursor);
                buffer_generation = buffer_generation.wrapping_add(1);
                input_dirty = false;
                state.set_input_rejected(false);
                committed_buffer_projection = input.clone();
                input.clone()
            }
        };

        let mut staged_runtime = super::decision::staged_runtime(&self.runtime);
        super::publication::publish_active_input(
            &mut staged_runtime,
            &published_input,
            state.raw_input(),
            &state,
        )?;
        let parameters = self.config.parameter_snapshot(
            &state,
            crate::input::InputSourceIdentity {
                frame: target,
                generation: buffer_generation,
            },
        )?;

        let context =
            self.view_context_at(index, &input, &state, buffer_generation, &staged_runtime)?;
        let prepared = self.dispatch_engine_normal_at(index, &mut staged_runtime, |runtime| {
            runtime.parameters(parameters, context.identity())
        })?;
        let mut source = self.prepared_parent_from_frame(index, true)?;
        source.input = input;
        source.committed_buffer_projection = committed_buffer_projection;
        source.state = state;
        source.buffer_generation = buffer_generation;
        source.input_dirty = input_dirty;
        source.input_deadline = input_deadline;
        source.reports = prepared.reports;
        source.publication = prepared.publication;
        source.work_pending = true;
        let prepared =
            self.prepare_active_commit_at(index, source, prepared.effect, &mut staged_runtime)?;

        Ok(PreparedParameterPatch {
            index,
            mount_id: target,
            prepared,
        })
    }

    fn commit_parameter_patch(
        &mut self,
        prepared: PreparedParameterPatch,
    ) -> Result<LauncherOutcome> {
        assert_eq!(
            prepared.index + 1,
            self.views.len(),
            "parameter patch commit requires the prepared mount to remain active"
        );
        assert_eq!(
            self.views[prepared.index].mount_id, prepared.mount_id,
            "parameter patch commit target changed after preparation"
        );

        Ok(self.commit_active(prepared.prepared))
    }
}

fn command_status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}
