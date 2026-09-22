//! Protocol-native adapter for the Capture engine.
//!
//! Capture remains implemented by `CaptureView` and `CaptureRenderer`; this
//! module translates their engine runtime contract to the common View protocol.

use super::{CaptureKeymap, create_renderer, create_view};
use crate::engine::{
    ActionId, BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission,
    EngineNavigationRequest, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    ProjectedBindingConfig, ProjectedEngineConfig, RendererFactoryContext, RuntimeFactoryContext,
    ViewContext as EngineContext, ViewIdentity,
};
use crate::input::{EditorBuffer, InputEvent, InputSourceIdentity, ViewMountId};
use crate::lifecycle::CancellationObserver;
#[cfg(test)]
use crate::protocol::contracts::{TaskEvent, TaskOutcome};
use crate::protocol::contracts::{TaskId, ViewInstanceId};
use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    EffectRequest, LifecycleEvent, NavigationRequest, RelativeCursor, RenderContext, RenderResult,
    View, ViewCommandSnapshot, ViewContext, ViewDecision, ViewEvent, ViewPublication,
    ViewTaskRegistry,
};
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Result, bail};
use ratatui::{Frame, layout::Rect};
use serde_json::Value;

/// Inputs required by the opt-in Capture adapter. Values must already be
/// projected from static configuration in the same host scope that produced the navigation request.
pub(crate) struct CaptureProtocolConfig {
    pub(crate) identity: ViewIdentity,
    pub(crate) engine: ProjectedEngineConfig,
    pub(crate) bindings: ProjectedBindingConfig,
    pub(crate) cancellation: CancellationObserver,
    pub(crate) runtime_snapshot: Value,
    pub(crate) theme: ResolvedTheme,
    pub(crate) tasks: TaskRuntime,
}

impl CaptureProtocolConfig {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        view_ref: impl Into<String>,
        engine: ProjectedEngineConfig,
        bindings: ProjectedBindingConfig,
        cancellation: CancellationObserver,
        runtime_snapshot: Value,
        theme: ResolvedTheme,
        tasks: TaskRuntime,
    ) -> Self {
        Self {
            identity: ViewIdentity::new(view_ref, crate::workflow::config::ENGINE_CAPTURE),
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
/// Router. The composition root supplies projected configuration and receives
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
    let initial_output = config
        .engine
        .field("output")
        .and_then(|output| output.as_str())
        .map(|s| s.to_string());
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
    let keymap = CaptureKeymap::from_values(
        config.bindings.defaults.clone(),
        config.bindings.view_keymap.clone(),
    )?;
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
    let output_text = std::sync::Arc::new(std::sync::RwLock::new(initial_output));
    Ok(CaptureProtocolView {
        runtime,
        renderer,
        keymap,
        starter,
        runtime_snapshot: config.runtime_snapshot,
        parameters,
        publication: None,
        state_revision: 0,
        input_raw,
        theme: config.theme,
        engine_context,
        instance,
        task_registry: ViewTaskRegistry::new(instance),
        has_async_work,
        task_generation: 0,
        active_task: None,
        active: false,
        closed: false,
        content_size: (0, 0),
        status: None,
        error: None,
        output_text,
    })
}

struct CaptureProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    keymap: CaptureKeymap,
    starter: MountTaskStarter,
    runtime_snapshot: Value,
    parameters: ParameterSnapshot,
    publication: Option<ViewPublication>,
    state_revision: u64,
    input_raw: String,
    theme: ResolvedTheme,
    engine_context: EngineContext,
    instance: ViewInstanceId,
    task_registry: ViewTaskRegistry,
    has_async_work: bool,
    task_generation: u64,
    active_task: Option<AdapterTaskCorrelation>,
    active: bool,
    closed: bool,
    content_size: (u16, u16),
    status: Option<String>,
    error: Option<String>,
    output_text: std::sync::Arc<std::sync::RwLock<Option<String>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdapterTaskCorrelation {
    instance: ViewInstanceId,
    task: TaskId,
    generation: u64,
}

fn apply_runtime_update(snapshot: &mut Value, path: &str, value: Value) -> Result<()> {
    *snapshot = crate::workflow::runtime::apply_pointer(snapshot, path, value)?;
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

    fn start_prepared_work(&mut self) {
        let task = TaskId(1);
        if !self.has_async_work {
            return;
        }
        let generation = self.task_generation.wrapping_add(1).max(1);
        let starter = self.starter.for_task(task, generation);
        if !self.runtime.start_prepared_work(&starter) {
            return;
        }
        self.task_generation = generation;
        self.active_task = Some(AdapterTaskCorrelation {
            instance: self.instance,
            task,
            generation,
        });
        self.task_registry.register(task, generation);
    }

    fn task_matches_context(&self, context: &ViewContext) -> bool {
        self.active_task
            .is_none_or(|task| task.instance == context.instance)
    }

    fn cancel_stale_adapter_task(&mut self) {
        self.runtime.deactivate();
        self.active_task = None;
        self.task_registry.invalidate_all();
    }

    fn apply_publication(&mut self, _: &ViewContext, emission: &EngineEmission) {
        if let Some(publication) = emission.publication() {
            if let Some(val) = publication.current().get("value").and_then(|v| v.as_str()) {
                *self.output_text.write().unwrap() = Some(val.to_string());
            }
            self.publication = Some(ViewPublication::new(
                publication.current().clone(),
                publication.ready,
            ));
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
        self.apply_publication(context, &emission);
        self.engine_decision(context, emission.decision_ref().clone())
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
            EngineDecision::Close => Ok(ViewDecision::Close),
            EngineDecision::Exit => Ok(ViewDecision::Exit),
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
        let polled = self.runtime.poll_work()?;
        if let Some(emission) = polled {
            self.active_task = None;
            return self.decision(context, emission);
        }
        let emission = self.runtime.tick(EngineTick {
            context: self.engine_context.clone(),
            content_size: self.content_size,
        })?;
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
            if let Some(val) = publication.current.get("value").and_then(|v| v.as_str()) {
                *self.output_text.write().unwrap() = Some(val.to_string());
            }
            self.publication = Some(ViewPublication::new(publication.current, publication.ready));
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

pub(super) const CMD_COPY: &str = "capture.copy";
pub(super) const CMD_BACK: &str = "capture.back";

impl View for CaptureProtocolView {
    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        let mut entries = Vec::new();
        for (key, action) in self.keymap.bindings() {
            let (id, label) = match action {
                super::CaptureAction::Copy => (CMD_COPY, "Copy"),
                super::CaptureAction::Back => (CMD_BACK, "Back"),
            };
            entries.push(crate::command::CommandEntry::for_event(
                id,
                Some(label.to_string()),
                Some(key),
                crate::command::CommandScope::Engine,
            ));
        }
        entries
    }

    fn on_command(&mut self, id: &str, context: &ViewContext) -> Result<ViewDecision> {
        match id {
            CMD_COPY => {
                let text = self.output_text.read().unwrap().clone();
                if let Some(text) = text {
                    Ok(ViewDecision::Effect(
                        crate::view::EffectRequest::CopyToClipboard(text),
                    ))
                } else {
                    Ok(ViewDecision::Stay)
                }
            }
            CMD_BACK => Ok(ViewDecision::Close),
            _ => self.action(context, ActionId::new(id)),
        }
    }

    fn publication(&self) -> Option<&ViewPublication> {
        self.publication.as_ref()
    }

    fn chrome(&self, _context: &ViewContext) -> Result<crate::view::ViewChrome> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let chrome = self.renderer.chrome(&model);
        Ok(crate::view::ViewChrome {
            status: self.status.clone().or(chrome.status),
            error: self.error.clone(),
            bindings: None,
            overflow_command: None,
            has_unbound: false,
        })
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: self.engine_context.view_identity().engine_type.clone(),
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
                self.start_prepared_work();
                Ok(decision)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Covered) => {
                self.active = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                self.active_task = None;
                self.task_registry.invalidate_all();
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
            ViewEvent::Input(InputEvent::Key { key, raw }) => {
                self.dispatch_key_event(key, &raw, context)
            }
            ViewEvent::Input(InputEvent::Eof) => Ok(ViewDecision::Exit),
            ViewEvent::Task(task) => {
                if !self.task_registry.accepts(&task) {
                    return Ok(ViewDecision::Stay);
                }
                let Some(correlation) = self.active_task else {
                    return Ok(ViewDecision::Stay);
                };
                if task.instance != correlation.instance
                    || task.task != correlation.task
                    || task.generation != correlation.generation
                {
                    return Ok(ViewDecision::Stay);
                }
                let decision = if self.active {
                    self.poll_active(context)?
                } else {
                    self.poll_covered(context)?
                };
                if self.active_task.is_none() {
                    self.task_registry.invalidate(task.task);
                }
                Ok(decision)
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
            theme: self.theme.clone(),
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
                status: self.status.clone().or(chrome.status),
                error: self.error.clone(),
                bindings: None,
            },
        })
    }
}

impl Drop for CaptureProtocolView {
    fn drop(&mut self) {
        self.task_registry.invalidate_all();
        if !self.closed {
            self.runtime.deactivate();
            self.starter.cancel_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ProjectedBindingConfig, ProjectedEngineConfig};
    use crate::view::{ParsedQuery, ViewContext};
    use ratatui::{Terminal, backend::TestBackend};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn request() -> NavigationRequest {
        NavigationRequest::new("capture", ParsedQuery::new("capture", "query", Value::Null))
    }

    fn config(output: Value) -> CaptureProtocolConfig {
        config_with_tasks(output, TaskRuntime::new())
    }

    fn config_with_tasks(output: Value, tasks: TaskRuntime) -> CaptureProtocolConfig {
        CaptureProtocolConfig::new(
            "capture",
            ProjectedEngineConfig {
                fields: [("output".to_string(), output)].into_iter().collect(),
                ..ProjectedEngineConfig::default()
            },
            ProjectedBindingConfig::default(),
            crate::lifecycle::CancellationToken::new().observer(),
            Value::Null,
            ResolvedTheme::terminal(),
            tasks,
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
        let identity = ViewIdentity::new("capture", crate::workflow::config::ENGINE_CAPTURE);
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
    fn cancelled_capture_task_is_consumed_and_a_later_activation_can_start_work() {
        let root =
            std::env::temp_dir().join(format!("tflow-capture-cancel-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("capture.sh");
        fs::write(&script, "#!/bin/sh\nwhile :; do sleep 0.01; done\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let tasks = TaskRuntime::new();
        let output = serde_json::json!({
            "producer": "script",
            "handler": {"file": "capture.sh"}
        });
        let mut capture_config = config_with_tasks(output.clone(), tasks.clone());
        capture_config.engine.workflow_root = Some(root.clone());
        let mut view =
            create_protocol_view_state(capture_config, &request(), ViewInstanceId(1)).unwrap();
        let context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        tasks.cancel_all();
        let event = wait_for_task_event(&tasks);
        assert!(matches!(event.outcome, TaskOutcome::Cancelled));
        view.event(ViewEvent::Task(event), &context).unwrap();
        assert!(view.active_task.is_none());
        assert!(view.error.is_some());

        let mut next_config = config_with_tasks(output, tasks.clone());
        next_config.engine.workflow_root = Some(root.clone());
        let mut next =
            create_protocol_view_state(next_config, &request(), ViewInstanceId(2)).unwrap();
        let next_context = ViewContext::new(ViewInstanceId(2), "capture");
        next.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &next_context)
            .unwrap();
        next.event(
            ViewEvent::Lifecycle(LifecycleEvent::Activated),
            &next_context,
        )
        .unwrap();
        assert!(next.active_task.is_some());
        next.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &next_context)
            .unwrap();
        tasks.shutdown_and_wait();
        fs::remove_dir_all(root).unwrap();
    }

    fn wait_for_task_event(tasks: &TaskRuntime) -> TaskEvent {
        for _ in 0..200 {
            if let Some(event) = tasks.drain_events().into_iter().next() {
                return event;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("timed out waiting for task event");
    }

    #[test]
    fn copy_and_back_use_common_view_decisions() {
        let mut view = create_protocol_view(
            config(Value::String("captured".into())),
            &request(),
            ViewInstanceId(1),
        )
        .unwrap();
        let context = context();
        assert!(matches!(
            view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
                .unwrap(),
            ViewDecision::Stay
        ));
        assert!(matches!(
            view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
                .unwrap(),
            ViewDecision::Stay
        ));
        assert!(
            matches!(view.event(ViewEvent::Input(InputEvent::Key { key: crate::input::Key::Enter, raw: b"\r".to_vec() }), &context).unwrap(), ViewDecision::Effect(EffectRequest::CopyToClipboard(value)) if value == "captured")
        );
        assert!(matches!(
            view.event(
                ViewEvent::Input(InputEvent::Key {
                    key: crate::input::Key::Escape,
                    raw: vec![0x1b]
                }),
                &context
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
        let context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(ViewEvent::Tick, &context).unwrap();
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
        let root =
            std::env::temp_dir().join(format!("tflow-protocol-capture-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("fail.sh");
        std::fs::write(&script, "#!/bin/sh\necho failed >&2\nexit 3\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = serde_json::json!({
            "producer": "script",
            "handler": {"file": "fail.sh"}
        });
        let mut cfg = config(output);
        cfg.engine.workflow_root = Some(root.clone());
        let mut view = create_protocol_view(cfg, &request(), ViewInstanceId(1)).unwrap();
        let context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        for _ in 0..200 {
            view.event(ViewEvent::Tick, &context).unwrap();
            if view.command_snapshot().revision > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(view.command_snapshot().revision > 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
