use super::command_adapter::CommandService;
use crate::command::{ChromeSnapshot, CommandHandler, CommandRegistry, CommandScope};
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

        if let ViewEvent::Input(InputEvent::Key { key, raw }) = &event {
            let entry_opt = self.registry.read().unwrap().resolve(*key).cloned();
            if let Some(entry) = entry_opt {
                if entry.scope == CommandScope::View {
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
                command_decision = match entry.handler {
                    CommandHandler::Action(action) => action.execute()?,
                    CommandHandler::Event => {
                        if let Some(active_instance) = self.router.active_mut() {
                            let context = &active_instance.context;
                            active_instance.view.on_command(&entry.id, context)?
                        } else {
                            ViewDecision::Stay
                        }
                    }
                };
                if let Some(source) = active {
                    self.router
                        .process_with_effects(command_decision.clone(), source, effects)?;
                }
            } else if let Some(active_instance) = self.router.active_mut() {
                let context = &active_instance.context;
                if let Some(receiver) = active_instance.view.fallback_receiver() {
                    executed_command = true;
                    command_decision = receiver.on_unbound_key(*key, raw, context)?;
                    if let Some(source) = active {
                        self.router.process_with_effects(
                            command_decision.clone(),
                            source,
                            effects,
                        )?;
                    }
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
            let view_entries = self.commands.build_view_commands(context, &snapshot)?;
            let engine_entries = active.view.engine_commands(context);
            (view_entries, engine_entries)
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
mod tests;
