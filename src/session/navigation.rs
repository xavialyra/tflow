use super::decision::{DecisionPolicy, decision_effect, preflight_engine_decision, staged_runtime};
use super::publication::publish_location;
use super::state::{
    CallBoundary, PreparedHostState, ReturnTransition, ViewFrame, mount_view_input_layers,
    prepared_action_effect,
};
use super::{AppSession, SessionOutcome, initialize_navigation_input, restricted_runtime_snapshot};
use crate::command::{
    CallRequest, CommandOrigin, InputEdit, NavigationMode, NavigationRequest, ViewEffect,
    ViewReturn,
};
#[cfg(test)]
use crate::engine::EngineDecision;
use anyhow::{Context, Result};

pub(super) struct PreparedSessionDeactivate {
    last_index: usize,
}

pub(super) enum PreparedHostEffect {
    Navigation(Box<PreparedNavigation>),
    Return(Box<PreparedReturn>),
    Pop(Box<PreparedPop>),
    Exit(Box<PreparedSessionDeactivate>),
    CompleteReturn {
        returned: Box<ViewReturn>,
        deactivation: Box<PreparedSessionDeactivate>,
    },
    Ready(Box<ViewEffect>),
}

pub(super) enum PreparedHostEffectResult {
    Continue,
    Effect(Box<ViewEffect>),
    Outcome(SessionOutcome),
}

#[derive(Clone, Copy)]
enum PrepareMode {
    Lifecycle,
    Normal,
}

impl PrepareMode {
    fn policy(self) -> DecisionPolicy {
        match self {
            Self::Lifecycle => DecisionPolicy::Lifecycle,
            Self::Normal => DecisionPolicy::Normal,
        }
    }
}

pub(super) struct PreparedDecision {
    pub(super) reports: Vec<crate::engine::EngineNotice>,
    pub(super) effect: Option<ViewEffect>,
    pub(super) publication: Option<crate::engine::ViewContextPublication>,
}

struct PreparedParentEdit {
    input: crate::input::EditorBuffer,
    committed_buffer_projection: crate::input::EditorBuffer,
    state: crate::parameter::ParameterState,
    parameters: crate::parameter::ParameterSnapshot,
    buffer_generation: u64,
}

pub(super) struct PreparedParent {
    pub(super) index: usize,
    pub(super) input: crate::input::EditorBuffer,
    pub(super) parameters: crate::parameter::ParameterSnapshot,
    pub(super) committed_buffer_projection: crate::input::EditorBuffer,
    pub(super) state: crate::parameter::ParameterState,
    pub(super) buffer_generation: u64,
    pub(super) input_dirty: bool,
    pub(super) input_deadline: Option<std::time::Instant>,
    pub(super) clear_pending_command: bool,
    pub(super) reports: Vec<crate::engine::EngineNotice>,
    pub(super) publication: Option<crate::engine::ViewContextPublication>,
    pub(super) work_pending: bool,
}

pub(super) struct PreparedActiveAcceptance {
    source: PreparedParent,
    runtime_value: serde_json::Value,
    effect: Option<ViewEffect>,
}

pub(super) struct PreparedActiveCommit {
    source: PreparedParent,
    runtime_value: serde_json::Value,
    effect: Option<PreparedHostEffect>,
}

pub(super) struct PreparedNavigation {
    source_index: usize,
    mount_id: crate::input::ViewMountId,
    view_ref: String,
    input: crate::input::EditorBuffer,
    state: crate::parameter::ParameterState,
    context: crate::engine::ViewContext,
    publication: Option<crate::engine::ViewContextPublication>,
    parameter_binding: crate::parameter::ParameterBinding,
    definition: crate::config::ViewDefinition,
    engine_definition: crate::engine::EngineDefinition,
    raw_receiver: Option<crate::input::ReceiverId>,
    input_bindings: Vec<crate::command::InputActionBinding>,
    runtime: Box<dyn crate::engine::EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    runtime_value: serde_json::Value,
    staged_initial: super::StagedInitialDecision,
    parent_edit: Option<PreparedParentEdit>,
    boundary: Option<CallBoundary>,
    mode: NavigationMode,
}

pub(super) struct PreparedReturn {
    boundary_index: usize,
    deactivation_count: usize,
    parent: PreparedParent,
    runtime_value: serde_json::Value,
    boundary: CallBoundary,
    returned: ViewReturn,
}

pub(super) struct PreparedPop {
    deactivate_child: bool,
    parent: PreparedParent,
    runtime_value: serde_json::Value,
    edit: Option<InputEdit>,
    restore_input: bool,
}

impl AppSession<'_> {
    pub(super) fn apply_navigation(
        &mut self,
        request: NavigationRequest,
        mode: NavigationMode,
        boundary: Option<CallBoundary>,
        parent_edit: Option<InputEdit>,
    ) -> Result<bool> {
        let target_view_ref = request.view_ref.clone();
        let prepared = match self.prepare_navigation_candidate(request, mode, boundary, parent_edit)
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let message = error.to_string();
                let rejected_route = self.views.last().is_some_and(|entry| entry.input_dirty);
                if rejected_route {
                    anyhow::ensure!(
                        self.deferred_effect.is_none(),
                        "route rejection requires an empty deferred effect slot"
                    );
                    if let Some(effect) = self.reject_input_for(&target_view_ref, &message)? {
                        self.deferred_effect = Some(effect);
                    }
                } else {
                    self.reject_navigation_error(&target_view_ref, &message)?;
                }
                return Ok(false);
            }
        };
        self.commit_navigation(prepared);
        Ok(true)
    }

    fn prepare_navigation_candidate(
        &mut self,
        request: NavigationRequest,
        mode: NavigationMode,
        boundary: Option<CallBoundary>,
        parent_edit: Option<InputEdit>,
    ) -> Result<PreparedNavigation> {
        let source_index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let parent_edit = parent_edit
            .map(|edit| self.prepare_parent_edit_at(source_index, edit))
            .transpose()?;
        let mut staged_runtime = staged_runtime(&self.runtime);
        self.prepare_navigation_at(
            request,
            mode,
            boundary,
            parent_edit,
            source_index,
            &mut staged_runtime,
        )
    }

    fn prepare_navigation_at(
        &mut self,
        request: NavigationRequest,
        mode: NavigationMode,
        boundary: Option<CallBoundary>,
        parent_edit: Option<PreparedParentEdit>,
        source_index: usize,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<PreparedNavigation> {
        if parent_edit.is_some() {
            anyhow::ensure!(
                mode == NavigationMode::Push,
                "a parent input edit requires push navigation"
            );
        }
        anyhow::ensure!(
            source_index < self.views.len(),
            "navigation source view disappeared during preparation"
        );
        let mut state = self.config.instantiate_parameters(&request.view_ref)?;
        let (input, parameter_input) = initialize_navigation_input(
            self.config,
            &mut state,
            request.input.as_ref(),
            request.parameters.as_ref(),
        )?;
        let mount_id = crate::input::ViewMountId(self.next_mount_id);
        publish_location(
            staged_runtime,
            self.config,
            &request.view_ref,
            &input,
            &parameter_input,
            &state,
        )?;
        let prepared = super::prepare_mount(super::MountPreparationInput {
            view_factory: &*self.view_factory,
            config: self.config,
            cancellation: &self.cancellation,
            view_ref: &request.view_ref,
            mount_id,
            input: input.clone(),
            state,
            staged_runtime,
        })?;
        Ok(PreparedNavigation {
            source_index,
            mount_id,
            view_ref: prepared.view_ref,
            input: prepared.input,
            state: prepared.state,
            context: prepared.context,
            publication: prepared.publication,
            parameter_binding: prepared.parameter_binding,
            definition: prepared.definition,
            engine_definition: prepared.engine_definition,
            input_bindings: prepared.input_bindings,
            raw_receiver: prepared.raw_receiver,
            runtime: prepared.runtime,
            renderer: prepared.renderer,
            runtime_value: prepared.runtime_value,
            staged_initial: prepared.staged_initial,
            parent_edit,
            boundary,
            mode,
        })
    }

    fn prepare_parent_edit_at(&self, index: usize, edit: InputEdit) -> Result<PreparedParentEdit> {
        let entry = self
            .views
            .get(index)
            .context("session has no parent view")?;
        let input = match edit {
            InputEdit::SetBuffer { raw, cursor } => entry
                .input
                .replaced_all(raw, cursor)
                .map_err(|error| anyhow::anyhow!("parent input edit was rejected: {error}"))?,
        };
        self.prepare_parent_edit_values(index, &input, &entry.state, entry.buffer_generation)
    }

    fn prepare_parent_edit_values(
        &self,
        index: usize,
        input: &crate::input::EditorBuffer,
        state: &crate::parameter::ParameterState,
        buffer_generation: u64,
    ) -> Result<PreparedParentEdit> {
        let entry = self
            .views
            .get(index)
            .context("session has no parent view")?;
        let mut state = state.clone();
        entry
            .parameter_binding
            .parse_input(&mut state, &input.raw)?;
        state.set_input_rejected(false);
        let next_generation = buffer_generation.wrapping_add(1);
        let parameters = self.config.parameter_snapshot(
            &state,
            crate::input::InputSourceIdentity {
                frame: entry.mount_id,
                generation: next_generation,
            },
        )?;
        Ok(PreparedParentEdit {
            input: input.clone(),
            committed_buffer_projection: input.clone(),
            state,
            parameters,
            buffer_generation: next_generation,
        })
    }

    fn commit_navigation(&mut self, prepared: PreparedNavigation) {
        assert_eq!(
            prepared.source_index + 1,
            self.views.len(),
            "navigation commit source is no longer the active view"
        );
        self.commit_deactivate_at(prepared.source_index);
        self.close_route_completion();
        let input_layers = mount_view_input_layers(
            &mut self.input_router,
            prepared.mount_id,
            prepared.engine_definition.input.strategy,
            prepared.engine_definition.input.buffer_target,
            prepared.raw_receiver,
        );
        let context = prepared
            .context
            .with_publication(prepared.publication.as_ref());
        if let Some(parent) = prepared.parent_edit {
            let runtime_snapshot = restricted_runtime_snapshot(&self.runtime);
            let entry = self
                .views
                .last_mut()
                .expect("navigation commit requires an active parent");
            entry.apply_prepared_host_state(PreparedHostState {
                input: parent.input,
                committed_buffer_projection: parent.committed_buffer_projection,
                state: parent.state,
                buffer_generation: parent.buffer_generation,
                input_dirty: false,
                input_deadline: None,
                parameters: parent.parameters,
                runtime_snapshot,
                publication: None,
            });
            entry.clear_pending_command();
        }
        let transferred_boundary = if prepared.mode == NavigationMode::Replace {
            self.views
                .last_mut()
                .expect("navigation commit requires an active parent")
                .call_boundary
                .take()
        } else {
            None
        };
        if prepared.mode == NavigationMode::Replace {
            let replaced = self
                .views
                .pop()
                .expect("navigation commit requires an active parent");
            self.input_router
                .remove_context(replaced.input_layers.context);
        }
        let call_boundary = prepared.boundary.or(transferred_boundary);
        self.runtime.replace(prepared.runtime_value);
        self.next_mount_id = self.next_mount_id.wrapping_add(1).max(1);
        self.views.push(ViewFrame {
            mount: super::state::ViewMount {
                mount_id: prepared.mount_id,
                context,
                raw_receiver: prepared.raw_receiver,
                buffer_generation: 0,
                parameter_binding: prepared.parameter_binding,
                definition: prepared.definition,
                engine_definition: prepared.engine_definition,
                view_ref: prepared.view_ref,
                committed_buffer_projection: prepared.input.clone(),
                input: prepared.input,
                input_dirty: false,
                input_deadline: None,
                state: prepared.state,
                input_layers,
                input_bindings: prepared.input_bindings,
                published_bindings: None,
                pending_command: None,
                runtime: prepared.runtime,
                renderer: prepared.renderer,
            },
            call_boundary,
        });
        let target_index = self.views.len() - 1;
        let starter = self
            .mount_task_starter(target_index)
            .expect("navigation commit must install the target view");
        let runtime_snapshot = self.runtime.snapshot().clone();
        self.views[target_index]
            .runtime
            .start_prepared_work(&starter, &runtime_snapshot);
        for notice in prepared.staged_initial.reports {
            self.apply_engine_notice(notice);
        }
    }

    pub(super) fn apply_call(&mut self, call: CallRequest) -> Result<()> {
        let suppresses_session_bindings = self
            .current_view_input_context()
            .is_ok_and(|context| context.strategy == crate::input::InputStrategy::RawIntercepted)
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

    pub(super) fn prepare_host_effect_at(
        &mut self,
        index: usize,
        effect: ViewEffect,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<PreparedHostEffect> {
        match effect {
            ViewEffect::Navigate {
                request,
                mode,
                parent_edit,
            } => {
                let parent_edit = parent_edit
                    .map(|edit| self.prepare_parent_edit_at(index, edit))
                    .transpose()?;
                Ok(PreparedHostEffect::Navigation(Box::new(
                    self.prepare_navigation_at(
                        request,
                        mode,
                        None,
                        parent_edit,
                        index,
                        staged_runtime,
                    )?,
                )))
            }
            ViewEffect::Call(call) => {
                let suppresses_session_bindings =
                    self.current_view_input_context().is_ok_and(|context| {
                        context.strategy == crate::input::InputStrategy::RawIntercepted
                    }) || matches!(&call.origin, crate::command::CommandOrigin::Session { .. });
                let boundary = CallBoundary {
                    origin: call.origin,
                    context: call.context,
                    then: call.then,
                    suppresses_session_bindings,
                };
                Ok(PreparedHostEffect::Navigation(Box::new(
                    self.prepare_navigation_at(
                        call.request,
                        NavigationMode::Push,
                        Some(boundary),
                        None,
                        index,
                        staged_runtime,
                    )?,
                )))
            }
            ViewEffect::Return(returned) => {
                let Some(boundary_index) = self
                    .views
                    .get(..=index)
                    .context("return preparation target disappeared")?
                    .iter()
                    .rposition(|entry| entry.call_boundary.is_some())
                else {
                    return Ok(PreparedHostEffect::CompleteReturn {
                        returned: Box::new(returned),
                        deactivation: Box::new(self.prepare_session_deactivate_through(index)?),
                    });
                };
                Ok(PreparedHostEffect::Return(Box::new(
                    self.prepare_return_candidate(returned, boundary_index, index, staged_runtime)?,
                )))
            }
            ViewEffect::Back(edit) => {
                if index < 1 && edit.is_none() {
                    return Ok(PreparedHostEffect::Exit(Box::new(
                        self.prepare_session_deactivate_through(index)?,
                    )));
                }
                Ok(PreparedHostEffect::Pop(Box::new(
                    self.prepare_pop_candidate(index, edit, staged_runtime)?,
                )))
            }
            ViewEffect::Exit => Ok(PreparedHostEffect::Exit(Box::new(
                self.prepare_session_deactivate_through(index)?,
            ))),
            ViewEffect::DispatchCommand(execution) => {
                let effect = prepared_action_effect(crate::command::prepare_command_action(
                    self.config,
                    execution,
                    &self.cancellation,
                )?);
                self.prepare_host_effect_at(index, effect, staged_runtime)
            }
            ViewEffect::EditInput(_) => {
                anyhow::bail!("EditInput requires the active mount preparation path")
            }
            effect @ (ViewEffect::Continue
            | ViewEffect::CopyToClipboard(_)
            | ViewEffect::RunCommand { .. }) => Ok(PreparedHostEffect::Ready(Box::new(effect))),
        }
    }

    pub(super) fn prepare_active_acceptance_at(
        &self,
        index: usize,
        mut source: PreparedParent,
        effect: Option<ViewEffect>,
        staged_runtime: &crate::runtime::RuntimeStore,
    ) -> Result<PreparedActiveAcceptance> {
        anyhow::ensure!(source.index == index, "active acceptance target changed");
        source.parameters = self.config.parameter_snapshot(
            &source.state,
            crate::input::InputSourceIdentity {
                frame: self.views[index].mount_id,
                generation: source.buffer_generation,
            },
        )?;
        Ok(PreparedActiveAcceptance {
            source,
            runtime_value: staged_runtime.snapshot().clone(),
            effect,
        })
    }

    pub(super) fn prepare_active_commit_at(
        &mut self,
        index: usize,
        mut source: PreparedParent,
        mut effect: Option<ViewEffect>,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<PreparedActiveCommit> {
        anyhow::ensure!(source.index == index, "active preparation target changed");
        let mut prepared_effect = None;
        for _ in 0..64 {
            let Some(next) = effect.take() else {
                break;
            };
            match next {
                ViewEffect::DispatchCommand(execution) => {
                    let action = crate::command::prepare_command_action(
                        self.config,
                        execution,
                        &self.cancellation,
                    )?;
                    effect = Some(prepared_action_effect(action));
                }
                ViewEffect::EditInput(edit) => {
                    effect = self.prepare_active_input_edit_at(
                        index,
                        &mut source,
                        edit,
                        staged_runtime,
                    )?;
                }
                ViewEffect::Exit => {
                    prepared_effect = Some(PreparedHostEffect::Exit(Box::new(
                        self.prepare_session_deactivate_through(index)?,
                    )));
                    break;
                }
                ViewEffect::Navigate {
                    request,
                    mode,
                    parent_edit,
                } => {
                    let parent_edit = parent_edit
                        .map(|edit| {
                            let input = match edit {
                                InputEdit::SetBuffer { raw, cursor } => {
                                    source.input.replaced_all(raw, cursor).map_err(|error| {
                                        anyhow::anyhow!("parent input edit was rejected: {error}")
                                    })?
                                }
                            };
                            self.prepare_parent_edit_values(
                                index,
                                &input,
                                &source.state,
                                source.buffer_generation,
                            )
                        })
                        .transpose()?;
                    prepared_effect = Some(PreparedHostEffect::Navigation(Box::new(
                        self.prepare_navigation_at(
                            request,
                            mode,
                            None,
                            parent_edit,
                            index,
                            staged_runtime,
                        )?,
                    )));
                    break;
                }
                next => {
                    prepared_effect =
                        Some(self.prepare_host_effect_at(index, next, staged_runtime)?);
                    break;
                }
            }
        }
        anyhow::ensure!(
            effect.is_none(),
            "command action recursion exceeded 64 prepared effects"
        );

        source.parameters = self.config.parameter_snapshot(
            &source.state,
            crate::input::InputSourceIdentity {
                frame: self.views[index].mount_id,
                generation: source.buffer_generation,
            },
        )?;
        let runtime_value = staged_runtime.snapshot().clone();
        Ok(PreparedActiveCommit {
            source,
            runtime_value,
            effect: prepared_effect,
        })
    }

    fn prepare_active_input_edit_at(
        &mut self,
        index: usize,
        source: &mut PreparedParent,
        edit: InputEdit,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<Option<ViewEffect>> {
        source.input = match edit {
            InputEdit::SetBuffer { raw, cursor } => source
                .input
                .replaced_all(raw, cursor)
                .map_err(|error| anyhow::anyhow!("input edit was rejected: {error}"))?,
        };
        source.buffer_generation = source.buffer_generation.wrapping_add(1);
        source.input_dirty = false;
        source.input_deadline = None;
        source.clear_pending_command = true;
        source.state.set_input_rejected(false);

        let route_available = self.route_input
            && index == 0
            && self.config.default_view.as_deref() == Some(self.views[index].view_ref.as_str())
            && self.views[index].input_focus() == crate::engine::InputFocus::Focused;
        let resolution = route_available.then(|| {
            self.router
                .resolve(&self.views[index].view_ref, &source.input.raw)
        });
        match resolution {
            Some(crate::router::RouteResolution::Navigate {
                target,
                query: parameter_input,
            }) => {
                let parameter_cursor = source
                    .input
                    .cursor
                    .saturating_sub(source.input.raw.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                Ok(Some(ViewEffect::Navigate {
                    request: NavigationRequest::routed(target, parameter_input, parameter_cursor),
                    mode: NavigationMode::Push,
                    parent_edit: Some(InputEdit::SetBuffer {
                        raw: String::new(),
                        cursor: 0,
                    }),
                }))
            }
            Some(crate::router::RouteResolution::Current {
                query: parameter_input,
            }) => {
                let parameter_cursor = source
                    .input
                    .cursor
                    .saturating_sub(source.input.raw.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                source.input.replace_all(parameter_input, parameter_cursor);
                self.prepare_parameter_input_at(index, source, staged_runtime)
            }
            Some(crate::router::RouteResolution::NotMatched) | None => {
                self.prepare_parameter_input_at(index, source, staged_runtime)
            }
        }
    }

    pub(super) fn commit_active_acceptance(
        &mut self,
        prepared: PreparedActiveAcceptance,
    ) -> Result<crate::command::LauncherOutcome> {
        let index = prepared.source.index;
        let mount_id = self.views[index].mount_id;
        let work_pending = prepared.source.work_pending;
        self.runtime.replace(prepared.runtime_value);
        self.commit_parent(prepared.source, false);

        let Some(effect) = prepared.effect else {
            if work_pending {
                self.start_prepared_work_if_active(index, mount_id);
            }
            return Ok(crate::command::LauncherOutcome::Continue);
        };

        let mut staged_runtime = staged_runtime(&self.runtime);
        let mut source = match self.prepared_parent_from_frame(index, false) {
            Ok(source) => source,
            Err(error) => {
                if work_pending {
                    self.start_prepared_work_if_active(index, mount_id);
                }
                return Err(error);
            }
        };
        source.work_pending = work_pending;
        let prepared =
            match self.prepare_active_commit_at(index, source, Some(effect), &mut staged_runtime) {
                Ok(prepared) => prepared,
                Err(error) => {
                    if work_pending {
                        self.start_prepared_work_if_active(index, mount_id);
                    }
                    return Err(error);
                }
            };
        Ok(self.commit_active(prepared))
    }

    pub(super) fn commit_active(
        &mut self,
        prepared: PreparedActiveCommit,
    ) -> crate::command::LauncherOutcome {
        let publish_source_runtime = prepared.effect.as_ref().is_none_or(|effect| {
            matches!(
                effect,
                PreparedHostEffect::Ready(_)
                    | PreparedHostEffect::Exit(_)
                    | PreparedHostEffect::CompleteReturn { .. }
            )
        });
        let start_source_work = prepared
            .effect
            .as_ref()
            .is_none_or(|effect| matches!(effect, PreparedHostEffect::Ready(_)));
        if publish_source_runtime {
            self.runtime.replace(prepared.runtime_value);
        }
        self.commit_parent(prepared.source, start_source_work);
        prepared
            .effect
            .map(|effect| self.consume_prepared_host_effect(effect))
            .unwrap_or(crate::command::LauncherOutcome::Continue)
    }

    pub(super) fn commit_host_effect(
        &mut self,
        effect: PreparedHostEffect,
    ) -> PreparedHostEffectResult {
        match effect {
            PreparedHostEffect::Navigation(prepared) => {
                self.commit_navigation(*prepared);
                PreparedHostEffectResult::Continue
            }
            PreparedHostEffect::Return(prepared) => match self.commit_return(*prepared) {
                ReturnTransition::Effect(effect) => PreparedHostEffectResult::Effect(effect),
                ReturnTransition::Outcome(outcome) => PreparedHostEffectResult::Outcome(outcome),
            },
            PreparedHostEffect::Pop(prepared) => {
                self.commit_pop(*prepared);
                PreparedHostEffectResult::Continue
            }
            PreparedHostEffect::Exit(prepared) => {
                self.commit_session_deactivate(*prepared);
                PreparedHostEffectResult::Outcome(SessionOutcome::Exited)
            }
            PreparedHostEffect::CompleteReturn {
                returned,
                deactivation,
            } => {
                self.commit_session_deactivate(*deactivation);
                PreparedHostEffectResult::Outcome(SessionOutcome::Completed(returned))
            }
            PreparedHostEffect::Ready(effect) => PreparedHostEffectResult::Effect(effect),
        }
    }

    pub(super) fn consume_prepared_host_effect(
        &mut self,
        effect: PreparedHostEffect,
    ) -> crate::command::LauncherOutcome {
        match self.commit_host_effect(effect) {
            PreparedHostEffectResult::Continue => crate::command::LauncherOutcome::Continue,
            PreparedHostEffectResult::Effect(effect) => {
                crate::command::LauncherOutcome::Effect(effect)
            }
            PreparedHostEffectResult::Outcome(outcome) => {
                self.committed_outcome = Some(outcome);
                crate::command::LauncherOutcome::Continue
            }
        }
    }

    pub(super) fn apply_return(&mut self, returned: ViewReturn) -> Result<ReturnTransition> {
        let mut staged_runtime = staged_runtime(&self.runtime);
        let prepared = self.prepare_host_effect_at(
            self.views.len().saturating_sub(1),
            ViewEffect::Return(returned),
            &mut staged_runtime,
        )?;
        match prepared {
            PreparedHostEffect::Return(prepared) => Ok(self.commit_return(*prepared)),
            PreparedHostEffect::CompleteReturn {
                returned,
                deactivation,
            } => {
                self.commit_session_deactivate(*deactivation);
                Ok(ReturnTransition::Outcome(SessionOutcome::Completed(
                    returned,
                )))
            }
            _ => unreachable!("Return preparation produced a non-return effect"),
        }
    }

    pub(super) fn pop_current(
        &mut self,
        edit: Option<InputEdit>,
    ) -> Result<Option<SessionOutcome>> {
        let mut staged_runtime = staged_runtime(&self.runtime);
        let prepared = self.prepare_host_effect_at(
            self.views.len().saturating_sub(1),
            ViewEffect::Back(edit),
            &mut staged_runtime,
        )?;
        match prepared {
            PreparedHostEffect::Pop(prepared) => {
                self.commit_pop(*prepared);
                Ok(self.committed_outcome.take())
            }
            PreparedHostEffect::Exit(prepared) => {
                self.commit_session_deactivate(*prepared);
                Ok(Some(SessionOutcome::Exited))
            }
            _ => unreachable!("Back preparation produced a non-back effect"),
        }
    }

    fn prepare_return_candidate(
        &mut self,
        returned: ViewReturn,
        boundary_index: usize,
        active_index: usize,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<PreparedReturn> {
        anyhow::ensure!(boundary_index > 0, "root View cannot own a call boundary");
        anyhow::ensure!(
            boundary_index <= active_index,
            "return boundary is outside the future active stack"
        );
        let boundary = self
            .views
            .get(boundary_index)
            .and_then(|entry| entry.call_boundary.as_ref())
            .context("call boundary disappeared during Return")?
            .clone();
        let deactivation_count = active_index + 1 - boundary_index;

        let parent_index = boundary_index - 1;
        let parent = self.prepared_parent_from_frame(parent_index, false)?;
        self.prepare_parent_location_at(parent_index, &parent, staged_runtime)?;
        Ok(PreparedReturn {
            boundary_index,
            deactivation_count,
            parent,
            runtime_value: staged_runtime.snapshot().clone(),
            boundary,
            returned,
        })
    }

    fn commit_return(&mut self, prepared: PreparedReturn) -> ReturnTransition {
        for offset in 0..prepared.deactivation_count {
            let index = self.views.len() - 1 - offset;
            self.commit_deactivate_at(index);
        }
        let external_notice = self.acknowledge_external_tick();
        self.close_route_completion();
        self.views
            .get_mut(prepared.boundary_index)
            .and_then(|entry| entry.call_boundary.take())
            .expect("return commit requires the prepared call boundary");
        while self.views.len() > prepared.boundary_index {
            let retired = self
                .views
                .pop()
                .expect("return commit requires a retired view");
            self.input_router
                .remove_context(retired.input_layers.context);
        }

        self.runtime.replace(prepared.runtime_value);
        self.commit_parent(prepared.parent, false);
        if let Some(notice) = external_notice {
            self.apply_engine_notice(notice);
        }

        match self.resume_parent_after_return(prepared.boundary, prepared.returned) {
            Ok(outcome) => self.return_transition(outcome),
            Err(error) => {
                self.report_consumed_transition_error(error);
                ReturnTransition::Effect(Box::new(ViewEffect::Continue))
            }
        }
    }

    fn prepare_pop_candidate(
        &mut self,
        active_index: usize,
        edit: Option<InputEdit>,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<PreparedPop> {
        anyhow::ensure!(
            active_index < self.views.len(),
            "pop preparation target disappeared"
        );
        let has_parent = active_index > 0;
        let parent_index = active_index.saturating_sub(1);
        let cancelled_call = has_parent && self.views[active_index].call_boundary.is_some();
        let restore_input = edit.is_none() || cancelled_call;
        let edit = if cancelled_call { None } else { edit };
        if let Some(edit) = edit.as_ref() {
            let entry = self
                .views
                .get(parent_index)
                .context("session has no parent view")?;
            match edit {
                InputEdit::SetBuffer { raw, cursor } => {
                    entry
                        .input
                        .replaced_all(raw.clone(), *cursor)
                        .map_err(|error| anyhow::anyhow!("input edit was rejected: {error}"))?;
                }
            }
        }
        let parent = self.prepared_parent_from_frame(parent_index, false)?;
        if has_parent {
            self.prepare_parent_location_at(parent_index, &parent, staged_runtime)?;
        }
        Ok(PreparedPop {
            deactivate_child: has_parent,
            parent,
            runtime_value: staged_runtime.snapshot().clone(),
            edit,
            restore_input,
        })
    }

    fn commit_pop(&mut self, prepared: PreparedPop) {
        if prepared.deactivate_child {
            let index = self.views.len() - 1;
            self.commit_deactivate_at(index);
        }
        let external_notice = self.acknowledge_external_tick();
        if prepared.deactivate_child {
            let removed = self
                .views
                .pop()
                .expect("pop commit requires a current view");
            self.input_router
                .remove_context(removed.input_layers.context);
        }
        self.close_route_completion();
        self.runtime.replace(prepared.runtime_value);
        self.commit_parent(prepared.parent, false);
        if let Some(notice) = external_notice {
            self.apply_engine_notice(notice);
        }

        if let Err(error) = self.resume_parent_after_pop(
            prepared.edit,
            prepared.deactivate_child,
            prepared.restore_input,
        ) {
            self.report_consumed_transition_error(error);
        }
    }

    fn commit_deactivate_at(&mut self, index: usize) {
        let entry = self
            .views
            .get_mut(index)
            .expect("deactivate commit requires the prepared view");
        entry.runtime.deactivate();
        entry.clear_pending_command();
    }

    fn prepare_session_deactivate(&self) -> Result<PreparedSessionDeactivate> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        self.prepare_session_deactivate_through(index)
    }

    fn prepare_session_deactivate_through(
        &self,
        last_index: usize,
    ) -> Result<PreparedSessionDeactivate> {
        anyhow::ensure!(
            last_index < self.views.len(),
            "deactivation target disappeared"
        );
        Ok(PreparedSessionDeactivate { last_index })
    }

    fn commit_session_deactivate(&mut self, prepared: PreparedSessionDeactivate) {
        for index in (0..=prepared.last_index).rev() {
            self.commit_deactivate_at(index);
        }
        self.close_route_completion();
    }

    pub(super) fn forced_termination(&mut self) -> SessionOutcome {
        self.discard_pending_external_tick();
        let prepared = self
            .prepare_session_deactivate()
            .expect("forced termination requires a mounted view");
        self.commit_session_deactivate(prepared);
        self.tasks.cancel_all();
        SessionOutcome::Exited
    }

    pub(super) fn exit_session(&mut self) {
        let prepared = self
            .prepare_session_deactivate()
            .expect("Session exit requires a mounted view");
        self.commit_session_deactivate(prepared);
    }

    pub(super) fn prepared_parent_from_frame(
        &self,
        index: usize,
        clear_pending_command: bool,
    ) -> Result<PreparedParent> {
        let entry = self
            .views
            .get(index)
            .context("session has no parent view")?;
        Ok(PreparedParent {
            index,
            input: entry.input.clone(),
            parameters: self
                .config
                .parameter_snapshot(&entry.state, entry.source_identity())?,
            committed_buffer_projection: entry.committed_buffer_projection.clone(),
            state: entry.state.clone(),
            buffer_generation: entry.buffer_generation,
            input_dirty: entry.input_dirty,
            input_deadline: entry.input_deadline,
            clear_pending_command,
            reports: Vec::new(),
            publication: None,
            work_pending: false,
        })
    }

    fn prepare_parent_location_at(
        &self,
        index: usize,
        parent: &PreparedParent,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<()> {
        assert_eq!(parent.index, index);
        publish_location(
            staged_runtime,
            self.config,
            &self.views[index].view_ref,
            &parent.committed_buffer_projection,
            parent.state.raw_input(),
            &parent.state,
        )?;
        Ok(())
    }

    fn resume_parent_after_pop(
        &mut self,
        edit: Option<InputEdit>,
        reactivate_parent: bool,
        restore_input: bool,
    ) -> Result<()> {
        let index = self.views.len() - 1;
        let mount_id = self.views[index].mount_id;
        let mut work_pending = false;
        let result = (|| {
            let effect = if let Some(edit) = edit {
                let route_input_available = self.route_input_available();
                let mut staged_runtime = staged_runtime(&self.runtime);
                let (parent, effect) = self.prepare_pop_input_at(
                    index,
                    edit,
                    route_input_available,
                    &mut staged_runtime,
                )?;
                work_pending |= parent.work_pending;
                self.runtime.replace(staged_runtime.snapshot().clone());
                self.commit_parent(parent, false);
                effect
            } else {
                None
            };

            if reactivate_parent {
                self.commit_parent_lifecycle(false)?;
                work_pending = true;
                if restore_input {
                    self.commit_parent_lifecycle(true)?;
                }
            }
            if let Some(effect) = effect {
                let outcome = self.prepare_and_commit_parent_effect(effect, work_pending)?;
                work_pending = false;
                if let crate::command::LauncherOutcome::Effect(effect) = outcome {
                    anyhow::ensure!(
                        self.deferred_effect.is_none(),
                        "pop commit requires an empty deferred effect slot"
                    );
                    self.deferred_effect = Some(*effect);
                }
            } else if work_pending {
                self.start_prepared_work_if_active(index, mount_id);
                work_pending = false;
            }
            Ok(())
        })();
        if result.is_err() && work_pending {
            self.start_prepared_work_if_active(index, mount_id);
        }
        result
    }

    fn resume_parent_after_return(
        &mut self,
        boundary: CallBoundary,
        returned: ViewReturn,
    ) -> Result<crate::command::LauncherOutcome> {
        let index = self.views.len() - 1;
        let mount_id = self.views[index].mount_id;
        let mut work_pending = false;
        let result = (|| {
            self.commit_parent_lifecycle(false)?;
            work_pending = true;
            self.commit_parent_lifecycle(true)?;

            let CallBoundary {
                origin,
                mut context,
                then,
                suppresses_session_bindings: _,
            } = boundary;
            let Some(then) = then else {
                self.start_prepared_work_if_active(index, mount_id);
                work_pending = false;
                return Ok(crate::command::LauncherOutcome::Continue);
            };
            context.runtime = self.runtime.snapshot().clone();
            let returned_value = crate::command::return_value(&returned);
            let action = crate::command::prepare_continuation(
                self.config,
                &then,
                origin,
                context,
                &returned_value,
                &self.cancellation,
            )?;
            let outcome = self
                .prepare_and_commit_parent_effect(prepared_action_effect(action), work_pending)?;
            work_pending = false;
            Ok(outcome)
        })();
        if result.is_err() && work_pending {
            self.start_prepared_work_if_active(index, mount_id);
        }
        result
    }

    fn commit_parent_lifecycle(&mut self, restore_input: bool) -> Result<()> {
        let index = self.views.len() - 1;
        let mut staged_runtime = staged_runtime(&self.runtime);
        let mut parent = self.prepared_parent_from_frame(index, false)?;
        let context = self.view_context_at(
            index,
            &parent.input,
            &parent.state,
            parent.buffer_generation,
            &staged_runtime,
        )?;
        let prepared =
            self.dispatch_engine_lifecycle_at(index, &mut staged_runtime, |runtime| {
                if restore_input {
                    runtime.restore_input(context)
                } else {
                    runtime.activate(context)
                }
            })?;
        parent.publication = prepared.publication;
        parent.reports = prepared.reports;
        parent.work_pending = true;
        self.runtime.replace(staged_runtime.snapshot().clone());
        self.commit_parent(parent, false);
        Ok(())
    }

    fn prepare_and_commit_parent_effect(
        &mut self,
        effect: ViewEffect,
        work_pending: bool,
    ) -> Result<crate::command::LauncherOutcome> {
        let index = self.views.len() - 1;
        let mut staged_runtime = staged_runtime(&self.runtime);
        let mut parent = self.prepared_parent_from_frame(index, false)?;
        parent.work_pending = work_pending;
        let prepared =
            self.prepare_active_commit_at(index, parent, Some(effect), &mut staged_runtime)?;
        Ok(self.commit_active(prepared))
    }

    fn return_transition(&mut self, outcome: crate::command::LauncherOutcome) -> ReturnTransition {
        match outcome {
            crate::command::LauncherOutcome::Continue => self
                .committed_outcome
                .take()
                .map(ReturnTransition::Outcome)
                .unwrap_or_else(|| ReturnTransition::Effect(Box::new(ViewEffect::Continue))),
            crate::command::LauncherOutcome::Effect(effect) => ReturnTransition::Effect(effect),
        }
    }

    pub(super) fn report_consumed_transition_error(&mut self, error: anyhow::Error) {
        let view_ref = self
            .views
            .last()
            .map(|entry| entry.view_ref.clone())
            .unwrap_or_else(|| "session".to_string());
        self.apply_engine_notice(crate::engine::EngineNotice::Error {
            view_ref,
            message: error.to_string(),
        });
    }

    pub(super) fn view_context_at(
        &self,
        index: usize,
        input: &crate::input::EditorBuffer,
        state: &crate::parameter::ParameterState,
        input_generation: u64,
        staged_runtime: &crate::runtime::RuntimeStore,
    ) -> Result<crate::engine::ViewContext> {
        let entry = self
            .views
            .get(index)
            .context("session has no active view")?;
        let parameters = self.config.parameter_snapshot(
            state,
            crate::input::InputSourceIdentity {
                frame: entry.mount_id,
                generation: input_generation,
            },
        )?;
        debug_assert_eq!(entry.context.view_identity().view_ref, entry.view_ref);
        Ok(entry.context.with_state(
            input.snapshot(),
            input_generation,
            parameters,
            state.input_rejected(),
            restricted_runtime_snapshot(staged_runtime),
        ))
    }

    fn dispatch_engine_at<F>(
        &mut self,
        index: usize,
        staged_runtime: &mut crate::runtime::RuntimeStore,
        mode: PrepareMode,
        dispatch: F,
    ) -> Result<PreparedDecision>
    where
        F: FnOnce(&mut dyn crate::engine::EngineRuntime) -> Result<crate::engine::EngineEmission>,
    {
        let emission = {
            let runtime = &mut self
                .views
                .get_mut(index)
                .context("session has no active view")?
                .runtime;
            dispatch(runtime.as_mut())?
        };
        let decision = emission.decision_ref().clone();
        let preflight = preflight_engine_decision(&decision, staged_runtime, mode.policy())?;
        let effect = match mode {
            PrepareMode::Lifecycle => None,
            PrepareMode::Normal => decision_effect(&decision, &self.views[index].view_ref)?,
        };
        Ok(PreparedDecision {
            reports: preflight.reports,
            effect,
            publication: emission.publication().cloned(),
        })
    }

    fn dispatch_engine_lifecycle_at<F>(
        &mut self,
        index: usize,
        staged_runtime: &mut crate::runtime::RuntimeStore,
        dispatch: F,
    ) -> Result<PreparedDecision>
    where
        F: FnOnce(&mut dyn crate::engine::EngineRuntime) -> Result<crate::engine::EngineEmission>,
    {
        self.dispatch_engine_at(index, staged_runtime, PrepareMode::Lifecycle, dispatch)
    }

    pub(super) fn dispatch_engine_normal_at<F>(
        &mut self,
        index: usize,
        staged_runtime: &mut crate::runtime::RuntimeStore,
        dispatch: F,
    ) -> Result<PreparedDecision>
    where
        F: FnOnce(&mut dyn crate::engine::EngineRuntime) -> Result<crate::engine::EngineEmission>,
    {
        self.dispatch_engine_at(index, staged_runtime, PrepareMode::Normal, dispatch)
    }

    fn commit_parent(&mut self, prepared: PreparedParent, start_work: bool) {
        assert_eq!(
            prepared.index + 1,
            self.views.len(),
            "parent commit requires the prepared parent to be active"
        );
        let parent_index = prepared.index;
        let should_start_work = start_work && prepared.work_pending;
        let context_runtime_snapshot = super::restricted_runtime_snapshot(&self.runtime);
        {
            let entry = self
                .views
                .last_mut()
                .expect("parent commit requires an active view");
            entry.apply_prepared_host_state(PreparedHostState {
                input: prepared.input,
                committed_buffer_projection: prepared.committed_buffer_projection,
                state: prepared.state,
                buffer_generation: prepared.buffer_generation,
                input_dirty: prepared.input_dirty,
                input_deadline: prepared.input_deadline,
                parameters: prepared.parameters,
                runtime_snapshot: context_runtime_snapshot,
                publication: prepared.publication.as_ref(),
            });
            if prepared.clear_pending_command {
                entry.clear_pending_command();
            }
        }
        if should_start_work {
            let starter = self
                .mount_task_starter(parent_index)
                .expect("prepared parent mount disappeared after commit");
            let runtime_snapshot = self.runtime.snapshot().clone();
            self.views[parent_index]
                .runtime
                .start_prepared_work(&starter, &runtime_snapshot);
        }
        for notice in prepared.reports {
            self.apply_engine_notice(notice);
        }
    }

    fn start_prepared_work_if_active(&mut self, index: usize, mount_id: crate::input::ViewMountId) {
        if index + 1 != self.views.len() || self.views[index].mount_id != mount_id {
            return;
        }
        let starter = self
            .mount_task_starter(index)
            .expect("accepted Engine mount disappeared before work start");
        let runtime_snapshot = self.runtime.snapshot().clone();
        self.views[index]
            .runtime
            .start_prepared_work(&starter, &runtime_snapshot);
    }

    fn prepare_pop_input_at(
        &mut self,
        index: usize,
        edit: InputEdit,
        route_input_available: bool,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<(PreparedParent, Option<ViewEffect>)> {
        let (
            view_ref,
            original_input,
            committed_buffer_projection,
            original_state,
            buffer_generation,
            input_deadline,
        ) = {
            let entry = self
                .views
                .get(index)
                .context("session has no parent view")?;
            (
                entry.view_ref.clone(),
                entry.input.clone(),
                entry.committed_buffer_projection.clone(),
                entry.state.clone(),
                entry.buffer_generation,
                entry.input_deadline,
            )
        };
        let input = match edit {
            InputEdit::SetBuffer { raw, cursor } => original_input
                .replaced_all(raw, cursor)
                .map_err(|error| anyhow::anyhow!("input edit was rejected: {error}"))?,
        };
        let parameters = self.config.parameter_snapshot(
            &original_state,
            crate::input::InputSourceIdentity {
                frame: self.views[index].mount_id,
                generation: buffer_generation,
            },
        )?;
        let mut parent = PreparedParent {
            index,
            input,
            parameters,
            committed_buffer_projection,
            state: original_state,
            buffer_generation: buffer_generation.wrapping_add(1),
            input_dirty: false,
            input_deadline,
            clear_pending_command: true,
            reports: Vec::new(),
            publication: None,
            work_pending: false,
        };
        let resolution = if route_input_available {
            Some(self.router.resolve(&view_ref, &parent.input.raw))
        } else {
            None
        };
        match resolution {
            Some(crate::router::RouteResolution::Navigate {
                target,
                query: parameter_input,
            }) => {
                let parameter_cursor = parent
                    .input
                    .cursor
                    .saturating_sub(parent.input.raw.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                Ok((
                    parent,
                    Some(ViewEffect::Navigate {
                        request: NavigationRequest::routed(
                            target,
                            parameter_input,
                            parameter_cursor,
                        ),
                        mode: NavigationMode::Push,
                        parent_edit: Some(InputEdit::SetBuffer {
                            raw: String::new(),
                            cursor: 0,
                        }),
                    }),
                ))
            }
            Some(crate::router::RouteResolution::Current {
                query: parameter_input,
            }) => {
                let parameter_cursor = parent
                    .input
                    .cursor
                    .saturating_sub(parent.input.raw.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                parent.input.replace_all(parameter_input, parameter_cursor);
                let effect = self.prepare_parameter_input_at(index, &mut parent, staged_runtime)?;
                Ok((parent, effect))
            }
            Some(crate::router::RouteResolution::NotMatched) | None => {
                let effect = self.prepare_parameter_input_at(index, &mut parent, staged_runtime)?;
                Ok((parent, effect))
            }
        }
    }

    fn prepare_parameter_input_at(
        &mut self,
        index: usize,
        parent: &mut PreparedParent,
        staged_runtime: &mut crate::runtime::RuntimeStore,
    ) -> Result<Option<ViewEffect>> {
        let binding = self.views[index].parameter_binding.clone();
        let mut candidate_state = parent.state.clone();
        if let Err(error) = binding.parse_input(&mut candidate_state, &parent.input.raw) {
            candidate_state = parent.state.clone();
            candidate_state.set_input_rejected(true);
            parent.state = candidate_state;
            parent.parameters = self.config.parameter_snapshot(
                &parent.state,
                crate::input::InputSourceIdentity {
                    frame: self.views[index].mount_id,
                    generation: parent.buffer_generation,
                },
            )?;
            parent.input_deadline = None;
            let view_ref = self.views[index].view_ref.clone();
            parent.reports.push(crate::engine::EngineNotice::Error {
                view_ref,
                message: error.to_string(),
            });
            let rejected_context = self.view_context_at(
                index,
                &parent.input,
                &parent.state,
                parent.buffer_generation,
                staged_runtime,
            )?;
            let prepared = self.dispatch_engine_normal_at(index, staged_runtime, |runtime| {
                runtime.input_rejected(rejected_context.identity())
            })?;
            if prepared.publication.is_some() {
                parent.publication = prepared.publication;
            }
            parent.reports.extend(prepared.reports);
            parent.work_pending = true;
            return Ok(prepared.effect);
        }

        candidate_state.set_input_rejected(false);
        super::publication::publish_active_input(
            staged_runtime,
            &parent.input,
            candidate_state.raw_input(),
            &candidate_state,
        )?;
        parent.committed_buffer_projection = parent.input.clone();
        parent.state = candidate_state;
        let context = self.view_context_at(
            index,
            &parent.input,
            &parent.state,
            parent.buffer_generation,
            staged_runtime,
        )?;
        let committed = self.dispatch_engine_normal_at(index, staged_runtime, |runtime| {
            runtime.input_committed(context.clone())
        })?;
        if committed.publication.is_some() {
            parent.publication = committed.publication;
        }
        parent.reports.extend(committed.reports);
        parent.work_pending = true;
        if committed.effect.is_some() {
            return Ok(committed.effect);
        }

        let parameters = context.parameter_snapshot().clone();
        parent.parameters = parameters.clone();
        let prepared = self.dispatch_engine_normal_at(index, staged_runtime, |runtime| {
            runtime.parameters(parameters, context.identity())
        })?;
        if prepared.publication.is_some() {
            parent.publication = prepared.publication;
        }
        parent.reports.extend(prepared.reports);
        parent.work_pending = true;
        if prepared.effect.is_some() {
            return Ok(prepared.effect);
        }
        parent.input_deadline = match self.views[index].engine_definition.mount_policy.refresh {
            crate::engine::InputRefreshPolicy::None => None,
            crate::engine::InputRefreshPolicy::Debounced(duration) => {
                Some(std::time::Instant::now() + duration)
            }
        };
        Ok(None)
    }

    #[cfg(test)]
    pub(super) fn apply_lifecycle_decision(&mut self, decision: EngineDecision) -> Result<()> {
        let mut staged_runtime = staged_runtime(&self.runtime);
        let preflight =
            preflight_engine_decision(&decision, &mut staged_runtime, DecisionPolicy::Lifecycle)?;
        if !preflight.runtime_updates.is_empty() {
            self.runtime.set_many(
                preflight
                    .runtime_updates
                    .iter()
                    .map(|update| (update.path.as_str(), update.value.clone())),
            )?;
        }
        for report in preflight.reports {
            self.apply_engine_notice(report);
        }
        Ok(())
    }

    fn reject_navigation_error(&mut self, view_ref: &str, message: &str) -> Result<()> {
        self.apply_engine_notice(crate::engine::EngineNotice::Error {
            view_ref: view_ref.to_string(),
            message: message.to_string(),
        });
        Ok(())
    }
}
