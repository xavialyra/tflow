//! Protocol-native adapter for the Capture engine.
//!
//! Capture remains implemented by `CaptureView` and `CaptureRenderer`; this
//! module only translates their engine runtime contract to the common View
//! protocol. It is intentionally opt-in and is not used by legacy

use super::{CaptureKeymap, create_input_bindings, create_renderer, create_view};
use crate::engine::{
    ActionId, BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission,
    EngineNavigationRequest, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    EvaluatedBindingConfig, EvaluatedEngineConfig, RendererFactoryContext, RuntimeFactoryContext,
    ViewContext as EngineContext, ViewIdentity,
};
use crate::input::{EditorBuffer, InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationObserver;
use crate::parameter::ParameterSnapshot;
use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
use crate::theme::ResolvedTheme;
use crate::view::{
    Binding, BindingSet, EffectRequest, InputEvent, LifecycleEvent, NavigationRequest,
    RelativeCursor, RenderContext, RenderResult, TaskId, TaskOutcome, View, ViewCommandSnapshot,
    ViewContext, ViewDecision, ViewEvent, ViewInstanceId, ViewPublication, ViewResult,
};
use anyhow::{Context, Result, bail};
use ratatui::{Frame, layout::Rect};
use serde_json::Value;

/// Inputs required by the opt-in Capture adapter. Values must already be
/// evaluated in the same host scope that produced the navigation request.
pub(crate) struct CaptureProtocolConfig {
    pub(crate) commands: crate::protocol::ViewCommandBindings,
    pub(crate) identity: ViewIdentity,
    pub(crate) engine: EvaluatedEngineConfig,
    pub(crate) bindings: EvaluatedBindingConfig,
    pub(crate) cancellation: CancellationObserver,
    pub(crate) runtime_snapshot: Value,
    pub(crate) theme: ResolvedTheme,
    pub(crate) tasks: TaskRuntime,
}

impl CaptureProtocolConfig {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        view_ref: impl Into<String>,
        engine: EvaluatedEngineConfig,
        bindings: EvaluatedBindingConfig,
        commands: crate::protocol::ViewCommandBindings,
        cancellation: CancellationObserver,
        runtime_snapshot: Value,
        theme: ResolvedTheme,
        tasks: TaskRuntime,
    ) -> Self {
        Self {
            commands,
            identity: ViewIdentity::new(view_ref, crate::config::ENGINE_CAPTURE),
            engine,
            bindings,
            cancellation,
            runtime_snapshot,
            theme,
            tasks,
        }
    }
}

/// Construct a protocol View without exposing the concrete Capture runtime to
/// Router. The composition root supplies evaluated configuration and receives
/// a boxed common-protocol View.
pub(crate) fn create_protocol_view(
    config: CaptureProtocolConfig,
    request: &NavigationRequest,
    instance: ViewInstanceId,
) -> Result<Box<dyn View>> {
    Ok(Box::new(create_protocol_view_state(
        config, request, instance,
    )?))
}

fn create_protocol_view_state(
    config: CaptureProtocolConfig,
    request: &NavigationRequest,
    instance: ViewInstanceId,
) -> Result<CaptureProtocolView> {
    anyhow::ensure!(
        request.query.target == config.identity.view_ref,
        "capture request target {:?} does not match configured View {:?}",
        request.query.target,
        config.identity.view_ref
    );
    let mount_id = ViewMountId(instance.0);
    let parameters = ParameterSnapshot::from_parts(
        request.query.values.clone(),
        request
            .input
            .as_ref()
            .map(|seed| seed.text.clone())
            .unwrap_or_default(),
        InputSourceIdentity {
            frame: mount_id,
            generation: 0,
        },
        0,
    );
    let identity = config.identity.clone();
    let has_async_work = config
        .engine
        .field("output")
        .is_some_and(|output| !output.is_string());
    let runtime_context = RuntimeFactoryContext {
        identity: identity.clone(),
        config: config.engine,
        parameters: parameters.clone(),
        cancellation: config.cancellation,
    };
    let binding_context = crate::engine::InputBindingFactoryContext {
        identity: identity.clone(),
        bindings: config.bindings.clone(),
    };
    let keymap = CaptureKeymap::from_values(
        config.bindings.defaults.clone(),
        config.bindings.view_keymap.clone(),
    )?;
    let bindings = create_input_bindings(binding_context)?;
    let renderer = create_renderer(RendererFactoryContext)?;
    let runtime = create_view(runtime_context)?;
    let engine_context = engine_context(
        instance,
        &identity,
        &parameters,
        &config.runtime_snapshot,
        0,
        None,
    );
    let starter = MountTaskStarter::from_lease(&config.tasks, MountTaskLease::new(mount_id));
    let input_raw = parameters.raw_input().to_string();
    Ok(CaptureProtocolView {
        runtime,
        renderer,
        keymap,
        bindings,
        commands: config.commands,
        starter,
        runtime_snapshot: config.runtime_snapshot,
        parameters,
        publication: None,
        state_revision: 0,
        input_raw,
        theme: config.theme,
        engine_context,
        instance,
        has_async_work,
        task_generation: 0,
        active_task: None,
        pending_command: None,
        active: false,
        closed: false,
        content_size: (0, 0),
        status: None,
        error: None,
    })
}

struct CaptureProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    keymap: CaptureKeymap,
    bindings: Vec<crate::command::InputActionBinding>,
    commands: crate::protocol::ViewCommandBindings,
    starter: MountTaskStarter,
    runtime_snapshot: Value,
    parameters: ParameterSnapshot,
    publication: Option<ViewPublication>,
    state_revision: u64,
    input_raw: String,
    theme: ResolvedTheme,
    engine_context: EngineContext,
    instance: ViewInstanceId,
    has_async_work: bool,
    task_generation: u64,
    active_task: Option<AdapterTaskCorrelation>,
    pending_command: Option<crate::view::CommandRequest>,
    active: bool,
    closed: bool,
    content_size: (u16, u16),
    status: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdapterTaskCorrelation {
    instance: ViewInstanceId,
    task: TaskId,
    generation: u64,
}

fn apply_runtime_update(snapshot: &mut Value, path: &str, value: Value) -> Result<()> {
    *snapshot = crate::runtime::apply_pointer(snapshot, path, value)?;
    Ok(())
}

fn engine_context(
    instance: ViewInstanceId,
    identity: &ViewIdentity,
    parameters: &ParameterSnapshot,
    runtime: &Value,
    revision: u64,
    current: Option<&Value>,
) -> EngineContext {
    let mount_id = ViewMountId(instance.0);
    EngineContext::from_parts(crate::engine::ViewContextParts {
        mount_id,
        identity: identity.clone(),
        input: EditorBuffer::from_raw(parameters.raw_input(), parameters.raw_input().len())
            .snapshot(),
        input_generation: 0,
        parameters: parameters.clone(),
        input_rejected: false,
        runtime: EngineRuntimeSnapshot::new(
            runtime
                .pointer("/view/current")
                .cloned()
                .unwrap_or_else(|| runtime.clone()),
        ),
        current: current.cloned().unwrap_or(Value::Null),
        revision,
    })
}

impl CaptureProtocolView {
    fn sync_context(&mut self, context: &ViewContext) -> Result<()> {
        anyhow::ensure!(
            context.instance == self.instance,
            "capture protocol context belongs to {:?}, expected {:?}",
            context.instance,
            self.instance
        );
        self.engine_context = engine_context(
            self.instance,
            &self.engine_context.identity,
            &self.parameters,
            &self.runtime_snapshot,
            self.state_revision,
            self.publication
                .as_ref()
                .map(|publication| &publication.current),
        );
        Ok(())
    }

    fn begin_adapter_task(&mut self) {
        if !self.has_async_work {
            self.active_task = None;
            return;
        }
        self.task_generation = self.task_generation.wrapping_add(1).max(1);
        self.active_task = Some(AdapterTaskCorrelation {
            instance: self.instance,
            task: TaskId(1),
            generation: self.task_generation,
        });
    }

    fn task_matches_context(&self, context: &ViewContext) -> bool {
        self.active_task
            .is_none_or(|task| task.instance == context.instance)
    }

    fn cancel_stale_adapter_task(&mut self) {
        self.runtime.deactivate();
        self.active_task = None;
    }

    fn apply_publication(&mut self, _: &ViewContext, emission: &EngineEmission) {
        if let Some(publication) = emission.publication() {
            self.publication = Some(ViewPublication {
                current: publication.current().clone(),
                ready: publication.ready,
            });
            self.state_revision = self.state_revision.wrapping_add(1);
        }
    }

    fn apply_notice(&mut self, notice: &crate::engine::EngineNotice) {
        match notice {
            crate::engine::EngineNotice::Info { message, .. } => {
                self.status = Some(message.clone());
                self.error = None;
            }
            crate::engine::EngineNotice::Error { message, .. } => {
                self.status = None;
                self.error = Some(message.clone());
            }
            crate::engine::EngineNotice::ClearError => self.error = None,
        }
    }

    fn decision(
        &mut self,
        context: &ViewContext,
        emission: EngineEmission,
    ) -> Result<ViewDecision> {
        let published = emission.publication().is_some();
        self.apply_publication(context, &emission);
        let decision = self.engine_decision(context, emission.decision_ref().clone())?;
        if !published {
            return Ok(decision);
        }
        let Some(pending) = self.dispatch_pending_command(context)? else {
            return Ok(decision);
        };
        Ok(match decision {
            ViewDecision::Stay => pending,
            ViewDecision::Invalidate => {
                ViewDecision::Batch(vec![ViewDecision::Invalidate, pending])
            }
            ViewDecision::Batch(mut decisions) if decisions.iter().all(passive_decision) => {
                decisions.push(pending);
                ViewDecision::Batch(decisions)
            }
            _ => bail!("a pending Capture command cannot follow a structural decision"),
        })
    }

    fn dispatch_pending_command(&mut self, _: &ViewContext) -> Result<Option<ViewDecision>> {
        if !self
            .publication
            .as_ref()
            .is_some_and(|publication| publication.ready)
        {
            return Ok(None);
        }
        Ok(self
            .pending_command
            .take()
            .map(ViewDecision::RequestCommand))
    }

    fn engine_decision(
        &mut self,
        context: &ViewContext,
        decision: EngineDecision,
    ) -> Result<ViewDecision> {
        match decision {
            EngineDecision::Continue => Ok(ViewDecision::Stay),
            EngineDecision::Invalidate => Ok(ViewDecision::Invalidate),
            EngineDecision::Report(notice) => {
                self.apply_notice(&notice);
                Ok(ViewDecision::Invalidate)
            }
            EngineDecision::RuntimeUpdate(update) => {
                apply_runtime_update(&mut self.runtime_snapshot, &update.path, update.value)?;
                self.state_revision = self.state_revision.wrapping_add(1);
                self.sync_context(context)?;
                Ok(ViewDecision::Invalidate)
            }
            EngineDecision::Execute(crate::engine::EffectRequest::CopyToClipboard(value)) => {
                Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(value)))
            }
            EngineDecision::Close => {
                self.pending_command = None;
                Ok(ViewDecision::Close)
            }
            EngineDecision::Exit => {
                self.pending_command = None;
                Ok(ViewDecision::Exit)
            }
            EngineDecision::Return(output) => {
                self.pending_command = None;
                Ok(ViewDecision::Return(ViewResult {
                    value: serde_json::to_value(output)
                        .context("could not serialize capture result")?,
                    adapter: None,
                }))
            }
            EngineDecision::Batch(decisions) => {
                let mut mapped = Vec::with_capacity(decisions.len());
                for decision in decisions {
                    mapped.push(self.engine_decision(context, decision)?);
                }
                Ok(ViewDecision::Batch(mapped))
            }
            EngineDecision::Navigate(EngineNavigationRequest { .. }) => {
                bail!("Capture protocol View cannot handle this Engine decision")
            }
        }
    }

    fn poll_active(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        if !self.task_matches_context(context) {
            self.cancel_stale_adapter_task();
            return Ok(ViewDecision::Stay);
        }
        let polled = match self.runtime.poll_work() {
            Ok(polled) => polled,
            Err(error) => {
                self.pending_command = None;
                return Err(error);
            }
        };
        if let Some(emission) = polled {
            self.active_task = None;
            return self.decision(context, emission);
        }
        let emission = match self.runtime.tick(EngineTick {
            context: self.engine_context.clone(),
            content_size: self.content_size,
        }) {
            Ok(emission) => emission,
            Err(error) => {
                self.pending_command = None;
                return Err(error);
            }
        };
        self.decision(context, emission)
    }

    fn poll_covered(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        if !self.task_matches_context(context) {
            self.cancel_stale_adapter_task();
            return Ok(ViewDecision::Stay);
        }
        let Some(BackgroundOutcome {
            notices,
            publication,
        }) = self.runtime.poll_background_work()?
        else {
            return Ok(ViewDecision::Stay);
        };
        self.active_task = None;
        for notice in notices {
            self.apply_notice(&notice);
        }
        if let Some(publication) = publication {
            self.publication = Some(ViewPublication {
                current: publication.current,
                ready: publication.ready,
            });
            self.state_revision = self.state_revision.wrapping_add(1);
        }
        Ok(ViewDecision::Invalidate)
    }

    fn action(&mut self, context: &ViewContext, action: ActionId) -> Result<ViewDecision> {
        let emission = self.runtime.action(EngineActionInput {
            invocation: crate::engine::ActionInvocation::new(action),
            context: self.engine_context.clone(),
        })?;
        self.decision(context, emission)
    }
}

fn passive_decision(decision: &ViewDecision) -> bool {
    match decision {
        ViewDecision::Stay | ViewDecision::Invalidate => true,
        ViewDecision::Batch(decisions) => decisions.iter().all(passive_decision),
        _ => false,
    }
}

impl View for CaptureProtocolView {
    fn bindings(&self, _: &ViewContext) -> BindingSet {
        let commands = self.commands.view_bindings();
        BindingSet::new(
            commands.entries().iter().cloned().chain(
                self.bindings
                    .iter()
                    .filter(|binding| binding.enabled)
                    .map(|binding| Binding {
                        key: binding.key,
                        label: binding.label.clone(),
                    }),
            ),
        )
    }

    fn chrome(&self, context: &ViewContext) -> Result<crate::view::ViewChrome> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let chrome = self.renderer.chrome(&model);
        Ok(crate::view::ViewChrome {
            title: chrome.title,
            status: self.status.clone().or(chrome.status),
            error: self.error.clone(),
            bindings: Some(self.bindings(context)),
        })
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            parameters: self.parameters.values().clone(),
            raw_input: self.input_raw.clone(),
            runtime: self.runtime_snapshot.clone(),
            publication: self.publication.clone(),
            revision: self.state_revision,
        }
    }

    fn event(&mut self, event: ViewEvent, context: &ViewContext) -> Result<ViewDecision> {
        self.sync_context(context)?;
        match event {
            ViewEvent::Lifecycle(LifecycleEvent::Mounted) => Ok(ViewDecision::Stay),
            ViewEvent::Lifecycle(LifecycleEvent::Activated) => {
                self.active = true;
                let emission = self.runtime.activate(self.engine_context.clone())?;
                let decision = self.decision(context, emission)?;
                self.begin_adapter_task();
                let starter = self.active_task.map_or_else(
                    || self.starter.clone(),
                    |task| self.starter.for_task(task.task, task.generation),
                );
                self.runtime
                    .start_prepared_work(&starter, &self.runtime_snapshot);
                Ok(decision)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Covered) => {
                self.active = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                self.active_task = None;
                self.pending_command = None;
                self.runtime.deactivate();
                self.starter.cancel_all();
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closed) => {
                self.closed = true;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(
                LifecycleEvent::TransitionCommitted { .. }
                | LifecycleEvent::TransitionRejected { .. },
            ) => Ok(ViewDecision::Stay),
            ViewEvent::Command(crate::view::CommandResult::EditInput { .. }) => Err(
                crate::view::operation_failure("Capture does not provide an editable input"),
            ),
            ViewEvent::Input(InputEvent::Key { key, raw: _ }) => {
                if let Some(binding) = self.commands.binding(key) {
                    if !self
                        .publication
                        .as_ref()
                        .is_some_and(|publication| publication.ready)
                    {
                        let ViewDecision::RequestCommand(request) =
                            self.commands.request(binding, None)
                        else {
                            unreachable!("command binding must produce a command request")
                        };
                        self.pending_command = Some(request);
                        return Ok(ViewDecision::Stay);
                    }
                    return Ok(self.commands.request(binding, None));
                }
                let Some(action) = self.keymap.action(key) else {
                    return Ok(ViewDecision::Stay);
                };
                let id = match action {
                    super::CaptureAction::Copy => "capture.copy",
                    super::CaptureAction::Back => "capture.back",
                };
                self.action(context, ActionId::new(id))
            }
            ViewEvent::Input(InputEvent::Eof) => Ok(ViewDecision::Exit),
            ViewEvent::Task(task) => {
                let Some(correlation) = self.active_task else {
                    return Ok(ViewDecision::Stay);
                };
                if task.instance != correlation.instance
                    || task.task != correlation.task
                    || task.generation != correlation.generation
                {
                    return Ok(ViewDecision::Stay);
                }
                match task.outcome {
                    TaskOutcome::Completed(_) if self.active => self.poll_active(context),
                    TaskOutcome::Completed(_) => self.poll_covered(context),
                    TaskOutcome::Failed(message) => {
                        self.active_task = None;
                        self.pending_command = None;
                        self.status = None;
                        self.error = Some(message);
                        Ok(ViewDecision::Invalidate)
                    }
                    TaskOutcome::Cancelled => {
                        self.active_task = None;
                        self.pending_command = None;
                        Ok(ViewDecision::Stay)
                    }
                }
            }
            ViewEvent::Tick if self.active => {
                #[cfg(test)]
                return self.poll_active(context);
                #[cfg(not(test))]
                let emission = self.runtime.tick(EngineTick {
                    context: self.engine_context.clone(),
                    content_size: self.content_size,
                })?;
                #[cfg(not(test))]
                return self.decision(context, emission);
            }
            ViewEvent::Tick => {
                #[cfg(test)]
                return self.poll_covered(context);
                #[cfg(not(test))]
                return Ok(ViewDecision::Stay);
            }
            ViewEvent::Resize(size) => {
                self.content_size = (size.width, size.height);
                Ok(ViewDecision::Invalidate)
            }
            ViewEvent::Input(InputEvent::Paste { .. }) | ViewEvent::Input(InputEvent::Bytes(_)) => {
                Ok(ViewDecision::Stay)
            }
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, _: &RenderContext) -> Result<RenderResult> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let context = crate::engine::RenderContext {
            theme: self.theme,
            image_picker: None,
        };
        self.renderer.render(&model, &context, frame, area);
        let chrome = self.renderer.chrome(&model);
        Ok(RenderResult {
            cursor: Some(RelativeCursor {
                x: 0,
                y: 0,
                visible: false,
            }),
            metadata: crate::view::ViewMetadata {
                title: chrome.title,
                status: self.status.clone().or(chrome.status),
                error: self.error.clone(),
                bindings: Some(self.bindings(&ViewContext::new(ViewInstanceId(0), "capture"))),
            },
        })
    }
}

impl Drop for CaptureProtocolView {
    fn drop(&mut self) {
        if !self.closed {
            self.runtime.deactivate();
            self.starter.cancel_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EvaluatedBindingConfig, EvaluatedEngineConfig};
    use crate::view::{ParsedQuery, ViewContext};
    use ratatui::{Terminal, backend::TestBackend};

    fn request() -> NavigationRequest {
        NavigationRequest::new("capture", ParsedQuery::new("capture", "query", Value::Null))
    }

    fn config(output: Value) -> CaptureProtocolConfig {
        CaptureProtocolConfig::new(
            "capture",
            EvaluatedEngineConfig {
                fields: [("output".to_string(), output)].into_iter().collect(),
                ..EvaluatedEngineConfig::default()
            },
            EvaluatedBindingConfig::default(),
            crate::protocol::ViewCommandBindings::new(
                &crate::config::load_test_fixture().unwrap(),
                "capture",
                crate::lifecycle::CancellationToken::new().observer(),
                &[],
            )
            .unwrap(),
            crate::lifecycle::CancellationToken::new().observer(),
            Value::Null,
            ResolvedTheme::terminal(),
            TaskRuntime::new(),
        )
    }

    fn context() -> ViewContext {
        ViewContext::new(ViewInstanceId(1), "capture")
    }

    #[test]
    fn engine_context_keeps_host_snapshot_identity_revision_and_publication() {
        let parameters = ParameterSnapshot::from_parts(
            serde_json::json!({"query": "value"}),
            "value".to_string(),
            InputSourceIdentity {
                frame: ViewMountId(9),
                generation: 3,
            },
            7,
        );
        let identity = ViewIdentity::new("capture", crate::config::ENGINE_CAPTURE);
        let publication = serde_json::json!({"value": "captured"});
        let context = engine_context(
            ViewInstanceId(9),
            &identity,
            &parameters,
            &serde_json::json!({
                "view": {"current": {"query": "value"}},
                "session": {"input": "value"}
            }),
            42,
            Some(&publication),
        );
        assert_eq!(context.mount_id(), ViewMountId(9));
        assert_eq!(context.revision(), 42);
        assert_eq!(context.current, publication);
        assert_eq!(
            context.runtime_snapshot().current(),
            &serde_json::json!({"query": "value"})
        );
    }

    #[test]
    fn runtime_updates_apply_json_pointer_without_replacing_the_host_snapshot() {
        let mut snapshot = serde_json::json!({
            "view": {"current": {"status": "old", "items": []}},
            "session": {"input": "kept"}
        });
        apply_runtime_update(
            &mut snapshot,
            "/view/current/status",
            Value::String("new".to_string()),
        )
        .unwrap();
        assert_eq!(snapshot["view"]["current"]["status"], "new");
        assert_eq!(snapshot["view"]["current"]["items"], serde_json::json!([]));
        assert_eq!(snapshot["session"]["input"], "kept");
    }

    #[test]
    fn mismatched_task_generation_or_revision_cannot_consume_capture_completion() {
        let output = serde_json::json!({"source": "script", "file": "capture.sh"});
        let mut view =
            create_protocol_view_state(config(output), &request(), ViewInstanceId(1)).unwrap();
        let mut context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &mut context)
            .unwrap();
        view.event(
            ViewEvent::Lifecycle(LifecycleEvent::Activated),
            &mut context,
        )
        .unwrap();
        let correlation = view.active_task.expect("async capture must be correlated");
        let before = view.command_snapshot();
        view.event(
            ViewEvent::Task(crate::view::TaskEvent {
                instance: correlation.instance,
                task: correlation.task,
                generation: correlation.generation + 1,
                outcome: crate::view::TaskOutcome::Completed(Value::Null),
            }),
            &context,
        )
        .unwrap();
        assert_eq!(view.command_snapshot(), before);
        view.event(
            ViewEvent::Task(crate::view::TaskEvent {
                instance: correlation.instance,
                task: TaskId(correlation.task.0 + 1),
                generation: correlation.generation,
                outcome: crate::view::TaskOutcome::Completed(Value::Null),
            }),
            &context,
        )
        .unwrap();
        assert_eq!(view.command_snapshot(), before);
    }

    #[test]
    fn copy_and_back_use_common_view_decisions() {
        let mut view = create_protocol_view(
            config(Value::String("captured".into())),
            &request(),
            ViewInstanceId(1),
        )
        .unwrap();
        let mut context = context();
        assert!(matches!(
            view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &mut context)
                .unwrap(),
            ViewDecision::Stay
        ));
        assert!(matches!(
            view.event(
                ViewEvent::Lifecycle(LifecycleEvent::Activated),
                &mut context
            )
            .unwrap(),
            ViewDecision::Stay
        ));
        assert!(
            matches!(view.event(ViewEvent::Input(InputEvent::Key { key: crate::view::Key::Enter, raw: b"\r".to_vec() }), &mut context).unwrap(), ViewDecision::Effect(EffectRequest::CopyToClipboard(value)) if value == "captured")
        );
        assert!(matches!(
            view.event(
                ViewEvent::Input(InputEvent::Key {
                    key: crate::view::Key::Escape,
                    raw: vec![0x1b]
                }),
                &mut context
            )
            .unwrap(),
            ViewDecision::Close
        ));
    }

    #[test]
    fn tick_publishes_capture_result_and_render_is_complete() {
        let mut view = create_protocol_view(
            config(Value::String("captured".into())),
            &request(),
            ViewInstanceId(1),
        )
        .unwrap();
        let mut context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &mut context)
            .unwrap();
        view.event(
            ViewEvent::Lifecycle(LifecycleEvent::Activated),
            &mut context,
        )
        .unwrap();
        view.event(ViewEvent::Tick, &mut context).unwrap();
        let publication = view.command_snapshot().publication.unwrap();
        assert_eq!(
            publication.current,
            serde_json::json!({"value": "captured"})
        );
        assert!(publication.ready);
        let mut terminal = Terminal::new(TestBackend::new(20, 3)).unwrap();
        let mut rendered = None;
        terminal
            .draw(|frame| {
                rendered = Some(
                    view.render(
                        frame,
                        frame.area(),
                        &RenderContext::for_terminal(crate::view::TerminalSize {
                            width: 20,
                            height: 3,
                        }),
                    )
                    .unwrap(),
                );
            })
            .unwrap();
        assert_eq!(
            rendered.unwrap().metadata.title.as_deref(),
            Some("capture: capture")
        );
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("captured"));
    }

    #[test]
    fn invalid_async_capture_reports_error_without_panicking() {
        let root = std::env::temp_dir().join(format!(
            "tui-launcher-protocol-capture-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("fail.sh");
        std::fs::write(&script, "#!/bin/sh\necho failed >&2\nexit 3\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = serde_json::json!({"source":"script", "file":"fail.sh"});
        let mut cfg = config(output);
        cfg.engine.plugin_root = Some(root.clone());
        let mut view = create_protocol_view(cfg, &request(), ViewInstanceId(1)).unwrap();
        let mut context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &mut context)
            .unwrap();
        view.event(
            ViewEvent::Lifecycle(LifecycleEvent::Activated),
            &mut context,
        )
        .unwrap();
        for _ in 0..200 {
            view.event(ViewEvent::Tick, &mut context).unwrap();
            if view.command_snapshot().revision > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(view.command_snapshot().revision > 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
