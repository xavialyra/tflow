use super::command_adapter::CommandService;
#[cfg(test)]
use super::command_adapter::map_prepared_action;
use crate::command::{ChromeSnapshot, CommandRegistry, CommandScope};
use crate::input::InputEvent;
use crate::protocol::contracts::{TaskEvent, ViewInstanceId};
#[cfg(test)]
use crate::protocol::contracts::{TaskId, TaskOutcome};
use crate::ui::chrome::{ContentHost, FooterModel, FooterRenderer};
use crate::view::{
    EffectExecutor, NavigationRequest, RenderContext, RenderResult, Router, TerminalSize,
    ViewDecision, ViewEvent, ViewResult,
};
#[cfg(test)]
use crate::view::{ViewCommandSnapshot, ViewContext};
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const INFO_MESSAGE_DURATION: Duration = Duration::from_secs(3);

struct InfoMessage {
    label: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProtocolRenderResult {
    pub(crate) view: RenderResult,
    pub(crate) footer: FooterModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorSource {
    StartupWarning,
    Session,
    View(ViewInstanceId),
}

pub(crate) struct ProtocolSession {
    router: Router,
    commands: Box<dyn CommandService>,
    pub(crate) registry: Arc<RwLock<CommandRegistry>>,
    chrome_snapshot: ChromeSnapshot,
    shared_snapshot: Arc<RwLock<ChromeSnapshot>>,
    pending_key: Option<crate::input::Key>,
    #[cfg(test)]
    effects: Option<Box<dyn EffectExecutor>>,
    terminal: TerminalSize,
    theme: crate::ui::theme::ResolvedTheme,
    runtime_log: Option<crate::diagnostics::RuntimeLog>,
    runtime_warning: Option<String>,
    active_error: Option<String>,
    active_info: Option<InfoMessage>,
    error_source: Option<ErrorSource>,
    last_diagnostic: Option<(ViewInstanceId, String)>,
}

#[cfg(test)]
struct TestCommandService;

#[cfg(test)]
impl CommandService for TestCommandService {
    fn build_host_commands(
        &self,
        _: std::sync::Arc<std::sync::RwLock<crate::command::ChromeSnapshot>>,
        _: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        Ok(Vec::new())
    }

    fn build_view_commands(
        &self,
        _: &ViewContext,
        _: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        Ok(Vec::new())
    }

    fn build_engine_commands(
        &self,
        _: &ViewContext,
        _: &ViewCommandSnapshot,
    ) -> Result<Vec<crate::command::CommandEntry>> {
        Ok(Vec::new())
    }
}

impl ProtocolSession {
    #[cfg(test)]
    pub(crate) fn new(router: Router, effects: Box<dyn EffectExecutor>) -> Self {
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let shared_snapshot = Arc::new(RwLock::new(ChromeSnapshot::default()));
        Self {
            router,
            commands: Box::new(TestCommandService),
            registry,
            chrome_snapshot: ChromeSnapshot::default(),
            shared_snapshot,
            pending_key: None,
            effects: Some(effects),
            terminal: TerminalSize::default(),
            theme: crate::ui::theme::ResolvedTheme::terminal(),
            runtime_log: None,
            runtime_warning: None,
            active_error: None,
            active_info: None,
            error_source: None,
            last_diagnostic: None,
        }
    }

    pub(crate) fn configured(
        router: Router,
        commands: Box<dyn CommandService>,
        theme: crate::ui::theme::ResolvedTheme,
        _default_view: String,
        mut runtime_log: crate::diagnostics::RuntimeLog,
    ) -> Self {
        let warning = runtime_log.take_warning_record();
        let error_source = warning.as_ref().map(|_| ErrorSource::StartupWarning);
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let shared_snapshot = Arc::new(RwLock::new(ChromeSnapshot::default()));
        let host_entries = commands
            .build_host_commands(Arc::clone(&shared_snapshot), Arc::clone(&registry))
            .unwrap_or_default();
        let _ = registry
            .write()
            .unwrap()
            .replace_scope(CommandScope::Host, host_entries);
        let active_id = router.active().map(|entry| entry.id);
        let chrome_snapshot = ChromeSnapshot::from_registry(&registry.read().unwrap())
            .with_active_instance(active_id);
        *shared_snapshot.write().unwrap() = chrome_snapshot.clone();
        Self {
            router,
            commands,
            registry,
            chrome_snapshot,
            shared_snapshot,
            pending_key: None,
            #[cfg(test)]
            effects: None,
            terminal: TerminalSize::default(),
            theme,
            runtime_log: Some(runtime_log),
            runtime_warning: warning.as_ref().map(|record| record.message.clone()),
            active_error: warning.map(|record| record.label),
            active_info: None,
            error_source,
            last_diagnostic: None,
        }
    }

    pub(crate) fn router(&self) -> &Router {
        &self.router
    }

    pub(crate) fn start_root(&mut self, request: NavigationRequest) -> Result<ViewInstanceId> {
        anyhow::ensure!(
            self.router.stack().is_empty(),
            "protocol session root has already been constructed"
        );
        let id = self.router.push(request)?;
        self.sync_active_commands()?;
        Ok(id)
    }

    #[cfg(test)]
    pub(crate) fn input(&mut self, event: InputEvent) -> Result<ViewDecision> {
        self.dispatch_with_owned_effects(ViewEvent::Input(event))
    }

    #[cfg(test)]
    pub(crate) fn task(&mut self, event: TaskEvent) -> Result<ViewDecision> {
        self.dispatch_with_owned_effects(ViewEvent::Task(event))
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn tick(&mut self) -> Result<ViewDecision> {
        self.dispatch_with_owned_effects(ViewEvent::Tick)
    }

    #[cfg(test)]
    pub(crate) fn resize(&mut self, terminal: TerminalSize) -> Result<ViewDecision> {
        self.terminal = terminal;
        self.dispatch_active_resize_with_owned_effects()
    }

    #[cfg(test)]
    pub(crate) fn eof(&mut self) -> Result<ViewDecision> {
        self.dispatch_with_owned_effects(ViewEvent::Input(InputEvent::Eof))
    }

    pub(crate) fn input_with_effects(
        &mut self,
        event: InputEvent,
        effects: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        self.dispatch_with_effects(ViewEvent::Input(event), effects)
    }

    pub(crate) fn task_with_effects(
        &mut self,
        event: TaskEvent,
        effects: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        self.dispatch_with_effects(ViewEvent::Task(event), effects)
    }

    pub(crate) fn tick_with_effects(
        &mut self,
        effects: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        self.dispatch_with_effects(ViewEvent::Tick, effects)
    }

    pub(crate) fn resize_with_effects(
        &mut self,
        terminal: TerminalSize,
        effects: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        self.terminal = terminal;
        self.dispatch_active_resize(effects)
    }

    #[cfg(test)]
    fn dispatch_with_owned_effects(&mut self, event: ViewEvent) -> Result<ViewDecision> {
        let mut effects = self
            .effects
            .take()
            .expect("test protocol session has no effect executor");
        let result = self.dispatch_with_effects(event, &mut *effects);
        self.effects = Some(effects);
        result
    }

    #[cfg(test)]
    fn dispatch_active_resize_with_owned_effects(&mut self) -> Result<ViewDecision> {
        let mut effects = self
            .effects
            .take()
            .expect("test protocol session has no effect executor");
        let result = self.dispatch_active_resize(&mut *effects);
        self.effects = Some(effects);
        result
    }

    fn dispatch_with_effects(
        &mut self,
        event: ViewEvent,
        effects: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        // A failed dispatch is reported from its returned error. Discard its
        // Router-side copy before the next event so it cannot be reported twice.
        let _ = self.router.take_recorded_error();
        let _ = self.router.take_info();
        if matches!(event, ViewEvent::Input(_)) {
            self.active_info = None;
        } else if matches!(event, ViewEvent::Tick) {
            self.expire_info(Instant::now());
        }
        if matches!(event, ViewEvent::Input(_)) && self.error_source == Some(ErrorSource::Session) {
            self.active_error = None;
            self.error_source = None;
            self.last_diagnostic = None;
        }
        let active = self.router.active().map(|entry| entry.id);

        let mut executed_command = false;
        let mut command_decision = ViewDecision::Stay;

        if let ViewEvent::Input(InputEvent::Key { key, .. }) = &event {
            let entry_opt = self.registry.read().unwrap().resolve(*key).cloned();
            if let Some(entry) = entry_opt {
                if entry.scope != CommandScope::Host {
                    let is_loading = self.router.active().is_some_and(|a| {
                        let snapshot = a.view.command_snapshot();
                        snapshot.engine_type == crate::workflow::config::ENGINE_PICKER
                            && snapshot.publication.as_ref().map_or(false, |p| !p.ready)
                    });
                    if is_loading {
                        self.pending_key = Some(*key);
                        return Ok(ViewDecision::Stay);
                    }
                }
                executed_command = true;
                command_decision = entry.action.execute()?;
                if let Some(source) = active {
                    self.router
                        .process_with_effects(command_decision.clone(), source, effects)?;
                }
            }
        }

        let decision = if !executed_command {
            self.router.dispatch_with_effects(event.clone(), effects)?
        } else {
            command_decision
        };

        self.resize_new_active_view(active, effects)?;
        let current = self.router.active().map(|entry| entry.id);
        if current != active {
            self.active_info = None;
        }
        if let Some((source, source_view, message)) = self.router.take_info() {
            if current == Some(source) {
                self.report_info(&message);
            } else if let Some(runtime_log) = self.runtime_log.as_mut() {
                runtime_log.record(
                    crate::diagnostics::LogLevel::Info,
                    Some(&source_view),
                    None,
                    &message,
                );
            }
        }
        if let Some(error) = self.router.take_recorded_error() {
            self.report_error(&error.message);
        }
        self.expire_info(Instant::now());
        self.sync_active_commands()?;

        if matches!(event, ViewEvent::Task(_)) {
            if let Some(key) = self.pending_key.take() {
                let is_ready = self.router.active().is_some_and(|a| {
                    let snapshot = a.view.command_snapshot();
                    snapshot.publication.as_ref().is_some_and(|p| p.ready)
                });
                if is_ready {
                    let re_event = ViewEvent::Input(InputEvent::Key {
                        key,
                        raw: Vec::new(),
                    });
                    return self.dispatch_with_effects(re_event, effects);
                } else {
                    self.pending_key = Some(key);
                }
            }
        }

        Ok(decision)
    }

    pub(crate) fn sync_active_commands(&mut self) -> Result<()> {
        let active_id = self.router.active().map(|entry| entry.id);
        let is_same_instance =
            self.chrome_snapshot.active_instance == active_id && active_id.is_some();
        let is_loading = self.router.active().is_some_and(|a| {
            let snapshot = a.view.command_snapshot();
            snapshot.publication.as_ref().is_some_and(|p| !p.ready)
        });
        let active_view = self
            .router
            .active()
            .map(|active| active.context.location.target.clone());
        let active_metadata = self.router.active().map(|active| {
            let snapshot = active.view.command_snapshot();
            (snapshot.parameters, snapshot.raw_input)
        });
        let active_parameters = active_metadata
            .as_ref()
            .map(|(parameters, _)| parameters.clone())
            .unwrap_or(serde_json::Value::Null);
        let active_raw_input = active_metadata
            .map(|(_, raw_input)| raw_input)
            .unwrap_or_default();
        if is_same_instance && is_loading {
            if self.chrome_snapshot.active_view != active_view
                || self.chrome_snapshot.active_parameters != active_parameters
                || self.chrome_snapshot.active_raw_input != active_raw_input
            {
                let reg = self.registry.read().unwrap();
                self.chrome_snapshot = ChromeSnapshot::from_registry(&reg)
                    .with_active_instance(active_id)
                    .with_active_view(
                        active_view.clone(),
                        active_parameters.clone(),
                        active_raw_input.clone(),
                    );
                *self.shared_snapshot.write().unwrap() = self.chrome_snapshot.clone();
            }
            return Ok(());
        }

        let (view_entries, engine_entries) = if let Some(active) = self.router.active() {
            let context = &active.context;
            let snapshot = active.view.command_snapshot();
            let view_entries = if let Some(custom) = active.view.engine_commands(context) {
                custom
            } else {
                let mut view_entries = self.commands.build_view_commands(context, &snapshot)?;
                view_entries.extend(active.view.view_commands(context));
                view_entries
            };
            (view_entries, Vec::new())
        } else {
            (Vec::new(), Vec::new())
        };

        let mut changed = false;
        let mut reg = self.registry.write().unwrap();
        if reg
            .replace_scope(CommandScope::View, view_entries)?
            .is_some()
        {
            changed = true;
        }
        if reg
            .replace_scope(CommandScope::Engine, engine_entries)?
            .is_some()
        {
            changed = true;
        }

        if changed
            || self.chrome_snapshot.active_instance != active_id
            || self.chrome_snapshot.active_view != active_view
            || self.chrome_snapshot.active_parameters != active_parameters
            || self.chrome_snapshot.active_raw_input != active_raw_input
        {
            self.chrome_snapshot = ChromeSnapshot::from_registry(&reg)
                .with_active_instance(active_id)
                .with_active_view(active_view, active_parameters, active_raw_input);
            *self.shared_snapshot.write().unwrap() = self.chrome_snapshot.clone();
        }
        Ok(())
    }

    fn resize_new_active_view(
        &mut self,
        previous: Option<ViewInstanceId>,
        effects: &mut dyn EffectExecutor,
    ) -> Result<()> {
        if self.terminal.width == 0 || self.terminal.height == 0 {
            return Ok(());
        }
        if self.router.active().map(|entry| entry.id) != previous {
            if previous.is_some() {
                self.active_error = None;
                self.error_source = None;
                self.last_diagnostic = None;
            }
            self.dispatch_active_resize(effects)?;
        }
        Ok(())
    }

    fn dispatch_active_resize(&mut self, effects: &mut dyn EffectExecutor) -> Result<ViewDecision> {
        let area = active_render_area(
            self.router.stack(),
            Rect::new(0, 0, self.terminal.width, self.terminal.height),
        );
        self.router.dispatch_with_effects(
            ViewEvent::Resize(TerminalSize {
                width: area.width,
                height: area.height,
            }),
            effects,
        )
    }

    pub(crate) fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        image_picker: Option<crate::terminal::ImagePicker>,
    ) -> Result<ProtocolRenderResult> {
        let active_index = self
            .router
            .stack()
            .len()
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("cannot render without an active View"))?;
        let chrome_snapshot = {
            let entry = &self.router.stack()[active_index];
            entry.view.chrome(&entry.context)?
        };
        let (chrome_instance, chrome_location) = {
            let entry = &self.router.stack()[active_index];
            (entry.id, entry.context.location.clone())
        };
        let footer_location = self.router.stack()[active_index].context.location.clone();
        self.surface_diagnostic(
            chrome_instance,
            &chrome_location.target,
            chrome_snapshot.error.as_deref(),
        );

        let content_host = ContentHost::default();
        let footer_renderer = FooterRenderer::default();

        content_host.render_frame_background(frame, area, &self.theme);

        let base_index = content_host.visible_base_index(self.router.stack(), active_index);
        let top_padding = base_index
            .or(Some(0))
            .and_then(|index| self.router.stack().get(index))
            .map(|entry| entry.view.preferred_top_inset())
            .unwrap_or(0);
        let content_area = content_host.content_area(area, top_padding);
        let render_context = RenderContext::new(self.terminal, image_picker);
        let (view, active_render_area, active_popup_rect) = content_host.render_views(
            frame,
            content_area,
            self.router.stack(),
            &render_context,
            |index, frame, rect, ctx| self.router.render_at(index, frame, rect, ctx),
        )?;

        if active_render_area.width > 0
            && active_render_area.height > 0
            && let Some(cursor) = &view.cursor
        {
            let x = active_render_area.x.saturating_add(cursor.x).min(
                active_render_area
                    .x
                    .saturating_add(active_render_area.width.saturating_sub(1)),
            );
            let y = active_render_area.y.saturating_add(cursor.y).min(
                active_render_area
                    .y
                    .saturating_add(active_render_area.height.saturating_sub(1)),
            );
            if cursor.visible {
                frame.set_cursor_position((x, y));
            }
        }

        let metadata = view.metadata.clone();
        let footer = FooterModel {
            location: footer_location,
            status: chrome_snapshot.status.or(metadata.status),
            error: self.active_error.clone().or(chrome_snapshot.error),
            info: self.active_info.as_ref().map(|info| info.label.clone()),
            commands: self.chrome_snapshot.footer_commands(),
            overflow_command: self.chrome_snapshot.overflow_command(),
        };
        let footer_area = content_host.footer_area(area);
        if let Some(popup_rect) = active_popup_rect {
            content_host.render_active_popup_border(frame, popup_rect, &footer, &self.theme);
            footer_renderer.render_blank(frame, footer_area, &self.theme);
        } else {
            footer_renderer.render(frame, footer_area, &footer, &self.theme);
        }

        Ok(ProtocolRenderResult { view, footer })
    }

    fn surface_diagnostic(
        &mut self,
        instance: ViewInstanceId,
        view_ref: &str,
        error: Option<&str>,
    ) {
        let Some(error) = error else {
            if self.error_source == Some(ErrorSource::View(instance)) {
                self.active_error = None;
                self.error_source = None;
                self.last_diagnostic = None;
            }
            return;
        };
        let identity = (instance, error.to_string());
        if self.last_diagnostic.as_ref() == Some(&identity) {
            return;
        }
        self.last_diagnostic = Some(identity);
        self.error_source = Some(ErrorSource::View(instance));
        self.active_error = Some(if let Some(runtime_log) = self.runtime_log.as_mut() {
            runtime_log
                .record(
                    crate::diagnostics::LogLevel::Error,
                    Some(view_ref),
                    None,
                    error,
                )
                .label
        } else {
            format!("ERROR [{view_ref}]: {error}")
        });
    }

    fn expire_info(&mut self, now: Instant) {
        if self
            .active_info
            .as_ref()
            .is_some_and(|info| now >= info.expires_at)
        {
            self.active_info = None;
        }
    }

    pub(crate) fn report_info(&mut self, message: &str) {
        let Some(active) = self.router.active() else {
            return;
        };
        let view_ref = &active.context.location.target;
        let label = if let Some(runtime_log) = self.runtime_log.as_mut() {
            runtime_log
                .record(
                    crate::diagnostics::LogLevel::Info,
                    Some(view_ref),
                    None,
                    message,
                )
                .label
        } else {
            format!(
                "INFO [{view_ref}]: {}",
                crate::terminal::sanitize_text(message)
            )
        };
        self.active_info = Some(InfoMessage {
            label,
            expires_at: Instant::now() + INFO_MESSAGE_DURATION,
        });
    }

    pub(crate) fn report_error(&mut self, message: &str) {
        self.active_info = None;
        let Some(active) = self.router.active() else {
            return;
        };
        let instance = active.id;
        let view_ref = active.context.location.target.clone();
        let label = if let Some(runtime_log) = self.runtime_log.as_mut() {
            runtime_log
                .record(
                    crate::diagnostics::LogLevel::Error,
                    Some(&view_ref),
                    None,
                    message,
                )
                .label
        } else {
            format!("ERROR [{view_ref}]: {message}")
        };
        self.active_error = Some(label);
        self.error_source = Some(ErrorSource::Session);
        self.last_diagnostic = Some((instance, message.to_string()));
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.runtime_warning.take()
    }

    pub(crate) fn take_result(&mut self) -> Option<ViewResult> {
        self.router.take_result()
    }

    #[cfg(test)]
    pub(crate) fn take_error(&mut self) -> Option<crate::view::RouterError> {
        self.router.take_error()
    }
}

fn active_render_area(stack: &[crate::view::ViewInstance], terminal: Rect) -> Rect {
    ContentHost::default().active_content_area(stack, terminal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandEntry, CommandRegistry, CommandScope};
    use crate::protocol::ProtocolCommandService;
    use crate::view::{
        EffectRequest, EffectResult, MapRouteCatalog, ParsedQuery, RouteCatalog, View, ViewContext,
        ViewFactory, ViewMetadata, ViewServices,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Position};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    fn test_invocation(
        config: &crate::workflow::config::CompiledConfig,
        root_view: &str,
    ) -> Arc<crate::workflow::InvocationContext> {
        Arc::new(
            crate::workflow::InvocationContext::new(
                root_view.to_string(),
                serde_json::json!({
                    "stdin": {"path": null, "length": 0, "is_tty": true}
                }),
                config.instantiate_parameters(root_view).unwrap(),
            )
            .unwrap(),
        )
    }

    fn request(target: &str) -> NavigationRequest {
        NavigationRequest::new(target, ParsedQuery::new(target, "query", Value::Null))
    }

    #[derive(Clone)]
    struct Factory {
        events: Rc<RefCell<Vec<InputEvent>>>,
    }

    struct SyntheticView {
        target: String,
        events: Rc<RefCell<Vec<InputEvent>>>,
        runtime: Value,
        publication: Option<crate::view::ViewPublication>,
        revision: u64,
    }

    impl View for SyntheticView {
        fn preferred_top_inset(&self) -> u16 {
            if self.target == "zero_inset" { 0 } else { 1 }
        }

        fn view_commands(&self, _: &ViewContext) -> Vec<CommandEntry> {
            vec![CommandEntry::new(
                "local",
                Some("ok".to_string()),
                Some(crate::input::Key::Enter),
                CommandScope::View,
                Arc::new(|| Ok(ViewDecision::Stay)),
            )]
        }

        fn command_snapshot(&self) -> ViewCommandSnapshot {
            ViewCommandSnapshot {
                engine_type: "test".to_string(),
                parameters: Value::Null,
                raw_input: String::new(),
                runtime: self.runtime.clone(),
                publication: self.publication.clone(),
                revision: self.revision,
                owner_view: None,
            }
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            match event {
                ViewEvent::Lifecycle(_) => Ok(ViewDecision::Stay),
                ViewEvent::Input(input) => {
                    self.events.borrow_mut().push(input.clone());
                    match input {
                        InputEvent::Key {
                            key: crate::input::Key::Char('p'),
                            ..
                        } => Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                            "payload".to_string(),
                        ))),
                        InputEvent::Key {
                            key: crate::input::Key::Char('n'),
                            ..
                        } => {
                            let query = ParsedQuery::new("child", "query", Value::Null);
                            let mut request = NavigationRequest::new("child", query);
                            request.presentation.mode =
                                crate::workflow::config::ViewPresentationMode::Popup;
                            request.presentation.width = Some(10);
                            request.presentation.height = Some(4);
                            Ok(ViewDecision::Transition(
                                crate::view::TransitionRequest::Push(request),
                            ))
                        }
                        InputEvent::Key {
                            key: crate::input::Key::Char('z'),
                            ..
                        } => Ok(ViewDecision::Command(
                            crate::view::CommandResult::EditInput {
                                value: "edited".to_string(),
                                cursor: 2,
                            },
                        )),
                        InputEvent::Key {
                            key: crate::input::Key::Char('q'),
                            ..
                        } => {
                            let target = if self.target == "root" {
                                "child"
                            } else {
                                "grandchild"
                            };
                            let query = ParsedQuery::new(target, "query", Value::Null);
                            let mut request = NavigationRequest::new(target, query);
                            request.presentation.mode =
                                crate::workflow::config::ViewPresentationMode::Popup;
                            if self.target == "root" {
                                request.presentation.width = Some(20);
                                request.presentation.height = Some(8);
                            } else {
                                request.presentation.width = Some(10);
                                request.presentation.height = Some(4);
                            }
                            Ok(ViewDecision::Transition(
                                crate::view::TransitionRequest::Push(request),
                            ))
                        }
                        InputEvent::Key {
                            key: crate::input::Key::Char('e'),
                            ..
                        } => anyhow::bail!("protocol View failure"),
                        InputEvent::Key {
                            key: crate::input::Key::Char('r'),
                            ..
                        } => Ok(ViewDecision::Return(ViewResult::new(Value::String(
                            self.target.clone(),
                        )))),
                        InputEvent::Eof => Ok(ViewDecision::Exit),
                        _ => Ok(ViewDecision::Invalidate),
                    }
                }
                ViewEvent::Command(crate::view::CommandResult::EditInput { value, cursor }) => {
                    self.runtime = serde_json::json!({
                        "edited": value,
                        "cursor": cursor,
                    });
                    self.revision = self.revision.wrapping_add(1);
                    Ok(ViewDecision::Invalidate)
                }
                ViewEvent::Task(task) => {
                    self.publication = Some(crate::view::ViewPublication::new(
                        serde_json::json!({
                            "instance": task.instance.0,
                            "generation": task.generation,
                        }),
                        true,
                    ));
                    self.revision = self.revision.wrapping_add(1);
                    Ok(ViewDecision::Invalidate)
                }
                ViewEvent::Resize(size) => {
                    self.runtime = serde_json::json!({
                        "width": size.width,
                        "height": size.height
                    });
                    self.revision = self.revision.wrapping_add(1);
                    Ok(ViewDecision::Invalidate)
                }
                _ => Ok(ViewDecision::Stay),
            }
        }

        fn render(
            &self,
            frame: &mut Frame,
            area: Rect,
            _context: &RenderContext,
        ) -> Result<RenderResult> {
            frame.render_widget(ratatui::widgets::Paragraph::new(self.target.clone()), area);
            Ok(RenderResult {
                cursor: Some(crate::view::RelativeCursor {
                    x: 1,
                    y: 1,
                    visible: true,
                }),
                metadata: ViewMetadata {
                    status: Some(self.target.clone()),
                    error: None,
                    bindings: None,
                },
            })
        }
    }

    impl ViewFactory for Factory {
        fn create(
            &self,
            request: &NavigationRequest,
            _: ViewInstanceId,
            services: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            let _ = services.routes.query_schema(&request.query.target);
            Ok(Box::new(SyntheticView {
                target: request.target.clone(),
                events: Rc::clone(&self.events),
                runtime: Value::Null,
                publication: None,
                revision: 0,
            }))
        }
    }

    struct Effects {
        calls: Rc<RefCell<Vec<EffectRequest>>>,
    }

    impl EffectExecutor for Effects {
        fn execute(
            &mut self,
            effect: EffectRequest,
            _: &crate::view::ViewContext,
        ) -> Result<EffectResult> {
            self.calls.borrow_mut().push(effect);
            Ok(EffectResult::Complete)
        }
    }

    #[allow(clippy::type_complexity)]
    fn session() -> (
        ProtocolSession,
        Rc<RefCell<Vec<InputEvent>>>,
        Rc<RefCell<Vec<EffectRequest>>>,
    ) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let effects = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        routes.insert("grandchild", "grandchild");
        routes.insert("zero_inset", "zero_inset");
        let router = Router::new(
            Box::new(routes),
            Box::new(Factory {
                events: Rc::clone(&events),
            }),
        );
        let session = ProtocolSession::new(
            router,
            Box::new(Effects {
                calls: Rc::clone(&effects),
            }),
        );
        (session, events, effects)
    }

    #[test]
    fn command_edit_completion_is_delivered_to_its_source_view() {
        let (mut session, _, _) = session();
        session.start_root(request("root")).unwrap();
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('z'),
                raw: vec![b'z'],
            })
            .unwrap();
        assert_eq!(
            session
                .router()
                .active()
                .unwrap()
                .view
                .command_snapshot()
                .runtime,
            serde_json::json!({"edited": "edited", "cursor": 2})
        );

        assert_eq!(
            {
                let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
                let invocation = test_invocation(&config, "core:default");
                map_prepared_action(
                    &config,
                    &invocation,
                    &crate::lifecycle::CancellationToken::new(),
                    crate::workflow::command::PreparedAction::EditInput {
                        value: "mapped".to_string(),
                        cursor: 3,
                    },
                    ViewInstanceId(1),
                )
                .unwrap()
            },
            ViewDecision::Command(crate::view::CommandResult::EditInput {
                value: "mapped".to_string(),
                cursor: 3,
            })
        );
    }

    #[test]
    fn command_registry_resolution_follows_view_over_engine_over_host() {
        let mut registry = CommandRegistry::new();
        registry
            .replace_scope(
                CommandScope::Host,
                vec![
                    CommandEntry::new(
                        "host_x",
                        Some("host-x".into()),
                        Some(crate::input::Key::Char('x')),
                        CommandScope::Host,
                        Arc::new(|| Ok(ViewDecision::Stay)),
                    ),
                    CommandEntry::new(
                        "host_g",
                        Some("host-only".into()),
                        Some(crate::input::Key::Char('g')),
                        CommandScope::Host,
                        Arc::new(|| Ok(ViewDecision::Stay)),
                    ),
                ],
            )
            .unwrap();
        registry
            .replace_scope(
                CommandScope::View,
                vec![
                    CommandEntry::new(
                        "view_x",
                        Some("view-x".into()),
                        Some(crate::input::Key::Char('x')),
                        CommandScope::View,
                        Arc::new(|| Ok(ViewDecision::Stay)),
                    ),
                    CommandEntry::new(
                        "view_l",
                        Some("view-only".into()),
                        Some(crate::input::Key::Char('l')),
                        CommandScope::View,
                        Arc::new(|| Ok(ViewDecision::Stay)),
                    ),
                ],
            )
            .unwrap();

        let snapshot = ChromeSnapshot::from_registry(&registry);
        let bindings = snapshot.to_binding_set();
        assert_eq!(bindings.entries().len(), 3);
        assert_eq!(
            registry.resolve(crate::input::Key::Char('x')).unwrap().id,
            "view_x"
        );
    }

    #[test]
    fn route_completion_falls_back_to_canonical_labels() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("core:default", "core:default");
        let candidates = routes.complete("core:");
        assert_eq!(candidates[0].label, "core:default");
        assert_eq!(candidates[0].target.label.as_deref(), Some("core:default"));
    }

    #[test]
    fn copy_feedback_renders_until_input_and_errors_take_priority() {
        let (mut session, _, _) = session();
        session.start_root(request("root")).unwrap();
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('p'),
                raw: vec![b'p'],
            })
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let render_message = |session: &mut ProtocolSession,
                              terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| {
                    session.render(frame, frame.area(), None).unwrap();
                })
                .unwrap();
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, 9)].symbol())
                .collect::<String>()
        };
        assert!(
            render_message(&mut session, &mut terminal)
                .contains("INFO [root]: Copied to clipboard")
        );
        session
            .dispatch_with_owned_effects(ViewEvent::Tick)
            .unwrap();
        assert!(render_message(&mut session, &mut terminal).contains("Copied to clipboard"));
        session.report_error("copy failed");
        session.report_info("another notification");
        let row = render_message(&mut session, &mut terminal);
        assert!(row.contains("ERROR [root]: copy failed"));
        assert!(!row.contains("another notification"));
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('a'),
                raw: vec![b'a'],
            })
            .unwrap();
        let row = render_message(&mut session, &mut terminal);
        assert!(!row.contains("INFO"));
        assert!(!row.contains("ERROR"));
        assert!(row.contains("ok"));
    }

    #[test]
    fn info_expiration_honors_deadline_and_replacement() {
        let (mut session, _, _) = session();
        session.start_root(request("root")).unwrap();
        let before = Instant::now();
        session.report_info("first");
        let deadline = session.active_info.as_ref().unwrap().expires_at;
        assert!(deadline >= before + Duration::from_secs(3));
        assert!(deadline <= Instant::now() + Duration::from_secs(3));
        session.expire_info(deadline - Duration::from_nanos(1));
        assert!(session.active_info.is_some());
        session.expire_info(deadline);
        assert!(session.active_info.is_none());

        session.report_info("old");
        let old_deadline = Instant::now() - Duration::from_secs(1);
        session.active_info.as_mut().unwrap().expires_at = old_deadline;
        session.report_info("replacement");
        session.expire_info(old_deadline);
        assert_eq!(
            session.active_info.as_ref().unwrap().label,
            "INFO [root]: replacement"
        );
        assert!(session.active_info.as_ref().unwrap().expires_at > old_deadline);
    }

    #[test]
    fn idle_tick_expires_info_in_footer_and_popup_without_clearing_errors() {
        for popup in [false, true] {
            let (mut session, _, _) = session();
            let mut root = request("root");
            if popup {
                root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
                root.presentation.width = Some(60);
                root.presentation.height = Some(6);
            }
            session.start_root(root).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
            let render_text = |session: &mut ProtocolSession,
                               terminal: &mut Terminal<TestBackend>| {
                terminal
                    .draw(|frame| {
                        session.render(frame, frame.area(), None).unwrap();
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                (0..12)
                    .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            session.report_info("temporary feedback");
            assert!(render_text(&mut session, &mut terminal).contains("temporary feedback"));
            session.active_info.as_mut().unwrap().expires_at = Instant::now();
            session.tick().unwrap();
            let text = render_text(&mut session, &mut terminal);
            assert!(!text.contains("temporary feedback"));
            assert!(text.contains("ok"));

            session.report_error("persistent error");
            session.report_info("hidden feedback");
            session.active_info.as_mut().unwrap().expires_at = Instant::now();
            session.tick().unwrap();
            assert!(session.active_info.is_none());
            assert!(render_text(&mut session, &mut terminal).contains("persistent error"));
        }
    }

    #[test]
    fn popup_copy_feedback_uses_bottom_border_and_clears_on_navigation() {
        let (mut session, _, _) = session();
        let mut root = request("root");
        root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
        root.presentation.width = Some(60);
        root.presentation.height = Some(6);
        session.start_root(root).unwrap();
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('p'),
                raw: vec![b'p'],
            })
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let contents = (0..12)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>();
        assert!(
            contents
                .iter()
                .any(|row| row.contains("INFO [root]: Copied to clipboard"))
        );
        assert!(contents[11].trim().is_empty());
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('q'),
                raw: vec![b'q'],
            })
            .unwrap();
        assert_eq!(
            session.router.active().unwrap().context.location.target,
            "child"
        );
        assert!(session.active_info.is_none());
    }

    #[test]
    fn delivers_lossless_input_and_honors_global_precedence() {
        let (mut session, events, effects) = session();
        session.start_root(request("root")).unwrap();
        let global_cmd = CommandEntry::new(
            "global.copy",
            None,
            Some(crate::input::Key::Char('g')),
            CommandScope::Host,
            Arc::new(|| {
                Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                    "global".to_string(),
                )))
            }),
        );
        session
            .registry
            .write()
            .unwrap()
            .replace_scope(CommandScope::Host, vec![global_cmd])
            .unwrap();
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('g'),
                raw: vec![0x1b, b'g'],
            })
            .unwrap();
        assert!(events.borrow().is_empty());
        assert_eq!(
            effects.borrow().as_slice(),
            &[EffectRequest::CopyToClipboard("global".to_string())]
        );
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('x'),
                raw: vec![b'x'],
            })
            .unwrap();
        session
            .input(InputEvent::Paste {
                text: Some("paste".to_string()),
                raw: b"\x1b[200~paste\x1b[201~".to_vec(),
            })
            .unwrap();
        session.input(InputEvent::Bytes(vec![0xff])).unwrap();
        session.input(InputEvent::Eof).unwrap();
        assert_eq!(events.borrow().len(), 4);
        assert_eq!(
            events.borrow()[0],
            InputEvent::Key {
                key: crate::input::Key::Char('x'),
                raw: vec![b'x']
            }
        );
        assert_eq!(events.borrow()[2], InputEvent::Bytes(vec![0xff]));
        assert_eq!(events.borrow()[3], InputEvent::Eof);
    }

    #[test]
    fn view_errors_are_visible_through_protocol_session_and_router() {
        let (mut session, _, _) = session();
        let root = session.start_root(request("root")).unwrap();
        let error = session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('e'),
                raw: vec![b'e'],
            })
            .unwrap_err();
        assert!(error.to_string().contains("protocol View failure"));
        let router_error = session.take_error().unwrap();
        assert_eq!(router_error.source, Some(root));
        assert!(router_error.message.contains("protocol View failure"));
        assert_eq!(session.router().active().unwrap().id, root);
    }

    #[test]
    fn passthrough_binding_still_prepares_its_command_decision() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let cancellation = crate::lifecycle::CancellationToken::new();
        let context = ViewContext::new(ViewInstanceId(40), "dmenu:main");
        let parameters = config.instantiate_parameters("dmenu:main").unwrap();
        let snapshot = ViewCommandSnapshot {
            engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
            parameters: config.parameter_values(&parameters).unwrap(),
            raw_input: "typed".to_string(),
            runtime: Value::Null,
            publication: Some(crate::view::ViewPublication::new(
                serde_json::json!({
                    "item": null,
                    "input": "typed",
                }),
                true,
            )),
            owner_view: None,
            revision: 0,
        };

        let invocation = test_invocation(&config, "dmenu:main");
        let service = ProtocolCommandService::new(Arc::new(config), invocation, cancellation);
        let view_cmds = service.build_view_commands(&context, &snapshot).unwrap();
        let accept_cmd = view_cmds
            .into_iter()
            .find(|entry| entry.id == "accept")
            .expect("view commands must contain accept");
        assert!(matches!(
            accept_cmd.action.execute().unwrap(),
            ViewDecision::Return(_)
        ));
    }

    #[test]
    fn command_call_records_caller_and_runs_non_null_return_continuation() {
        let config = crate::workflow::config::load_test_fixture().unwrap();
        let cancellation = crate::lifecycle::CancellationToken::new();
        let caller = ViewContext::new(ViewInstanceId(41), "dmenu:main");
        let parameters = config.instantiate_parameters("dmenu:main").unwrap();
        let snapshot = ViewCommandSnapshot {
            engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
            parameters: config.parameter_values(&parameters).unwrap(),
            raw_input: String::new(),
            runtime: serde_json::json!({"revision": 2}),
            publication: Some(crate::view::ViewPublication::new(
                serde_json::json!({
                    "item": {
                        "text": "first",
                        "value": "0",
                        "metadata": {},
                        "owner_view": "dmenu:main"
                    },
                    "input": ""
                }),
                true,
            )),
            owner_view: Some("dmenu:main".to_string()),
            revision: 2,
        };

        let stdin_path =
            std::env::temp_dir().join(format!("tlaunch-protocol-session-{}", std::process::id()));
        std::fs::write(&stdin_path, b"first\n").unwrap();
        let invocation = Arc::new(
            crate::workflow::InvocationContext::new(
                "dmenu:main".to_string(),
                serde_json::json!({
                    "stdin": {
                        "path": stdin_path.to_string_lossy(),
                        "length": 6,
                        "is_tty": false
                    }
                }),
                config.instantiate_parameters("dmenu:main").unwrap(),
            )
            .unwrap(),
        );
        let service = ProtocolCommandService::new(Arc::new(config), invocation, cancellation);
        let shared_snapshot = Arc::new(RwLock::new(ChromeSnapshot::default()));
        let registry = Arc::new(RwLock::new(CommandRegistry::new()));
        let host_cmds = service
            .build_host_commands(shared_snapshot.clone(), Arc::clone(&registry))
            .unwrap();

        let engine_cmds = service.build_engine_commands(&caller, &snapshot).unwrap();
        registry
            .write()
            .unwrap()
            .replace_scope(CommandScope::Engine, engine_cmds)
            .unwrap();
        *shared_snapshot.write().unwrap() =
            ChromeSnapshot::from_registry(&registry.read().unwrap())
                .with_active_instance(Some(caller.instance));

        let cmd_entry = host_cmds.into_iter().find(|e| e.id == "commands").unwrap();
        let call_decision = cmd_entry.action.execute().unwrap();
        let ViewDecision::Transition(crate::view::TransitionRequest::Call {
            request: _req,
            continuation: crate::view::Continuation::Call(boundary),
        }) = call_decision
        else {
            panic!("commands must call");
        };

        let selected_command = serde_json::json!({"ref": {"id": "accept"}});
        let continued = boundary
            .handler
            .resume(
                &crate::view::ViewLocation::new("__selectors:commands"),
                &caller,
                &snapshot,
                &ViewResult::command_selection(selected_command),
            )
            .unwrap();
        let ViewDecision::Return(result) = continued else {
            panic!("selected command must continue into its configured return");
        };
        assert_eq!(result.value, serde_json::json!("first"));

        // Expired or unknown command ID does not execute old callback and returns Stay
        let unknown_command = serde_json::json!({"ref": {"id": "nonexistent"}});
        let continued_unknown = boundary
            .handler
            .resume(
                &crate::view::ViewLocation::new("__selectors:commands"),
                &caller,
                &snapshot,
                &ViewResult::command_selection(unknown_command),
            )
            .unwrap();
        assert!(matches!(continued_unknown, ViewDecision::Stay));

        std::fs::remove_file(stdin_path).unwrap();
    }

    #[test]
    fn root_popup_uses_the_same_content_host_geometry_as_nested_popups() {
        let (mut session, _, _) = session();
        let mut root = request("root");
        root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
        root.presentation.width = Some(12);
        root.presentation.height = Some(6);
        session.start_root(root).unwrap();
        session
            .resize(TerminalSize {
                width: 40,
                height: 10,
            })
            .unwrap();
        assert_eq!(
            session
                .router()
                .active()
                .unwrap()
                .view
                .command_snapshot()
                .runtime,
            serde_json::json!({"width": 10, "height": 4})
        );

        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            Position { x: 16, y: 4 }
        );
        assert_eq!(
            terminal.backend().buffer().cell((14, 2)).unwrap().symbol(),
            "┌"
        );
        assert_eq!(
            terminal.backend().buffer().cell((15, 3)).unwrap().symbol(),
            "r"
        );
    }

    #[test]
    fn resize_and_render_follow_nested_content_host_geometry() {
        let (mut session, _, _) = session();
        session.start_root(request("root")).unwrap();
        session
            .resize(TerminalSize {
                width: 40,
                height: 10,
            })
            .unwrap();
        assert_eq!(
            session
                .router()
                .active()
                .unwrap()
                .view
                .command_snapshot()
                .runtime,
            serde_json::json!({"width": 38, "height": 8})
        );

        for expected in [
            ("child", serde_json::json!({"width": 18, "height": 6})),
            ("grandchild", serde_json::json!({"width": 8, "height": 2})),
        ] {
            session
                .input(InputEvent::Key {
                    key: crate::input::Key::Char('q'),
                    raw: vec![b'q'],
                })
                .unwrap();
            let active = session.router().active().unwrap();
            assert_eq!(active.context.location.target, expected.0);
            assert_eq!(active.view.command_snapshot().runtime, expected.1);
        }

        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut rendered = None;
        terminal
            .draw(|frame| rendered = Some(session.render(frame, frame.area(), None).unwrap()))
            .unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            Position { x: 17, y: 5 }
        );
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((1, 1)).unwrap().symbol(), "r");
        assert_eq!(buffer.cell((10, 1)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((11, 2)).unwrap().symbol(), "c");
        assert_eq!(buffer.cell((15, 3)).unwrap().symbol(), "┌");
        assert_eq!(buffer.cell((16, 4)).unwrap().symbol(), "g");
        assert_eq!(rendered.unwrap().footer.location.label(), "grandchild");
    }

    #[test]
    fn transitions_effects_tasks_and_popup_render_footer_are_hosted() {
        let (mut session, events, effects) = session();
        session.start_root(request("root")).unwrap();
        assert!(session.start_root(request("root")).is_err());
        assert_eq!(
            session.router().active().unwrap().context.presentation.mode,
            crate::workflow::config::ViewPresentationMode::Inline
        );
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('p'),
                raw: vec![b'p'],
            })
            .unwrap();
        assert_eq!(effects.borrow().len(), 1);
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('n'),
                raw: vec![b'n'],
            })
            .unwrap();
        assert_eq!(session.router().stack().len(), 2);
        let child = session.router().active().unwrap().id;
        session
            .task(TaskEvent {
                instance: child,
                task: TaskId(1),
                generation: 1,
                outcome: TaskOutcome::Completed(Value::Null),
            })
            .unwrap();
        assert_eq!(events.borrow().len(), 2);
        assert_eq!(
            session
                .router()
                .active()
                .unwrap()
                .view
                .command_snapshot()
                .publication
                .map(|publication| publication.current),
            Some(serde_json::json!({"instance": child.0, "generation": 1}))
        );
        assert_eq!(
            session.router().active().unwrap().context.presentation.mode,
            crate::workflow::config::ViewPresentationMode::Popup
        );
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let mut rendered = None;
        terminal
            .draw(|frame| {
                rendered = Some(session.render(frame, frame.area(), None).unwrap());
            })
            .unwrap();
        let rendered = rendered.unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            Position { x: 17, y: 5 }
        );
        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().symbol(),
            " "
        );
        assert_eq!(
            terminal.backend().buffer().cell((1, 0)).unwrap().symbol(),
            " "
        );
        assert_eq!(
            terminal.backend().buffer().cell((1, 1)).unwrap().symbol(),
            "r"
        );
        assert_eq!(
            terminal.backend().buffer().cell((15, 3)).unwrap().symbol(),
            "┌"
        );
        assert_eq!(
            terminal.backend().buffer().cell((16, 4)).unwrap().symbol(),
            "c"
        );
        // Popup bottom border contains hints
        assert_eq!(
            terminal.backend().buffer().cell((15, 6)).unwrap().symbol(),
            "└"
        );
        assert_eq!(
            terminal.backend().buffer().cell((24, 6)).unwrap().symbol(),
            "┘"
        );
        let bottom_border: String = (15..=24)
            .map(|x| terminal.backend().buffer().cell((x, 6)).unwrap().symbol())
            .collect();
        assert!(bottom_border.contains("ok"));

        // Global footer row (y = 9) is blank while popup is active
        for x in 0..40 {
            assert_eq!(
                terminal.backend().buffer().cell((x, 9)).unwrap().symbol(),
                " "
            );
        }

        assert_eq!(rendered.footer.location.label(), "child");
        assert_eq!(rendered.footer.status.as_deref(), Some("child"));
        session.eof().unwrap();
        assert!(session.router().stack().is_empty());
    }

    #[test]
    fn view_diagnostic_clears_when_resolved_and_session_error_clears_on_input() {
        let view_error = Rc::new(RefCell::new(None));
        struct DiagView(Rc<RefCell<Option<String>>>);
        impl View for DiagView {
            fn command_snapshot(&self) -> ViewCommandSnapshot {
                ViewCommandSnapshot {
                    engine_type: "test".to_string(),
                    parameters: Value::Null,
                    raw_input: String::new(),
                    runtime: Value::Null,
                    publication: None,
                    owner_view: None,
                    revision: 0,
                }
            }
            fn event(&mut self, _: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
                Ok(ViewDecision::Stay)
            }
            fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
                Ok(RenderResult {
                    cursor: None,
                    metadata: ViewMetadata {
                        status: None,
                        error: None,
                        bindings: None,
                    },
                })
            }
            fn chrome(&self, _context: &ViewContext) -> Result<crate::view::ViewChrome> {
                Ok(crate::view::ViewChrome {
                    status: None,
                    error: self.0.borrow().clone(),
                    ..Default::default()
                })
            }
        }
        struct DiagFactory(Rc<RefCell<Option<String>>>);
        impl ViewFactory for DiagFactory {
            fn create(
                &self,
                _: &NavigationRequest,
                _: ViewInstanceId,
                _: &ViewServices<'_>,
            ) -> Result<Box<dyn View>> {
                Ok(Box::new(DiagView(Rc::clone(&self.0))))
            }
        }
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let router = Router::new(
            Box::new(routes),
            Box::new(DiagFactory(Rc::clone(&view_error))),
        );
        let mut session = ProtocolSession::new(
            router,
            Box::new(Effects {
                calls: Rc::new(RefCell::new(Vec::new())),
            }),
        );
        session.start_root(request("root")).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let render_footer_error =
            |session: &mut ProtocolSession, terminal: &mut Terminal<TestBackend>| {
                let mut res = None;
                terminal
                    .draw(|frame| {
                        res = Some(
                            session
                                .render(frame, frame.area(), None)
                                .unwrap()
                                .footer
                                .error,
                        );
                    })
                    .unwrap();
                res.unwrap()
            };

        // 1. Initially no error
        assert_eq!(render_footer_error(&mut session, &mut terminal), None);

        // 2. View produces an error
        *view_error.borrow_mut() = Some("invalid input syntax".to_string());
        assert_eq!(
            render_footer_error(&mut session, &mut terminal),
            Some("ERROR [root]: invalid input syntax".to_string())
        );

        // 3. View clears its error (e.g. user corrected the input)
        *view_error.borrow_mut() = None;
        assert_eq!(render_footer_error(&mut session, &mut terminal), None);
        assert_eq!(session.active_error, None);

        // 4. Session reports an error
        session.report_error("session navigation failure");
        // Render does NOT clear session error even though view_error is None
        assert_eq!(
            render_footer_error(&mut session, &mut terminal),
            Some("ERROR [root]: session navigation failure".to_string())
        );
        assert_eq!(
            render_footer_error(&mut session, &mut terminal),
            Some("ERROR [root]: session navigation failure".to_string())
        );

        // 5. Next user input clears the session error
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('a'),
                raw: vec![b'a'],
            })
            .unwrap();
        assert_eq!(session.active_error, None);
        assert_eq!(render_footer_error(&mut session, &mut terminal), None);
    }

    #[test]
    fn focus_exclusive_status_lifecycle() {
        let (mut session, _events, _effects) = session();
        session.start_root(request("root")).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

        // 1. Root is focused: global footer row (y=9) has root's label and local bindings
        terminal
            .draw(|frame| {
                let _ = session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();
        let footer_row: String = (0..40)
            .map(|x| terminal.backend().buffer().cell((x, 9)).unwrap().symbol())
            .collect();
        assert!(footer_row.contains("root"));
        assert!(footer_row.contains("ok"));

        // 2. Open child popup ('n' key)
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('n'),
                raw: vec![b'n'],
            })
            .unwrap();

        terminal
            .draw(|frame| {
                let _ = session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();

        // Global footer is blank
        for x in 0..40 {
            assert_eq!(
                terminal.backend().buffer().cell((x, 9)).unwrap().symbol(),
                " "
            );
        }
        // Child popup (15..=24, y=6) bottom border has local key hints
        let child_bottom: String = (15..=24)
            .map(|x| terminal.backend().buffer().cell((x, 6)).unwrap().symbol())
            .collect();
        assert!(child_bottom.contains("ok"));

        // 3. Child returns to root ('r' key)
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('r'),
                raw: vec![b'r'],
            })
            .unwrap();

        terminal
            .draw(|frame| {
                let _ = session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();

        // Global footer is restored with root's label and local bindings
        let restored_footer: String = (0..40)
            .map(|x| terminal.backend().buffer().cell((x, 9)).unwrap().symbol())
            .collect();
        assert!(restored_footer.contains("root"));
        assert!(restored_footer.contains("ok"));
    }

    #[test]
    fn view_preferred_top_inset_controls_content_area_top_offset_and_resize() {
        let (mut session, _, _) = session();
        session.start_root(request("zero_inset")).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

        terminal
            .draw(|frame| {
                let _ = session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();

        // zero_inset has preferred_top_inset() == 0, so content starts at y = 0
        assert_eq!(
            terminal.backend().buffer().cell((1, 0)).unwrap().symbol(),
            "z"
        );

        // resize dispatches height = 10 - 0 (top) - 1 (footer) = 9
        session
            .resize(TerminalSize {
                width: 40,
                height: 10,
            })
            .unwrap();
        assert_eq!(
            session.router().stack()[0].view.command_snapshot().runtime,
            serde_json::json!({
                "width": 38,
                "height": 9
            })
        );
    }

    #[test]
    fn custom_view_commands_switch_to_exclusive_commands_in_session() {
        let (mut session, _events, _effects) = session();
        session.start_root(request("root")).unwrap();

        // Initially, root view commands are active (SyntheticView returns "local")
        assert!(
            session
                .registry
                .read()
                .unwrap()
                .resolve_id("local")
                .is_some()
        );

        // When custom commands are returned by active view (e.g. completion)
        let accept = CommandEntry::new(
            "completion.accept",
            Some("Accept".to_string()),
            Some(crate::input::Key::Enter),
            CommandScope::View,
            Arc::new(|| Ok(ViewDecision::Stay)),
        );
        let cancel = CommandEntry::new(
            "completion.cancel",
            Some("Cancel".to_string()),
            Some(crate::input::Key::Escape),
            CommandScope::View,
            Arc::new(|| Ok(ViewDecision::Stay)),
        );

        session
            .registry
            .write()
            .unwrap()
            .replace_scope(CommandScope::View, vec![accept, cancel])
            .unwrap();
        session
            .registry
            .write()
            .unwrap()
            .replace_scope(CommandScope::Engine, Vec::new())
            .unwrap();

        assert_eq!(
            session
                .registry
                .read()
                .unwrap()
                .resolve(crate::input::Key::Enter)
                .unwrap()
                .id,
            "completion.accept"
        );
        assert_eq!(
            session
                .registry
                .read()
                .unwrap()
                .resolve(crate::input::Key::Escape)
                .unwrap()
                .id,
            "completion.cancel"
        );
        assert!(
            session
                .registry
                .read()
                .unwrap()
                .resolve_id("local")
                .is_none()
        );
    }
}
