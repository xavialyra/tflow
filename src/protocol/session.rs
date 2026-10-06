mod handoff;

use self::handoff::{NavigationHandoff, SettledFrame, paint_retained};
use super::command_adapter::CommandService;
use crate::command::{BindingLayer, ChromeSnapshot, CommandRegistry};
use crate::input::{InputEvent, Key};
use crate::protocol::contracts::{TaskEvent, ViewInstanceId};
#[cfg(test)]
use crate::protocol::contracts::{TaskId, TaskOutcome};
use crate::ui::chrome::{ContentHost, FooterModel, FooterRenderer, PaneLayout};
use crate::view::{
    EffectExecutor, NavigationRequest, RenderContext, RenderResult, Router, TerminalSize,
    ViewDecision, ViewEvent, ViewResult,
};
#[cfg(test)]
use crate::view::{ViewCommandSnapshot, ViewContext};
use anyhow::Result;
use ratatui::{
    Frame,
    layout::{Position, Rect},
};
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
    navigation: NavigationHandoff,
    last_view_revision: u64,
    layout_sizes: std::collections::BTreeMap<ViewInstanceId, TerminalSize>,
}

#[cfg(test)]
struct TestCommandService;

#[cfg(test)]
impl CommandService for TestCommandService {
    fn registry(&self) -> std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>> {
        std::sync::Arc::new(std::sync::RwLock::new(
            crate::command::CommandRegistry::new(),
        ))
    }

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

    fn is_view_dynamic(&self, _: &str) -> bool {
        false
    }

    fn chrome_commands_show(&self, _: Option<&str>) -> Result<Vec<String>> {
        Ok(vec!["enter".to_string(), "ctrl+k".to_string()])
    }

    fn unbind_rules(&self, _: Option<&str>) -> Result<crate::command::UnbindRules> {
        Ok(crate::command::UnbindRules::default())
    }

    fn update_active_snapshot(&self, _: &ViewCommandSnapshot) {}

    fn execute_entry(
        &self,
        entry: &crate::command::CommandEntry,
        _: crate::protocol::contracts::ViewInstanceId,
        _: Option<u64>,
    ) -> Result<ViewDecision> {
        // Mirrors the real dispatcher: an engine action is not a definition, so
        // it travels as a decision for the instance's own engine to run.
        if entry.id == crate::command::OPEN_COMPANION {
            return Ok(ViewDecision::OpenCompanion);
        }
        if entry.layer == crate::command::BindingLayer::Engine {
            return Ok(ViewDecision::EngineAction(entry.id.clone()));
        }
        if entry.id == "global.copy" {
            Ok(ViewDecision::Effect(
                crate::view::EffectRequest::CopyToClipboard("global".to_string()),
            ))
        } else {
            Ok(ViewDecision::Stay)
        }
    }

    fn host_companion_keys(&self) -> Vec<Key> {
        vec![Key::Ctrl('l')]
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
            navigation: NavigationHandoff::default(),
            last_view_revision: 0,
            layout_sizes: Default::default(),
        }
    }

    pub(crate) fn configured(
        router: Router,
        commands: Box<dyn CommandService>,
        theme: crate::ui::theme::ResolvedTheme,
        mut runtime_log: crate::diagnostics::RuntimeLog,
    ) -> Self {
        let warning = runtime_log.take_warning_record();
        let error_source = warning.as_ref().map(|_| ErrorSource::StartupWarning);
        let registry = commands.registry();
        let shared_snapshot = Arc::new(RwLock::new(ChromeSnapshot::default()));
        let host_entries = commands
            .build_host_commands(Arc::clone(&shared_snapshot), Arc::clone(&registry))
            .unwrap_or_default();
        let _ = registry
            .write()
            .unwrap()
            .replace_layer(BindingLayer::Host, host_entries);
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
            navigation: NavigationHandoff::default(),
            last_view_revision: 0,
            layout_sizes: Default::default(),
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
        let had_info = self.active_info.is_some();
        let expired_retention =
            matches!(event, ViewEvent::Tick) && self.navigation.expire(Instant::now());
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
        let layout_invalidated = self.dispatch_layout_resize(effects)? != ViewDecision::Stay;
        if let Some(active_instance) = self.router.active() {
            let snapshot = active_instance.command_snapshot();
            self.commands.update_active_snapshot(&snapshot);
        }
        let active = self.router.active().map(|entry| entry.id);

        let mut executed_command = false;
        let mut command_decision = ViewDecision::Stay;

        if let ViewEvent::Input(InputEvent::Key { key, raw }) = &event {
            let is_escape = *key == Key::Escape;

            if !executed_command {
                let entry_opt = {
                    let registry = self.registry.read().unwrap();
                    registry.resolve(*key).cloned()
                };
                if let Some(entry) = entry_opt {
                    if entry.layer == BindingLayer::View {
                        let is_loading = self.router.active().is_some_and(|a| {
                            let snapshot = a.view.command_snapshot();
                            snapshot.engine_type == crate::workflow::config::ENGINE_PICKER
                                && snapshot.publication.as_ref().is_some_and(|p| !p.ready)
                        });
                        if is_loading && self.pending_key.is_none() {
                            self.pending_key = Some(*key);
                            return Ok(ViewDecision::Stay);
                        }
                    }
                    executed_command = true;
                    let caller = active.unwrap_or(crate::protocol::contracts::ViewInstanceId(1));
                    command_decision = self.commands.execute_entry(&entry, caller, None)?;
                    if let Some(source) = active {
                        command_decision =
                            self.router
                                .process_with_effects(command_decision, source, effects)?;
                    }
                } else if is_escape
                    && self
                        .router
                        .active()
                        .is_some_and(|a| a.input.mode.is_visible() && !a.input.is_empty())
                {
                    executed_command = true;
                    if let Some(source) = active {
                        command_decision = self.router.process_with_effects(
                            ViewDecision::EditInput(crate::view::InputEdit::Clear),
                            source,
                            effects,
                        )?;
                    }
                } else {
                    let unhandled = self
                        .router
                        .active()
                        .map(|a| a.view.unhandled_input_behavior())
                        .unwrap_or_default();
                    let mut handled = false;
                    if unhandled == crate::view::UnhandledInputBehavior::ForwardToOmnibar
                        && self
                            .router
                            .active()
                            .is_some_and(|a| a.input.mode.is_visible())
                        && let Some(source) = active
                    {
                        command_decision = self
                            .router
                            .edit_host_input(source, crate::view::InputEdit::Key(*key))?;
                        if command_decision != ViewDecision::Stay {
                            handled = true;
                            executed_command = true;
                            command_decision = self.router.process_with_effects(
                                command_decision,
                                source,
                                effects,
                            )?;
                        }
                    }

                    if !handled
                        && unhandled != crate::view::UnhandledInputBehavior::Ignore
                        && let Some(active_instance) = self.router.active_mut()
                    {
                        let context = &active_instance.context;
                        if let Some(receiver) = active_instance.view.fallback_receiver() {
                            executed_command = true;
                            command_decision = receiver.on_unbound_key(*key, raw, context)?;
                            if let Some(source) = active {
                                command_decision = self.router.process_with_effects(
                                    command_decision.clone(),
                                    source,
                                    effects,
                                )?;
                            }
                        }
                    }
                }
            }
        } else if let ViewEvent::Input(InputEvent::Paste {
            text: Some(text), ..
        }) = &event
            && self.router.active().is_some_and(|a| {
                a.input.mode.is_visible()
                    && a.view.unhandled_input_behavior()
                        == crate::view::UnhandledInputBehavior::ForwardToOmnibar
            })
        {
            executed_command = true;
            if let Some(source) = active {
                command_decision = self.router.process_with_effects(
                    ViewDecision::EditInput(crate::view::InputEdit::Paste(text.clone())),
                    source,
                    effects,
                )?;
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
        let companion_invalidated = self.sync_companion_data_flow()?;

        if matches!(event, ViewEvent::Task(_))
            && let Some(key) = self.pending_key.take()
        {
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

        if decision == ViewDecision::Stay
            && (layout_invalidated
                || companion_invalidated
                || expired_retention
                || (had_info && self.active_info.is_none()))
        {
            Ok(ViewDecision::Invalidate)
        } else {
            Ok(decision)
        }
    }

    pub(crate) fn sync_active_commands(&mut self) -> Result<()> {
        let active_info = self
            .router
            .active()
            .map(|active| (active.id, active.context.clone(), active.command_snapshot()));
        let host_changed = {
            let mut registry = self.registry.write().unwrap();
            let mut entries = registry
                .layer_entries(BindingLayer::Host)
                .iter()
                .filter(|e| e.id != crate::command::OPEN_COMPANION)
                .cloned()
                .collect::<Vec<_>>();
            let companion_keys = self.commands.host_companion_keys();
            if self.router.active_companion().is_some() {
                for key in companion_keys {
                    if !entries.iter().any(|entry| entry.key == Some(key)) {
                        entries.push(crate::command::CommandEntry::new(
                            crate::command::OPEN_COMPANION,
                            crate::command::host_action_label(crate::command::OPEN_COMPANION)
                                .map(str::to_string),
                            Some(key),
                            BindingLayer::Host,
                        ));
                    }
                }
            }
            registry
                .replace_layer(BindingLayer::Host, entries)?
                .is_some()
        };

        if let Some((_, _, ref snapshot)) = active_info {
            self.commands.update_active_snapshot(snapshot);
        }

        let active_id = active_info.as_ref().map(|(id, _, _)| *id);
        let active_view = active_info
            .as_ref()
            .map(|(_, context, _)| context.location.target.clone());
        let active_parameters = active_info
            .as_ref()
            .map(|(_, _, snapshot)| snapshot.parameters.clone())
            .unwrap_or(serde_json::Value::Null);
        let active_raw_input = active_info
            .as_ref()
            .map(|(_, _, snapshot)| snapshot.raw_input.clone())
            .unwrap_or_default();
        let active_revision = active_info
            .as_ref()
            .map(|(_, _, snapshot)| snapshot.revision)
            .unwrap_or(0);

        let is_same_instance =
            self.chrome_snapshot.active_instance == active_id && active_id.is_some();
        let is_dynamic = active_view
            .as_ref()
            .is_some_and(|target| self.commands.is_view_dynamic(target));
        let revision_changed = active_revision != self.last_view_revision;
        let unbind_rules = self.commands.unbind_rules(active_view.as_deref())?;
        let unbind_changed = self.registry.write().unwrap().replace_unbinds(unbind_rules);

        if is_same_instance
            && (!is_dynamic || !revision_changed)
            && !unbind_changed
            && !host_changed
        {
            if self.chrome_snapshot.active_view != active_view
                || self.chrome_snapshot.active_parameters != active_parameters
                || self.chrome_snapshot.active_raw_input != active_raw_input
            {
                let reg = self.registry.read().unwrap();
                self.chrome_snapshot = ChromeSnapshot::from_registry(&reg)
                    .with_active_instance(active_id)
                    .with_chrome_commands_show(
                        self.commands.chrome_commands_show(active_view.as_deref())?,
                    )
                    .with_active_view(
                        active_view.clone(),
                        active_parameters.clone(),
                        active_raw_input.clone(),
                    );
                *self.shared_snapshot.write().unwrap() = self.chrome_snapshot.clone();
            }
            return Ok(());
        }

        self.last_view_revision = active_revision;

        let (view_entries, engine_entries) =
            if let Some((_, ref context, ref snapshot)) = active_info {
                let view_entries = self.commands.build_view_commands(context, snapshot)?;
                let engine_entries = self
                    .router
                    .active()
                    .map(|a| a.view.engine_commands(context))
                    .unwrap_or_default();
                (view_entries, engine_entries)
            } else {
                (Vec::new(), Vec::new())
            };

        let mut changed = false;
        let mut reg = self.registry.write().unwrap();
        if reg
            .replace_layer(BindingLayer::View, view_entries)?
            .is_some()
        {
            changed = true;
        }
        if reg
            .replace_layer(BindingLayer::Engine, engine_entries)?
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
                .with_chrome_commands_show(
                    self.commands.chrome_commands_show(active_view.as_deref())?,
                )
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
        self.dispatch_layout_resize(effects)?;
        Ok(())
    }

    fn dispatch_active_resize(&mut self, effects: &mut dyn EffectExecutor) -> Result<ViewDecision> {
        self.dispatch_layout_resize(effects)
    }

    fn pane_layout(&self, index: usize, area: Rect) -> PaneLayout {
        let instance = &self.router.stack()[index];
        let default_size = crate::workflow::config::CompanionSize::default();
        let companion_size = instance
            .companion
            .as_ref()
            .map(|c| c.size.as_ref().unwrap_or(&default_size));
        PaneLayout::new(
            area,
            instance.input.mode.is_visible(),
            instance.view.input_divider(),
            companion_size,
        )
    }

    fn dispatch_layout_resize(&mut self, effects: &mut dyn EffectExecutor) -> Result<ViewDecision> {
        if self.terminal.width == 0 || self.terminal.height == 0 {
            return Ok(ViewDecision::Stay);
        }
        let host = ContentHost::default();
        let terminal = Rect::new(0, 0, self.terminal.width, self.terminal.height);
        let stack = self.router.stack();
        let Some(active_index) = stack.len().checked_sub(1) else {
            return Ok(ViewDecision::Stay);
        };
        let first = host.visible_base_index(stack, active_index).unwrap_or(0);
        let mut sizes = Vec::new();
        for (index, instance) in stack.iter().enumerate().skip(first) {
            let area = host.view_content_area(stack, index, terminal);
            let layout = self.pane_layout(index, area);
            sizes.push((
                instance.id,
                TerminalSize {
                    width: layout.primary.width,
                    height: layout.primary.height,
                },
            ));
            if let Some(companion) = &instance.companion {
                let area = layout.companion.unwrap_or_default();
                sizes.push((
                    companion.instance.id,
                    TerminalSize {
                        width: area.width,
                        height: area.height,
                    },
                ));
            }
        }
        let live_ids = stack
            .iter()
            .flat_map(|entry| {
                std::iter::once(entry.id).chain(entry.companion.iter().map(|c| c.instance.id))
            })
            .collect::<Vec<_>>();
        self.layout_sizes.retain(|id, _| live_ids.contains(id));
        let mut invalidated = false;
        for (id, size) in sizes {
            if self.layout_sizes.get(&id) != Some(&size) {
                self.router.resize_instance(id, size, effects)?;
                self.layout_sizes.insert(id, size);
                invalidated = true;
            }
        }
        Ok(if invalidated {
            ViewDecision::Invalidate
        } else {
            ViewDecision::Stay
        })
    }

    pub(crate) fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        image_picker: Option<crate::terminal::ImagePicker>,
    ) -> Result<ProtocolRenderResult> {
        self.render_at(frame, area, image_picker, Instant::now())
    }

    fn render_at(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        image_picker: Option<crate::terminal::ImagePicker>,
        now: Instant,
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
        let base_entry = base_index.and_then(|index| self.router.stack().get(index));
        let active_instance_id = self.router.stack()[active_index].id;
        let is_active_loading = self.router.stack()[active_index]
            .view
            .command_snapshot()
            .publication
            .as_ref()
            .is_some_and(|p| !p.ready);

        // Grace is keyed on the topmost instance, so a freshly pushed popup
        // participates exactly like a freshly mounted base View: the surface it
        // would take over stays on screen until the instance publishes or the
        // window expires.
        let retained = self
            .navigation
            .begin(Some(active_instance_id), is_active_loading, now);

        let top_padding = base_index
            .or(Some(0))
            .and_then(|index| self.router.stack().get(index))
            .map(|entry| entry.view.preferred_top_inset())
            .unwrap_or(0);
        let content_area = content_host.content_area(area, top_padding);

        let mut host_cursor = None;
        let (view, active_render_area, active_popup_rect) = content_host.render_views(
            frame,
            area,
            content_area,
            self.router.stack(),
            &self.theme,
            |index, frame, rect| {
                let instance = &self.router.stack()[index];
                // Keep the image-capable context for every visible layer. A
                // terminal graphics protocol is stateful; suppressing the
                // covered layer makes its image disappear permanently in tmux
                // because the protocol may not transmit again after the popup.
                let layout = self.pane_layout(index, rect);
                if let Some(omnibar) = layout.omnibar {
                    let cursor = crate::ui::chrome::render_omnibar_widget(
                        frame,
                        omnibar,
                        &instance.input,
                        &self.theme,
                    );
                    if index == active_index {
                        host_cursor = cursor;
                    }
                }
                if let Some(divider) = layout.divider {
                    frame.render_widget(
                        ratatui::widgets::Paragraph::new(ratatui::text::Line::styled(
                            "─".repeat(divider.width as usize),
                            self.theme.chrome.divider,
                        )),
                        divider,
                    );
                }
                let primary = if layout.primary.width > 0 && layout.primary.height > 0 {
                    self.router.render_at(
                        index,
                        frame,
                        layout.primary,
                        &RenderContext::new(self.terminal, image_picker),
                    )?
                } else {
                    RenderResult::default()
                };
                if let Some(separator) = layout.separator {
                    let border = if separator.height == 1 && separator.width > 1 {
                        ratatui::widgets::Borders::TOP
                    } else {
                        ratatui::widgets::Borders::LEFT
                    };
                    frame.render_widget(
                        ratatui::widgets::Block::new()
                            .borders(border)
                            .border_style(self.theme.capture.document.border),
                        separator,
                    );
                }
                if let Some(companion) = layout.companion {
                    self.router.render_companion_for_instance(
                        index,
                        frame,
                        companion,
                        &RenderContext::new(self.terminal, image_picker),
                    )?;
                }
                Ok(primary)
            },
        )?;

        let active_render_area = self.pane_layout(active_index, active_render_area).primary;
        // While the target is loading, keep the previous frame's pixels. A
        // freshly pushed popup is covered instead of cleared, so the surface
        // underneath stays put until the popup can render itself.
        let covered = match &retained {
            Some(retained) => {
                let retain_area = active_popup_rect.or_else(|| {
                    base_index.zip(base_entry).and_then(|(index, entry)| {
                        entry
                            .view
                            .retained_content_area(self.pane_layout(index, content_area).body)
                    })
                });
                paint_retained(frame, retained, content_area, retain_area)
            }
            None => None,
        };

        if let Some((x, y)) = host_cursor {
            if !covered.is_some_and(|covered| covered.contains(Position { x, y })) {
                frame.set_cursor_position((x, y));
            }
        } else if active_render_area.width > 0
            && active_render_area.height > 0
            && let Some(cursor) = &view.cursor
            && cursor.visible
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
            // Retained pixels are not this View's own content, so a cursor that
            // lands inside the covered region would point at stale content.
            if !covered.is_some_and(|covered| covered.contains(Position { x, y })) {
                frame.set_cursor_position((x, y));
            }
        }

        let metadata = view.metadata.clone();
        let footer_commands = self.chrome_snapshot.footer_commands();
        let current_footer = FooterModel {
            location: footer_location,
            status: chrome_snapshot.status.or(metadata.status),
            error: self.active_error.clone().or(chrome_snapshot.error),
            info: self.active_info.as_ref().map(|info| info.label.clone()),
            commands: footer_commands,
        };
        // Chrome grace uses the same retention decision as the pixels: while the
        // active instance loads, keep the settled status and commands so the
        // footer does not blank before the target publishes. When a popup is
        // loading, its entire chrome is deferred along with its surface, so the
        // footer fully preserves the settled base View without prematurely
        // leaking the popup's title or bindings onto the global footer row.
        let footer = match &retained {
            Some(retained) => {
                if active_popup_rect.is_some() {
                    let mut footer = retained.settled.footer.clone();
                    if let Some(error) = &self.active_error {
                        footer.error = Some(error.clone());
                    }
                    if let Some(info) = &self.active_info {
                        footer.info = Some(info.label.clone());
                    }
                    footer
                } else {
                    FooterModel {
                        location: current_footer.location.clone(),
                        status: current_footer
                            .status
                            .clone()
                            .or_else(|| retained.settled.footer.status.clone()),
                        error: current_footer.error.clone(),
                        info: current_footer.info.clone(),
                        commands: if current_footer.commands.is_empty() {
                            retained.settled.footer.commands.clone()
                        } else {
                            current_footer.commands.clone()
                        },
                    }
                }
            }
            None => current_footer,
        };
        let footer_area = content_host.footer_area(area);
        if let Some(popup_rect) = active_popup_rect.filter(|_| retained.is_none()) {
            footer_renderer.render_blank(frame, footer_area, &self.theme);
            let show_title = self.router.stack()[active_index]
                .context
                .presentation
                .show_title;
            content_host.render_active_popup_border(
                frame,
                popup_rect,
                &footer,
                &self.theme,
                show_title,
            );
            if self.theme.chrome.dim_backdrop {
                content_host.dim_backdrop(frame, area, popup_rect, self.theme.chrome.backdrop);
            }
        } else {
            footer_renderer.render(frame, footer_area, &footer, &self.theme);
        }

        // A settled top instance owns the surface; this frame becomes the
        // candidate the next navigation or popup push may retain. A popup frame
        // is never settled, so returning to the base never resurrects overlay
        // pixels.
        if !is_active_loading && active_popup_rect.is_none() {
            self.navigation.settle(SettledFrame {
                instance: active_instance_id,
                buffer: Arc::new(frame.buffer_mut().clone()),
                area: content_area,
                footer: footer.clone(),
            });
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

    fn sync_companion_data_flow(&mut self) -> Result<bool> {
        let mut invalidated = false;
        if let Some(active_instance) = self.router.active() {
            let snapshot = active_instance.view.command_snapshot();
            if let Some(companion) = self.router.active_companion() {
                if !companion.instance.view.follows_companion_data() {
                    return Ok(false);
                }

                if let Some(args_tmpl) = &companion.args_template {
                    let current_val = snapshot
                        .publication
                        .as_ref()
                        .map(|p| p.current.clone())
                        .unwrap_or(serde_json::Value::Null);
                    let proj_ctx = crate::workflow::projection::ContextSource {
                        selection: if current_val.is_object() || current_val.is_array() {
                            if let Some(item) = current_val.get("item") {
                                Some(item)
                            } else {
                                Some(&current_val)
                            }
                        } else {
                            None
                        },
                        input: if snapshot.raw_input.is_empty() {
                            None
                        } else {
                            Some(&snapshot.raw_input)
                        },
                        query: Some(&snapshot.parameters),
                    };
                    let json_tmpl = crate::workflow::config::toml_to_json(args_tmpl)?;
                    let mut new_query =
                        crate::workflow::projection::project_value(&json_tmpl, &proj_ctx);
                    if (matches!(
                        &new_query,
                        serde_json::Value::Object(_) | serde_json::Value::Array(_)
                    )) && self
                        .router
                        .validate_query(&companion.target, &new_query)
                        .is_err()
                    {
                        new_query = serde_json::Value::String(new_query.to_string());
                    }
                    if companion.last_query.as_ref() != Some(&new_query)
                        && let Some(companion_mut) = self.router.active_companion_mut()
                    {
                        companion_mut.last_query = Some(new_query.clone());
                        companion_mut.instance.context.query.values = new_query.clone();
                        let decision = companion_mut.instance.view.on_companion_data_changed(
                            &new_query,
                            &companion_mut.instance.context,
                        )?;
                        invalidated = decision != ViewDecision::Stay;
                    }
                }
            }
        }
        Ok(invalidated)
    }

    #[cfg(test)]
    pub(crate) fn take_error(&mut self) -> Option<crate::view::RouterError> {
        self.router.take_error()
    }
}

#[cfg(test)]
mod host_tests;
#[cfg(test)]
mod tests;
