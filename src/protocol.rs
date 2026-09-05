//! Host for the engine-neutral Input And Navigation protocol.
//!
//! The application composition root owns this facade as its single protocol

use crate::view::{
    CallBoundary, CallReturnHandler, EffectExecutor, InputEvent, NavigationRequest,
    RenderContext, RenderResult, Router, TaskEvent, TerminalSize, ViewCommandSnapshot, ViewContext,
    ViewDecision, ViewEvent, ViewInstanceId, ViewLocation, ViewResult,
};
use crate::chrome::{ContentHost, FooterModel, FooterRenderer};
use anyhow::{Context, Result};
use ratatui::{Frame, layout::Rect};

/// Immutable command bindings projected into protocol-native Views.
/// Runtime preparation remains owned by `ProtocolCommandService`.
#[derive(Clone)]
pub(crate) struct ViewCommandBindings {
    pub(crate) bindings: Vec<ProtocolCommandBinding>,
    pub(crate) overflow_binding: Option<ProtocolCommandBinding>,
    pub(crate) has_unbound: bool,
    view_invocations:
        std::collections::BTreeMap<(String, String), crate::command::CommandInvocation>,
    pub(crate) current_fields: &'static [&'static str],
}

#[derive(Clone)]
pub(crate) struct ProtocolCommandBinding {
    pub(crate) key: crate::view::Key,
    pub(crate) label: Option<String>,
    #[allow(dead_code)]
    pub(crate) visibility: crate::config::CommandBindingVisibility,
    pub(crate) invocation: crate::command::CommandInvocation,
}

impl ViewCommandBindings {
    pub(crate) fn new(
        config: &crate::config::Config,
        view_ref: &str,
        _cancellation: crate::lifecycle::CancellationObserver,
        current_fields: &'static [&'static str],
    ) -> anyhow::Result<Self> {
        let mut bindings = Vec::new();
        let mut overflow_binding = None;
        let mut add = |id: String,
                       binding: &crate::config::CommandBinding,
                       invocation|
         -> anyhow::Result<()> {
            let Some(key) = binding.key(&id) else {
                return Ok(());
            };
            let visibility = binding
                .visibility(&id)
                .unwrap_or(crate::config::CommandBindingVisibility::Always);
            let key = crate::view::Key::parse_binding(&crate::config::normalize_key(key)?)?;
            let entry = ProtocolCommandBinding {
                key,
                label: binding.label(&id).map(str::to_string),
                visibility,
                invocation,
            };
            if visibility == crate::config::CommandBindingVisibility::Overflow {
                overflow_binding = Some(entry);
            } else {
                bindings.push(entry);
            }
            Ok(())
        };
        if let Some(view) = config.view(view_ref) {
            for (id, command) in &view.commands {
                add(
                    id.clone(),
                    &crate::config::CommandBinding {
                        key: command.key.clone(),
                        label: Some(command.label.clone()),
                        visibility: None,
                        action: Some(command.action.clone()),
                    },
                    crate::command::CommandInvocation::view(
                        crate::command::CommandRef {
                            view: view_ref.to_string(),
                            id: id.clone(),
                        },
                        command.clone(),
                    ),
                )?;
            }
        }
        let mut globals = config.commands.bindings.clone();
        if !globals.contains_key("commands") && config.view("selectors:commands").is_some() {
            globals.insert(
                "commands".to_string(),
                crate::config::CommandBinding::builtin_commands(),
            );
        }
        for (id, binding) in globals {
            let Some(command) = binding.as_command(&id) else {
                continue;
            };
            add(
                id.clone(),
                &binding,
                crate::command::CommandInvocation::session_command(view_ref, id, command),
            )?;
        }
        let view_invocations = config
            .iter_views()
            .flat_map(|(owner, view)| {
                view.commands.iter().map(move |(id, command)| {
                    (
                        (owner.clone(), id.clone()),
                        crate::command::CommandInvocation::view(
                            crate::command::CommandRef {
                                view: owner.clone(),
                                id: id.clone(),
                            },
                            command.clone(),
                        ),
                    )
                })
            })
            .collect();
        let has_unbound = config
            .view(view_ref)
            .is_some_and(|view| view.commands.values().any(|command| command.key.is_none()))
            || config
                .session_commands()
                .values()
                .any(|command| command.key.is_none());
        Ok(Self {
            bindings,
            overflow_binding,
            has_unbound,
            view_invocations,
            current_fields,
        })
    }

    pub(crate) fn overflow_command(&self) -> Option<(String, String)> {
        let b = self.overflow_binding.as_ref()?;
        Some((b.key.binding_name()?, b.label.as_ref()?.clone()))
    }

    pub(crate) fn has_unbound(&self) -> bool {
        self.has_unbound
    }

    pub(crate) fn is_palette_active(
        &self,
        width: usize,
        title: Option<&str>,
        status: Option<&str>,
    ) -> bool {
        if self.overflow_binding.is_none() {
            return false;
        }
        if self.has_unbound {
            return true;
        }
        let regular_commands = self
            .bindings
            .iter()
            .filter_map(|b| Some((b.key.binding_name()?, b.label.as_ref()?.clone())))
            .collect::<Vec<_>>();
        crate::chrome::is_palette_active(width, title, status, &regular_commands, self.has_unbound)
    }

    pub(crate) fn binding(&self, key: crate::view::Key) -> Option<&ProtocolCommandBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.key.binding_identity() == key.binding_identity())
            .or_else(|| {
                self.overflow_binding
                    .as_ref()
                    .filter(|binding| binding.key.binding_identity() == key.binding_identity())
            })
    }

    pub(crate) fn view_bindings(&self) -> crate::view::BindingSet {
        crate::view::BindingSet::new(self.bindings.iter().map(|binding| crate::view::Binding {
            key: binding.key,
            label: binding.label.clone(),
        }))
    }

    pub(crate) fn request(
        &self,
        binding: &ProtocolCommandBinding,
        owner: Option<crate::command::CommandOwnerContext>,
    ) -> crate::view::ViewDecision {
        self.request_for_invocation(binding.invocation.clone(), owner)
    }

    pub(crate) fn request_for_invocation(
        &self,
        invocation: crate::command::CommandInvocation,
        owner: Option<crate::command::CommandOwnerContext>,
    ) -> crate::view::ViewDecision {
        crate::view::ViewDecision::RequestCommand(crate::view::CommandRequest {
            invocation,
            owner,
            current_fields: self.current_fields,
        })
    }

    pub(crate) fn view_invocation(
        &self,
        owner: &str,
        id: &str,
    ) -> anyhow::Result<crate::command::CommandInvocation> {
        self.view_invocations
            .get(&(owner.to_string(), id.to_string()))
            .cloned()
            .with_context(|| format!("dynamic command {id:?} for View {owner:?} is unavailable"))
    }
}

pub(crate) trait CommandService {
    fn execute(
        &mut self,
        request: crate::view::CommandRequest,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<ViewDecision>;
}

pub(crate) struct ProtocolCommandService {
    config: crate::config::Config,
    cancellation: crate::lifecycle::CancellationToken,
}

impl ProtocolCommandService {
    pub(crate) fn new(
        config: &crate::config::Config,
        cancellation: crate::lifecycle::CancellationToken,
    ) -> Self {
        Self {
            config: config.clone(),
            cancellation,
        }
    }
}

impl CommandService for ProtocolCommandService {
    fn execute(
        &mut self,
        request: crate::view::CommandRequest,
        context: &ViewContext,
        snapshot: &ViewCommandSnapshot,
    ) -> Result<ViewDecision> {
        let page = command_page_owner(context, snapshot);
        let owner = request.owner.unwrap_or_else(|| page.clone());
        let execution = crate::command::CommandExecution {
            invocation: request.invocation,
            context: crate::command::CommandContext {
                page,
                owner,
                current: snapshot
                    .publication
                    .as_ref()
                    .map(|publication| publication.current.clone())
                    .unwrap_or(serde_json::Value::Null),
                current_fields: request.current_fields,
                runtime: snapshot.runtime.clone(),
            },
        };
        let prepared =
            crate::command::prepare_command_action(&self.config, execution, &self.cancellation)
                .map_err(crate::view::operation_failure)?;
        map_prepared_action(&self.config, &self.cancellation, prepared, context.instance)
            .map_err(crate::view::operation_failure)
    }
}

fn command_page_owner(
    context: &ViewContext,
    snapshot: &ViewCommandSnapshot,
) -> crate::command::CommandOwnerContext {
    let parameters = crate::parameter::ParameterSnapshot::from_parts(
        snapshot.parameters.clone(),
        snapshot.raw_input.clone(),
        crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(context.instance.0),
            generation: snapshot.revision,
        },
        snapshot.revision,
    );
    crate::command::CommandOwnerContext {
        view_ref: context.location.target.clone(),
        parameters,
        binding_raw: snapshot.raw_input.clone(),
    }
}

#[derive(Clone)]
struct ProtocolCallReturnHandler {
    config: crate::config::Config,
    cancellation: crate::lifecycle::CancellationToken,
    origin: crate::command::CommandOrigin,
    context: crate::command::CommandContext,
    then: Option<Box<crate::config::CommandAction>>,
}

impl CallReturnHandler for ProtocolCallReturnHandler {
    fn resume(
        &self,
        source: &ViewLocation,
        caller: &ViewContext,
        snapshot: &ViewCommandSnapshot,
        result: &ViewResult,
    ) -> Result<ViewDecision> {
        let Some(then) = &self.then else {
            return Ok(ViewDecision::Stay);
        };
        let output = serde_json::from_value(result.value.clone())
            .context("called View returned an invalid command output")?;
        let returned = crate::command::ViewReturn {
            source_view: source.target.clone(),
            output,
            adapter: result.adapter.clone(),
        };
        let mut context = self.context.clone();
        context.runtime = snapshot.runtime.clone();
        let action = crate::command::prepare_continuation(
            &self.config,
            then,
            self.origin.clone(),
            context,
            &crate::command::return_value(&returned),
            &self.cancellation,
        )?;
        map_prepared_action(&self.config, &self.cancellation, action, caller.instance)
    }
}

fn map_prepared_action(
    config: &crate::config::Config,
    cancellation: &crate::lifecycle::CancellationToken,
    action: crate::command::PreparedAction,
    caller: ViewInstanceId,
) -> anyhow::Result<crate::view::ViewDecision> {
    use crate::command::PreparedAction;
    use crate::view::{TransitionRequest, ViewDecision, ViewResult};
    match action {
        PreparedAction::Navigate { request, mode } => {
            let request = protocol_navigation_request(config, request)?;
            Ok(ViewDecision::Transition(match mode {
                crate::command::NavigationMode::Push => TransitionRequest::Push(request),
                crate::command::NavigationMode::Replace => TransitionRequest::Replace(request),
            }))
        }
        PreparedAction::Call(call) => {
            let request = protocol_navigation_request(config, call.request)?;
            let boundary = CallBoundary {
                caller,
                handler: std::sync::Arc::new(ProtocolCallReturnHandler {
                    config: config.clone(),
                    cancellation: cancellation.clone(),
                    origin: call.origin,
                    context: call.context,
                    then: call.then,
                }),
            };
            Ok(ViewDecision::Transition(TransitionRequest::Call {
                request,
                continuation: crate::view::Continuation::Call(boundary),
            }))
        }
        PreparedAction::Return(returned) => Ok(ViewDecision::Return(ViewResult {
            value: serde_json::to_value(returned.output)?,
            adapter: returned.adapter,
        })),
        PreparedAction::EditInput { value, cursor } => Ok(ViewDecision::Command(
            crate::view::CommandResult::EditInput { value, cursor },
        )),
        PreparedAction::Invoke(execution) => map_prepared_action(
            config,
            cancellation,
            crate::command::prepare_command_action(config, execution, cancellation)?,
            caller,
        ),
        PreparedAction::Execute { prepared, exit, .. } => {
            let effect = ViewDecision::Effect(crate::view::EffectRequest::RunPrepared {
                argv: prepared.argv,
                environment: prepared.environment,
                current_dir: prepared.current_dir,
            });
            Ok(if exit {
                ViewDecision::Batch(vec![effect, ViewDecision::Exit])
            } else {
                effect
            })
        }
    }
}

fn protocol_navigation_request(
    config: &crate::config::Config,
    request: crate::command::NavigationRequest,
) -> anyhow::Result<crate::view::NavigationRequest> {
    let mut state = config.instantiate_parameters(&request.view_ref)?;
    config.sanitize_initial_parameter_values(&mut state)?;

    let editable = if let Some(parameters) = request.parameters.as_ref() {
        if !parameters.is_null() {
            config.update_sanitized_initial_parameter_values(&mut state, parameters)?;
        }
        None
    } else if let Some(input) = request.input.as_ref() {
        let text = crate::terminal::sanitize_terminal_text(&input.params);
        config.update_initial_parameter_input(&mut state, &text)?;
        let cursor = if text == input.params {
            input.cursor
        } else {
            crate::terminal::sanitize_terminal_text(&input.params[..input.cursor]).len()
        };
        Some((text, cursor))
    } else {
        let text = crate::terminal::sanitize_terminal_text(&config.render_parameter_input(&state)?);
        Some((text.clone(), text.len()))
    };

    let values = config.parameter_values(&state)?;
    let query = crate::view::ParsedQuery::new(&request.view_ref, "query", values);
    let mut protocol_request = crate::view::NavigationRequest::new(&request.view_ref, query);
    if let Some((text, cursor)) = editable {
        protocol_request = protocol_request.with_input(text, cursor)?;
    }
    protocol_request.presentation = request.presentation;
    protocol_request.engine_options = request.engine_options;
    Ok(protocol_request)
}

fn command_request(decision: &ViewDecision) -> Option<&crate::view::CommandRequest> {
    match decision {
        ViewDecision::RequestCommand(request) => Some(request),
        ViewDecision::Batch(decisions) => decisions.iter().find_map(command_request),
        _ => None,
    }
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
    effects: Box<dyn EffectExecutor>,
    terminal: TerminalSize,
    theme: crate::theme::ResolvedTheme,
    runtime_log: Option<crate::diagnostics::RuntimeLog>,
    runtime_warning: Option<String>,
    active_error: Option<String>,
    error_source: Option<ErrorSource>,
    last_diagnostic: Option<(ViewInstanceId, String)>,
}

#[cfg(test)]
struct TestCommandService;

#[cfg(test)]
impl CommandService for TestCommandService {
    fn execute(
        &mut self,
        _: crate::view::CommandRequest,
        _: &ViewContext,
        _: &ViewCommandSnapshot,
    ) -> Result<ViewDecision> {
        anyhow::bail!("command service is unavailable in test session")
    }
}

impl ProtocolSession {
    #[cfg(test)]
    pub(crate) fn new(router: Router, effects: Box<dyn EffectExecutor>) -> Self {
        Self {
            router,
            commands: Box::new(TestCommandService),
            effects,
            terminal: TerminalSize::default(),
            theme: crate::theme::ResolvedTheme::terminal(),
            runtime_log: None,
            runtime_warning: None,
            active_error: None,
            error_source: None,
            last_diagnostic: None,
        }
    }

    pub(crate) fn configured(
        router: Router,
        commands: Box<dyn CommandService>,
        effects: Box<dyn EffectExecutor>,
        theme: crate::theme::ResolvedTheme,
        _default_view: String,
        mut runtime_log: crate::diagnostics::RuntimeLog,
    ) -> Self {
        let warning = runtime_log.take_warning_record();
        let error_source = warning.as_ref().map(|_| ErrorSource::StartupWarning);
        Self {
            router,
            commands,
            effects,
            terminal: TerminalSize::default(),
            theme,
            runtime_log: Some(runtime_log),
            runtime_warning: warning.as_ref().map(|record| record.message.clone()),
            active_error: warning.map(|record| record.label),
            error_source,
            last_diagnostic: None,
        }
    }

    pub(crate) fn router(&self) -> &Router {
        &self.router
    }

    #[cfg(test)]
    pub(crate) fn router_mut(&mut self) -> &mut Router {
        &mut self.router
    }

    pub(crate) fn take_popup_closed(&mut self) -> bool {
        self.router.take_popup_closed()
    }

    pub(crate) fn start_root(&mut self, request: NavigationRequest) -> Result<ViewInstanceId> {
        anyhow::ensure!(
            self.router.stack().is_empty(),
            "protocol session root has already been constructed"
        );
        let instance = self.router.push(request)?;
        self.resize_new_active_view(None)?;
        Ok(instance)
    }

    pub(crate) fn input(&mut self, event: InputEvent) -> Result<ViewDecision> {
        self.dispatch(ViewEvent::Input(event))
    }

    pub(crate) fn task(&mut self, event: TaskEvent) -> Result<ViewDecision> {
        self.dispatch(ViewEvent::Task(event))
    }

    pub(crate) fn tick(&mut self) -> Result<ViewDecision> {
        self.dispatch(ViewEvent::Tick)
    }

    pub(crate) fn resize(&mut self, terminal: TerminalSize) -> Result<ViewDecision> {
        self.terminal = terminal;
        self.dispatch_active_resize()
    }

    #[cfg(test)]
    pub(crate) fn eof(&mut self) -> Result<ViewDecision> {
        self.dispatch(ViewEvent::Input(InputEvent::Eof))
    }

    fn dispatch(&mut self, event: ViewEvent) -> Result<ViewDecision> {
        if matches!(event, ViewEvent::Input(_)) && self.error_source == Some(ErrorSource::Session) {
            self.active_error = None;
            self.error_source = None;
            self.last_diagnostic = None;
        }
        let active = self.router.active().map(|entry| entry.id);
        let decision = self
            .router
            .dispatch_with_effects(event, &mut *self.effects)?;
        if let Some(request) = command_request(&decision).cloned() {
            let source = active.context("command request has no source View")?;
            let entry = self
                .router
                .stack()
                .iter()
                .find(|entry| entry.id == source)
                .context("command source View is no longer mounted")?;
            let context = entry.context.clone();
            let snapshot = entry.view.command_snapshot();
            let next = match self.commands.execute(request, &context, &snapshot) {
                Ok(next) => next,
                Err(error) => {
                    self.router.record_error(Some(source), &error);
                    return Err(error);
                }
            };
            self.router
                .process_with_effects(next, source, &mut *self.effects)?;
        }
        self.resize_new_active_view(active)?;
        Ok(decision)
    }

    fn resize_new_active_view(&mut self, previous: Option<ViewInstanceId>) -> Result<()> {
        if self.terminal.width == 0 || self.terminal.height == 0 {
            return Ok(());
        }
        if self.router.active().map(|entry| entry.id) != previous {
            if previous.is_some() {
                self.active_error = None;
                self.error_source = None;
                self.last_diagnostic = None;
            }
            self.dispatch_active_resize()?;
        }
        Ok(())
    }

    fn dispatch_active_resize(&mut self) -> Result<ViewDecision> {
        let area = active_render_area(
            self.router.stack(),
            Rect::new(0, 0, self.terminal.width, self.terminal.height),
        );
        self.router.dispatch_with_effects(
            ViewEvent::Resize(TerminalSize {
                width: area.width,
                height: area.height,
            }),
            &mut *self.effects,
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
        let (chrome_instance, chrome_location, fallback_bindings) = {
            let entry = &self.router.stack()[active_index];
            (
                entry.id,
                entry.context.location.clone(),
                entry.view.bindings(&entry.context),
            )
        };
        let footer_location = self.router.stack()[active_index].context.location.clone();
        self.surface_diagnostic(
            chrome_instance,
            &chrome_location.target,
            chrome_snapshot.error.as_deref(),
        );
        let local_bindings = chrome_snapshot
            .bindings
            .clone()
            .unwrap_or(fallback_bindings);
        let bindings = merge_bindings(self.router.global_bindings(), &local_bindings);

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
            bindings,
            overflow_command: chrome_snapshot.overflow_command,
            has_unbound: chrome_snapshot.has_unbound,
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

    pub(crate) fn report_error(&mut self, message: &str) {
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

fn merge_bindings(
    global: &crate::view::BindingSet,
    local: &crate::view::BindingSet,
) -> crate::view::BindingSet {
    let mut seen = std::collections::HashSet::new();
    let entries = global
        .entries()
        .iter()
        .chain(local.entries().iter())
        .filter(|binding| seen.insert(binding.key.binding_identity()))
        .cloned();
    crate::view::BindingSet::new(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{
        Binding, BindingSet, EffectRequest, EffectResult, MapRouteCatalog, ParsedQuery, RouteCatalog, View,
        ViewContext, ViewFactory, ViewMetadata, ViewServices,
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Position};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::rc::Rc;

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
            if self.target == "zero_inset" {
                0
            } else {
                1
            }
        }

        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::new([Binding {
                key: crate::view::Key::Char('l'),
                label: Some("local".to_string()),
            }])
        }

        fn command_snapshot(&self) -> ViewCommandSnapshot {
            ViewCommandSnapshot {
                parameters: Value::Null,
                raw_input: String::new(),
                runtime: self.runtime.clone(),
                publication: self.publication.clone(),
                revision: self.revision,
            }
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            match event {
                ViewEvent::Lifecycle(_) => Ok(ViewDecision::Stay),
                ViewEvent::Input(input) => {
                    self.events.borrow_mut().push(input.clone());
                    match input {
                        InputEvent::Key {
                            key: crate::view::Key::Char('p'),
                            ..
                        } => Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                            "payload".to_string(),
                        ))),
                        InputEvent::Key {
                            key: crate::view::Key::Char('n'),
                            ..
                        } => {
                            let query = ParsedQuery::new("child", "query", Value::Null);
                            let mut request = NavigationRequest::new("child", query);
                            request.presentation.mode = crate::config::ViewPresentationMode::Popup;
                            request.presentation.width = Some(10);
                            request.presentation.height = Some(4);
                            Ok(ViewDecision::Transition(
                                crate::view::TransitionRequest::Push(request),
                            ))
                        }
                        InputEvent::Key {
                            key: crate::view::Key::Char('z'),
                            ..
                        } => Ok(ViewDecision::Command(
                            crate::view::CommandResult::EditInput {
                                value: "edited".to_string(),
                                cursor: 2,
                            },
                        )),
                        InputEvent::Key {
                            key: crate::view::Key::Char('q'),
                            ..
                        } => {
                            let target = if self.target == "root" {
                                "child"
                            } else {
                                "grandchild"
                            };
                            let query = ParsedQuery::new(target, "query", Value::Null);
                            let mut request = NavigationRequest::new(target, query);
                            request.presentation.mode = crate::config::ViewPresentationMode::Popup;
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
                            key: crate::view::Key::Char('e'),
                            ..
                        } => anyhow::bail!("protocol View failure"),
                        InputEvent::Key {
                            key: crate::view::Key::Char('r'),
                            ..
                        } => Ok(ViewDecision::Return(ViewResult {
                            value: Value::String(self.target.clone()),
                            adapter: None,
                        })),
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
                    title: None,
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
                key: crate::view::Key::Char('z'),
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
            map_prepared_action(
                &crate::config::load_test_fixture().unwrap(),
                &crate::lifecycle::CancellationToken::new(),
                crate::command::PreparedAction::EditInput {
                    value: "mapped".to_string(),
                    cursor: 3,
                },
                ViewInstanceId(1),
            )
            .unwrap(),
            ViewDecision::Command(crate::view::CommandResult::EditInput {
                value: "mapped".to_string(),
                cursor: 3,
            })
        );
    }

    #[test]
    fn footer_binding_merge_keeps_global_precedence_without_duplicates() {
        let global = BindingSet::new([
            Binding {
                key: crate::view::Key::Char('x'),
                label: Some("global".to_string()),
            },
            Binding {
                key: crate::view::Key::Char('g'),
                label: Some("other".to_string()),
            },
        ]);
        let local = BindingSet::new([
            Binding {
                key: crate::view::Key::Char('X'),
                label: Some("local".to_string()),
            },
            Binding {
                key: crate::view::Key::Char('l'),
                label: Some("local-only".to_string()),
            },
        ]);
        let merged = merge_bindings(&global, &local);
        assert_eq!(merged.entries().len(), 3);
        assert_eq!(merged.entries()[0].label.as_deref(), Some("global"));
        assert_eq!(merged.entries()[1].label.as_deref(), Some("other"));
        assert_eq!(merged.entries()[2].label.as_deref(), Some("local-only"));
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
    fn delivers_lossless_input_and_honors_global_precedence() {
        let (mut session, events, effects) = session();
        session.start_root(request("root")).unwrap();
        session
            .router_mut()
            .set_global_action(
                crate::view::Key::Char('g'),
                ViewDecision::Effect(EffectRequest::CopyToClipboard("global".to_string())),
            )
            .unwrap();
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('g'),
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
                key: crate::view::Key::Char('x'),
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
                key: crate::view::Key::Char('x'),
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
                key: crate::view::Key::Char('e'),
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
        let config = crate::config::load_test_fixture().unwrap();
        let cancellation = crate::lifecycle::CancellationToken::new();
        let adapter =
            ViewCommandBindings::new(&config, "dmenu:main", cancellation.observer(), &[]).unwrap();
        let mut binding = adapter
            .bindings
            .iter()
            .find(|binding| binding.invocation.id() == "accept")
            .cloned()
            .expect("fixture exposes the dmenu accept binding");
        binding.invocation.command.passthrough = true;
        let context = ViewContext::new(ViewInstanceId(40), "dmenu:main");
        let parameters = config.instantiate_parameters("dmenu:main").unwrap();
        let snapshot = ViewCommandSnapshot {
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
            revision: 0,
        };

        let ViewDecision::RequestCommand(request) = adapter.request(&binding, None) else {
            panic!("binding must produce a command request");
        };
        let mut service = ProtocolCommandService::new(&config, cancellation);
        assert!(matches!(
            service.execute(request, &context, &snapshot).unwrap(),
            ViewDecision::Return(_)
        ));
    }

    #[test]
    fn command_call_records_caller_and_runs_non_null_return_continuation() {
        let config = crate::config::load_test_fixture().unwrap();
        let cancellation = crate::lifecycle::CancellationToken::new();
        let adapter =
            ViewCommandBindings::new(&config, "dmenu:main", cancellation.observer(), &[]).unwrap();
        let binding = adapter
            .overflow_binding
            .as_ref()
            .or_else(|| {
                adapter
                    .bindings
                    .iter()
                    .find(|binding| binding.invocation.id() == "commands")
            })
            .expect("fixture exposes the built-in commands binding");
        let caller = ViewContext::new(ViewInstanceId(41), "dmenu:main");
        let parameters = config.instantiate_parameters("dmenu:main").unwrap();
        let snapshot = ViewCommandSnapshot {
            parameters: config.parameter_values(&parameters).unwrap(),
            raw_input: String::new(),
            runtime: serde_json::json!({"revision": 2}),
            publication: Some(crate::view::ViewPublication::new(
                serde_json::json!({
                    "item": {
                        "text": "first",
                        "value": null,
                        "metadata": {},
                        "owner_view": "dmenu:main"
                    },
                    "source": "dmenu:main",
                    "input": ""
                }),
                true,
            )),
            revision: 2,
        };

        let ViewDecision::RequestCommand(request) = adapter.request(binding, None) else {
            panic!("binding must produce a command request");
        };
        let mut service = ProtocolCommandService::new(&config, cancellation);
        let decision = service.execute(request, &caller, &snapshot).unwrap();
        let ViewDecision::Transition(crate::view::TransitionRequest::Call {
            continuation: crate::view::Continuation::Call(boundary),
            ..
        }) = decision
        else {
            panic!("commands binding must prepare a protocol Call");
        };
        assert_eq!(boundary.caller, caller.instance);

        let selected_command = crate::command::ViewOutput::Value {
            value: serde_json::json!({"view": "dmenu:main", "id": "accept"}),
        };
        let continued = boundary
            .handler
            .resume(
                &ViewLocation::new("selectors:commands"),
                &caller,
                &snapshot,
                &ViewResult {
                    value: serde_json::to_value(selected_command).unwrap(),
                    adapter: None,
                },
            )
            .unwrap();
        let ViewDecision::Return(result) = continued else {
            panic!("selected command must continue into its configured return");
        };
        let output: crate::command::ViewOutput = serde_json::from_value(result.value).unwrap();
        assert!(matches!(
            output,
            crate::command::ViewOutput::Selected {
                item: Some(item),
                input,
            } if item.text == "first" && input.is_empty()
        ));
    }

    #[test]
    fn root_popup_uses_the_same_content_host_geometry_as_nested_popups() {
        let (mut session, _, _) = session();
        let mut root = request("root");
        root.presentation.mode = crate::config::ViewPresentationMode::Popup;
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
                    key: crate::view::Key::Char('q'),
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
            crate::config::ViewPresentationMode::Inline
        );
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('p'),
                raw: vec![b'p'],
            })
            .unwrap();
        assert_eq!(effects.borrow().len(), 1);
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('n'),
                raw: vec![b'n'],
            })
            .unwrap();
        assert_eq!(session.router().stack().len(), 2);
        let child = session.router().active().unwrap().id;
        session
            .task(TaskEvent {
                instance: child,
                task: crate::view::TaskId(1),
                generation: 1,
                outcome: crate::view::TaskOutcome::Completed(Value::Null),
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
            crate::config::ViewPresentationMode::Popup
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
        assert!(bottom_border.contains("local"));

        // Global footer row (y = 9) is blank while popup is active
        for x in 0..40 {
            assert_eq!(
                terminal.backend().buffer().cell((x, 9)).unwrap().symbol(),
                " "
            );
        }

        assert_eq!(rendered.footer.location.label(), "child");
        assert_eq!(rendered.footer.status.as_deref(), Some("child"));
        assert_eq!(
            rendered.footer.bindings.entries()[0].label.as_deref(),
            Some("local")
        );
        session.eof().unwrap();
        assert!(session.router().stack().is_empty());
    }

    #[test]
    fn view_diagnostic_clears_when_resolved_and_session_error_clears_on_input() {
        let view_error = Rc::new(RefCell::new(None));
        struct DiagView(Rc<RefCell<Option<String>>>);
        impl View for DiagView {
            fn bindings(&self, _: &ViewContext) -> BindingSet {
                BindingSet::default()
            }
            fn command_snapshot(&self) -> ViewCommandSnapshot {
                ViewCommandSnapshot {
                    parameters: Value::Null,
                    raw_input: String::new(),
                    runtime: Value::Null,
                    publication: None,
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
                        title: None,
                        status: None,
                        error: None,
                        bindings: None,
                    },
                })
            }
            fn chrome(&self, context: &ViewContext) -> Result<crate::view::ViewChrome> {
                Ok(crate::view::ViewChrome {
                    title: None,
                    status: None,
                    error: self.0.borrow().clone(),
                    bindings: Some(self.bindings(context)),
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
        let router = Router::new(Box::new(routes), Box::new(DiagFactory(Rc::clone(&view_error))));
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
                        res = Some(session.render(frame, frame.area(), None).unwrap().footer.error);
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
                key: crate::view::Key::Char('a'),
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
        assert!(footer_row.contains("local"));

        // 2. Open child popup ('n' key)
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('n'),
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
        assert!(child_bottom.contains("local"));

        // 3. Child returns to root ('r' key)
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('r'),
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
        assert!(restored_footer.contains("local"));
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
    fn session_take_popup_closed_tracks_popup_lifecycle() {
        let (mut session, _events, _effects) = session();
        session.start_root(request("root")).unwrap();
        assert!(!session.take_popup_closed());

        // Open child popup ('n' key)
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('n'),
                raw: vec![b'n'],
            })
            .unwrap();
        assert!(!session.take_popup_closed());

        // Child returns to root ('r' key)
        session
            .input(InputEvent::Key {
                key: crate::view::Key::Char('r'),
                raw: vec![b'r'],
            })
            .unwrap();
        assert!(session.take_popup_closed());
        assert!(!session.take_popup_closed());
    }
}

