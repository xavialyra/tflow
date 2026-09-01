mod chrome;
mod completion;
mod decision;
mod effects;
mod input_dispatch;
mod navigation;
#[cfg(test)]
use chrome::render_route_completion;
#[cfg(test)]
use completion::{CompletionCandidate, CompletionRuntime, route_completion_edit};
#[cfg(test)]
use input_dispatch::apply_editor_key;
pub(crate) mod input;
mod publication;
mod state;

use publication::{publish_location, publish_view_catalog};
use state::{
    CompletionState, RegisteredBinding, ViewFrame, ViewMount, mount_view_input_layers,
    validate_engine_input_bindings,
};
#[cfg(test)]
use state::{InputBinding, ReturnTransition, RouteAction};

use self::input::InputPipeline;
#[cfg(test)]
use crate::command::NavigationMode;
#[cfg(test)]
use crate::command::{CallRequest, CommandOrigin};
#[cfg(test)]
use crate::command::{CommandContext, LauncherOutcome};
use crate::command::{InputSeed, NavigationRequest, ViewEffect, ViewReturn};
use crate::config::{Config, EvaluationSnapshot, InvocationScope, OwnerViewScope, SessionScope};
use crate::diagnostics::{LogRecord, RuntimeLog};
#[cfg(test)]
use crate::engine::EngineRegistry;
#[cfg(test)]
use crate::engine::InputEdit;
use crate::engine::{EngineDecision, EngineRuntime, ViewContext, ViewFactory};
#[cfg(test)]
use crate::input::DecodedInput;
use crate::input::EditorBuffer;
#[cfg(test)]
use crate::input::Key;
use crate::input::keymap::InstructionTable;
use crate::lifecycle::CancellationToken;
use crate::parameter::ParameterState;
use crate::task::TaskRuntime;
use crate::terminal::Terminal;
use crate::terminal::sanitize_terminal_text;
use crate::theme::ResolvedTheme;
use anyhow::{Context, Result};
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const ERROR_DISPLAY_DURATION: Duration = Duration::from_secs(5);

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    theme: ResolvedTheme,
    view_factory: Box<dyn ViewFactory>,
    views: Vec<ViewFrame>,
    tasks: TaskRuntime,
    runtime: crate::runtime::RuntimeStore,
    runtime_log: RuntimeLog,
    router: Arc<crate::router::Router>,
    route_input: bool,
    route_completion: Option<CompletionState>,
    input_pipeline: InputPipeline,
    input_router: InstructionTable<RegisteredBinding>,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
    runtime_warning: Option<String>,
    deferred_effect: Option<ViewEffect>,
    committed_outcome: Option<SessionOutcome>,
    pending_external_tick: Option<PendingExternalTick>,
    cancellation: CancellationToken,
    next_mount_id: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(Box<ViewReturn>),
}

struct PendingExternalTick {
    mount_id: crate::input::ViewMountId,
    action: crate::engine::ExternalTickAction,
    notice: Option<crate::engine::EngineNotice>,
}

pub(super) struct StagedInitialDecision {
    pub(super) reports: Vec<crate::engine::EngineNotice>,
    #[cfg(test)]
    pub(super) runtime_updates: Vec<crate::engine::RuntimeUpdate>,
}

pub(super) struct FactoryContextInput<'a> {
    pub(super) config: &'a Config,
    pub(super) cancellation: &'a CancellationToken,
    pub(super) view_ref: &'a str,
    pub(super) mount_id: crate::input::ViewMountId,
    pub(super) state: &'a ParameterState,
    pub(super) engine_definition: &'a crate::engine::EngineDefinition,
    pub(super) runtime_snapshot: &'a serde_json::Value,
    pub(super) mount_data: Option<&'a crate::engine::MountRuntimeData>,
}

pub(super) fn factory_contexts(
    input: FactoryContextInput<'_>,
) -> Result<(
    crate::engine::RuntimeFactoryContext,
    crate::engine::RendererFactoryContext,
    crate::engine::InputBindingFactoryContext,
)> {
    let FactoryContextInput {
        config,
        cancellation,
        view_ref,
        mount_id,
        state,
        engine_definition,
        runtime_snapshot,
        mount_data,
    } = input;
    let identity = crate::engine::ViewIdentity::new(view_ref, engine_definition.kind);
    let parameters = config.parameter_snapshot(state, state_source_identity(mount_id))?;

    let evaluation = EvaluationSnapshot::new(
        InvocationScope::new(&config.input_value),
        SessionScope::new(runtime_snapshot),
        Some(OwnerViewScope::new(view_ref, &parameters)),
        Some(cancellation),
    );
    let runtime_config =
        crate::engine::evaluate_engine_config(config, view_ref, engine_definition, &evaluation)?;
    let binding_config =
        crate::engine::evaluate_binding_config(config, view_ref, engine_definition, &evaluation)?;

    Ok((
        crate::engine::RuntimeFactoryContext {
            identity: identity.clone(),
            config: runtime_config,
            parameters,
            cancellation: cancellation.observer(),
            data: mount_data.cloned(),
        },
        crate::engine::RendererFactoryContext {
            identity: crate::engine::RendererIdentity {
                view_ref: identity.view_ref.clone(),
                engine_type: identity.engine_type.clone(),
                renderer_type: engine_definition.renderer.to_string(),
            },
        },
        crate::engine::InputBindingFactoryContext {
            identity,
            bindings: binding_config,
        },
    ))
}

pub(super) struct PreparedMount {
    pub(super) view_ref: String,
    pub(super) input: EditorBuffer,
    pub(super) state: ParameterState,
    pub(super) context: crate::engine::ViewContext,
    pub(super) publication: Option<crate::engine::ViewContextPublication>,
    pub(super) parameter_binding: crate::parameter::ParameterBinding,
    pub(super) definition: crate::config::ViewDefinition,
    pub(super) engine_definition: crate::engine::EngineDefinition,
    pub(super) input_bindings: Vec<crate::command::InputActionBinding>,
    pub(super) raw_receiver: Option<crate::input::ReceiverId>,
    pub(super) runtime: Box<dyn EngineRuntime>,
    pub(super) renderer: Box<dyn crate::engine::ViewRenderer>,
    pub(super) staged_initial: StagedInitialDecision,
    pub(super) runtime_value: serde_json::Value,
}

pub(super) struct MountPreparationInput<'a> {
    pub(super) view_factory: &'a dyn ViewFactory,
    pub(super) config: &'a Config,
    pub(super) cancellation: &'a CancellationToken,
    pub(super) view_ref: &'a str,
    pub(super) mount_id: crate::input::ViewMountId,
    pub(super) input: EditorBuffer,
    pub(super) state: ParameterState,
    pub(super) staged_runtime: &'a mut crate::runtime::RuntimeStore,
}

pub(super) fn prepare_mount(input: MountPreparationInput<'_>) -> Result<PreparedMount> {
    let MountPreparationInput {
        view_factory,
        config,
        cancellation,
        view_ref,
        mount_id,
        input,
        state,
        staged_runtime,
    } = input;
    let engine_definition = view_factory.definition(config, view_ref)?;
    let identity = crate::engine::ViewIdentity::new(view_ref, engine_definition.kind);
    let mount_data = view_factory.create_mount_data(
        config,
        &identity,
        crate::task::MountTaskLease::new(mount_id),
    )?;
    let (runtime_context, renderer_context, binding_context) =
        factory_contexts(FactoryContextInput {
            config,
            cancellation,
            view_ref,
            mount_id,
            state: &state,
            engine_definition: &engine_definition,
            runtime_snapshot: staged_runtime.snapshot(),
            mount_data: mount_data.as_ref(),
        })?;
    let input_bindings = view_factory.create_input_bindings(binding_context)?;
    validate_engine_input_bindings(&input_bindings, &engine_definition)?;
    let parameters = runtime_context.parameters.clone();
    let preparation_context =
        crate::engine::ViewContext::from_parts(crate::engine::ViewContextParts {
            mount_id,
            identity,
            input: input.snapshot(),
            input_generation: 0,
            parameters: parameters.clone(),
            input_rejected: state.input_rejected(),
            runtime: restricted_runtime_snapshot(staged_runtime),
            current: serde_json::Value::Null,
            revision: 0,
        });
    let mut runtime = view_factory.create_view(runtime_context)?;
    let emission = runtime.parameters(parameters.clone(), preparation_context.identity())?;
    let publication = emission.publication().cloned();
    let decision = emission.decision_ref().clone();
    let staged_initial = stage_initial_engine_decision(staged_runtime, decision)?;
    let renderer = view_factory.create_renderer(renderer_context)?;
    renderer.validate_model(&runtime.render_model())?;
    let raw_receiver = discover_raw_receiver_after_parameters(
        &mut *runtime,
        mount_id,
        engine_definition.input.strategy,
    )?;
    let parameter_binding = config.parameter_binding(view_ref)?;
    let definition = config
        .view(view_ref)
        .with_context(|| format!("View {view_ref} disappeared during mount"))?
        .clone();
    let context = preparation_context.with_state(
        input.snapshot(),
        0,
        parameters,
        state.input_rejected(),
        restricted_runtime_snapshot(staged_runtime),
    );
    Ok(PreparedMount {
        view_ref: view_ref.to_string(),
        input,
        state,
        context,
        publication,
        parameter_binding,
        definition,
        engine_definition,
        input_bindings,
        raw_receiver,
        runtime,
        renderer,
        staged_initial,
        runtime_value: staged_runtime.snapshot().clone(),
    })
}

fn state_source_identity(mount_id: crate::input::ViewMountId) -> crate::input::InputSourceIdentity {
    crate::input::InputSourceIdentity {
        frame: mount_id,
        generation: 0,
    }
}

pub(super) fn restricted_runtime_snapshot(
    runtime: &crate::runtime::RuntimeStore,
) -> crate::engine::EngineRuntimeSnapshot {
    crate::engine::EngineRuntimeSnapshot::new(
        runtime
            .snapshot()
            .pointer("/view/current")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
}

/// Receiver discovery is part of mount construction and happens only after the
/// initial Parameters dispatch has initialized the runtime capability.
fn discover_raw_receiver_after_parameters(
    runtime: &mut dyn EngineRuntime,
    mount_id: crate::input::ViewMountId,
    strategy: crate::input::InputStrategy,
) -> Result<Option<crate::input::ReceiverId>> {
    let receiver = runtime
        .raw_receiver()
        .is_some()
        .then_some(crate::input::ReceiverId(mount_id.0));
    anyhow::ensure!(
        strategy != crate::input::InputStrategy::RawIntercepted || receiver.is_some(),
        "raw-intercepted Engine mount did not provide a raw input receiver after Parameters"
    );
    Ok(receiver)
}

impl StagedInitialDecision {
    pub(super) fn into_decision(self) -> EngineDecision {
        let reports = self
            .reports
            .into_iter()
            .map(EngineDecision::Report)
            .collect::<Vec<_>>();
        match reports.len() {
            0 => EngineDecision::Continue,
            1 => reports
                .into_iter()
                .next()
                .expect("one report was collected"),
            _ => EngineDecision::Batch(reports),
        }
    }
}

pub(super) fn stage_initial_engine_decision(
    runtime: &mut crate::runtime::RuntimeStore,
    decision: EngineDecision,
) -> anyhow::Result<StagedInitialDecision> {
    let staged =
        decision::preflight_engine_decision(&decision, runtime, decision::DecisionPolicy::Initial)?;
    Ok(StagedInitialDecision {
        reports: staged.reports,
        #[cfg(test)]
        runtime_updates: staged.runtime_updates,
    })
}

struct RootAssemblyInput<'a> {
    config: &'a Config,
    theme: ResolvedTheme,
    runtime_log: RuntimeLog,
    view_factory: Box<dyn ViewFactory>,
    view_ref: String,
    state: ParameterState,
    initial_input: String,
    route_input: bool,
    cancellation: CancellationToken,
}

impl<'a> AppSession<'a> {
    #[cfg(test)]
    pub(crate) fn new<V>(config: &'a Config, runtime_log: RuntimeLog, engines: V) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let cancellation = CancellationToken::new();
        Self::new_with_theme(
            config,
            ResolvedTheme::terminal(),
            runtime_log,
            engines,
            &cancellation,
        )
    }

    fn assemble_root_session(input: RootAssemblyInput<'a>) -> Result<Self> {
        let RootAssemblyInput {
            config,
            theme,
            runtime_log,
            view_factory,
            view_ref,
            state,
            initial_input,
            route_input,
            cancellation,
        } = input;
        let mut runtime = crate::runtime::RuntimeStore::new();
        let tasks = TaskRuntime::new();
        let router = Arc::new(crate::router::Router::new(config));
        let request = NavigationRequest::new(&view_ref, initial_input);
        let input_seed = request
            .input
            .as_ref()
            .context("root navigation request has no input seed")?;
        let input = input_buffer_from_seed(input_seed);
        publish_location(
            &mut runtime,
            config,
            &view_ref,
            &input,
            &input_seed.params,
            &state,
        )?;
        publish_view_catalog(&mut runtime, config)?;
        let mount_id = crate::input::ViewMountId(1);
        let mut staged_runtime = crate::runtime::RuntimeStore::new();
        staged_runtime.replace(runtime.snapshot().clone());
        let prepared = prepare_mount(MountPreparationInput {
            view_factory: &*view_factory,
            config,
            cancellation: &cancellation,
            view_ref: &view_ref,
            mount_id,
            input: input.clone(),
            state,
            staged_runtime: &mut staged_runtime,
        })?;
        runtime.replace(prepared.runtime_value.clone());
        let context = prepared
            .context
            .with_publication(prepared.publication.as_ref());
        let initial_decision = prepared.staged_initial.into_decision();
        let mut input_router = InstructionTable::default();
        let input_layers = mount_view_input_layers(
            &mut input_router,
            mount_id,
            prepared.engine_definition.input.strategy,
            prepared.engine_definition.input.buffer_target,
            prepared.raw_receiver,
        );
        let parameter_binding = prepared.parameter_binding;
        let definition = prepared.definition;
        let mut session = Self {
            config,
            theme,
            view_factory,
            views: vec![ViewFrame {
                mount: ViewMount {
                    mount_id,
                    context,
                    raw_receiver: prepared.raw_receiver,
                    parameter_binding,
                    definition,
                    engine_definition: prepared.engine_definition,
                    buffer_generation: 0,
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
                call_boundary: None,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input,
            route_completion: None,
            input_pipeline: InputPipeline::default(),
            input_router,
            active_error: None,
            active_error_deadline: None,
            runtime_warning: None,
            deferred_effect: None,
            committed_outcome: None,
            pending_external_tick: None,
            cancellation,

            next_mount_id: 2,
        };
        let starter = session.mount_task_starter(0)?;
        let runtime_snapshot = session.runtime.snapshot().clone();
        session
            .views
            .last_mut()
            .expect("root mount must be installed")
            .runtime
            .start_prepared_work(&starter, &runtime_snapshot);
        session.defer_engine_decision(initial_decision)?;
        Ok(session)
    }

    pub(crate) fn new_with_theme<V>(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: V,
        cancellation: &CancellationToken,
    ) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let view_ref = config
            .default_view
            .clone()
            .context("no default_view configured for the session")?;
        let uses_invocation_parameters = config.invocation_parameters.view_ref() == view_ref;
        let state = if uses_invocation_parameters {
            config.invocation_parameters.clone()
        } else {
            config.instantiate_parameters(&view_ref)?
        };
        let mut state = state;
        let initial_input =
            sanitized_initial_parameter_input(config, &mut state, uses_invocation_parameters)?;
        Self::assemble_root_session(RootAssemblyInput {
            config,
            theme,
            runtime_log,
            view_factory: Box::new(engines),
            view_ref,
            state,
            initial_input,
            route_input: true,
            cancellation: cancellation.clone(),
        })
    }

    pub(crate) fn single_root_with_theme<V>(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: V,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let uses_invocation_parameters = config.invocation_parameters.view_ref() == view_ref;
        let state = if uses_invocation_parameters {
            config.invocation_parameters.clone()
        } else {
            config.instantiate_parameters(view_ref)?
        };
        let mut state = state;
        let initial_input =
            sanitized_initial_parameter_input(config, &mut state, uses_invocation_parameters)?;
        Self::assemble_root_session(RootAssemblyInput {
            config,
            theme,
            runtime_log,
            view_factory: Box::new(engines),
            view_ref: view_ref.to_string(),
            state,
            initial_input,
            route_input: false,
            cancellation: cancellation.clone(),
        })
    }

    pub(super) fn view_context_for(&self, index: usize) -> Result<ViewContext> {
        let entry = self
            .views
            .get(index)
            .context("session has no requested view")?;
        self.view_context_at(
            index,
            &entry.input,
            &entry.state,
            entry.buffer_generation,
            &self.runtime,
        )
    }

    pub(super) fn current_view_context(&self) -> Result<ViewContext> {
        self.view_context_for(self.views.len().saturating_sub(1))
    }

    pub(super) fn mount_task_starter(&self, index: usize) -> Result<crate::task::MountTaskStarter> {
        let mount_id = self
            .views
            .get(index)
            .context("mount task starter requires an active view")?
            .mount_id;
        Ok(crate::task::MountTaskStarter::from_lease(
            &self.tasks,
            crate::task::MountTaskLease::new(mount_id),
        ))
    }

    pub(crate) fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(self.forced_termination());
            }
            self.clear_expired_error();
            let effect = self.step(terminal)?;
            let outcome = match self.process_effect(effect, terminal) {
                Ok(outcome) => outcome,
                Err(_) if self.cancellation.is_cancelled() => {
                    return Ok(self.forced_termination());
                }
                Err(error) => return Err(error),
            };
            self.commit_external_tick();
            self.surface_runtime_log_warning();
            if let Some(outcome) = self.resolve_outcome_after_step(outcome) {
                return Ok(outcome);
            }
            self.render(terminal)?;
            self.runtime_warning = None;
        }
    }

    fn resolve_outcome_after_step(
        &mut self,
        outcome: Option<SessionOutcome>,
    ) -> Option<SessionOutcome> {
        outcome.or_else(|| {
            self.cancellation
                .is_cancelled()
                .then(|| self.forced_termination())
        })
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.runtime_warning.take()
    }

    fn surface_runtime_log_warning(&mut self) {
        if let Some(record) = self.runtime_log.take_warning_record() {
            self.runtime_warning = Some(record.message.clone());
            self.active_error = Some(record);
            self.active_error_deadline = Some(Instant::now() + ERROR_DISPLAY_DURATION);
        }
    }

    fn clear_expired_error(&mut self) {
        if self
            .active_error_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.active_error = None;
            self.active_error_deadline = None;
        }
    }
}

impl Drop for AppSession<'_> {
    fn drop(&mut self) {
        self.tasks.shutdown_and_wait();
    }
}

fn sanitized_initial_parameter_input(
    config: &Config,
    state: &mut ParameterState,
    _typed_values: bool,
) -> Result<String> {
    // Object-query defaults and typed values are sanitized before rendering so
    // structured values never go through the interactive text parser.
    config.sanitize_initial_parameter_values(state)?;
    let rendered = config.render_parameter_input(state)?;
    Ok(sanitize_terminal_text(&rendered))
}

fn initialize_navigation_input(
    config: &Config,
    state: &mut ParameterState,
    seed: Option<&InputSeed>,
    parameters: Option<&serde_json::Value>,
) -> Result<(EditorBuffer, String)> {
    if let Some(parameters) = parameters {
        // Sanitize instantiated defaults before merging a partial typed object;
        // the initialization merge intentionally retains omitted fields.
        config.sanitize_initial_parameter_values(state)?;
        if !parameters.is_null() {
            config.update_sanitized_initial_parameter_values(state, parameters)?;
        }
        let input = config.render_parameter_input(state)?;
        return Ok((EditorBuffer::from_raw(input.clone(), input.len()), input));
    }
    let Some(seed) = seed else {
        let input = sanitized_initial_parameter_input(config, state, false)?;
        return Ok((EditorBuffer::from_raw(input.clone(), input.len()), input));
    };
    config.sanitize_initial_parameter_values(state)?;
    let raw = sanitize_terminal_text(&seed.raw);
    let parameter_input = sanitize_terminal_text(&seed.params);
    config.update_initial_parameter_input(state, &parameter_input)?;
    let cursor = if raw == seed.raw {
        seed.cursor
    } else {
        sanitize_terminal_text(&seed.raw[..seed.cursor]).len()
    };
    Ok((EditorBuffer::from_raw(raw, cursor), parameter_input))
}

fn input_buffer_from_seed(seed: &InputSeed) -> EditorBuffer {
    EditorBuffer::from_raw(seed.raw.clone(), seed.cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandRef;
    use crate::input::EditorBuffer;
    use ratatui::Terminal as RatatuiTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Style};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MismatchedRuntime;

    #[derive(Clone)]
    struct TransactionalMarkerView {
        marker: usize,
        fail_parameters: Arc<std::sync::atomic::AtomicBool>,
        fail_restore: bool,
    }

    impl TransactionalMarkerView {
        fn transition(
            &mut self,
            increments: usize,
            parameters: bool,
            restore: bool,
        ) -> Result<crate::engine::EngineEmission> {
            if parameters && self.fail_parameters.load(Ordering::SeqCst) {
                anyhow::bail!("transactional parameters dispatch failed");
            }
            if restore && self.fail_restore {
                anyhow::bail!("transactional restore dispatch failed");
            }
            self.marker += increments;
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }
    }

    impl crate::engine::EngineRuntime for TransactionalMarkerView {
        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(0, false, false)
        }

        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(1, true, false)
        }

        fn activate(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(1, false, false)
        }

        fn restore_input(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(1, false, true)
        }

        fn input_committed(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(1, false, false)
        }

        fn input_ready(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(1, false, true)
        }

        fn input_rejected(
            &mut self,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            self.transition(0, false, false)
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("transactional-marker", self.marker)
        }
    }

    impl crate::engine::EngineRuntime for MismatchedRuntime {
        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("wrong", ())
        }
    }

    struct MismatchedRenderer;

    impl crate::engine::ViewRenderer for MismatchedRenderer {
        fn validate_model(&self, _model: &crate::engine::RenderModel) -> Result<()> {
            anyhow::bail!("test renderer/model mismatch")
        }

        fn render(
            &self,
            _model: &crate::engine::RenderModel,
            _context: &crate::engine::RenderContext,
            _frame: &mut ratatui::Frame,
            _area: ratatui::layout::Rect,
        ) {
        }
    }

    #[derive(Clone)]
    struct RuntimeUpdateView {
        view_ref: String,
        clobber_workflow: bool,
        parameters_fail: bool,
        parameter_events: Option<Arc<Mutex<Vec<String>>>>,
    }

    impl crate::engine::EngineRuntime for RuntimeUpdateView {
        fn parameters(
            &mut self,
            snapshot: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            if self.parameters_fail && self.view_ref == "apps:main" {
                anyhow::bail!("initial parameters dispatch failed");
            }
            let decision = if self.view_ref == "apps:main" {
                let (path, value) = if self.clobber_workflow {
                    ("/workflow/test".to_string(), serde_json::json!(true))
                } else {
                    ("/session/custom".to_string(), serde_json::json!(true))
                };
                crate::engine::EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path,
                    value,
                })
            } else {
                crate::engine::EngineDecision::Invalidate
            };
            if let Some(events) = &self.parameter_events {
                events
                    .lock()
                    .unwrap()
                    .push(snapshot.raw_input().to_string());
            }
            Ok(crate::engine::EngineEmission::decision(decision))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("runtime-update", ())
        }
    }

    #[derive(Clone)]
    struct EffectNavigationView {
        changes: Arc<AtomicUsize>,
        starts: Arc<AtomicUsize>,
    }

    impl EffectNavigationView {
        fn new(changes: Arc<AtomicUsize>, starts: Arc<AtomicUsize>) -> Self {
            Self { changes, starts }
        }
    }

    impl crate::engine::EngineRuntime for EffectNavigationView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            self.changes.fetch_add(1, Ordering::SeqCst);
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Navigate(crate::engine::EngineNavigationRequest {
                    target: "missing:view".to_string(),
                    parameters: None,
                    replace: false,
                }),
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn start_prepared_work(
            &mut self,
            _starter: &crate::task::MountTaskStarter,
            _runtime_snapshot: &serde_json::Value,
        ) {
            self.starts.fetch_add(1, Ordering::SeqCst);
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("effect-navigation", ())
        }
    }

    fn changed_effect_emission(
        runtime: &mut dyn crate::engine::EngineRuntime,
        decision: crate::engine::EngineDecision,
        context: crate::engine::ViewContext,
    ) -> Result<crate::engine::EngineEmission> {
        runtime.action(crate::engine::EngineActionInput {
            invocation: crate::engine::ActionInvocation::new("test.changed"),
            context,
        })?;
        Ok(crate::engine::EngineEmission::decision(decision))
    }

    #[test]
    fn navigation_preparation_failure_keeps_engine_acceptance() {
        let config = crate::config::load_test_fixture().unwrap();
        let commits = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut engines = EngineRegistry::new();
        engines.register(crate::engine::EngineRegistration::new(
            crate::engine::EngineDefinition::new(crate::config::ENGINE_PICKER, "effect-navigation")
                .with_actions([crate::engine::ActionSpec::unit("test.navigate")]),
            {
                let commits = Arc::clone(&commits);
                let starts = Arc::clone(&starts);
                move |_context| {
                    Ok(Box::new(EffectNavigationView::new(
                        Arc::clone(&commits),
                        Arc::clone(&starts),
                    )))
                }
            },
        ));
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        let runtime_before = session.runtime.snapshot().clone();
        let starts_before = starts.load(Ordering::SeqCst);
        let context = session.current_view_context().unwrap();
        let emission = session.views[0]
            .runtime
            .action(crate::engine::EngineActionInput {
                invocation: crate::engine::ActionInvocation::new("test.navigate"),
                context,
            })
            .unwrap();

        let error = match session.commit_engine_emission_at(0, emission) {
            Ok(_) => panic!("target navigation preparation must fail"),
            Err(error) => error,
        };

        assert!(!error.to_string().is_empty());
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(starts.load(Ordering::SeqCst), starts_before + 1);
        assert_eq!(session.runtime.snapshot(), &runtime_before);
        assert!(session.committed_outcome.is_none());
    }

    #[test]
    fn changed_dispatch_command_failure_keeps_acceptance_batch() {
        let config = crate::config::load_test_fixture().unwrap();
        let commits = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(EffectNavigationView::new(
            Arc::clone(&commits),
            Arc::clone(&starts),
        ));
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();
        let runtime_before = session.runtime.snapshot().clone();
        let execution = crate::command::CommandExecution {
            invocation: crate::command::CommandInvocation::session_command(
                "core:default",
                "nested-navigation",
                crate::config::Command {
                    key: "enter".to_string(),
                    label: "Nested navigation".to_string(),
                    scope: crate::config::CommandScope::View,
                    requires: crate::config::CommandRequirement::Input,
                    passthrough: false,
                    action: crate::config::CommandAction::Navigate {
                        payload: crate::config::NavigatePayload {
                            target: toml::Value::String("missing:view".to_string()),
                            query: None,
                            replace: false,
                        },
                    },
                },
            ),
            context: call_context(&session),
        };
        let context = session.views[0].context.clone();
        let emission = changed_effect_emission(
            &mut *session.views[0].runtime,
            EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/accepted".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::Report(crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "accepted report".to_string(),
                }),
                EngineDecision::DispatchCommand(Box::new(execution)),
            ]),
            context,
        )
        .unwrap();

        let error = match session.commit_engine_emission_at(0, emission) {
            Ok(_) => panic!("nested command navigation preparation must fail"),
            Err(error) => error,
        };

        assert!(format!("{error:#}").contains("missing:view"));
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
        assert_eq!(session.runtime.snapshot()["session"]["accepted"], true);
        assert_ne!(session.runtime.snapshot(), &runtime_before);
        assert_eq!(
            session
                .active_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("accepted report")
        );
    }

    #[test]
    fn changed_edit_input_failure_keeps_engine_acceptance_and_host_state() {
        let config = crate::config::load_test_fixture().unwrap();
        let commits = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(EffectNavigationView::new(
            Arc::clone(&commits),
            Arc::clone(&starts),
        ));
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();
        let engine_kind_before = session.views[0].runtime.render_model().kind();
        let runtime_before = session.runtime.snapshot().clone();
        let context = session.views[0].context.clone();
        let emission = changed_effect_emission(
            &mut *session.views[0].runtime,
            EngineDecision::Edit(InputEdit::SetBuffer {
                raw: "candidate".to_string(),
                cursor: 99,
            }),
            context,
        )
        .unwrap();

        let error = match session.commit_engine_emission_at(0, emission) {
            Ok(_) => panic!("invalid edit must fail after Engine acceptance"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("input edit was rejected"));
        assert_eq!(commits.load(Ordering::SeqCst), 1);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
        assert_eq!(
            session.views[0].runtime.render_model().kind(),
            engine_kind_before
        );
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn decision_preflight_failure_does_not_rollback_engine_local_change() {
        let config = crate::config::load_test_fixture().unwrap();
        let changes = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(EffectNavigationView::new(
            Arc::clone(&changes),
            Arc::clone(&starts),
        ));
        let context = session.views[0].context.clone();
        let runtime_before = session.runtime.snapshot().clone();
        let emission = changed_effect_emission(
            &mut *session.views[0].runtime,
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/missing/parent".to_string(),
                value: serde_json::json!(true),
            }),
            context,
        )
        .unwrap();

        let error = match session.commit_engine_emission_at(0, emission) {
            Ok(_) => panic!("invalid RuntimeUpdate must fail Host preflight"),
            Err(error) => error,
        };

        assert!(!error.to_string().is_empty());
        assert_eq!(changes.load(Ordering::SeqCst), 1);
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn apply_engine_decision_failure_keeps_runtime_and_report_acceptance() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();

        let error = session
            .apply_engine_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/accepted_apply".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::Report(crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "accepted apply report".to_string(),
                }),
                EngineDecision::Navigate(crate::engine::EngineNavigationRequest {
                    target: "missing:view".to_string(),
                    parameters: None,
                    replace: false,
                }),
            ]))
            .err()
            .expect("navigation preparation must fail after decision acceptance");

        assert!(format!("{error:#}").contains("missing:view"));
        assert_eq!(
            session.runtime.snapshot()["session"]["accepted_apply"],
            true
        );
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
        assert_eq!(
            session
                .active_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("accepted apply report")
        );
    }

    fn transactional_marker_registration(
        fail_parameters: Arc<std::sync::atomic::AtomicBool>,
        fail_restore: bool,
    ) -> crate::engine::EngineRegistration {
        crate::engine::EngineRegistration::new(
            crate::engine::EngineDefinition::new(
                crate::config::ENGINE_PICKER,
                "transactional-marker",
            ),
            move |_context| {
                Ok(Box::new(TransactionalMarkerView {
                    marker: 0,
                    fail_parameters: Arc::clone(&fail_parameters),
                    fail_restore,
                }))
            },
        )
    }

    #[test]
    fn root_mount_rejects_renderer_model_mismatch() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(
            crate::engine::EngineRegistration::new(
                crate::engine::EngineDefinition::new(
                    crate::config::ENGINE_PICKER,
                    "mismatched-renderer",
                ),
                |_context| Ok(Box::new(MismatchedRuntime)),
            )
            .with_renderer_factory(|_context| Ok(Box::new(MismatchedRenderer))),
        );

        let error =
            match AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines) {
                Ok(_) => panic!("a mismatched renderer must reject root mount preparation"),
                Err(error) => error,
            };
        assert!(error.to_string().contains("test renderer/model mismatch"));
    }

    #[test]
    fn default_renderer_rejects_model_kind_mismatch() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(crate::engine::EngineRegistration::new(
            crate::engine::EngineDefinition::new(crate::config::ENGINE_PICKER, "expected"),
            |_context| Ok(Box::new(MismatchedRuntime)),
        ));

        let error =
            match AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines) {
                Ok(_) => panic!("the default renderer must reject an incompatible model kind"),
                Err(error) => error,
            };
        assert!(
            error
                .to_string()
                .contains("null renderer/model pairing mismatch")
        );
    }

    #[test]
    fn reconcile_dispatch_failure_preserves_host_state() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        let fail_parameters = Arc::new(std::sync::atomic::AtomicBool::new(false));
        engines.register(transactional_marker_registration(
            Arc::clone(&fail_parameters),
            false,
        ));
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        let marker_before = *session.views[0]
            .runtime
            .render_model()
            .downcast_ref::<usize>()
            .unwrap();
        session.views[0]
            .replace_input("new input".to_string(), 9)
            .unwrap();
        assert!(session.views[0].input_dirty);
        let before = parameter_patch_host_snapshot(&session);
        fail_parameters.store(true, Ordering::SeqCst);

        let error = match session.reconcile_input() {
            Ok(_) => panic!("failed Parameters dispatch must abort reconciliation"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("transactional parameters dispatch failed")
        );
        assert_parameter_patch_host_unchanged(&session, &before);
        assert!(session.views[0].input_dirty);
        assert_eq!(
            *session.views[0]
                .runtime
                .render_model()
                .downcast_ref::<usize>()
                .unwrap(),
            marker_before + 1
        );
    }

    #[test]
    fn input_ready_failure_preserves_deadline_and_engine_state() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(transactional_marker_registration(
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            true,
        ));
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        let deadline = std::time::Instant::now() - std::time::Duration::from_secs(1);
        session.views[0].input_deadline = Some(deadline);
        let marker_before = *session.views[0]
            .runtime
            .render_model()
            .downcast_ref::<usize>()
            .unwrap();

        let error = match session.dispatch_input_ready() {
            Ok(_) => panic!("InputReady failure must abort preparation"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("transactional restore dispatch failed")
        );
        assert_eq!(session.views[0].input_deadline, Some(deadline));
        assert_eq!(
            *session.views[0]
                .runtime
                .render_model()
                .downcast_ref::<usize>()
                .unwrap(),
            marker_before
        );
    }

    #[test]
    fn reconcile_success_updates_live_engine_state() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(transactional_marker_registration(
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            false,
        ));
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        let marker_before = *session.views[0]
            .runtime
            .render_model()
            .downcast_ref::<usize>()
            .unwrap();
        session.views[0]
            .replace_input("committed".to_string(), 9)
            .unwrap();

        session.reconcile_input().unwrap();

        assert!(!session.views[0].input_dirty);
        assert_eq!(
            *session.views[0]
                .runtime
                .render_model()
                .downcast_ref::<usize>()
                .unwrap(),
            marker_before + 2
        );
    }

    #[test]
    fn pop_restore_failure_keeps_committed_parent_activation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(transactional_marker_registration(
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            true,
        ));
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        assert!(
            session
                .apply_navigation(
                    NavigationRequest::new("apps:main", ""),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        let marker_before = *session.views[0]
            .runtime
            .render_model()
            .downcast_ref::<usize>()
            .unwrap();

        assert!(session.pop_current(None).unwrap().is_none());

        assert_eq!(
            session.active_error.as_ref().unwrap().message,
            "transactional restore dispatch failed"
        );
        assert_eq!(session.views.len(), 1);
        assert_eq!(
            *session.views[0]
                .runtime
                .render_model()
                .downcast_ref::<usize>()
                .unwrap(),
            marker_before + 1
        );
    }

    struct RuntimeUpdateEngine {
        clobber_workflow: bool,
        duplicate_bindings: bool,
        parameters_fail: bool,
        parameter_events: Option<Arc<Mutex<Vec<String>>>>,
    }

    impl RuntimeUpdateEngine {
        fn registration(self) -> crate::engine::EngineRegistration {
            let RuntimeUpdateEngine {
                clobber_workflow,
                duplicate_bindings,
                parameters_fail,
                parameter_events,
            } = self;
            crate::engine::EngineRegistration::new(
                crate::engine::EngineDefinition::new(
                    crate::config::ENGINE_PICKER,
                    "runtime-update",
                )
                .with_actions([
                    crate::engine::ActionSpec::unit("custom.one"),
                    crate::engine::ActionSpec::unit("custom.two"),
                ]),
                move |context| {
                    Ok(Box::new(RuntimeUpdateView {
                        view_ref: context.identity.view_ref.clone(),
                        clobber_workflow,
                        parameters_fail,
                        parameter_events: parameter_events.clone(),
                    }))
                },
            )
            .with_input_binding_factory(move |context| {
                if !duplicate_bindings || context.identity.view_ref != "apps:main" {
                    return Ok(Vec::new());
                }
                Ok(vec![
                    crate::command::InputActionBinding {
                        key: crate::input::Key::Escape,
                        action: crate::command::ResolvedInputAction::Engine(
                            crate::engine::ActionId::new("custom.one"),
                        ),
                        label: None,
                        enabled: true,
                    },
                    crate::command::InputActionBinding {
                        key: crate::input::Key::Escape,
                        action: crate::command::ResolvedInputAction::Engine(
                            crate::engine::ActionId::new("custom.two"),
                        ),
                        label: None,
                        enabled: true,
                    },
                ])
            })
        }
    }

    #[derive(Clone)]
    struct ParameterPatchDecisionView {
        decision: Option<crate::engine::EngineDecision>,
        dispatch_error: Option<&'static str>,
    }

    impl crate::engine::EngineRuntime for ParameterPatchDecisionView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            if let Some(error) = self.dispatch_error {
                anyhow::bail!(error);
            }
            Ok(crate::engine::EngineEmission::decision(
                self.decision
                    .clone()
                    .unwrap_or(crate::engine::EngineDecision::Continue),
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("parameter-patch-decision", ())
        }
    }

    #[derive(Clone)]
    struct RecordingView {
        events: Arc<Mutex<Vec<String>>>,
        record_parameters: bool,
        record_deactivate: bool,
    }

    impl RecordingView {
        fn new(
            events: Arc<Mutex<Vec<String>>>,
            record_parameters: bool,
            record_deactivate: bool,
        ) -> Self {
            Self {
                events,
                record_parameters,
                record_deactivate,
            }
        }
    }

    impl crate::engine::RawInputReceiver for RecordingView {
        fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
            self.events.lock().unwrap().push(format!("raw:{bytes:?}"));
            Ok(())
        }
    }

    impl crate::engine::EngineRuntime for RecordingView {
        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Invalidate,
            ))
        }

        fn parameters(
            &mut self,
            snapshot: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            if self.record_parameters {
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("parameters:{}", snapshot.raw_input()));
            }
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn activate(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.events
                .lock()
                .unwrap()
                .push(format!("activate:{}", context.input_raw()));
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn restore_input(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.events
                .lock()
                .unwrap()
                .push(format!("restore:{}", context.input_raw()));
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_committed(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.events
                .lock()
                .unwrap()
                .push(format!("committed:{}", context.input_raw()));
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_ready(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.events
                .lock()
                .unwrap()
                .push(format!("ready:{}", context.input_raw()));
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_rejected(
            &mut self,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            self.events.lock().unwrap().push("rejected".to_string());
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Invalidate,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn raw_receiver(&mut self) -> Option<&mut dyn crate::engine::RawInputReceiver> {
            Some(self)
        }

        fn deactivate(&mut self) {
            if self.record_deactivate {
                self.events.lock().unwrap().push("deactivate".to_string());
            }
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("recording", ())
        }
    }

    #[derive(Clone)]
    struct RawAfterParametersView {
        ready: bool,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl crate::engine::RawInputReceiver for RawAfterParametersView {
        fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
            self.events.lock().unwrap().push(format!("raw:{bytes:?}"));
            Ok(())
        }
    }

    impl crate::engine::EngineRuntime for RawAfterParametersView {
        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            self.ready = true;
            self.events.lock().unwrap().push("parameters".to_string());
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn raw_receiver(&mut self) -> Option<&mut dyn crate::engine::RawInputReceiver> {
            self.events.lock().unwrap().push("probe".to_string());
            self.ready
                .then_some(self as &mut dyn crate::engine::RawInputReceiver)
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("raw-test", ())
        }
    }

    #[derive(Clone)]
    struct ParentTransactionView {
        events: Arc<Mutex<Vec<String>>>,
        fail_commit: bool,
        fail_activate: bool,
        fail_restore: bool,
    }

    impl ParentTransactionView {
        fn new(
            events: Arc<Mutex<Vec<String>>>,
            fail_commit: bool,
            fail_activate: bool,
            fail_restore: bool,
        ) -> Self {
            Self {
                events,
                fail_commit,
                fail_activate,
                fail_restore,
            }
        }
    }

    impl crate::engine::EngineRuntime for ParentTransactionView {
        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn activate(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            if self.fail_activate {
                anyhow::bail!("parent activation failed");
            }
            self.events.lock().unwrap().push("activate".to_string());
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn restore_input(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            if self.fail_restore {
                anyhow::bail!("parent restore failed");
            }
            self.events.lock().unwrap().push("restore".to_string());
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_committed(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            let view_ref = context
                .runtime_snapshot()
                .current()
                .get("ref")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("missing");
            if self.fail_commit {
                anyhow::bail!("parent commit failed");
            }
            self.events
                .lock()
                .unwrap()
                .push(format!("commit:{view_ref}:{}", context.input_raw()));
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("parent", ())
        }
    }

    struct ParameterPatchHostSnapshot {
        runtime: serde_json::Value,
        runtime_revision: u64,
        input: EditorBuffer,
        committed_buffer_projection: EditorBuffer,
        state: crate::parameter::ParameterState,
        buffer_generation: u64,
        input_dirty: bool,
        input_deadline: Option<std::time::Instant>,
        pending_command_source: Option<crate::input::InputSourceIdentity>,
    }

    fn parameter_patch_host_snapshot(session: &AppSession<'_>) -> ParameterPatchHostSnapshot {
        let entry = &session.views[0];
        ParameterPatchHostSnapshot {
            runtime: session.runtime.snapshot().clone(),
            runtime_revision: session.runtime.revision(),
            input: entry.input.clone(),
            committed_buffer_projection: entry.committed_buffer_projection.clone(),
            state: entry.state.clone(),
            buffer_generation: entry.buffer_generation,
            input_dirty: entry.input_dirty,
            input_deadline: entry.input_deadline,
            pending_command_source: entry.pending_command.as_ref().map(|pending| pending.source),
        }
    }

    fn assert_parameter_patch_host_unchanged(
        session: &AppSession<'_>,
        before: &ParameterPatchHostSnapshot,
    ) {
        let entry = &session.views[0];
        assert_eq!(session.runtime.snapshot(), &before.runtime);
        assert_eq!(session.runtime.revision(), before.runtime_revision);
        assert_eq!(entry.input, before.input);
        assert_eq!(
            entry.committed_buffer_projection,
            before.committed_buffer_projection
        );
        assert_eq!(entry.state, before.state);
        assert_eq!(entry.buffer_generation, before.buffer_generation);
        assert_eq!(entry.input_dirty, before.input_dirty);
        assert_eq!(entry.input_deadline, before.input_deadline);
        assert_eq!(
            entry.pending_command.as_ref().map(|pending| pending.source),
            before.pending_command_source
        );
    }

    fn parameter_patch_config() -> crate::config::Config {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.test_config_value_mut()["plugins"]["core"]["views"]["default"]["query"] = serde_json::json!({
            "type": "object",
            "input_order": ["text"],
            "text": {"type": "string", "default": ""},
            "enabled": {"type": "boolean", "default": false}
        });
        config.test_rebuild_parameter_registry().unwrap();
        config
    }

    fn required_parameter_config(view_ref: &str, input_order: &[&str]) -> crate::config::Config {
        let mut config = crate::config::load_test_fixture().unwrap();
        let mut parts = view_ref.split(':');
        let plugin = parts.next().expect("test view reference has a plugin");
        let view = parts.next().expect("test view reference has a view");
        config.test_config_value_mut()["plugins"][plugin]["views"][view]["query"] = serde_json::json!({
            "type": "object",
            "input_order": input_order,
            "visible": {"type": "string", "default": ""},
            "token": {"type": "string"}
        });
        config.test_rebuild_parameter_registry().unwrap();
        config
    }

    fn control_sequence_parameter_config(default: &str) -> crate::config::Config {
        let mut config = required_parameter_config("apps:main", &["visible"]);
        config.test_config_value_mut()["plugins"]["apps"]["views"]["main"]["query"]["visible"]["default"] =
            serde_json::json!(default);
        config.test_rebuild_parameter_registry().unwrap();
        config
    }

    fn structured_parameter_config() -> crate::config::Config {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.test_config_value_mut()["plugins"]["apps"]["views"]["main"]["query"] = serde_json::json!({
            "type": "object",
            "input_order": ["items", "metadata"],
            "items": {"type": "array<string>"},
            "metadata": {"type": "object"}
        });
        config.test_rebuild_parameter_registry().unwrap();
        config
    }

    fn structured_default_parameter_config(input_order: &[&str]) -> crate::config::Config {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.test_config_value_mut()["plugins"]["apps"]["views"]["main"]["query"] = serde_json::json!({
            "type": "object",
            "input_order": input_order,
            "items": {
                "type": "array<string>",
                "default": ["item\u{1b}[31m"]
            },
            "metadata": {
                "type": "object",
                "default": {
                    "label": "meta\u{1b}[32mdata\u{1b}[0m",
                    "nested": ["deep\u{1b}[34mvalue\u{1b}[0m"]
                }
            }
        });
        config.test_rebuild_parameter_registry().unwrap();
        config
    }

    #[test]
    fn root_and_navigation_sanitize_nested_typed_parameter_values() {
        let items = serde_json::json!(["before\u{1b}[31mred\u{1b}[0m"]);
        let metadata = serde_json::json!({
            "label": "meta\u{1b}[32mdata\u{1b}[0m",
            "nested": ["deep\u{1b}[34mvalue\u{1b}[0m"]
        });
        let typed_parameters = serde_json::json!({
            "items": items,
            "metadata": metadata
        });
        let expected_parameters = serde_json::json!({
            "items": ["beforered"],
            "metadata": {
                "label": "metadata",
                "nested": ["deepvalue"]
            }
        });

        let mut root_config = structured_parameter_config();
        root_config.default_view = Some("apps:main".to_string());
        let invocation = root_config
            .bind_invocation_parameters(
                "apps:main",
                &[
                    format!("--items:={}", items),
                    format!("--metadata:={}", metadata),
                ],
            )
            .unwrap();
        root_config.set_invocation(Value::Null, invocation);
        let root_session = AppSession::new(
            &root_config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let root_entry = &root_session.views[0];
        assert_eq!(
            root_config.parameter_values(&root_entry.state).unwrap(),
            expected_parameters
        );
        assert_eq!(root_entry.input.raw, root_entry.state.raw_input());
        assert_eq!(
            root_entry.committed_buffer_projection.raw,
            root_entry.state.raw_input()
        );
        assert!(!root_entry.state.raw_input().contains('\u{1b}'));
        assert_eq!(
            root_session
                .current_view_context()
                .unwrap()
                .parameter_snapshot()
                .values(),
            &expected_parameters
        );
        assert_eq!(
            root_session.runtime.snapshot()["view"]["current"]["input"],
            root_entry.state.raw_input()
        );

        let navigation_config = structured_parameter_config();
        let mut navigation_session = AppSession::new(
            &navigation_config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert!(
            navigation_session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main").with_parameters(typed_parameters),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        let navigation_entry = navigation_session.views.last().unwrap();
        assert_eq!(
            navigation_config
                .parameter_values(&navigation_entry.state)
                .unwrap(),
            expected_parameters
        );
        assert_eq!(
            navigation_entry.input.raw,
            navigation_entry.state.raw_input()
        );
        assert_eq!(
            navigation_entry.committed_buffer_projection.raw,
            navigation_entry.state.raw_input()
        );
        assert!(!navigation_entry.state.raw_input().contains('\u{1b}'));
        assert_eq!(
            navigation_session
                .current_view_context()
                .unwrap()
                .parameter_snapshot()
                .values(),
            &expected_parameters
        );
        assert_eq!(
            navigation_session.runtime.snapshot()["view"]["current"]["input"],
            navigation_entry.state.raw_input()
        );
    }

    #[test]
    fn empty_plain_initialization_keeps_state_and_runtime_revisions_at_zero() {
        for configured_string in [false, true] {
            let mut config = crate::config::load_test_fixture().unwrap();
            if configured_string {
                config.test_config_value_mut()["plugins"]["core"]["views"]["default"]["query"] =
                    serde_json::json!({"type": "string"});
                config.test_rebuild_parameter_registry().unwrap();
            }
            let session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            let entry = &session.views[0];
            assert_eq!(entry.state.revision(), 0);
            assert_eq!(entry.state.raw_input(), "");

            let parameter_snapshot = config
                .parameter_snapshot(&entry.state, entry.source_identity())
                .unwrap();
            assert_eq!(parameter_snapshot.revision(), 0);
            assert_eq!(
                session
                    .current_view_context()
                    .unwrap()
                    .parameter_snapshot()
                    .revision(),
                0
            );
            assert_eq!(
                session.runtime.snapshot()["view"]["current"]["state_revision"],
                0
            );
        }
    }

    #[test]
    fn root_initialization_allows_missing_required_parameters() {
        let config = required_parameter_config("core:default", &[]);
        let session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .expect("root initialization must not validate missing required fields");

        assert_eq!(session.views[0].input.raw, "");
        assert_eq!(session.views[0].committed_input(), "");
    }

    #[test]
    fn navigation_initialization_allows_missing_required_parameters() {
        let config = required_parameter_config("apps:main", &["visible"]);
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .expect("navigation initialization must not validate missing required fields")
        );

        assert_eq!(session.views.last().unwrap().input.raw, "");
        assert_eq!(session.views.last().unwrap().committed_input(), "");
    }

    #[test]
    fn typed_navigation_initialization_allows_missing_required_parameters() {
        let config = required_parameter_config("apps:main", &["visible"]);
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main")
                        .with_parameters(serde_json::json!({"visible": "typed"})),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .expect("partial typed navigation values may omit required fields")
        );

        let entry = session.views.last().unwrap();
        let values = config.parameter_values(&entry.state).unwrap();
        assert_eq!(values["visible"], "typed");
        assert!(!values.as_object().unwrap().contains_key("token"));
        assert_eq!(entry.input.raw, entry.state.raw_input());
        assert_eq!(entry.committed_input(), entry.state.raw_input());
    }

    #[test]
    fn seeded_navigation_without_ordered_fields_keeps_raw_projection_consistent() {
        let config = required_parameter_config("apps:main", &[]);
        for seed in ["seeded", ""] {
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();

            assert!(
                session
                    .apply_navigation(
                        NavigationRequest::new("apps:main", seed),
                        NavigationMode::Push,
                        None,
                        None,
                    )
                    .expect("seeded navigation must not parse or validate unordered fields")
            );

            let entry = session.views.last().unwrap();
            assert_eq!(entry.input.raw, seed);
            assert_eq!(entry.committed_input(), seed);
            assert_eq!(entry.state.raw_input(), seed);
            assert_eq!(
                config.parameter_values(&entry.state).unwrap(),
                serde_json::json!({"visible": ""})
            );
            assert_eq!(
                config
                    .parameter_snapshot(&entry.state, entry.source_identity())
                    .unwrap()
                    .raw_input(),
                seed
            );
            assert_eq!(
                session
                    .current_view_context()
                    .unwrap()
                    .parameter_snapshot()
                    .raw_input(),
                seed
            );
            assert_eq!(
                session.runtime.snapshot()["view"]["current"]["raw_input"],
                seed
            );
            assert_eq!(session.runtime.snapshot()["view"]["current"]["input"], seed);
        }
    }

    #[test]
    fn navigation_initialization_sanitizes_default_and_parameter_snapshots() {
        let config = control_sequence_parameter_config("\u{1b}[31mdefault\u{1b}[0m");
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "default");
        assert_eq!(entry.state.raw_input(), "default");
        assert_eq!(
            config.parameter_values(&entry.state).unwrap()["visible"],
            "default"
        );
        let snapshot = config
            .parameter_snapshot(&entry.state, entry.source_identity())
            .unwrap();
        assert_eq!(snapshot.raw_input(), "default");
        assert_eq!(snapshot.values()["visible"], "default");
        assert_eq!(
            session
                .current_view_context()
                .unwrap()
                .parameter_snapshot()
                .raw_input(),
            "default"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "default"
        );
    }

    #[test]
    fn navigation_initialization_sanitizes_structured_defaults_and_null_parameters() {
        let config = structured_default_parameter_config(&["items", "metadata"]);
        let expected = serde_json::json!({
            "items": ["item"],
            "metadata": {
                "label": "metadata",
                "nested": ["deepvalue"]
            }
        });

        for request in [
            NavigationRequest::with_defaults("apps:main"),
            NavigationRequest::with_defaults("apps:main").with_parameters(Value::Null),
        ] {
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            assert!(
                session
                    .apply_navigation(request, NavigationMode::Push, None, None)
                    .expect("default navigation initialization must succeed")
            );

            let entry = session.views.last().unwrap();
            assert_eq!(config.parameter_values(&entry.state).unwrap(), expected);
            assert!(!serde_json::to_string(&expected).unwrap().contains('\u{1b}'));
            assert_eq!(entry.input.raw, entry.state.raw_input());
            assert_eq!(entry.committed_input(), entry.state.raw_input());
            assert_eq!(
                session.runtime.snapshot()["view"]["current"]["input"],
                entry.state.raw_input()
            );
            assert_eq!(
                session
                    .current_view_context()
                    .unwrap()
                    .parameter_snapshot()
                    .values(),
                &expected
            );
        }

        let config = structured_default_parameter_config(&[]);
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .expect("empty input_order defaults must initialize")
        );
        let entry = session.views.last().unwrap();
        assert_eq!(config.parameter_values(&entry.state).unwrap(), expected);
        assert_eq!(entry.input.raw, "");
        assert_eq!(entry.state.raw_input(), "");
        assert_eq!(session.runtime.snapshot()["view"]["current"]["input"], "");
    }

    #[test]
    fn partial_typed_navigation_sanitizes_omitted_nested_defaults() {
        let config = structured_default_parameter_config(&["items", "metadata"]);
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main").with_parameters(
                        serde_json::json!({
                            "items": ["provided\u{1b}[35mvalue\u{1b}[0m"]
                        })
                    ),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );

        let entry = session.views.last().unwrap();
        let expected = serde_json::json!({
            "items": ["providedvalue"],
            "metadata": {
                "label": "metadata",
                "nested": ["deepvalue"]
            }
        });
        assert_eq!(config.parameter_values(&entry.state).unwrap(), expected);
        assert_eq!(entry.input.raw, entry.state.raw_input());
        assert_eq!(entry.committed_input(), entry.state.raw_input());

        let snapshot = config
            .parameter_snapshot(&entry.state, entry.source_identity())
            .unwrap();
        assert_eq!(snapshot.values(), &expected);
        assert_eq!(snapshot.raw_input(), entry.state.raw_input());
        let engine_snapshot = session
            .current_view_context()
            .unwrap()
            .parameter_snapshot()
            .clone();
        assert_eq!(engine_snapshot, snapshot);

        let current = &session.runtime.snapshot()["view"]["current"];
        assert_eq!(current["query"], entry.state.raw_input());
        assert_eq!(current["input"], entry.state.raw_input());
        assert_eq!(current["raw_input"], entry.input.raw);
    }

    #[test]
    fn navigation_initialization_sanitizes_typed_parameter_snapshots() {
        let config = control_sequence_parameter_config("");
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::new("apps:main", "").with_parameters(serde_json::json!({
                        "visible": "\u{1b}[32mtyped\u{1b}[0m",
                        "token": "required"
                    })),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "typed");
        assert_eq!(entry.state.raw_input(), "typed");
        assert_eq!(entry.state.revision(), 1);
        assert_eq!(
            config.parameter_values(&entry.state).unwrap()["visible"],
            "typed"
        );
        assert_eq!(
            config.parameter_values(&entry.state).unwrap()["token"],
            "required"
        );
        let snapshot = config
            .parameter_snapshot(&entry.state, entry.source_identity())
            .unwrap();
        assert_eq!(snapshot.raw_input(), "typed");
        assert_eq!(snapshot.values()["visible"], "typed");
        assert_eq!(snapshot.revision(), 1);
        assert_eq!(
            session
                .current_view_context()
                .unwrap()
                .parameter_snapshot()
                .revision(),
            1
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "typed"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["state_revision"],
            1
        );
    }

    #[test]
    fn empty_seed_navigation_allows_required_ordered_parameters() {
        let config = required_parameter_config("apps:main", &["visible", "token"]);
        for request in [
            NavigationRequest::new("apps:main", ""),
            NavigationRequest::routed("apps:main", "", 0),
        ] {
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            assert!(
                session
                    .apply_navigation(request, NavigationMode::Push, None, None)
                    .expect("empty navigation seed must allow missing required fields")
            );
            let entry = session.views.last().unwrap();
            assert_eq!(entry.input.raw, "");
            assert_eq!(entry.state.raw_input(), "");
            let values = config.parameter_values(&entry.state).unwrap();
            assert!(!values.as_object().unwrap().contains_key("token"));
            assert_eq!(
                session.runtime.snapshot()["view"]["current"]["raw_input"],
                ""
            );
        }
    }

    #[test]
    fn nonempty_incomplete_navigation_seed_remains_strict() {
        let config = required_parameter_config("apps:main", &["visible", "token"]);
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        assert!(
            !session
                .apply_navigation(
                    NavigationRequest::new("apps:main", "visible-only"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        assert_eq!(session.views.len(), 1);
        assert!(session.active_error.is_some());
    }

    fn install_test_pending_command(session: &mut AppSession<'_>) {
        let command = session
            .config
            .commands
            .bindings
            .get("commands")
            .and_then(|binding| binding.as_command("commands"))
            .expect("fixture must define the Commands binding");
        let source = session.views[0].source_identity();
        session.views[0].pending_command = Some(crate::session::state::PendingCommand {
            invocation: crate::command::CommandInvocation::session_command(
                "core:default",
                "commands",
                command,
            ),
            source,
        });
    }

    struct PreparedWorkTrackingView {
        starts: Arc<AtomicUsize>,
    }

    impl crate::engine::EngineRuntime for PreparedWorkTrackingView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn start_prepared_work(
            &mut self,
            _starter: &crate::task::MountTaskStarter,
            _runtime_snapshot: &serde_json::Value,
        ) {
            self.starts.fetch_add(1, Ordering::SeqCst);
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("prepared-work-tracking", ())
        }
    }

    #[derive(Clone)]
    struct BackgroundMutationView {
        ticks: Arc<Mutex<Vec<crate::engine::ViewContextIdentity>>>,
    }

    impl crate::engine::EngineRuntime for BackgroundMutationView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn background_tick(
            &mut self,
            tick: crate::engine::BackgroundEngineTick,
        ) -> Result<crate::engine::BackgroundOutcome> {
            anyhow::ensure!(tick.mount_id == tick.expected.mount_id);
            self.ticks.lock().unwrap().push(tick.expected);
            Ok(crate::engine::BackgroundOutcome {
                notices: vec![crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "background completed".to_string(),
                }],
                publication: Some(crate::engine::ViewContextPublication::new(
                    serde_json::json!({"background": true}),
                )),
            })
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("background-mutation", ())
        }
    }

    #[derive(Clone)]
    struct ExternalCompletionView {
        acknowledgements: Arc<AtomicUsize>,
    }

    impl crate::engine::EngineRuntime for ExternalCompletionView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::External
        }

        fn drive_tick(
            &mut self,
            _tick: crate::engine::EngineTick,
        ) -> Result<crate::engine::ExternalTickResult> {
            Ok(crate::engine::ExternalTickResult::close_with(Some(
                crate::engine::EngineNotice::Error {
                    view_ref: "external".to_string(),
                    message: "completed".to_string(),
                },
            )))
        }

        fn commit_external_tick(&mut self) {
            self.acknowledgements.fetch_add(1, Ordering::SeqCst);
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("external-completion", ())
        }
    }

    #[derive(Clone)]
    struct SessionDeactivateView {
        name: &'static str,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl crate::engine::EngineRuntime for SessionDeactivateView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn deactivate(&mut self) {
            self.events
                .lock()
                .unwrap()
                .push(format!("commit:{}", self.name));
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("session-deactivate", ())
        }
    }

    #[derive(Clone)]
    struct ExitOnCommitView;

    impl crate::engine::EngineRuntime for ExitOnCommitView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn action(
            &mut self,
            _input: crate::engine::EngineActionInput,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn activate(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_committed(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Exit,
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("exit-on-commit", ())
        }
    }

    #[derive(Clone)]
    struct ReconciliationMarkerView {
        activation_observations: Arc<Mutex<Vec<bool>>>,
    }

    impl crate::engine::EngineRuntime for ReconciliationMarkerView {
        fn parameters(
            &mut self,
            _parameters: crate::parameter::ParameterSnapshot,
            _expected: crate::engine::ViewContextIdentity,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn activate(
            &mut self,
            context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            self.activation_observations.lock().unwrap().push(
                context.runtime_snapshot().current()["reconciled"] == serde_json::json!(true),
            );
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::Continue,
            ))
        }

        fn input_committed(
            &mut self,
            _context: crate::engine::ViewContext,
        ) -> Result<crate::engine::EngineEmission> {
            Ok(crate::engine::EngineEmission::decision(
                crate::engine::EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/view/current/reconciled".to_string(),
                    value: serde_json::json!(true),
                }),
            ))
        }

        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("reconciliation-marker", ())
        }
    }

    #[test]
    fn initial_parameters_runtime_updates_are_staged_and_validated() {
        let mut runtime = crate::runtime::RuntimeStore::new();
        runtime.replace(serde_json::json!({"view": {}}));
        let decision = stage_initial_engine_decision(
            &mut runtime,
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/view/current".to_string(),
                value: serde_json::json!({"ref": "core:default"}),
            }),
        )
        .unwrap();
        assert!(decision.reports.is_empty());
        assert_eq!(decision.runtime_updates.len(), 1);
        assert_eq!(runtime.snapshot()["view"]["current"]["ref"], "core:default");

        let error = match stage_initial_engine_decision(
            &mut runtime,
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/missing/parent".to_string(),
                value: serde_json::Value::Null,
            }),
        ) {
            Ok(_) => panic!("invalid initial runtime updates must fail before mount commit"),
            Err(error) => error,
        };
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn initial_parameters_reject_structural_decisions() {
        let mut runtime = crate::runtime::RuntimeStore::new();
        let error = match stage_initial_engine_decision(&mut runtime, EngineDecision::Close) {
            Ok(_) => panic!("initial parameters must not close a mount"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("initial Engine parameters"));
    }

    #[test]
    fn normal_batch_invalid_update_after_valid_update_is_atomic() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.runtime.replace(serde_json::json!({"session": {}}));
        let runtime_before = session.runtime.snapshot().clone();

        let error = session
            .apply_engine_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/marker".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/missing/parent".to_string(),
                    value: serde_json::Value::Null,
                }),
            ]))
            .err()
            .expect("an invalid normal update must prevent the whole batch");

        assert!(!error.to_string().is_empty());
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn normal_batch_preflights_nested_effect_order() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.runtime.replace(serde_json::json!({"session": {}}));
        let runtime_before = session.runtime.snapshot().clone();

        let error = session
            .apply_engine_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/marker".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::Batch(vec![
                    EngineDecision::Execute(crate::engine::EffectRequest::CopyToClipboard(
                        "value".to_string(),
                    )),
                    EngineDecision::Report(crate::engine::EngineNotice::Info {
                        view_ref: "core:default".to_string(),
                        message: "unreachable".to_string(),
                    }),
                ]),
            ]))
            .err()
            .expect("nested effect ordering must be rejected before mutation");
        assert!(
            error
                .to_string()
                .contains("an effectful Engine decision must be the final item in a Batch")
        );
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn normal_batch_commits_reports_and_runtime_updates_in_order() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.runtime.replace(serde_json::json!({"session": {}}));
        let revision_before = session.runtime.revision();

        let outcome = session
            .apply_engine_decision(EngineDecision::Batch(vec![
                EngineDecision::Report(crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "first".to_string(),
                }),
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/first".to_string(),
                    value: serde_json::json!({"value": 1}),
                }),
                EngineDecision::Report(crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "second".to_string(),
                }),
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/first/value".to_string(),
                    value: serde_json::json!(2),
                }),
            ]))
            .unwrap();

        assert!(matches!(outcome, LauncherOutcome::Continue));
        assert_eq!(session.runtime.revision(), revision_before + 1);
        assert_eq!(session.runtime.snapshot()["session"]["first"]["value"], 2);
        assert_eq!(
            session
                .active_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("second")
        );
    }

    #[test]
    fn normal_batch_rejects_parameter_patches_without_partial_mutation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.runtime.replace(serde_json::json!({"session": {}}));
        let runtime_before = session.runtime.snapshot().clone();
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();
        let target = session.views[0].mount_id;

        let error = session
            .apply_engine_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/marker".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::ParameterPatch(crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                )),
            ]))
            .err()
            .expect("parameter patches must have explicit batch semantics");

        assert!(
            error
                .to_string()
                .contains("ParameterPatch cannot be used in a Batch")
        );
        assert_eq!(session.runtime.snapshot(), &runtime_before);
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
    }

    #[test]
    fn parameter_patch_policy_preserves_candidates_and_rerenders_committed_input() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let entry = &mut session.views[0];
        let binding = entry.parameter_binding.clone();
        binding.parse_input(&mut entry.state, "A").unwrap();
        entry.input = EditorBuffer::new("B");
        entry.committed_buffer_projection = EditorBuffer::new("A");
        entry.input_dirty = true;
        entry.state.set_input_rejected(true);
        let generation = entry.buffer_generation;
        let revision = entry.state.revision();
        let target = entry.mount_id;

        let decision = session
            .apply_parameter_patch_request(crate::parameter::ParameterPatchRequest::new(
                target,
                serde_json::json!({"enabled": true}),
                Some(revision),
                crate::parameter::ParameterInputPolicy::Preserve,
            ))
            .unwrap();
        assert!(matches!(decision, LauncherOutcome::Continue));
        assert_eq!(session.views[0].input.raw, "B");
        assert!(session.views[0].input_dirty);
        assert!(session.views[0].state.input_rejected());
        assert_eq!(session.views[0].committed_buffer_projection.raw, "A");
        assert_eq!(session.views[0].state.raw_input(), "A");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "A"
        );
        assert_eq!(session.views[0].state.revision(), revision + 1);

        let decision = session
            .apply_parameter_patch_request(crate::parameter::ParameterPatchRequest::new(
                target,
                serde_json::json!({"text": "C"}),
                Some(session.views[0].state.revision()),
                crate::parameter::ParameterInputPolicy::Rerender,
            ))
            .unwrap();
        assert!(matches!(decision, LauncherOutcome::Continue));
        assert_eq!(session.views[0].input.raw, "C");
        assert_eq!(session.views[0].committed_buffer_projection.raw, "C");
        assert_eq!(session.views[0].state.raw_input(), "C");
        assert!(!session.views[0].input_dirty);
        assert!(!session.views[0].state.input_rejected());
        assert_eq!(session.views[0].buffer_generation, generation + 1);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "C"
        );
    }

    #[test]
    fn parameter_patch_direct_parameters_leaf_preserves_host_state() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(ParameterPatchDecisionView {
            decision: Some(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    session.views[0].mount_id,
                    serde_json::json!({"enabled": true}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            )),
            dispatch_error: None,
        });
        install_test_pending_command(&mut session);
        let before = parameter_patch_host_snapshot(&session);
        let target = session.views[0].mount_id;

        let error = session
            .apply_engine_decision(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"text": "rejected"}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            ))
            .err()
            .expect("a direct Parameters leaf must be rejected by preflight");

        assert!(error.to_string().contains("ParameterPatch"));
        assert_parameter_patch_host_unchanged(&session, &before);
    }

    #[test]
    fn parameter_patch_pre_dispatch_target_and_revision_fail_atomically() {
        for (target, revision_mismatch) in
            [(Some(crate::input::ViewMountId(9999)), false), (None, true)]
        {
            let config = parameter_patch_config();
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            let actual_target = session.views[0].mount_id;
            let requested_target = target.unwrap_or(actual_target);
            let revision = revision_mismatch.then(|| session.views[0].state.revision() + 1);
            install_test_pending_command(&mut session);
            let before = parameter_patch_host_snapshot(&session);

            let error = session
                .apply_parameter_patch_request(crate::parameter::ParameterPatchRequest::new(
                    requested_target,
                    serde_json::json!({"enabled": true}),
                    revision,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ))
                .err()
                .expect("pre-dispatch target or revision failure must reject");

            assert!(!error.to_string().is_empty());
            assert_parameter_patch_host_unchanged(&session, &before);
        }
    }

    #[test]
    fn parameter_patch_dispatch_error_preserves_host_runtime_and_pending_command() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(ParameterPatchDecisionView {
            decision: None,
            dispatch_error: Some("parameter dispatch failed"),
        });
        session.views[0].input_deadline = Some(std::time::Instant::now());
        install_test_pending_command(&mut session);
        let before = parameter_patch_host_snapshot(&session);
        let target = session.views[0].mount_id;

        let error = session
            .apply_engine_decision(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"enabled": true}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            ))
            .err()
            .expect("Parameters dispatch failure must reject the patch");

        assert!(error.to_string().contains("parameter dispatch failed"));
        assert_parameter_patch_host_unchanged(&session, &before);
    }

    #[test]
    fn parameter_patch_invalid_decisions_preserve_host_runtime_and_pending_command() {
        let decisions = [
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/missing/parent".to_string(),
                value: serde_json::json!(true),
            }),
            EngineDecision::Batch(vec![
                EngineDecision::Execute(crate::engine::EffectRequest::CopyToClipboard(
                    "value".to_string(),
                )),
                EngineDecision::Report(crate::engine::EngineNotice::Info {
                    view_ref: "core:default".to_string(),
                    message: "illegal trailing report".to_string(),
                }),
            ]),
        ];

        for decision in decisions {
            let config = parameter_patch_config();
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            session.views[0].runtime = Box::new(ParameterPatchDecisionView {
                decision: Some(decision),
                dispatch_error: None,
            });
            install_test_pending_command(&mut session);
            let before = parameter_patch_host_snapshot(&session);
            let target = session.views[0].mount_id;

            session
                .apply_engine_decision(EngineDecision::ParameterPatch(
                    crate::parameter::ParameterPatchRequest::new(
                        target,
                        serde_json::json!({"enabled": true}),
                        None,
                        crate::parameter::ParameterInputPolicy::Preserve,
                    ),
                ))
                .err()
                .expect("invalid Parameters decision must reject the patch");

            assert_parameter_patch_host_unchanged(&session, &before);
        }
    }

    #[test]
    fn parameter_patch_success_commits_runtime_report_once() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(ParameterPatchDecisionView {
            decision: Some(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/view/current/patch_marker".to_string(),
                    value: serde_json::json!("committed"),
                }),
                EngineDecision::Report(crate::engine::EngineNotice::Error {
                    view_ref: "core:default".to_string(),
                    message: "patched once".to_string(),
                }),
            ])),
            dispatch_error: None,
        });
        install_test_pending_command(&mut session);
        let revision_before = session.runtime.revision();
        let state_revision_before = session.views[0].state.revision();
        let target = session.views[0].mount_id;

        let outcome = session
            .apply_engine_decision(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"enabled": true}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            ))
            .unwrap();

        assert!(matches!(outcome, LauncherOutcome::Continue));
        assert_eq!(session.runtime.revision(), revision_before + 1);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["patch_marker"],
            "committed"
        );
        assert_eq!(session.views[0].state.revision(), state_revision_before + 1);
        assert!(session.views[0].pending_command.is_none());
        assert_eq!(
            session
                .active_error
                .as_ref()
                .map(|error| error.message.as_str()),
            Some("patched once")
        );
    }

    #[test]
    fn parameter_patch_success_returns_effect_once_without_recursive_execution() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].runtime = Box::new(ParameterPatchDecisionView {
            decision: Some(EngineDecision::Execute(
                crate::engine::EffectRequest::CopyToClipboard("effect once".to_string()),
            )),
            dispatch_error: None,
        });
        let revision_before = session.runtime.revision();
        let state_revision_before = session.views[0].state.revision();
        let target = session.views[0].mount_id;

        let outcome = session
            .apply_engine_decision(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"enabled": true}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            ))
            .unwrap();

        assert!(matches!(
            outcome,
            LauncherOutcome::Effect(effect)
                if matches!(*effect, ViewEffect::CopyToClipboard(ref value) if value == "effect once")
        ));
        assert_eq!(session.runtime.revision(), revision_before + 1);
        assert_eq!(session.views[0].state.revision(), state_revision_before + 1);
    }

    #[test]
    fn parameter_patch_rejects_nested_parameter_patch_without_host_mutation() {
        let config = parameter_patch_config();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let target = session.views[0].mount_id;
        session.views[0].runtime = Box::new(ParameterPatchDecisionView {
            decision: Some(EngineDecision::Batch(vec![EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"enabled": false}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            )])),
            dispatch_error: None,
        });
        install_test_pending_command(&mut session);
        let before = parameter_patch_host_snapshot(&session);

        let error = session
            .apply_engine_decision(EngineDecision::ParameterPatch(
                crate::parameter::ParameterPatchRequest::new(
                    target,
                    serde_json::json!({"enabled": true}),
                    None,
                    crate::parameter::ParameterInputPolicy::Preserve,
                ),
            ))
            .err()
            .expect("nested parameter patch must be rejected");

        assert!(
            error
                .to_string()
                .contains("ParameterPatch cannot be used in a Batch")
        );
        assert_parameter_patch_host_unchanged(&session, &before);
    }

    #[test]
    fn navigated_mount_retains_non_location_initial_runtime_updates() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: false,
                parameters_fail: false,
                parameter_events: None,
            }
            .registration(),
        );
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        assert!(applied, "navigation failed: {:?}", session.active_error);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
        assert_eq!(session.runtime.snapshot()["session"]["custom"], true);
    }

    #[test]
    fn no_boundary_return_deactivates_every_frame_top_to_root() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("sys:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(SessionDeactivateView {
            name: "root",
            events: Arc::clone(&events),
        });
        session.views[1].runtime = Box::new(SessionDeactivateView {
            name: "top",
            events: Arc::clone(&events),
        });

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("done"),
                },
                adapter: None,
            })
            .unwrap();

        assert!(matches!(
            transition,
            ReturnTransition::Outcome(SessionOutcome::Completed(_))
        ));
        assert_eq!(*events.lock().unwrap(), ["commit:top", "commit:root"]);
    }

    #[test]
    fn root_back_deactivates_the_root_frame_once() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(SessionDeactivateView {
            name: "root",
            events: Arc::clone(&events),
        });

        let outcome = session.pop_current(None).unwrap();

        assert!(matches!(outcome, Some(SessionOutcome::Exited)));
        assert_eq!(*events.lock().unwrap(), ["commit:root"]);
    }

    #[test]
    fn forced_cancellation_deactivates_all_frames_once() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("sys:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        for (index, name) in ["root", "top"].into_iter().enumerate() {
            session.views[index].runtime = Box::new(SessionDeactivateView {
                name,
                events: Arc::clone(&events),
            });
        }

        session.cancellation.cancel();
        let outcome = session.resolve_outcome_after_step(None);

        assert!(matches!(outcome, Some(SessionOutcome::Exited)));
        assert_eq!(*events.lock().unwrap(), ["commit:top", "commit:root"]);
    }

    #[test]
    fn completed_outcome_wins_over_simultaneous_cancellation_without_redeactivation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(SessionDeactivateView {
            name: "root",
            events: Arc::clone(&events),
        });
        session.exit_session();
        session.cancellation.cancel();

        let outcome = session.resolve_outcome_after_step(Some(SessionOutcome::Exited));

        assert!(matches!(outcome, Some(SessionOutcome::Exited)));
        assert_eq!(*events.lock().unwrap(), ["commit:root"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn explicit_exit_effect_deactivates_the_root_frame_once() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(SessionDeactivateView {
            name: "root",
            events: Arc::clone(&events),
        });
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut terminal = crate::terminal::Terminal::enter_with_fds_and_cancellation(
            slave,
            slave,
            crate::terminal::ImageProtocol::default(),
            crate::lifecycle::CancellationToken::new(),
        )
        .unwrap();

        let outcome = session
            .process_effect(ViewEffect::Exit, &mut terminal)
            .unwrap();

        drop(terminal);
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        assert!(matches!(outcome, Some(SessionOutcome::Exited)));
        assert_eq!(*events.lock().unwrap(), ["commit:root"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pop_exit_effect_runs_in_the_same_process_effect_loop() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("sys:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        session.views[0].runtime = Box::new(ExitOnCommitView);

        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut terminal = crate::terminal::Terminal::enter_with_fds_and_cancellation(
            slave,
            slave,
            crate::terminal::ImageProtocol::default(),
            crate::lifecycle::CancellationToken::new(),
        )
        .unwrap();
        let result = session.process_effect(
            ViewEffect::Back(Some(InputEdit::SetBuffer {
                raw: "edited".to_string(),
                cursor: 6,
            })),
            &mut terminal,
        );
        drop(terminal);
        unsafe {
            libc::close(master);
            libc::close(slave);
        }

        assert!(matches!(result.unwrap(), Some(SessionOutcome::Exited)));
        assert_eq!(session.views.len(), 1);
        assert!(session.deferred_effect.is_none());
    }

    #[test]
    fn pop_reconciliation_runtime_update_is_visible_to_activation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("sys:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let observations = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(ReconciliationMarkerView {
            activation_observations: Arc::clone(&observations),
        });

        session
            .pop_current(Some(InputEdit::SetBuffer {
                raw: "edited".to_string(),
                cursor: 6,
            }))
            .unwrap();

        assert_eq!(*observations.lock().unwrap(), [true]);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["reconciled"],
            true
        );
    }

    #[test]
    fn runtime_update_is_published_before_exit_without_starting_source_work() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let starts = Arc::new(AtomicUsize::new(0));
        session.views[0].runtime = Box::new(PreparedWorkTrackingView {
            starts: Arc::clone(&starts),
        });
        let emission = crate::engine::EngineEmission::decision(EngineDecision::Batch(vec![
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/session/exit_marker".to_string(),
                value: serde_json::json!("published"),
            }),
            EngineDecision::Exit,
        ]));

        let outcome = session.commit_engine_emission_at(0, emission).unwrap();

        assert!(matches!(outcome, LauncherOutcome::Continue));
        assert!(matches!(
            session.committed_outcome,
            Some(SessionOutcome::Exited)
        ));
        assert_eq!(
            session.runtime.snapshot()["session"]["exit_marker"],
            "published"
        );
        assert_eq!(starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn runtime_update_is_published_before_complete_return_without_starting_source_work() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let starts = Arc::new(AtomicUsize::new(0));
        session.views[0].runtime = Box::new(PreparedWorkTrackingView {
            starts: Arc::clone(&starts),
        });
        let emission = crate::engine::EngineEmission::decision(EngineDecision::Batch(vec![
            EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                path: "/session/return_marker".to_string(),
                value: serde_json::json!("published"),
            }),
            EngineDecision::Return(crate::engine::ViewOutput::Value {
                value: serde_json::json!("done"),
            }),
        ]));

        let outcome = session.commit_engine_emission_at(0, emission).unwrap();

        assert!(matches!(outcome, LauncherOutcome::Continue));
        assert!(matches!(
            session.committed_outcome,
            Some(SessionOutcome::Completed(_))
        ));
        assert_eq!(
            session.runtime.snapshot()["session"]["return_marker"],
            "published"
        );
        assert_eq!(starts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_pop_utf8_cursor_keeps_child_and_does_not_deactivate_it() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[1].runtime = Box::new(SessionDeactivateView {
            name: "child",
            events: Arc::clone(&events),
        });

        let error = session
            .pop_current(Some(InputEdit::SetBuffer {
                raw: "é".to_string(),
                cursor: 1,
            }))
            .expect_err("a non-boundary UTF-8 cursor must fail before Pop consumption");

        assert!(error.to_string().contains("input edit was rejected"));
        assert_eq!(session.views.len(), 2);
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn pop_activation_failure_is_reported_after_child_is_consumed() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        session.views[0].runtime = Box::new(ParentTransactionView::new(
            Arc::new(Mutex::new(Vec::new())),
            false,
            true,
            false,
        ));

        assert!(session.pop_current(None).unwrap().is_none());

        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(
            session.active_error.as_ref().unwrap().message,
            "parent activation failed"
        );
    }

    #[test]
    fn return_deactivates_top_to_boundary_before_parent_resume() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context: call_context(&session),
                then: None,
            })
            .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            false,
            false,
            false,
        ));
        session.views[1].runtime = Box::new(SessionDeactivateView {
            name: "boundary",
            events: Arc::clone(&events),
        });
        session.views[2].runtime = Box::new(SessionDeactivateView {
            name: "top",
            events: Arc::clone(&events),
        });

        session
            .apply_return(ViewReturn {
                source_view: "apps:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("returned"),
                },
                adapter: None,
            })
            .unwrap();

        assert_eq!(session.views.len(), 1);
        assert_eq!(
            *events.lock().unwrap(),
            ["commit:top", "commit:boundary", "activate", "restore"]
        );
    }

    #[test]
    fn return_restore_failure_is_reported_after_boundary_is_consumed() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context: call_context(&session),
                then: None,
            })
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].mount.runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            false,
            false,
            true,
        ));

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("returned"),
                },
                adapter: None,
            })
            .unwrap();

        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 1);
        assert_eq!(*events.lock().unwrap(), ["activate"]);
        assert_eq!(
            session.active_error.as_ref().unwrap().message,
            "parent restore failed"
        );
    }

    #[test]
    fn return_continuation_failure_is_reported_after_boundary_is_consumed() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "missing-command".to_string(),
                }),
                context: call_context(&session),
                then: Some(Box::new(crate::config::CommandAction::EditInput {
                    payload: crate::config::EditInputPayload {
                        value: toml::Value::String("{{ result.output.value }}".to_string()),
                        cursor: None,
                    },
                })),
            })
            .unwrap();

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("returned"),
                },
                adapter: None,
            })
            .unwrap();

        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 1);
        assert!(
            session
                .active_error
                .as_ref()
                .unwrap()
                .message
                .contains("continuation origin")
        );
    }

    #[test]
    fn return_continuation_edit_reconciliation_failure_keeps_prior_commits() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            true,
            false,
            false,
        ));
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::Session {
                    view: "core:default".to_string(),
                    command: "commands".to_string(),
                    definition: Box::new(
                        config
                            .commands
                            .bindings
                            .get("commands")
                            .unwrap()
                            .as_command("commands")
                            .unwrap(),
                    ),
                },
                context: call_context(&session),
                then: Some(Box::new(crate::config::CommandAction::EditInput {
                    payload: crate::config::EditInputPayload {
                        value: toml::Value::String("{{ result.output.value }}".to_string()),
                        cursor: None,
                    },
                })),
            })
            .unwrap();
        let root_input_before = session.views[0].input.clone();
        let root_state_before = session.views[0].state.clone();

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("candidate"),
                },
                adapter: None,
            })
            .unwrap();

        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(*events.lock().unwrap(), ["activate", "restore"]);
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].input, root_input_before);
        assert_eq!(session.views[0].state, root_state_before);
        assert_eq!(
            session.active_error.as_ref().unwrap().message,
            "parent commit failed"
        );
    }

    #[test]
    fn initial_parameters_dispatch_failure_rejects_the_parent_route() {
        let config = crate::config::load_test_fixture().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: false,
                parameters_fail: true,
                parameter_events: None,
            }
            .registration(),
        );
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        session.views[0].runtime = Box::new(RecordingView::new(Arc::clone(&events), false, false));
        session.views[0]
            .replace_input("apps:main".to_string(), "apps:main".len())
            .unwrap();

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();

        assert!(!applied);
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(session.views[0].input.raw, "apps:main");
        assert!(!session.views[0].input_dirty);
        assert!(session.views[0].state.input_rejected());
        assert!(session.views[0].context.input_rejected());
        assert_eq!(*events.lock().unwrap(), ["rejected"]);
        assert!(session.active_error.is_some());
    }

    #[test]
    fn target_initial_parameters_failure_does_not_deactivate_current_engine_or_change_host() {
        let config = crate::config::load_test_fixture().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: false,
                parameters_fail: true,
                parameter_events: None,
            }
            .registration(),
        );
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        session.views.last_mut().unwrap().runtime =
            Box::new(RecordingView::new(Arc::clone(&events), false, true));
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();
        let runtime_before = session.runtime.snapshot().clone();
        let context_before = session.current_view_context().unwrap();
        let view_ref_before = session.views[0].view_ref.clone();
        let mount_id_before = session.views[0].mount_id;

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();

        assert!(!applied);
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, view_ref_before);
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
        assert_eq!(session.views[0].mount_id, mount_id_before);
        let context_after = session.current_view_context().unwrap();
        assert_eq!(context_after.input, context_before.input);
        assert_eq!(context_after.parameters, context_before.parameters);
        assert_eq!(context_after.runtime, context_before.runtime);
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn parent_edit_is_host_only_and_does_not_dispatch_inactive_parent() {
        let config = crate::config::load_test_fixture().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views.last_mut().unwrap().runtime =
            Box::new(RecordingView::new(Arc::clone(&events), true, false));

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                Some(InputEdit::SetBuffer {
                    raw: "changed".to_string(),
                    cursor: 7,
                }),
            )
            .unwrap();

        assert!(applied);
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views[0].input.raw, "changed");
        assert_eq!(session.views[0].committed_input(), "changed");
        assert_eq!(session.views[0].buffer_generation, 1);
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_parent_edit_keeps_host_state_and_completion_open() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let runtime_before = session.runtime.snapshot().clone();
        let input_before = session.views[0].input.clone();
        let state_before = session.views[0].state.clone();
        let generation_before = session.views[0].buffer_generation;
        session.open_route_completion().unwrap();

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                Some(InputEdit::SetBuffer {
                    raw: "é".to_string(),
                    cursor: 1,
                }),
            )
            .unwrap();

        assert!(!applied);
        assert!(session.route_completion.is_some());
        assert_eq!(session.views[0].input, input_before);
        assert_eq!(session.views[0].state, state_before);
        assert_eq!(session.views[0].buffer_generation, generation_before);
        assert_eq!(session.runtime.snapshot(), &runtime_before);
    }

    #[test]
    fn duplicate_engine_binding_is_rejected_before_navigation_commit() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: true,
                parameters_fail: false,
                parameter_events: None,
            }
            .registration(),
        );
        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();

        let applied = session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();

        assert!(!applied);
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert!(session.active_error.is_some());
    }

    #[test]
    fn duplicate_engine_binding_is_rejected_for_root_mount() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.default_view = Some("apps:main".to_string());
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: true,
                parameters_fail: false,
                parameter_events: None,
            }
            .registration(),
        );
        let result = AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines);
        let error = match result {
            Ok(_) => panic!("duplicate root bindings must fail before mount commit"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("registered more than once"));
    }

    #[test]
    fn duplicate_engine_binding_is_rejected_for_explicit_root_mount() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.default_view = Some("apps:main".to_string());
        let parameters = config.bind_invocation_parameters("apps:main", &[]).unwrap();
        config.set_invocation(Value::Null, parameters);
        let cancellation = CancellationToken::new();
        let mut engines = EngineRegistry::new();
        engines.register(
            RuntimeUpdateEngine {
                clobber_workflow: false,
                duplicate_bindings: true,
                parameters_fail: false,
                parameter_events: None,
            }
            .registration(),
        );

        let result = AppSession::single_root_with_theme(
            &config,
            crate::theme::ResolvedTheme::terminal(),
            crate::diagnostics::RuntimeLog::disabled(),
            engines,
            "apps:main",
            &cancellation,
        );
        let error = match result {
            Ok(_) => panic!("duplicate explicit-root bindings must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("registered more than once"));
    }

    #[test]
    fn lifecycle_batches_validate_before_runtime_commit() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.runtime.replace(serde_json::json!({"session": {}}));
        let error = session
            .apply_lifecycle_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/marker".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/missing/parent".to_string(),
                    value: serde_json::Value::Null,
                }),
            ]))
            .expect_err("an invalid lifecycle update must prevent the whole batch");
        assert!(!error.to_string().is_empty());
        assert!(session.runtime.snapshot()["session"]["marker"].is_null());

        let error = session
            .apply_lifecycle_decision(EngineDecision::Batch(vec![
                EngineDecision::RuntimeUpdate(crate::engine::RuntimeUpdate {
                    path: "/session/marker".to_string(),
                    value: serde_json::json!(true),
                }),
                EngineDecision::Close,
            ]))
            .expect_err("a lifecycle transition must prevent the whole batch");
        assert!(!error.to_string().is_empty());
        assert!(session.runtime.snapshot()["session"]["marker"].is_null());
    }

    #[test]
    fn external_completion_ack_is_once_and_notice_is_post_commit() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        session.views[0].runtime = Box::new(ExternalCompletionView {
            acknowledgements: Arc::clone(&acknowledgements),
        });
        let result = crate::engine::ExternalTickResult::close_with(Some(
            crate::engine::EngineNotice::Error {
                view_ref: "external".to_string(),
                message: "completed".to_string(),
            },
        ));

        assert!(matches!(
            session
                .apply_external_tick_result(0, result.clone())
                .unwrap(),
            crate::command::ViewEffect::Back(None)
        ));
        assert!(session.active_error.is_none());
        let changed_notice = crate::engine::ExternalTickResult::close_with(Some(
            crate::engine::EngineNotice::Info {
                view_ref: "external".to_string(),
                message: "changed".to_string(),
            },
        ));
        assert!(
            session
                .apply_external_tick_result(0, changed_notice)
                .is_err()
        );
        assert!(session.active_error.is_none());
        session.apply_external_tick_result(0, result).unwrap();
        session.commit_external_tick();
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
        assert_eq!(session.active_error.as_ref().unwrap().message, "completed");
        session.commit_external_tick();
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn external_failure_acknowledges_once_and_publishes_notice_once() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        session.views[0].runtime = Box::new(ExternalCompletionView {
            acknowledgements: Arc::clone(&acknowledgements),
        });
        let result = crate::engine::ExternalTickResult::fail_with(
            "external failure".to_string(),
            Some(crate::engine::EngineNotice::Error {
                view_ref: "external".to_string(),
                message: "failed".to_string(),
            }),
        );

        let error = match session.apply_external_tick_result(0, result) {
            Ok(_) => panic!("external failure must remain an error"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), "external failure");
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
        assert_eq!(session.active_error.as_ref().unwrap().message, "failed");
        assert!(session.pending_external_tick.is_none());

        session.commit_external_tick();
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
        assert_eq!(session.active_error.as_ref().unwrap().message, "failed");
    }

    #[test]
    fn forced_termination_discards_pending_external_completion_without_ack() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        session.views[0].runtime = Box::new(ExternalCompletionView {
            acknowledgements: Arc::clone(&acknowledgements),
        });
        session
            .apply_external_tick_result(0, crate::engine::ExternalTickResult::close_with(None))
            .unwrap();
        assert!(session.pending_external_tick.is_some());

        assert!(matches!(
            session.forced_termination(),
            SessionOutcome::Exited
        ));
        assert!(session.pending_external_tick.is_none());
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn removed_external_source_is_acknowledged_before_drop() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        session.views[1].runtime = Box::new(ExternalCompletionView {
            acknowledgements: Arc::clone(&acknowledgements),
        });
        session.views[0].runtime = Box::new(ParentTransactionView::new(
            Arc::new(Mutex::new(Vec::new())),
            false,
            true,
            false,
        ));
        session
            .apply_external_tick_result(1, crate::engine::ExternalTickResult::close_with(None))
            .unwrap();
        session.pop_current(None).unwrap();
        session.commit_external_tick();
        assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
        assert_eq!(session.views.len(), 1);
        assert_eq!(
            session.active_error.as_ref().unwrap().message,
            "parent activation failed"
        );
    }

    #[test]
    fn prepared_background_outcome_uses_mount_identity_and_restricted_output() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        let ticks = Arc::new(Mutex::new(Vec::new()));
        session.views[0].runtime = Box::new(BackgroundMutationView {
            ticks: Arc::clone(&ticks),
        });
        let expected = session.views[0].context.identity();
        let runtime_before = session.runtime.snapshot().clone();

        session.poll_background_views().unwrap();

        assert_eq!(*ticks.lock().unwrap(), [expected]);
        assert_eq!(session.views[0].context.current()["background"], true);
        assert_eq!(session.runtime.snapshot(), &runtime_before);
        assert_eq!(
            session
                .active_error
                .as_ref()
                .map(|record| record.message.as_str()),
            Some("background completed")
        );
    }

    #[test]
    fn raw_receiver_is_discovered_after_initial_parameters_for_root_and_navigation() {
        let config = crate::config::load_test_fixture().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut engines = EngineRegistry::new();
        let registration_events = Arc::clone(&events);
        engines.register(crate::engine::EngineRegistration::new(
            crate::engine::EngineDefinition::new(crate::config::ENGINE_PICKER, "raw-test"),
            move |_| {
                Ok(Box::new(RawAfterParametersView {
                    ready: false,
                    events: Arc::clone(&registration_events),
                }))
            },
        ));

        let mut session =
            AppSession::new(&config, crate::diagnostics::RuntimeLog::disabled(), engines).unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();

        let events = events.lock().unwrap().clone();
        assert_eq!(&events[..4], ["parameters", "probe", "parameters", "probe"]);
    }

    #[test]
    fn default_session_reuses_bound_invocation_parameters() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.default_view = Some("trans:main".to_string());
        let parameters = config
            .bind_invocation_parameters("trans:main", &["--source=bound".to_string()])
            .unwrap();
        config.set_invocation(Value::Null, parameters);
        let session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert_eq!(
            config.parameter_values(&session.views[0].state).unwrap()["source"],
            "bound"
        );
        assert_eq!(session.views[0].input.raw, "bound '' ''");
    }

    #[test]
    fn explicit_root_reinstantiates_parameters_for_the_target_view() {
        let config = crate::config::load_test_fixture().unwrap();
        let cancellation = CancellationToken::new();
        let session = AppSession::single_root_with_theme(
            &config,
            ResolvedTheme::terminal(),
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
            "apps:main",
            &cancellation,
        )
        .unwrap();
        assert_eq!(session.views[0].state.view_ref(), "apps:main");
    }

    #[test]
    fn routed_navigation_preserves_the_active_cursor() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert_eq!(session.current_chrome(80).unwrap().input_line(), "");
        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "app query".to_string();
        entry.input.cursor = "app que".len();
        session.mark_input_changed().unwrap();

        let effect = session.reconcile_input().unwrap().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = effect
        else {
            panic!("route input did not produce navigation");
        };
        assert_eq!(request.input.as_ref().unwrap().cursor, "que".len());
        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );
        assert_eq!(session.views[0].input.raw, "");
        assert_eq!(session.views[0].committed_input(), "");
        let input = &session.views.last().unwrap().input;
        assert_eq!(input.raw, "query");
        assert_eq!(input.cursor, "que".len());
        assert_eq!(
            session.current_chrome(80).unwrap().input_line(),
            "app query"
        );
    }

    #[test]
    fn routed_parent_edit_commits_host_state_without_parent_dispatch() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            false,
            false,
            false,
        ));
        entry.input.raw = "app ".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = session.reconcile_input().unwrap().unwrap()
        else {
            panic!("route input did not produce navigation");
        };

        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
    }

    #[test]
    fn routed_parent_edit_does_not_use_engine_commit_as_a_gate() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            true,
            false,
            false,
        ));
        entry.input.raw = "app ".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = session.reconcile_input().unwrap().unwrap()
        else {
            panic!("route input did not produce navigation");
        };

        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views[0].input.raw, "");
        assert_eq!(session.views[0].committed_input(), "");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            ""
        );
    }

    #[test]
    fn routed_parent_edit_does_not_dispatch_parameters_or_input_committed() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.runtime = Box::new(ParentTransactionView::new(
            Arc::clone(&events),
            true,
            true,
            false,
        ));
        entry.input.raw = "app ".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = session.reconcile_input().unwrap().unwrap()
        else {
            panic!("route input did not produce navigation");
        };

        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views[0].input.raw, "");
        assert_eq!(session.views[0].committed_input(), "");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
    }

    #[test]
    fn routed_navigation_clears_the_default_before_replace_and_return() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "app ".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();

        let effect = session.reconcile_input().unwrap().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = effect
        else {
            panic!("route input did not produce navigation");
        };
        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );
        assert_eq!(session.views[0].input.raw, "");
        assert_eq!(session.views[0].committed_input(), "");

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main"),
                    NavigationMode::Replace,
                    None,
                    None,
                )
                .unwrap()
        );
        assert!(session.pop_current(None).unwrap().is_none());
        let input = &session.views.last().unwrap().input;
        assert_eq!(input.raw, "");
        assert_eq!(session.views.last().unwrap().committed_input(), "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn nondefault_root_shows_its_prefix_without_enabling_routes() {
        let mut config = crate::config::load_test_fixture().unwrap();
        let parameters = config.bind_invocation_parameters("apps:main", &[]).unwrap();
        config.set_invocation(Value::Null, parameters);
        let cancellation = CancellationToken::new();
        let mut session = AppSession::single_root_with_theme(
            &config,
            crate::theme::ResolvedTheme::terminal(),
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
            "apps:main",
            &cancellation,
        )
        .unwrap();

        assert_eq!(session.current_chrome(80).unwrap().input_line(), "app ");
        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "sys query".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].committed_input(), "sys query");
    }

    #[test]
    fn route_prefixes_are_only_resolved_by_the_default_root() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "app ".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        let ViewEffect::Navigate {
            request,
            mode,
            parent_edit,
        } = session.reconcile_input().unwrap().unwrap()
        else {
            panic!("default root did not resolve a route prefix");
        };
        assert!(
            session
                .apply_navigation(request, mode, None, parent_edit)
                .unwrap()
        );

        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "sys nested".to_string();
        entry.input.cursor = entry.input.raw.len();
        session.mark_input_changed().unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views.last().unwrap().view_ref, "apps:main");
        assert_eq!(
            session.views.last().unwrap().committed_input(),
            "sys nested"
        );
    }

    #[test]
    fn empty_child_input_backspace_returns_to_its_parent() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("apps:main"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        assert_eq!(session.views.len(), 2);
        assert!(!session.delete_backward().unwrap());
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
    }

    #[test]
    fn edited_back_commits_before_activation_without_restoring_stale_input() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let parent = session.views.last_mut().unwrap();
        parent.input.raw = "app old".to_string();
        parent.input.cursor = parent.input.raw.len();
        parent.runtime = Box::new(RecordingView::new(Arc::clone(&events), false, false));

        assert!(
            session
                .apply_navigation(
                    NavigationRequest::new("sys:main", ""),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        events.lock().unwrap().clear();

        assert!(
            session
                .pop_current(Some(InputEdit::SetBuffer {
                    raw: "edited".to_string(),
                    cursor: 6,
                }))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            *events.lock().unwrap(),
            ["committed:edited", "activate:edited"]
        );
        let parent = session.views.last().unwrap();
        assert_eq!(session.runtime.snapshot()["view"]["current"]["cursor"], 6);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["buffer_revision"],
            parent.input.revision
        );
    }

    fn call_context(session: &AppSession<'_>) -> CommandContext {
        let entry = session.views.last().unwrap();
        let parameters = session
            .config
            .parameter_snapshot(&entry.state, entry.source_identity())
            .unwrap();
        CommandContext {
            page: crate::engine::CommandOwnerContext {
                view_ref: entry.view_ref.clone(),
                parameters: parameters.clone(),
                binding_raw: entry.committed_input().to_string(),
            },
            owner: crate::engine::CommandOwnerContext {
                view_ref: entry.view_ref.clone(),
                parameters,
                binding_raw: entry.committed_input().to_string(),
            },
            current: entry.context.current.clone(),
            current_fields: entry.engine_definition.current_fields,
            runtime: session.runtime.snapshot().clone(),
        }
    }

    #[test]
    fn return_continuation_return_completes_against_the_future_active_stack() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let command = config
            .commands
            .bindings
            .get("commands")
            .unwrap()
            .as_command("commands")
            .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::Session {
                    view: "core:default".to_string(),
                    command: "commands".to_string(),
                    definition: Box::new(command),
                },
                context: call_context(&session),
                then: Some(Box::new(crate::config::CommandAction::Return {
                    payload: crate::config::ReturnPayload {
                        value: Some(toml::Value::String("{{ result.output.value }}".to_string())),
                        handler: None,
                        args: None,
                    },
                })),
            })
            .unwrap();

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("continued"),
                },
                adapter: None,
            })
            .unwrap();

        let ReturnTransition::Outcome(SessionOutcome::Completed(returned)) = transition else {
            panic!("Return continuation must complete the boundary-free future stack");
        };
        assert!(matches!(
            returned.output,
            crate::engine::ViewOutput::Value { value }
                if value == serde_json::json!("continued")
        ));
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert!(session.views[0].call_boundary.is_none());
    }

    #[test]
    fn return_unwinds_the_nearest_call_branch_and_prepares_the_continuation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let context = call_context(&session);
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::Session {
                    view: "core:default".to_string(),
                    command: "commands".to_string(),
                    definition: Box::new(
                        config
                            .commands
                            .bindings
                            .get("commands")
                            .unwrap()
                            .as_command("commands")
                            .unwrap(),
                    ),
                },
                context,
                then: Some(Box::new(crate::config::CommandAction::EditInput {
                    payload: crate::config::EditInputPayload {
                        value: toml::Value::String("{{ result.output.value }}".to_string()),
                        cursor: None,
                    },
                })),
            })
            .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Push,
                None,
                None,
            )
            .unwrap();
        assert_eq!(session.views.len(), 3);

        let transition = session
            .apply_return(ViewReturn {
                source_view: "apps:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("restored"),
                },
                adapter: None,
            })
            .unwrap();
        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert!(session.runtime.snapshot().get("return").is_none());
        assert_eq!(session.views[0].input.raw, "restored");
        assert_eq!(session.views[0].input.cursor, "restored".len());
        assert_eq!(session.views[0].committed_input(), "restored");
        assert_eq!(session.views[0].state.raw_input(), "restored");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "core:default"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "restored"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["query"],
            "restored"
        );
    }

    #[test]
    fn return_continuation_call_commits_only_the_new_active_branch() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::Session {
                    view: "core:default".to_string(),
                    command: "commands".to_string(),
                    definition: Box::new(
                        config
                            .commands
                            .bindings
                            .get("commands")
                            .unwrap()
                            .as_command("commands")
                            .unwrap(),
                    ),
                },
                context: call_context(&session),
                then: Some(Box::new(crate::config::CommandAction::Call {
                    payload: crate::config::CallPayload {
                        target: toml::Value::String("apps:main".to_string()),
                        query: None,
                        then: None,
                    },
                })),
            })
            .unwrap();

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("returned"),
                },
                adapter: None,
            })
            .unwrap();

        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert!(session.views[0].call_boundary.is_none());
        assert_eq!(session.views[1].view_ref, "apps:main");
        assert!(session.views[1].call_boundary.is_some());
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
    }

    #[test]
    fn nested_return_only_unwinds_the_nearest_call() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let root_input_context = session.views.last().unwrap().input_layers.context;
        let outer_context = call_context(&session);
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context: outer_context,
                then: None,
            })
            .unwrap();
        let outer_input_context = session.views.last().unwrap().input_layers.context;
        assert_ne!(outer_input_context, root_input_context);
        let inner_context = call_context(&session);
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("apps:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "sys:main".to_string(),
                    id: "inner".to_string(),
                }),
                context: inner_context,
                then: None,
            })
            .unwrap();
        let inner_input_context = session.views.last().unwrap().input_layers.context;
        assert_ne!(inner_input_context, outer_input_context);

        let transition = session
            .apply_return(ViewReturn {
                source_view: "apps:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("inner"),
                },
                adapter: None,
            })
            .unwrap();
        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views.last().unwrap().view_ref, "sys:main");
        assert_eq!(
            session.views.last().unwrap().input_layers.context,
            outer_input_context
        );
        assert!(session.views.last().unwrap().call_boundary.is_some());

        let transition = session
            .apply_return(ViewReturn {
                source_view: "sys:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("outer"),
                },
                adapter: None,
            })
            .unwrap();
        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(session.views[0].input_layers.context, root_input_context);
    }

    #[test]
    fn replace_transfers_a_call_boundary_for_return_and_back() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context: call_context(&session),
                then: None,
            })
            .unwrap();
        session
            .apply_navigation(
                NavigationRequest::with_defaults("apps:main"),
                NavigationMode::Replace,
                None,
                None,
            )
            .unwrap();
        assert!(session.views.last().unwrap().call_boundary.is_some());
        let transition = session
            .apply_return(ViewReturn {
                source_view: "apps:main".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("replaced"),
                },
                adapter: None,
            })
            .unwrap();
        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");

        let context = call_context(&session);
        session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("sys:main"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "views".to_string(),
                }),
                context,
                then: None,
            })
            .unwrap();
        assert!(session.pop_current(None).unwrap().is_none());
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
    }

    #[test]
    fn session_command_hint_is_visible_until_a_modal_router_owns_its_key() {
        let mut config = crate::config::load_test_fixture().unwrap();
        let binding = config.commands.bindings.get_mut("commands").unwrap();
        binding.visibility = Some(crate::config::CommandBindingVisibility::Always);
        binding.key = Some("ctrl+k".to_string());
        {
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            assert!(
                session
                    .current_chrome(120)
                    .unwrap()
                    .footer
                    .contains("Commands")
            );
        }

        config.commands.bindings.get_mut("commands").unwrap().key = Some("enter".to_string());
        {
            let mut session = AppSession::new(
                &config,
                crate::diagnostics::RuntimeLog::disabled(),
                EngineRegistry::new(),
            )
            .unwrap();
            assert!(
                session
                    .current_chrome(120)
                    .unwrap()
                    .footer
                    .contains("Commands")
            );
        }

        config.commands.bindings.get_mut("commands").unwrap().key = Some("ctrl+k".to_string());
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert!(
            session
                .current_chrome(120)
                .unwrap()
                .footer
                .contains("Commands")
        );
        session.open_route_completion().unwrap();
        assert!(matches!(
            session.resolve_key_binding(Key::Tab),
            Some(InputBinding::Completion(RouteAction::Next))
        ));
        assert!(session.resolve_key_binding(Key::Ctrl('k')).is_none());
        assert!(
            !session
                .current_chrome(120)
                .unwrap()
                .footer
                .contains("Commands")
        );
    }

    #[test]
    fn route_completion_replays_unbound_control_to_the_restored_context() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.commands.bindings.get_mut("commands").unwrap().key = Some("ctrl+k".to_string());
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.refresh_active_bindings().unwrap();
        session.open_route_completion().unwrap();
        assert!(session.resolve_key_binding(Key::Ctrl('k')).is_none());
        let outcome = session
            .dispatch_decoded_input(DecodedInput {
                key: Some(Key::Ctrl('k')),
                raw: vec![0x0b],
            })
            .unwrap();
        assert!(matches!(outcome, LauncherOutcome::Continue));
        assert!(session.route_completion.is_none());
        assert_eq!(session.views.len(), 2);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(session.views[1].view_ref, "selectors:commands");
        assert!(session.views[1].call_boundary.is_some());
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "selectors:commands"
        );
    }

    #[test]
    fn route_completion_hands_editor_actions_to_the_restored_view() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views.last_mut().unwrap().input = EditorBuffer::new("two words");

        for (key, raw, expected) in [
            (Key::Backspace, vec![0x7f], "two word"),
            (Key::Ctrl('w'), vec![0x17], "two "),
            (Key::Ctrl('u'), vec![0x15], ""),
        ] {
            session.open_route_completion().unwrap();
            assert!(matches!(
                session
                    .dispatch_decoded_input(DecodedInput {
                        key: Some(key),
                        raw,
                    })
                    .unwrap(),
                LauncherOutcome::Continue
            ));
            assert!(session.route_completion.is_none());
            assert_eq!(session.views.last().unwrap().input.raw, expected);
        }
    }

    #[test]
    fn route_completion_replays_opaque_input_without_forwarding_to_decoded_view() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views.last_mut().unwrap().runtime =
            Box::new(RecordingView::new(events.clone(), false, false));
        let opaque = DecodedInput {
            key: None,
            raw: b"\x1b[999~".to_vec(),
        };

        session.open_route_completion().unwrap();
        session.dispatch_decoded_input(opaque.clone()).unwrap();
        assert!(session.route_completion.is_none());
        assert!(events.lock().unwrap().is_empty());

        session.dispatch_decoded_input(opaque).unwrap();
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn configurable_editor_keys_are_not_session_fallbacks() {
        let mut input = EditorBuffer::new("word");
        assert_eq!(apply_editor_key(&mut input, Key::Backspace), None);
        assert_eq!(apply_editor_key(&mut input, Key::Ctrl('u')), None);
        assert_eq!(apply_editor_key(&mut input, Key::Ctrl('w')), None);
        assert_eq!(input.raw, "word");
        assert_eq!(apply_editor_key(&mut input, Key::Delete), Some(false));
    }

    fn commit_valid_then_reject_invalid(
        session: &mut AppSession<'_>,
    ) -> (serde_json::Value, u64, EditorBuffer) {
        session
            .apply_input_edit(InputEdit::SetBuffer {
                raw: "2".to_string(),
                cursor: 1,
            })
            .unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let committed_runtime = session.runtime.snapshot()["view"]["current"].clone();
        let committed_state_revision = session.views.last().unwrap().state.revision();
        let committed_buffer = session
            .views
            .last()
            .unwrap()
            .committed_buffer_projection
            .clone();

        session
            .apply_input_edit(InputEdit::SetBuffer {
                raw: "bad".to_string(),
                cursor: 3,
            })
            .unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "bad");
        assert_eq!(entry.committed_input(), "2");
        assert!(entry.state.input_rejected());
        assert_eq!(entry.state.revision(), committed_state_revision);
        assert_eq!(entry.committed_buffer_projection, committed_buffer);

        (
            committed_runtime,
            committed_state_revision,
            committed_buffer,
        )
    }

    fn assert_rejected_parent_restored(
        session: &AppSession<'_>,
        committed_runtime: &serde_json::Value,
        committed_state_revision: u64,
        committed_buffer: &EditorBuffer,
    ) {
        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "bad");
        assert_eq!(entry.input.cursor, 3);
        assert_eq!(entry.committed_input(), "2");
        assert!(entry.state.input_rejected());
        assert_eq!(entry.state.revision(), committed_state_revision);
        assert_eq!(&entry.committed_buffer_projection, committed_buffer);
        for field in [
            "raw_input",
            "input",
            "cursor",
            "buffer_revision",
            "state_revision",
        ] {
            assert_eq!(
                session.runtime.snapshot()["view"]["current"][field],
                committed_runtime[field],
                "restored runtime field {field} differs from the committed projection"
            );
        }
        assert_eq!(
            session.runtime.snapshot()["session"]["input"],
            serde_json::json!({
                "raw": committed_runtime["raw_input"].clone(),
                "params": committed_runtime["input"].clone(),
                "cursor": committed_runtime["cursor"].clone(),
                "revision": committed_runtime["buffer_revision"].clone(),
            })
        );
    }

    #[test]
    fn rejected_parent_candidate_does_not_replace_committed_projection_on_pop_or_return() {
        let root = std::env::temp_dir().join(format!(
            "tui-launcher-parent-projection-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let core_root = root.join("plugins/core");
        std::fs::create_dir_all(&core_root).unwrap();
        std::fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"

            [views.default.query]
            type = "object"
            input_order = ["count"]
            count = { type = "integer", default = 1 }

            [views.child.engine]
            type = "picker"
            "#,
        )
        .unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, "default_view = \"core:default\"\n").unwrap();
        let config = Config::load(&path).unwrap();

        let mut pop_session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let (committed_runtime, committed_state_revision, committed_buffer) =
            commit_valid_then_reject_invalid(&mut pop_session);
        assert!(
            pop_session
                .apply_navigation(
                    NavigationRequest::with_defaults("core:child"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        pop_session.pop_current(None).unwrap();
        assert_rejected_parent_restored(
            &pop_session,
            &committed_runtime,
            committed_state_revision,
            &committed_buffer,
        );

        let mut return_session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let (committed_runtime, committed_state_revision, committed_buffer) =
            commit_valid_then_reject_invalid(&mut return_session);
        let context = call_context(&return_session);
        return_session
            .apply_call(CallRequest {
                request: NavigationRequest::with_defaults("core:child"),
                origin: CommandOrigin::View(CommandRef {
                    view: "core:default".to_string(),
                    id: "child".to_string(),
                }),
                context,
                then: None,
            })
            .unwrap();
        let transition = return_session
            .apply_return(ViewReturn {
                source_view: "core:child".to_string(),
                output: crate::engine::ViewOutput::Value {
                    value: serde_json::json!("returned"),
                },
                adapter: None,
            })
            .unwrap();
        assert!(matches!(
            transition,
            ReturnTransition::Effect(effect) if matches!(*effect, ViewEffect::Continue)
        ));
        assert_rejected_parent_restored(
            &return_session,
            &committed_runtime,
            committed_state_revision,
            &committed_buffer,
        );

        drop(pop_session);
        drop(return_session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejected_query_and_pop_edit_preserve_committed_runtime_projection() {
        let root = std::env::temp_dir().join(format!(
            "tui-launcher-input-transaction-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let core_root = root.join("plugins/core");
        std::fs::create_dir_all(&core_root).unwrap();
        std::fs::write(
            core_root.join("plugin.toml"),
            r#"
            [plugin]
            name = "core"

            [views.default.engine]
            type = "picker"

            [views.default.query]
            type = "object"
            input_order = ["count"]
            count = { type = "integer", default = 1 }

            [views.child.engine]
            type = "picker"
            "#,
        )
        .unwrap();
        let path = root.join("config.toml");
        std::fs::write(
            &path,
            r#"
            default_view = "core:default"
            "#,
        )
        .unwrap();

        let config = Config::load(&path).unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        session.views.last_mut().unwrap().input.raw = "2".to_string();
        session.views.last_mut().unwrap().input.cursor = 1;
        session.mark_input_changed().unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let committed_revision = session.views.last().unwrap().state.revision();
        assert_eq!(session.views.last().unwrap().committed_input(), "2");
        assert_eq!(
            config
                .render_parameter_input(&session.views.last().unwrap().state)
                .unwrap(),
            "2"
        );
        assert_eq!(session.runtime.snapshot()["view"]["current"]["input"], "2");

        session.views.last_mut().unwrap().input.raw = "bad".to_string();
        session.views.last_mut().unwrap().input.cursor = 3;
        session.mark_input_changed().unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "bad");
        assert_eq!(entry.committed_input(), "2");
        assert!(entry.state.input_rejected());
        assert_eq!(entry.state.revision(), committed_revision);
        assert_eq!(config.render_parameter_input(&entry.state).unwrap(), "2");
        assert_eq!(session.runtime.snapshot()["view"]["current"]["input"], "2");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "2"
        );

        session
            .apply_input_edit(InputEdit::SetBuffer {
                raw: "2".to_string(),
                cursor: 1,
            })
            .unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let committed_projection = session.runtime.snapshot()["view"]["current"].clone();
        assert!(
            session
                .apply_navigation(
                    NavigationRequest::with_defaults("core:child"),
                    NavigationMode::Push,
                    None,
                    None,
                )
                .unwrap()
        );
        session.views[0].runtime = Box::new(ParentTransactionView::new(
            Arc::new(Mutex::new(Vec::new())),
            false,
            false,
            false,
        ));

        session
            .pop_current(Some(InputEdit::SetBuffer {
                raw: "bad".to_string(),
                cursor: 3,
            }))
            .unwrap();

        let entry = session.views.last().unwrap();
        assert_eq!(entry.input.raw, "bad");
        assert_eq!(entry.input.cursor, 3);
        assert_eq!(entry.committed_input(), "2");
        assert!(entry.state.input_rejected());
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["input"],
            committed_projection["input"]
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            committed_projection["raw_input"]
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["cursor"],
            committed_projection["cursor"]
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["buffer_revision"],
            committed_projection["buffer_revision"]
        );
        assert_eq!(
            session.runtime.snapshot()["session"]["input"],
            serde_json::json!({
                "raw": committed_projection["raw_input"].clone(),
                "params": committed_projection["input"].clone(),
                "cursor": committed_projection["cursor"].clone(),
                "revision": committed_projection["buffer_revision"].clone(),
            })
        );

        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn route_completion_uses_shared_picker_bindings() {
        let completion = CompletionState {
            runtime: Box::new(
                CompletionRuntime::new(vec![
                    CompletionCandidate {
                        view_ref: "apps:main".to_string(),
                        alias: Some("app".to_string()),
                        plugin_name: "Applications".to_string(),
                        engine_type: "picker".to_string(),
                    },
                    CompletionCandidate {
                        view_ref: "system:main".to_string(),
                        alias: Some("sys".to_string()),
                        plugin_name: "System".to_string(),
                        engine_type: "capture".to_string(),
                    },
                ])
                .with_selected(1),
            ),
            ..CompletionState::default()
        };
        let mut theme = ResolvedTheme::terminal();
        theme.picker.text = Style::new().fg(Color::Red).bg(Color::Black);
        theme.picker.muted = Style::new().fg(Color::Green).bg(Color::Black);
        theme.picker.selected = Style::new().fg(Color::Yellow).bg(Color::Blue);
        theme.picker.marker = Style::new().fg(Color::Magenta).bg(Color::Black);
        let model = completion.render_model();
        let mut terminal = RatatuiTerminal::new(TestBackend::new(60, 2)).unwrap();

        terminal
            .draw(|frame| {
                render_route_completion(frame, frame.area(), &model, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let label = buffer.cell((2, 0)).unwrap();
        assert_eq!(label.symbol(), "a");
        assert_eq!(label.style().fg, Some(Color::Red));
        assert_eq!(label.style().bg, Some(Color::Black));
        let metadata = buffer.cell((7, 0)).unwrap();
        assert_eq!(metadata.symbol(), "a");
        assert_eq!(metadata.style().fg, Some(Color::Green));
        assert_eq!(metadata.style().bg, Some(Color::Black));
        let marker = buffer.cell((0, 1)).unwrap();
        assert_eq!(marker.symbol(), "▌");
        assert_eq!(marker.style().fg, Some(Color::Magenta));
        let selected = buffer.cell((2, 1)).unwrap();
        assert_eq!(selected.symbol(), "s");
        assert_eq!(selected.style().fg, Some(Color::Yellow));
        assert_eq!(selected.style().bg, Some(Color::Blue));
    }

    #[test]
    fn empty_route_completion_uses_the_muted_binding() {
        let completion = CompletionState::default();
        let mut theme = ResolvedTheme::terminal();
        theme.picker.muted = Style::new().fg(Color::Magenta).bg(Color::Green);
        let model = completion.render_model();
        let mut terminal = RatatuiTerminal::new(TestBackend::new(30, 1)).unwrap();

        terminal
            .draw(|frame| {
                render_route_completion(frame, frame.area(), &model, &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "(");
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn route_completion_replaces_the_selector_and_preserves_the_query_cursor() {
        let input = EditorBuffer {
            raw: "app query".to_string(),
            cursor: 7,
            revision: 0,
        };
        let completion = CompletionState {
            runtime: Box::new(CompletionRuntime::new(vec![CompletionCandidate {
                view_ref: "apps:main".to_string(),
                alias: Some("app".to_string()),
                plugin_name: "apps".to_string(),
                engine_type: "picker".to_string(),
            }])),
            host: crate::input::CompletionHost {
                parent: crate::input::CompletionSnapshot {
                    replacement_range: 0..3,
                    ..Default::default()
                },
                ..Default::default()
            },
        };

        assert_eq!(
            route_completion_edit(
                &input,
                &completion,
                &CompletionCandidate {
                    view_ref: "apps:main".to_string(),
                    alias: Some("app".to_string()),
                    plugin_name: "apps".to_string(),
                    engine_type: "picker".to_string(),
                },
            ),
            Some(crate::input::CompletionEdit {
                source_identity: crate::input::InputSourceIdentity {
                    frame: crate::input::ViewMountId(0),
                    generation: 0,
                },
                source_revision: 0,
                range: 0..3,
                replacement: "apps:main".to_string(),
                cursor: 13,
            })
        );
    }

    #[test]
    fn publishing_a_location_preserves_unrelated_runtime_data() {
        let mut runtime = crate::runtime::RuntimeStore::new();
        runtime
            .set("/invocation_marker", json!({"items": [1, 2]}))
            .unwrap();

        let config = crate::config::load_test_fixture().unwrap();
        let state = config.instantiate_parameters("core:default").unwrap();
        let input = EditorBuffer::new("query");
        publish_location(
            &mut runtime,
            &config,
            "core:default",
            &input,
            "query",
            &state,
        )
        .unwrap();
        publish_view_catalog(&mut runtime, &config).unwrap();

        assert_eq!(
            runtime.snapshot()["invocation_marker"]["items"],
            json!([1, 2])
        );
        assert_eq!(runtime.snapshot()["view"]["current"]["query"], "query");
        assert_eq!(runtime.snapshot()["view"]["current"]["command"], json!([]));
        let views = runtime.snapshot()["session"]["views"].as_array().unwrap();
        assert!(views.iter().any(|view| view["value"] == "core:default"));
    }
}
