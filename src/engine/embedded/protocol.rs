//! Protocol-native adapter for the Embedded engine.
//!
//! The PTY and terminal implementation remain owned by the existing
//! Embedded Engine runtime. This adapter only translates the runtime's

use super::{create_input_bindings, create_renderer, create_view};
use crate::engine::{
    ActionId, EngineActionInput, EngineDecision, EngineEmission, EngineNavigationRequest,
    EngineRuntime, EngineRuntimeSnapshot, EngineTick, ExternalTickAction, ExternalTickResult,
    InputBindingFactoryContext, ProjectedBindingConfig, ProjectedEngineConfig, RawInputReceiver,
    RendererFactoryContext, RuntimeFactoryContext, ViewContext as EngineContext, ViewIdentity,
};
use crate::input::InputEvent;
use crate::input::{EditorSnapshot, ViewMountId};
use crate::lifecycle::CancellationObserver;
use crate::protocol::contracts::ViewInstanceId;
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    EffectRequest, LifecycleEvent, RelativeCursor, RenderContext, RenderResult, View,
    ViewCommandSnapshot, ViewContext, ViewDecision, ViewEvent, ViewPublication, ViewResult,
};
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Context, Result, bail};
use ratatui::{Frame, layout::Rect};
use serde_json::Value;

pub(crate) struct EmbeddedProtocolConfig {
    pub(crate) identity: ViewIdentity,
    pub(crate) engine: ProjectedEngineConfig,
    pub(crate) bindings: ProjectedBindingConfig,
    pub(crate) cancellation: CancellationObserver,
    pub(crate) runtime_snapshot: Value,
    pub(crate) parameters: ParameterSnapshot,
    pub(crate) theme: ResolvedTheme,
}

impl EmbeddedProtocolConfig {
    pub(crate) fn new(
        view_ref: impl Into<String>,
        engine: ProjectedEngineConfig,
        bindings: ProjectedBindingConfig,
        cancellation: CancellationObserver,
        runtime_snapshot: Value,
        parameters: ParameterSnapshot,
        theme: ResolvedTheme,
    ) -> Self {
        Self {
            identity: ViewIdentity::new(view_ref, crate::workflow::config::ENGINE_EMBEDDED),
            engine,
            bindings,
            cancellation,
            runtime_snapshot,
            parameters,
            theme,
        }
    }
}

pub(crate) fn create_protocol_view(
    config: EmbeddedProtocolConfig,
    request: &crate::view::NavigationRequest,
    instance: ViewInstanceId,
) -> Result<Box<dyn View>> {
    Ok(Box::new(create_protocol_view_state(
        config, request, instance,
    )?))
}

fn create_protocol_view_state(
    config: EmbeddedProtocolConfig,
    request: &crate::view::NavigationRequest,
    instance: ViewInstanceId,
) -> Result<EmbeddedProtocolView> {
    anyhow::ensure!(
        request.query.target == config.identity.view_ref,
        "embedded request target {:?} does not match configured View {:?}",
        request.query.target,
        config.identity.view_ref
    );
    let parameters = config.parameters;
    let identity = config.identity.clone();
    let runtime = create_view(RuntimeFactoryContext {
        identity: identity.clone(),
        config: config.engine,
        parameters: parameters.clone(),
        cancellation: config.cancellation,
    })?;
    let bindings = create_input_bindings(InputBindingFactoryContext {
        identity: identity.clone(),
        bindings: config.bindings,
    })?;
    let renderer = create_renderer(RendererFactoryContext)?;
    let engine_context = engine_context(
        instance,
        &identity,
        &parameters,
        &config.runtime_snapshot,
        0,
        None,
    );
    let input_raw = parameters.raw_input().to_string();
    Ok(EmbeddedProtocolView {
        runtime,
        renderer,
        bindings,
        theme: config.theme,
        input_raw,
        parameters,
        runtime_snapshot: config.runtime_snapshot,
        publication: None,
        state_revision: 0,
        engine_context,
        instance,
        // A usable fallback also covers a first Tick before the host sends its
        // initial Resize event. The host's actual size replaces this value.
        content_size: (80, 24),
        active: false,
        closed: false,
        terminal_finished: false,
        external_ack_pending: false,
        status: None,
        error: None,
    })
}

struct EmbeddedProtocolView {
    runtime: Box<dyn EngineRuntime>,
    renderer: Box<dyn crate::engine::ViewRenderer>,
    bindings: Vec<crate::workflow::command::InputActionBinding>,
    theme: ResolvedTheme,
    input_raw: String,
    parameters: ParameterSnapshot,
    runtime_snapshot: Value,
    publication: Option<ViewPublication>,
    state_revision: u64,
    engine_context: EngineContext,
    instance: ViewInstanceId,
    content_size: (u16, u16),
    active: bool,
    closed: bool,
    terminal_finished: bool,
    external_ack_pending: bool,
    status: Option<String>,
    error: Option<String>,
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
        input: EditorSnapshot {
            raw: parameters.raw_input().to_string(),
            cursor: parameters.raw_input().len(),
            revision: 0,
        },
        input_generation: 0,
        parameters: parameters.clone(),
        input_rejected: false,
        runtime: EngineRuntimeSnapshot::new(runtime.clone()),
        current: current.cloned().unwrap_or(Value::Null),
        revision,
    })
}

impl EmbeddedProtocolView {
    fn sync_context(&mut self, context: &ViewContext) -> Result<()> {
        anyhow::ensure!(
            context.instance == self.instance,
            "embedded protocol context belongs to {:?}, expected {:?}",
            context.instance,
            self.instance
        );
        self.engine_context = engine_context(
            self.instance,
            self.engine_context.view_identity(),
            &self.parameters,
            &self.runtime_snapshot,
            self.state_revision,
            self.publication
                .as_ref()
                .map(|publication| &publication.current),
        );
        Ok(())
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

    fn apply_publication(&mut self, _: &ViewContext, emission: &EngineEmission) {
        if let Some(publication) = emission.publication() {
            self.publication = Some(ViewPublication::new(
                publication.current().clone(),
                publication.ready,
            ));
            self.state_revision = self.state_revision.wrapping_add(1);
        }
    }

    fn map_decision(
        &mut self,
        context: &ViewContext,
        emission: EngineEmission,
    ) -> Result<ViewDecision> {
        self.apply_publication(context, &emission);
        self.map_engine_decision(context, emission.decision_ref().clone())
    }

    fn map_engine_decision(
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
                self.runtime_snapshot = update.value;
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
                    mapped.push(self.map_engine_decision(context, decision)?);
                }
                Ok(ViewDecision::Batch(mapped))
            }
            EngineDecision::Navigate(EngineNavigationRequest { .. }) => {
                bail!("Embedded protocol View cannot handle this Engine decision")
            }
        }
    }

    fn action(&mut self, context: &ViewContext, action: ActionId) -> Result<ViewDecision> {
        let emission = self.runtime.action(EngineActionInput {
            invocation: crate::engine::ActionInvocation::new(action),
            context: self.engine_context.clone(),
        })?;
        self.map_decision(context, emission)
    }

    fn external_tick(&mut self, context: &ViewContext) -> Result<ViewDecision> {
        if self.terminal_finished {
            return Ok(ViewDecision::Stay);
        }
        let result = self.runtime.drive_tick(EngineTick {
            context: self.engine_context.clone(),
            content_size: self.content_size,
        })?;
        self.map_external_result(context, result)
    }

    fn map_external_result(
        &mut self,
        _context: &ViewContext,
        result: ExternalTickResult,
    ) -> Result<ViewDecision> {
        if matches!(result.action, ExternalTickAction::Continue) && result.notice.is_some() {
            bail!("embedded external Continue result cannot carry a notice")
        }
        if let Some(notice) = &result.notice {
            self.apply_notice(notice);
        }
        match result.action {
            ExternalTickAction::Continue => Ok(ViewDecision::Invalidate),
            ExternalTickAction::Return(value) => {
                self.terminal_finished = true;
                self.external_ack_pending = true;
                Ok(ViewDecision::Return(ViewResult::new(value)))
            }
            ExternalTickAction::Close => {
                self.terminal_finished = true;
                self.external_ack_pending = true;
                Ok(ViewDecision::Close)
            }
            ExternalTickAction::Fail(error) => {
                let error = format!("view {}: {error}", self.engine_context.view_ref());
                self.terminal_finished = true;
                self.error = Some(error.clone());
                self.state_revision = self.state_revision.wrapping_add(1);
                // The Router closes the failed View transactionally and reports
                // this error after the caller has been restored.
                self.runtime.commit_external_tick();
                self.external_ack_pending = false;
                Ok(ViewDecision::CloseWithError(error))
            }
        }
    }
}

impl RawInputReceiver for EmbeddedProtocolView {
    fn push_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.runtime
            .raw_receiver()
            .context("embedded runtime does not provide a raw input receiver")?
            .push_input(bytes)
    }
}

pub(super) const CMD_CANCEL: &str = "embedded.cancel";

impl View for EmbeddedProtocolView {
    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        let mut entries = Vec::new();
        for binding in &self.bindings {
            if !binding.enabled {
                continue;
            }
            if let crate::workflow::command::ResolvedInputAction::Engine(action) = &binding.action
                && action.as_str() == CMD_CANCEL
            {
                entries.push(crate::command::CommandEntry::for_event(
                    CMD_CANCEL,
                    Some("Cancel".to_string()),
                    Some(binding.key),
                    crate::command::CommandScope::Engine,
                ));
            }
        }
        entries
    }

    fn on_command(&mut self, id: &str, context: &ViewContext) -> Result<ViewDecision> {
        self.action(context, ActionId::new(id))
    }

    fn fallback_receiver(&mut self) -> Option<&mut dyn crate::view::FallbackInputReceiver> {
        Some(self)
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
                if self.external_ack_pending {
                    self.terminal_finished = false;
                }
                let emission = self.runtime.activate(self.engine_context.clone())?;
                self.map_decision(context, emission)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Covered) => {
                self.active = false;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closing) => {
                self.active = false;
                // Closing is reversible while Router stages parent activation.
                // Resource release and external completion ack happen at Closed.
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Lifecycle(LifecycleEvent::Closed) => {
                if self.external_ack_pending {
                    self.runtime.commit_external_tick();
                    self.external_ack_pending = false;
                }
                self.runtime.deactivate();
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
            ViewEvent::Input(InputEvent::Paste { raw, .. })
            | ViewEvent::Input(InputEvent::Bytes(raw)) => {
                self.push_raw(&raw)?;
                Ok(ViewDecision::Invalidate)
            }
            ViewEvent::Input(InputEvent::Eof) => Ok(ViewDecision::Close),
            ViewEvent::Task(task) => {
                if task.instance != context.instance {
                    return Ok(ViewDecision::Stay);
                }
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Tick if self.active => self.external_tick(context),
            ViewEvent::Tick => {
                self.runtime.drive_background_tick()?;
                Ok(ViewDecision::Stay)
            }
            ViewEvent::Resize(size) => {
                self.content_size = (size.width.max(1), size.height.max(1));
                Ok(ViewDecision::Invalidate)
            }
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, _: &RenderContext) -> Result<RenderResult> {
        let model = self.runtime.render_model();
        self.renderer.validate_model(&model)?;
        let engine_context = crate::engine::RenderContext {
            theme: self.theme.clone(),
            image_picker: None,
        };
        self.renderer.render(&model, &engine_context, frame, area);
        let chrome = self.renderer.chrome(&model);
        let cursor = match model.downcast_ref::<super::EmbeddedRenderModel>() {
            Some(model) => model
                .screen
                .as_ref()
                .and_then(|screen| screen.cursor())
                .map(|(x, y)| RelativeCursor {
                    x: x.min(u16::MAX as usize) as u16,
                    y: y.min(u16::MAX as usize) as u16,
                    visible: true,
                }),
            None => None,
        };
        Ok(RenderResult {
            cursor,
            metadata: crate::view::ViewMetadata {
                status: self.status.clone().or(chrome.status),
                error: self.error.clone(),
                bindings: None,
            },
        })
    }
}

impl EmbeddedProtocolView {
    fn push_raw(&mut self, raw: &[u8]) -> Result<()> {
        self.runtime
            .raw_receiver()
            .context("embedded runtime does not provide a raw input receiver")?
            .push_input(raw)
    }
}

impl crate::view::FallbackInputReceiver for EmbeddedProtocolView {
    fn on_unbound_key(
        &mut self,
        _key: crate::input::Key,
        raw: &[u8],
        _context: &ViewContext,
    ) -> Result<ViewDecision> {
        self.push_raw(raw)?;
        Ok(ViewDecision::Invalidate)
    }
}

impl Drop for EmbeddedProtocolView {
    fn drop(&mut self) {
        if !self.closed {
            self.runtime.deactivate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ProjectedBindingConfig;
    use crate::view::{NavigationRequest, ParsedQuery};
    use ratatui::{Terminal, backend::TestBackend};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn request() -> NavigationRequest {
        NavigationRequest::new(
            "embedded",
            ParsedQuery::new("embedded", "query", Value::Null),
        )
    }

    fn config(command: &[&str]) -> EmbeddedProtocolConfig {
        EmbeddedProtocolConfig::new(
            "embedded",
            ProjectedEngineConfig {
                fields: [(
                    "command".to_string(),
                    Value::Array(
                        command
                            .iter()
                            .map(|value| Value::String((*value).into()))
                            .collect(),
                    ),
                )]
                .into_iter()
                .collect(),
                ..ProjectedEngineConfig::default()
            },
            ProjectedBindingConfig::default(),
            crate::lifecycle::CancellationToken::new().observer(),
            serde_json::json!({"view": {"current": {}}}),
            ParameterSnapshot::from_parts(
                Value::Null,
                String::new(),
                crate::input::InputSourceIdentity {
                    frame: ViewMountId(1),
                    generation: 0,
                },
                0,
            ),
            ResolvedTheme::terminal(),
        )
    }

    fn context() -> ViewContext {
        ViewContext::new(ViewInstanceId(1), "embedded")
    }

    fn mounted_view(config: EmbeddedProtocolConfig) -> (Box<dyn View>, ViewContext) {
        let mut view = create_protocol_view(config, &request(), ViewInstanceId(1)).unwrap();
        let context = context();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 40,
                height: 6,
            }),
            &context,
        )
        .unwrap();
        (view, context)
    }

    #[test]
    fn embedded_results_are_direct_values() {
        let value = serde_json::json!({"ok": true});
        assert_eq!(
            serde_json::from_value::<Value>(value.clone()).unwrap(),
            value
        );
        assert_eq!(
            serde_json::from_value::<Value>(Value::Null).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn covered_ticks_use_background_polling_without_forwarding_input() {
        let (mut view, context) = mounted_view(config(&["/bin/sh", "-c", "sleep 0.02"]));
        view.event(ViewEvent::Tick, &context).unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
            .unwrap();
        assert!(matches!(
            view.event(ViewEvent::Tick, &context).unwrap(),
            ViewDecision::Stay
        ));
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn covered_completion_is_retained_until_foreground_activation() {
        let (mut view, context) = mounted_view(config(&["/bin/sh", "-c", "exit 0"]));
        view.event(ViewEvent::Tick, &context).unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
            .unwrap();
        for _ in 0..50 {
            view.event(ViewEvent::Tick, &context).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        let mut closed = false;
        for _ in 0..50 {
            if matches!(
                view.event(ViewEvent::Tick, &context).unwrap(),
                ViewDecision::Close
            ) {
                closed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            closed,
            "covered completion must remain available on activation"
        );
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn external_completion_replays_after_closing_is_rolled_back() {
        let (mut view, context) = mounted_view(config(&["/bin/sh", "-c", "exit 0"]));
        let mut completed = false;
        for _ in 0..100 {
            if matches!(
                view.event(ViewEvent::Tick, &context).unwrap(),
                ViewDecision::Close
            ) {
                completed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(completed, "embedded completion did not become ready");

        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        assert!(matches!(
            view.event(ViewEvent::Tick, &context).unwrap(),
            ViewDecision::Close
        ));
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn raw_key_paste_and_bytes_are_forwarded_losslessly() {
        let script = "read -r line; printf '%s' \"$line\"";
        let (mut view, context) = mounted_view(config(&["/bin/sh", "-c", script]));
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: crate::input::Key::Char('x'),
                raw: vec![0x1b, b'[', b'1', b'~'],
            }),
            &context,
        )
        .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Paste {
                text: Some("ignored".into()),
                raw: b"\x1b[200~payload\x1b[201~".to_vec(),
            }),
            &context,
        )
        .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Bytes(vec![0xff, 0x00])),
            &context,
        )
        .unwrap();
        for _ in 0..80 {
            view.event(ViewEvent::Tick, &context).unwrap();
            if view.command_snapshot().revision > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal
            .draw(|frame| {
                view.render(
                    frame,
                    frame.area(),
                    &RenderContext::for_terminal(crate::view::TerminalSize {
                        width: 40,
                        height: 6,
                    }),
                )
                .unwrap();
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(output.contains("payload"));
    }

    #[test]
    fn cancel_binding_returns_without_forwarding_escape() {
        let (mut view, context) = mounted_view(config(&["/bin/sh", "-c", "sleep 2"]));
        let decision = view
            .event(
                ViewEvent::Input(InputEvent::Key {
                    key: crate::input::Key::Escape,
                    raw: vec![0x1b],
                }),
                &context,
            )
            .unwrap();
        assert!(matches!(decision, ViewDecision::Close));
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn failed_external_completion_closes_recoverably_and_is_acknowledged() {
        let mut cfg = config(&["/bin/sh", "-c", "exit 0"]);
        cfg.engine.fields.insert(
            "result".to_string(),
            serde_json::json!({"format": "text", "required": true}),
        );
        let (mut view, context) = mounted_view(cfg);
        let mut failed = false;
        for _ in 0..100 {
            match view.event(ViewEvent::Tick, &context) {
                Ok(ViewDecision::CloseWithError(error)) => {
                    assert!(error.contains("produced no result"));
                    failed = true;
                    break;
                }
                Ok(decision) => {
                    assert!(matches!(
                        decision,
                        ViewDecision::Invalidate | ViewDecision::Stay
                    ));
                }
                Err(error) => panic!("embedded failure must close recoverably: {error}"),
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(failed, "embedded failure should reach the protocol host");
        assert!(matches!(
            view.event(ViewEvent::Tick, &context).unwrap(),
            ViewDecision::Stay
        ));
        let mut terminal = Terminal::new(TestBackend::new(20, 2)).unwrap();
        let mut rendered = None;
        terminal
            .draw(|frame| {
                rendered = Some(
                    view.render(
                        frame,
                        frame.area(),
                        &RenderContext::for_terminal(crate::view::TerminalSize {
                            width: 20,
                            height: 2,
                        }),
                    )
                    .unwrap(),
                );
            })
            .unwrap();
        assert!(rendered.unwrap().metadata.error.is_some());
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn first_start_and_resize_use_host_content_dimensions() {
        let mut request = request();
        request.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
        request.presentation.width = Some(12);
        request.presentation.height = Some(6);
        let mut view = create_protocol_view_state(
            config(&["/bin/sh", "-c", "sleep 1"]),
            &request,
            ViewInstanceId(1),
        )
        .unwrap();
        let mut context = context();
        context.presentation = request.presentation;
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 10,
                height: 4,
            }),
            &context,
        )
        .unwrap();
        view.event(ViewEvent::Tick, &context).unwrap();
        let model = view.runtime.render_model();
        let model = model
            .downcast_ref::<super::super::EmbeddedRenderModel>()
            .unwrap();
        assert_eq!(
            model.screen.as_ref().unwrap().dimensions(),
            (10, 4),
            "PTY starts at popup inner dimensions"
        );

        context.presentation.width = Some(20);
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 18,
                height: 4,
            }),
            &context,
        )
        .unwrap();
        view.event(ViewEvent::Tick, &context).unwrap();
        let model = view.runtime.render_model();
        let model = model
            .downcast_ref::<super::super::EmbeddedRenderModel>()
            .unwrap();
        assert_eq!(model.screen.as_ref().unwrap().dimensions(), (18, 4));
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
    }

    #[test]
    fn external_completion_keeps_final_screen_then_returns_result() {
        let root =
            std::env::temp_dir().join(format!("tlaunch-protocol-embedded-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let script = root.join("embedded.sh");
        fs::write(&script, "#!/bin/sh\nprintf 'done'; sleep 0.01\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let mut cfg = config(&[script.to_str().unwrap()]);
        cfg.engine.fields.insert(
            "result".to_string(),
            serde_json::json!({"format":"text", "required":false}),
        );
        let (mut view, context) = mounted_view(cfg);
        let mut saw_final_render = false;
        let mut returned = false;
        for _ in 0..200 {
            let decision = view.event(ViewEvent::Tick, &context).unwrap();
            if matches!(decision, ViewDecision::Invalidate) {
                saw_final_render = true;
            }
            if matches!(decision, ViewDecision::Return(_)) {
                returned = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(saw_final_render);
        assert!(returned);
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context)
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
