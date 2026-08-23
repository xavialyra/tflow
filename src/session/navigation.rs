use super::publication::publish_location;
use super::state::{
    CallBoundary, ReturnTransition, ViewEntry, mount_view_input_layers, prepared_action_effect,
};
use super::{AppSession, SessionOutcome, initialize_navigation_input};
use crate::command::{
    CallRequest, CommandOrigin, InputEdit, NavigationMode, NavigationRequest, ViewEffect,
    ViewReturn,
};
use crate::config::{EvaluationSnapshot, InvocationScope, OwnerViewScope, SessionScope};
use crate::engine::{EngineHost, ViewContext};
use anyhow::{Context, Result};

impl AppSession<'_> {
    pub(super) fn apply_navigation(
        &mut self,
        request: NavigationRequest,
        mode: NavigationMode,
        boundary: Option<CallBoundary>,
        parent_edit: Option<InputEdit>,
    ) -> Result<bool> {
        self.close_route_completion();
        let parent_runtime = self.runtime.snapshot().clone();
        self.deactivate_current()?;
        let mut state = self.config.instantiate_state(&request.view_ref)?;
        let input = match initialize_navigation_input(
            self.config,
            &mut state,
            request.input.as_ref(),
            request.query.as_ref(),
        ) {
            Ok(input) => input,
            Err(error) => {
                self.activate_current()?;
                self.reject_navigation_error(&request.view_ref, &error.to_string())?;
                self.restore_current_input()?;
                return Ok(false);
            }
        };
        publish_location(
            &mut self.runtime,
            self.config,
            &request.view_ref,
            &input,
            &state,
        )?;
        let runtime_snapshot = self.runtime.snapshot().clone();
        let evaluation = EvaluationSnapshot::new(
            InvocationScope::new(&self.config.input_value),
            SessionScope::new(&runtime_snapshot),
            Some(OwnerViewScope::new(&state)),
            Some(&self.cancellation),
        );
        let view = self.view_factory.create_view(ViewContext {
            config: self.config,
            request: &request,
            input: &input,
            state: &state,
            evaluation,
            tasks: self.tasks.clone(),
            cancellation: self.cancellation.clone(),
        });
        let view = match view {
            Ok(view) => view,
            Err(error) => {
                self.activate_current()?;
                self.reject_navigation_error(&request.view_ref, &error.to_string())?;
                self.restore_current_input()?;
                return Ok(false);
            }
        };
        if let Some(edit) = parent_edit {
            anyhow::ensure!(
                mode == NavigationMode::Push,
                "a parent input edit requires push navigation"
            );
            self.apply_inactive_parent_edit(edit, &parent_runtime)?;
        }
        let transferred_boundary = if mode == NavigationMode::Replace {
            self.views
                .last_mut()
                .context("session has no active view")?
                .call_boundary
                .take()
        } else {
            None
        };
        if mode == NavigationMode::Replace {
            let replaced = self.views.pop().context("session has no active view")?;
            self.input_router
                .remove_context(replaced.input_layers.context);
        }
        let input_layers = mount_view_input_layers(&mut self.input_router);
        self.views.push(ViewEntry {
            view_ref: request.view_ref,
            input,
            input_dirty: false,
            input_deadline: None,
            state,
            input_layers,
            published_bindings: None,
            call_boundary: boundary.or(transferred_boundary),
            instance: view,
        });
        self.publish_current_location()?;
        Ok(true)
    }

    pub(super) fn apply_call(&mut self, call: CallRequest) -> Result<()> {
        let suppresses_session_bindings = self
            .current_view_input_context()
            .is_ok_and(|context| context.grammar == super::state::InputGrammar::Raw)
            || matches!(&call.origin, CommandOrigin::Session { .. });
        let boundary = CallBoundary {
            origin: call.origin,
            context: call.context,
            then: call.then,
            suppresses_session_bindings,
        };
        self.apply_navigation(call.request, NavigationMode::Push, Some(boundary), None)
            .map(|_| ())
    }

    pub(super) fn apply_return(&mut self, returned: ViewReturn) -> Result<ReturnTransition> {
        self.close_route_completion();
        let Some(boundary_index) = self
            .views
            .iter()
            .rposition(|entry| entry.call_boundary.is_some())
        else {
            return Ok(ReturnTransition::Outcome(SessionOutcome::Completed(
                Box::new(returned),
            )));
        };
        anyhow::ensure!(boundary_index > 0, "root View cannot own a call boundary");
        self.deactivate_current()?;
        let boundary = self.views[boundary_index]
            .call_boundary
            .take()
            .context("call boundary disappeared during Return")?;
        let retired_contexts = self.views[boundary_index..]
            .iter()
            .map(|entry| entry.input_layers.context)
            .collect::<Vec<_>>();
        self.views.truncate(boundary_index);
        for context in retired_contexts {
            self.input_router.remove_context(context);
        }
        self.activate_current()?;
        self.restore_current_input()?;

        let Some(then) = boundary.then else {
            return Ok(ReturnTransition::Effect(Box::new(ViewEffect::Continue)));
        };
        let mut context = boundary.context;
        context.runtime = self.runtime.snapshot().clone();
        let returned_value = crate::command::return_value(&returned);
        let cancellation = self.cancellation.clone();
        let action = crate::command::prepare_continuation(
            self.config,
            &then,
            boundary.origin,
            context,
            &returned_value,
            &cancellation,
        )?;
        Ok(ReturnTransition::Effect(Box::new(prepared_action_effect(
            action,
        ))))
    }

    pub(super) fn pop_current(
        &mut self,
        edit: Option<InputEdit>,
    ) -> Result<Option<SessionOutcome>> {
        self.close_route_completion();
        if self.views.len() <= 1 {
            let Some(edit) = edit else {
                return Ok(Some(SessionOutcome::Exited));
            };
            self.apply_input_edit(edit)?;
            if let Some(ViewEffect::Navigate {
                request,
                mode,
                parent_edit,
            }) = self.reconcile_input()?
            {
                self.apply_navigation(request, mode, None, parent_edit)?;
            }
            return Ok(None);
        }

        let current = self.views.last().context("session has no active view")?;
        let cancelled_call = current.call_boundary.is_some();
        self.deactivate_current()?;
        let removed = self.views.pop().context("session has no active view")?;
        self.input_router
            .remove_context(removed.input_layers.context);
        if cancelled_call {
            self.activate_current()?;
            self.restore_current_input()?;
            return Ok(None);
        }
        let Some(edit) = edit else {
            self.activate_current()?;
            self.restore_current_input()?;
            return Ok(None);
        };

        self.apply_input_edit(edit)?;
        self.publish_current_location()?;
        let effect = self.reconcile_input()?;
        self.activate_current()?;
        if let Some(ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        }) = effect
        {
            self.apply_navigation(request, mode, None, parent_edit)?;
        }
        Ok(None)
    }

    pub(super) fn restore_current_input(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
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
        entry.instance.restore_input(&mut host)
    }

    fn deactivate_current(&mut self) -> Result<()> {
        self.views
            .last_mut()
            .context("session has no active view")?
            .instance
            .deactivate()
    }

    pub(super) fn publish_current_location(&mut self) -> Result<()> {
        let entry = self.views.last().context("session has no active view")?;
        publish_location(
            &mut self.runtime,
            self.config,
            &entry.view_ref,
            &entry.input,
            &entry.state,
        )
    }

    pub(super) fn activate_current(&mut self) -> Result<()> {
        self.publish_current_location()?;
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
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
        entry.instance.activate(&mut host)
    }

    fn reject_navigation_error(&mut self, view_ref: &str, message: &str) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view while reporting navigation error")?;
        entry.input.rejected = true;
        entry.input_deadline = None;
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
        host.record_error_message(Some(view_ref), None, message);
        entry.instance.input_rejected(&mut host)
    }
}
