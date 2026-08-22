use super::{
    CallRequest, CommandContext, CommandExecution, CommandInvocation, CommandOrigin,
    CommandSession, EngineHost, EngineRegistry, InputFocus, InputRefreshPolicy, InputSeed,
    NavigationMode, NavigationRequest, PassthroughEvent, TaskScheduler, ViewContext, ViewEffect,
    ViewInputMode, ViewInstance, ViewReturn,
};
use crate::cancellation::CancellationToken;
use crate::chrome::InputBuffer;
use crate::config::{
    CommandBindingVisibility, CommandRequirement, CommandScope, Config, EvaluationSnapshot,
    InvocationScope, OwnerViewScope, SessionScope,
};
use crate::engine::api::{
    EditorAction, InputEdit, LauncherOutcome, ResolvedInputAction, SelectionBindingState,
};
use crate::engine::keymap::{
    BindingEntry, BindingRecord, BindingState, InputContextId, InputRouter, LayerId,
};
use crate::input::{DecodedInput, Key};
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::state::StateInstance;
use crate::terminal::{InputRead, Terminal};
use crate::text::sanitize_terminal_text;
use crate::theme::ResolvedTheme;
use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

struct CallBoundary {
    origin: CommandOrigin,
    context: CommandContext,
    then: Option<Box<crate::config::CommandAction>>,
    suppresses_session_bindings: bool,
}

#[derive(Debug, Clone, Copy)]
struct ViewInputLayers {
    context: InputContextId,
    actions: LayerId,
    view_commands: LayerId,
    selection_commands: LayerId,
    session_commands: LayerId,
}

struct ViewEntry {
    view_ref: String,
    input: InputBuffer,
    input_dirty: bool,
    input_deadline: Option<Instant>,
    state: StateInstance,
    input_layers: ViewInputLayers,
    published_bindings: Option<PublishedBindings>,
    call_boundary: Option<CallBoundary>,
    instance: Box<dyn ViewInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishedBindings {
    input_mode: ViewInputMode,
    actions: Vec<crate::engine::api::InputActionBinding>,
    selection: SelectionBindingState,
    session_suppressed: bool,
}

#[derive(Debug, Clone)]
struct RouteCompletion {
    input_context: InputContextId,
    candidates: Vec<crate::router::ViewCandidate>,
    selected: usize,
    selector_end: usize,
}

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    theme: ResolvedTheme,
    engines: EngineRegistry,
    views: Vec<ViewEntry>,
    tasks: TaskScheduler,
    runtime: super::RuntimeStore,
    runtime_log: RuntimeLog,
    router: Arc<crate::router::Router>,
    route_input: bool,
    route_completion: Option<RouteCompletion>,
    command_session: CommandSession,
    input_router: InputRouter<RegisteredBinding>,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
    runtime_warning: Option<String>,
    cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(Box<ViewReturn>),
}

enum ReturnTransition {
    Effect(Box<ViewEffect>),
    Outcome(SessionOutcome),
}

enum InputBinding {
    ViewCommand(Key),
    PendingViewCommand(Key),
    SessionCommand(Key),
    Action(ResolvedInputAction),
    Route(RouteAction),
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteAction {
    Next,
    Previous,
    Accept,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingTarget {
    Action(ResolvedInputAction),
    ViewCommand(Key),
    SessionCommand(Key),
    Route(RouteAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingHint {
    label: String,
    visibility: CommandBindingVisibility,
}

type ChromeHint = (String, String);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredBinding {
    target: BindingTarget,
    hint: Option<BindingHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputGrammar {
    Decoded,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputFallback {
    View,
    ForwardRaw,
    PopAndRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputContext {
    id: InputContextId,
    grammar: InputGrammar,
    fallback: InputFallback,
}

const ACTION_PRIORITY: i16 = 100;
const VIEW_COMMAND_PRIORITY: i16 = 200;
const SELECTION_COMMAND_PRIORITY: i16 = 210;
const SESSION_COMMAND_PRIORITY: i16 = 300;

fn mount_view_input_layers(router: &mut InputRouter<RegisteredBinding>) -> ViewInputLayers {
    let context = router.create_context();
    ViewInputLayers {
        context,
        actions: router.mount_layer(context, ACTION_PRIORITY),
        view_commands: router.mount_layer(context, VIEW_COMMAND_PRIORITY),
        selection_commands: router.mount_layer(context, SELECTION_COMMAND_PRIORITY),
        session_commands: router.mount_layer(context, SESSION_COMMAND_PRIORITY),
    }
}

fn prepared_action_effect(action: crate::engine::command::PreparedAction) -> ViewEffect {
    match action {
        crate::engine::command::PreparedAction::Navigate { request, mode } => {
            ViewEffect::Navigate {
                request,
                mode,
                parent_edit: None,
            }
        }
        crate::engine::command::PreparedAction::Call(call) => ViewEffect::Call(call),
        crate::engine::command::PreparedAction::Return(returned) => ViewEffect::Return(returned),
        crate::engine::command::PreparedAction::EditInput { value, cursor } => {
            ViewEffect::EditInput(InputEdit::SetBuffer { raw: value, cursor })
        }
        crate::engine::command::PreparedAction::Invoke(execution) => {
            ViewEffect::DispatchCommand(execution)
        }
        crate::engine::command::PreparedAction::Execute {
            invocation,
            prepared,
            exit,
        } => ViewEffect::RunCommand {
            invocation,
            prepared,
            exit,
        },
    }
}

impl<'a> AppSession<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
    ) -> Result<Self> {
        let cancellation = CancellationToken::new();
        Self::new_with_theme(
            config,
            ResolvedTheme::terminal(),
            runtime_log,
            engines,
            &cancellation,
        )
    }

    pub(crate) fn new_with_theme(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        let mut runtime = super::RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let router = Arc::new(crate::router::Router::new(config));
        let view_ref = config
            .default_view
            .clone()
            .context("no default_view configured for the session")?;
        let state = if config.invocation_state.view_ref() == view_ref {
            config.invocation_state.clone()
        } else {
            config.instantiate_state(&view_ref)?
        };
        let initial_input = sanitize_terminal_text(&config.render_query_input(&state)?);
        let request = NavigationRequest::new(&view_ref, initial_input);
        let input = input_buffer_from_seed(
            request
                .input
                .as_ref()
                .context("root navigation request has no input seed")?,
        );
        publish_location(&mut runtime, config, &view_ref, &input, &state)?;
        publish_view_catalog(&mut runtime, config)?;
        let runtime_snapshot = runtime.snapshot().clone();
        let evaluation = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime_snapshot),
            Some(OwnerViewScope::new(&state)),
            Some(cancellation),
        );
        let root_context = ViewContext {
            config,
            request: &request,
            input: &input,
            state: &state,
            evaluation,
            tasks: tasks.clone(),
            cancellation: cancellation.clone(),
        };
        let root = engines.create_view(root_context)?;
        let mut input_router = InputRouter::default();
        let input_layers = mount_view_input_layers(&mut input_router);
        Ok(Self {
            config,
            theme,
            engines,
            views: vec![ViewEntry {
                view_ref,
                input,
                input_dirty: false,
                input_deadline: None,
                state,
                input_layers,
                published_bindings: None,
                call_boundary: None,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: true,
            route_completion: None,
            command_session: CommandSession::from_config(config),
            input_router,
            active_error: None,
            active_error_deadline: None,
            runtime_warning: None,
            cancellation: cancellation.clone(),
        })
    }

    pub(crate) fn single_root_with_theme(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        let mut runtime = super::RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let router = Arc::new(crate::router::Router::new(config));
        let state = config.invocation_state.clone();
        let input = sanitize_terminal_text(&config.render_query_input(&state)?);
        let request = NavigationRequest::new(view_ref, input);
        let input = input_buffer_from_seed(
            request
                .input
                .as_ref()
                .context("explicit view request has no input seed")?,
        );
        publish_location(&mut runtime, config, view_ref, &input, &state)?;
        publish_view_catalog(&mut runtime, config)?;
        let runtime_snapshot = runtime.snapshot().clone();
        let evaluation = EvaluationSnapshot::new(
            InvocationScope::new(&config.input_value),
            SessionScope::new(&runtime_snapshot),
            Some(OwnerViewScope::new(&state)),
            Some(cancellation),
        );
        let root = engines.create_view(ViewContext {
            config,
            request: &request,
            input: &input,
            state: &state,
            evaluation,
            tasks: tasks.clone(),
            cancellation: cancellation.clone(),
        })?;
        let mut input_router = InputRouter::default();
        let input_layers = mount_view_input_layers(&mut input_router);
        Ok(Self {
            config,
            theme,
            engines,
            views: vec![ViewEntry {
                view_ref: view_ref.to_string(),
                input,
                input_dirty: false,
                input_deadline: None,
                state,
                input_layers,
                published_bindings: None,
                call_boundary: None,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: false,
            route_completion: None,
            command_session: CommandSession::from_config(config),
            input_router,
            active_error: None,
            active_error_deadline: None,
            runtime_warning: None,
            cancellation: cancellation.clone(),
        })
    }

    pub(crate) fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        loop {
            if self.cancellation.is_cancelled() {
                self.tasks.cancel_all();
                return Ok(SessionOutcome::Exited);
            }
            self.clear_expired_error();
            let effect = self.step(terminal)?;
            let outcome = self.process_effect(effect, terminal)?;
            self.surface_runtime_log_warning();
            if self.cancellation.is_cancelled() {
                self.tasks.cancel_all();
                return Ok(SessionOutcome::Exited);
            }
            if let Some(outcome) = outcome {
                return Ok(outcome);
            }
            self.render(terminal)?;
            self.runtime_warning = None;
        }
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.runtime_warning.take()
    }

    fn current_view_input_context(&self) -> Result<InputContext> {
        let entry = self.views.last().context("session has no active view")?;
        Ok(match entry.instance.input_mode() {
            ViewInputMode::Keymap => InputContext {
                id: entry.input_layers.context,
                grammar: InputGrammar::Decoded,
                fallback: InputFallback::View,
            },
            ViewInputMode::Passthrough => InputContext {
                id: entry.input_layers.context,
                grammar: InputGrammar::Raw,
                fallback: InputFallback::ForwardRaw,
            },
        })
    }

    fn active_input_context(&self) -> Result<InputContext> {
        if let Some(completion) = &self.route_completion {
            return Ok(InputContext {
                id: completion.input_context,
                grammar: InputGrammar::Decoded,
                fallback: InputFallback::PopAndRetry,
            });
        }
        self.current_view_input_context()
    }

    fn session_bindings_suppressed(&self) -> bool {
        self.views.iter().any(|entry| {
            entry
                .call_boundary
                .as_ref()
                .is_some_and(|boundary| boundary.suppresses_session_bindings)
        })
    }

    fn route_input_available(&self) -> bool {
        self.route_input
            && self.views.len() == 1
            && self.views.last().is_some_and(|entry| {
                self.config.default_view.as_deref() == Some(entry.view_ref.as_str())
                    && entry.instance.input_focus() == InputFocus::Focused
            })
    }

    fn open_route_completion(&mut self) -> Result<()> {
        let entry = self.views.last().context("session has no active view")?;
        let input = &entry.input;
        let selector_end = input
            .raw
            .find(char::is_whitespace)
            .unwrap_or(input.raw.len());
        let query_end = input.cursor.min(selector_end);
        let candidates = self
            .router
            .complete_views(&input.raw[..query_end], &entry.view_ref);
        let input_context = self.input_router.create_context();
        let input_layer = self
            .input_router
            .mount_layer(input_context, ACTION_PRIORITY);
        let route_entries = [
            (Key::Tab, RouteAction::Next, None),
            (Key::Down, RouteAction::Next, None),
            (Key::BackTab, RouteAction::Previous, None),
            (Key::Up, RouteAction::Previous, None),
            (Key::Enter, RouteAction::Accept, Some("Open")),
            (Key::Escape, RouteAction::Close, Some("Close")),
        ]
        .into_iter()
        .map(|(key, action, label)| {
            (
                key,
                BindingEntry::Bind(BindingRecord {
                    target: RegisteredBinding {
                        target: BindingTarget::Route(action),
                        hint: label.map(|label| BindingHint {
                            label: label.to_string(),
                            visibility: CommandBindingVisibility::Always,
                        }),
                    },
                    state: BindingState::Ready,
                }),
            )
        })
        .collect();
        if let Err(error) = self
            .input_router
            .replace_layers(vec![(input_layer, route_entries)])
        {
            self.input_router.remove_context(input_context);
            return Err(error);
        }
        self.route_completion = Some(RouteCompletion {
            input_context,
            candidates,
            selected: 0,
            selector_end,
        });
        Ok(())
    }

    fn close_route_completion(&mut self) -> Option<RouteCompletion> {
        let completion = self.route_completion.take()?;
        self.input_router.remove_context(completion.input_context);
        Some(completion)
    }

    fn handle_route_action(&mut self, action: RouteAction) -> Result<()> {
        match action {
            RouteAction::Next => {
                let completion = self
                    .route_completion
                    .as_mut()
                    .context("route completion is not active")?;
                if !completion.candidates.is_empty() {
                    completion.selected = (completion.selected + 1) % completion.candidates.len();
                }
            }
            RouteAction::Previous => {
                let completion = self
                    .route_completion
                    .as_mut()
                    .context("route completion is not active")?;
                if !completion.candidates.is_empty() {
                    completion.selected = completion
                        .selected
                        .checked_sub(1)
                        .unwrap_or(completion.candidates.len() - 1);
                }
            }
            RouteAction::Accept => {
                let completion = self
                    .close_route_completion()
                    .context("route completion is not active")?;
                if let Some(edit) = route_completion_edit(
                    &self
                        .views
                        .last()
                        .context("session has no active view")?
                        .input,
                    &completion,
                ) {
                    self.apply_input_edit(edit)?;
                }
            }
            RouteAction::Close => {
                self.close_route_completion();
            }
        }
        Ok(())
    }

    fn step(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let effect = self.dispatch_input_ready()?;
        if !matches!(effect, ViewEffect::Continue) {
            return Ok(effect);
        }

        let effect = {
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
            entry.instance.step(&mut host, terminal)?
        };
        if matches!(effect, ViewEffect::Continue) {
            let background_effect = self.poll_background_views(terminal)?;
            if !matches!(background_effect, ViewEffect::Continue) {
                return Ok(background_effect);
            }
        }
        if !matches!(effect, ViewEffect::Continue) {
            return Ok(effect);
        }

        self.refresh_active_bindings()?;

        let timeout = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            let input_timeout = entry.input_deadline.map(input_timeout_until);
            let host = EngineHost {
                config: self.config,
                theme: self.theme,
                input: &mut entry.input,
                state: &mut entry.state,
                runtime: &mut self.runtime,
                runtime_log: &mut self.runtime_log,
                active_error: &mut self.active_error,
                active_error_deadline: &mut self.active_error_deadline,
            };
            match (entry.instance.launcher_input_timeout(&host), input_timeout) {
                (Some(engine), Some(input)) => Some(engine.min(input)),
                (engine, input) => engine.or(input),
            }
        };
        let input_context = self.active_input_context()?;
        let snapshot = self.input_router.snapshot(input_context.id);
        if input_context.grammar == InputGrammar::Raw {
            let revision = snapshot.revision();
            let intercepted_keys = snapshot.bindings().map(|(key, _)| key).collect::<Vec<_>>();
            if !self.command_session.passthrough_active() {
                let pending = self.command_session.take_all_pending_raw();
                self.command_session
                    .enter_passthrough(intercepted_keys.clone());
                self.command_session.feed_passthrough(&pending);
            }
            self.command_session
                .sync_passthrough_keys(revision, intercepted_keys);
            return self.step_passthrough(terminal, timeout.unwrap_or(40));
        }
        if self.command_session.passthrough_active() {
            let pending = self.command_session.leave_passthrough();
            self.command_session.feed_pending_raw_to_normal(&pending);
        }
        if let Some(timeout) = timeout {
            if self.command_session.pending_is_empty() {
                match self.command_session.read_normal(terminal, timeout)? {
                    InputRead::Eof => return Ok(ViewEffect::Exit),
                    InputRead::Data(_) | InputRead::Timeout => {}
                }
            }
            while let Some(queued) = self.command_session.pop_input() {
                self.refresh_active_bindings()?;
                let input_context = self.active_input_context()?;
                let Some(key) = queued.key else {
                    if input_context.fallback == InputFallback::PopAndRetry {
                        self.close_route_completion();
                        self.refresh_active_bindings()?;
                        self.command_session.push_front(queued);
                    }
                    continue;
                };
                let captures_editor_input = self
                    .views
                    .last()
                    .context("session has no active view")?
                    .instance
                    .captures_editor_input();
                let binding = self.resolve_key_binding(key);
                if binding.is_none() && input_context.fallback == InputFallback::PopAndRetry {
                    self.close_route_completion();
                    self.refresh_active_bindings()?;
                    self.command_session.push_front(queued);
                    continue;
                }
                if binding.is_none()
                    && input_context.fallback == InputFallback::View
                    && key == Key::Tab
                    && self.route_input_available()
                {
                    self.open_route_completion()?;
                    self.refresh_active_bindings()?;
                    continue;
                }
                let outcome = match binding {
                    Some(InputBinding::ViewCommand(binding_key)) => self
                        .dispatch_registered_view_binding(
                            binding_key,
                            queued.clone(),
                            false,
                            true,
                        )?,
                    Some(InputBinding::PendingViewCommand(binding_key)) => self
                        .dispatch_registered_view_binding(
                            binding_key,
                            queued.clone(),
                            true,
                            true,
                        )?,
                    Some(InputBinding::SessionCommand(binding_key)) => {
                        self.dispatch_session_binding(binding_key, queued.clone(), true)?
                    }
                    Some(InputBinding::Action(ResolvedInputAction::Edit(action))) => {
                        self.dispatch_editor_action(action)?
                    }
                    Some(InputBinding::Action(ResolvedInputAction::View(action))) => {
                        self.dispatch_view_action(action, queued.clone(), true)?
                    }
                    Some(InputBinding::Route(action)) => {
                        self.handle_route_action(action)?;
                        if matches!(action, RouteAction::Accept)
                            && let Some(effect) = self.reconcile_input()?
                        {
                            LauncherOutcome::Effect(Box::new(effect))
                        } else {
                            LauncherOutcome::Continue
                        }
                    }
                    Some(InputBinding::Disabled) => LauncherOutcome::Continue,
                    None => {
                        if !captures_editor_input
                            && let Some(changed) = {
                                let entry = self
                                    .views
                                    .last_mut()
                                    .context("session has no active view")?;
                                apply_editor_key(&mut entry.input, key)
                            }
                            && changed
                        {
                            self.mark_input_changed()?;
                        }
                        LauncherOutcome::Continue
                    }
                };
                match outcome {
                    LauncherOutcome::Continue => {}
                    LauncherOutcome::Effect(effect) => return Ok(*effect),
                }
            }
        }

        Ok(self.reconcile_input()?.unwrap_or(ViewEffect::Continue))
    }

    fn poll_background_views(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let count = self.views.len().saturating_sub(1);
        for index in 0..count {
            let entry = &mut self.views[index];
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
            let effect = entry.instance.background_step(&mut host, terminal)?;
            if !matches!(effect, ViewEffect::Continue) {
                return Ok(effect);
            }
        }
        Ok(ViewEffect::Continue)
    }

    fn refresh_active_bindings(&mut self) -> Result<()> {
        let session_suppressed = self.session_bindings_suppressed();
        let (view_ref, layers, published) = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            let host = EngineHost {
                config: self.config,
                theme: self.theme,
                input: &mut entry.input,
                state: &mut entry.state,
                runtime: &mut self.runtime,
                runtime_log: &mut self.runtime_log,
                active_error: &mut self.active_error,
                active_error_deadline: &mut self.active_error_deadline,
            };
            let published = PublishedBindings {
                input_mode: entry.instance.input_mode(),
                actions: entry.instance.input_action_bindings(&host),
                selection: entry.instance.selection_binding_state(&host),
                session_suppressed,
            };
            (entry.view_ref.clone(), entry.input_layers, published)
        };
        if self
            .views
            .last()
            .is_some_and(|entry| entry.published_bindings.as_ref() == Some(&published))
        {
            return Ok(());
        }
        let PublishedBindings {
            input_mode,
            actions,
            selection: selection_state,
            session_suppressed,
        } = &published;

        let action_entries = actions
            .iter()
            .filter(|action| action.mode == *input_mode)
            .map(|action| {
                (
                    action.key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            target: BindingTarget::Action(action.action),
                            hint: action.label.clone().map(|label| BindingHint {
                                label,
                                visibility: CommandBindingVisibility::Always,
                            }),
                        },
                        state: if action.enabled {
                            BindingState::Ready
                        } else {
                            BindingState::Disabled
                        },
                    }),
                )
            })
            .collect::<Vec<_>>();

        let mut view_entries = Vec::new();
        if let Some(view) = self.config.view(&view_ref) {
            let page_pending = matches!(selection_state, SelectionBindingState::Pending(_));
            for command in view
                .commands
                .values()
                .filter(|command| *input_mode == ViewInputMode::Keymap || command.passthrough)
            {
                let key = Key::parse_binding(&crate::config::normalize_key(&command.key)?)?;
                view_entries.push((
                    key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            target: BindingTarget::ViewCommand(key),
                            hint: Some(BindingHint {
                                label: command.label.clone(),
                                visibility: CommandBindingVisibility::Always,
                            }),
                        },
                        state: if page_pending
                            && command.requires == CommandRequirement::Items
                            && !command.passthrough
                        {
                            BindingState::Pending
                        } else {
                            BindingState::Ready
                        },
                    }),
                ));
            }
        }

        let owner_states = match selection_state {
            SelectionBindingState::None | SelectionBindingState::Ready(None) => Vec::new(),
            SelectionBindingState::Ready(Some(owner)) => {
                vec![(owner.as_str(), BindingState::Ready)]
            }
            SelectionBindingState::Pending(owners) => owners
                .iter()
                .map(|owner| (owner.as_str(), BindingState::Pending))
                .collect(),
        };
        let mut selection_entries = BTreeMap::new();
        for (owner, state) in owner_states {
            let Some(view) = self.config.view(owner) else {
                continue;
            };
            for command in view
                .commands
                .values()
                .filter(|command| command.scope == CommandScope::Selection)
            {
                if *input_mode == ViewInputMode::Passthrough && !command.passthrough {
                    continue;
                }
                let key_name = crate::config::normalize_key(&command.key)?;
                let key = Key::parse_binding(&key_name)?;
                selection_entries.entry(key_name).or_insert_with(|| {
                    (
                        key,
                        BindingEntry::Bind(BindingRecord {
                            target: RegisteredBinding {
                                target: BindingTarget::ViewCommand(key),
                                hint: (state == BindingState::Ready).then(|| BindingHint {
                                    label: command.label.clone(),
                                    visibility: CommandBindingVisibility::Always,
                                }),
                            },
                            state,
                        }),
                    )
                });
            }
        }

        let mut session_entries = Vec::new();
        if !session_suppressed {
            for (id, binding) in self.command_session.session_commands() {
                let Some(key) = binding
                    .key(id)
                    .and_then(|key| crate::config::normalize_key(key).ok())
                    .and_then(|key| Key::parse_binding(&key).ok())
                else {
                    continue;
                };
                let (Some(label), Some(visibility)) = (binding.label(id), binding.visibility(id))
                else {
                    continue;
                };
                if *input_mode == ViewInputMode::Passthrough && id != "commands" {
                    continue;
                }
                session_entries.push((
                    key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            target: BindingTarget::SessionCommand(key),
                            hint: Some(BindingHint {
                                label: label.to_string(),
                                visibility,
                            }),
                        },
                        state: BindingState::Ready,
                    }),
                ));
            }
        }

        self.input_router.replace_layers(vec![
            (layers.actions, action_entries),
            (layers.view_commands, view_entries),
            (
                layers.selection_commands,
                selection_entries.into_values().collect(),
            ),
            (layers.session_commands, session_entries),
        ])?;
        self.views
            .last_mut()
            .context("session has no active view")?
            .published_bindings = Some(published);
        Ok(())
    }

    fn resolve_key_binding(&self, key: Key) -> Option<InputBinding> {
        let context = self.active_input_context().ok()?;
        let record = self.input_router.resolve(context.id, key)?;
        if record.state == BindingState::Disabled {
            return Some(InputBinding::Disabled);
        }
        Some(match record.target.target {
            BindingTarget::Action(action) => InputBinding::Action(action),
            BindingTarget::ViewCommand(key) if record.state == BindingState::Pending => {
                InputBinding::PendingViewCommand(key)
            }
            BindingTarget::ViewCommand(key) => InputBinding::ViewCommand(key),
            BindingTarget::SessionCommand(key) => InputBinding::SessionCommand(key),
            BindingTarget::Route(action) => InputBinding::Route(action),
        })
    }

    fn dispatch_editor_action(&mut self, action: EditorAction) -> Result<LauncherOutcome> {
        let edited = match action {
            EditorAction::DeleteBackward => self.delete_backward()?,
            EditorAction::ClearInput | EditorAction::DeleteWord => {
                let entry = self
                    .views
                    .last_mut()
                    .context("session has no active view")?;
                match action {
                    EditorAction::ClearInput => entry.input.clear(),
                    EditorAction::DeleteWord => entry.input.delete_word(),
                    EditorAction::DeleteBackward => unreachable!(),
                }
            }
        };
        if edited {
            self.mark_input_changed()?;
        }
        Ok(LauncherOutcome::Continue)
    }

    fn dispatch_session_binding(
        &mut self,
        key: Key,
        input: DecodedInput,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile && let Some(effect) = self.reconcile_input()? {
            self.command_session.push_front(input);
            return Ok(LauncherOutcome::Effect(Box::new(effect)));
        }
        let view_ref = self
            .views
            .last()
            .context("session has no active view")?
            .view_ref
            .clone();
        let Some(invocation) = self.command_session.session_command_for_key(&view_ref, key) else {
            return Ok(LauncherOutcome::Continue);
        };
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
        let execution = CommandExecution {
            invocation,
            context: entry.instance.view_command_context(&mut host)?,
        };
        Ok(LauncherOutcome::Effect(Box::new(
            ViewEffect::DispatchCommand(execution),
        )))
    }

    fn dispatch_registered_view_binding(
        &mut self,
        key: Key,
        input: DecodedInput,
        pending: bool,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile && let Some(effect) = self.reconcile_input()? {
            self.command_session.push_front(input);
            return Ok(LauncherOutcome::Effect(Box::new(effect)));
        }
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
        if pending {
            entry
                .instance
                .handle_pending_view_binding(&mut host, key, input)
        } else {
            entry.instance.handle_view_binding(&mut host, key, input)
        }
    }

    fn dispatch_view_action(
        &mut self,
        action: crate::engine::api::ViewAction,
        input: DecodedInput,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile && let Some(effect) = self.reconcile_input()? {
            self.command_session.push_front(input);
            return Ok(LauncherOutcome::Effect(Box::new(effect)));
        }
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
        entry.instance.handle_view_action(&mut host, action, input)
    }

    fn step_passthrough(&mut self, terminal: &mut Terminal, timeout: i32) -> Result<ViewEffect> {
        loop {
            if let Some(event) = self.command_session.next_passthrough_event() {
                if let LauncherOutcome::Effect(effect) = self.dispatch_passthrough_event(event)? {
                    return Ok(*effect);
                }
                continue;
            }

            match self.command_session.read_passthrough(terminal, timeout)? {
                InputRead::Data(bytes) => {
                    if bytes.is_empty() {
                        return Ok(ViewEffect::Continue);
                    }
                    continue;
                }
                InputRead::Eof => {
                    if let LauncherOutcome::Effect(effect) = self.handle_terminal_eof()? {
                        return Ok(*effect);
                    }
                    return Ok(ViewEffect::Continue);
                }
                InputRead::Timeout => {
                    if let Some(event) = self.command_session.flush_passthrough_due()
                        && let LauncherOutcome::Effect(effect) =
                            self.dispatch_passthrough_event(event)?
                    {
                        return Ok(*effect);
                    }
                    return Ok(ViewEffect::Continue);
                }
            }
        }
    }

    fn dispatch_passthrough_event(&mut self, event: PassthroughEvent) -> Result<LauncherOutcome> {
        match event {
            PassthroughEvent::Forward(bytes) => self.dispatch_unbound_input(&bytes),
            PassthroughEvent::Switch(input) => self.dispatch_passthrough_switch(input),
        }
    }

    fn dispatch_passthrough_switch(&mut self, input: DecodedInput) -> Result<LauncherOutcome> {
        let key = input.key.context("passthrough switch has no key")?;
        let context = self.active_input_context()?;
        let Some(record) = self.input_router.resolve(context.id, key).cloned() else {
            return Ok(LauncherOutcome::Continue);
        };
        if record.state == BindingState::Disabled {
            return Ok(LauncherOutcome::Continue);
        }

        match record.target.target {
            BindingTarget::SessionCommand(binding_key) => {
                self.dispatch_session_binding(binding_key, input, false)
            }
            BindingTarget::ViewCommand(binding_key) => self.dispatch_registered_view_binding(
                binding_key,
                input,
                record.state == BindingState::Pending,
                false,
            ),
            BindingTarget::Action(ResolvedInputAction::View(action)) => {
                self.dispatch_view_action(action, input, false)
            }
            BindingTarget::Action(ResolvedInputAction::Edit(action)) => {
                self.dispatch_editor_action(action)
            }
            BindingTarget::Route(_) => Ok(LauncherOutcome::Continue),
        }
    }

    fn handle_terminal_eof(&mut self) -> Result<LauncherOutcome> {
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
        entry.instance.handle_terminal_eof(&mut host)
    }

    fn dispatch_unbound_input(&mut self, bytes: &[u8]) -> Result<LauncherOutcome> {
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
        entry.instance.handle_unbound_input(&mut host, bytes)
    }

    fn process_effect(
        &mut self,
        mut effect: ViewEffect,
        terminal: &mut Terminal,
    ) -> Result<Option<SessionOutcome>> {
        for _ in 0..64 {
            effect = match effect {
                ViewEffect::Continue => return Ok(None),
                ViewEffect::CopyToClipboard(value) => {
                    terminal.copy_to_clipboard(&value)?;
                    ViewEffect::Continue
                }
                ViewEffect::Exit => return Ok(Some(SessionOutcome::Exited)),
                ViewEffect::DispatchCommand(execution) => {
                    prepared_action_effect(crate::engine::command::prepare_command_action(
                        self.config,
                        execution,
                        &self.cancellation,
                    )?)
                }
                ViewEffect::RunCommand {
                    invocation,
                    prepared,
                    exit,
                } => {
                    terminal.leave()?;
                    let status = prepared.status(&self.cancellation);
                    if !exit {
                        terminal.reenter()?;
                    }
                    self.record_command_result(&invocation, status);
                    if exit {
                        ViewEffect::Exit
                    } else {
                        ViewEffect::Continue
                    }
                }
                ViewEffect::EditInput(edit) => {
                    self.apply_input_edit(edit)?;
                    self.reconcile_input()?.unwrap_or(ViewEffect::Continue)
                }
                ViewEffect::Navigate {
                    request,
                    mode,
                    parent_edit,
                } => {
                    self.apply_navigation(request, mode, None, parent_edit)?;
                    ViewEffect::Continue
                }
                ViewEffect::Call(call) => {
                    self.apply_call(call)?;
                    ViewEffect::Continue
                }
                ViewEffect::Return(returned) => match self.apply_return(returned)? {
                    ReturnTransition::Effect(effect) => *effect,
                    ReturnTransition::Outcome(outcome) => return Ok(Some(outcome)),
                },
                ViewEffect::Back(edit) => {
                    if let Some(outcome) = self.pop_current(edit)? {
                        return Ok(Some(outcome));
                    }
                    ViewEffect::Continue
                }
            };
        }
        anyhow::bail!("command action recursion exceeded 64 effects")
    }

    fn record_command_result(
        &mut self,
        invocation: &CommandInvocation,
        status: std::io::Result<std::process::ExitStatus>,
    ) {
        let entry = self
            .views
            .last_mut()
            .expect("session has no active view while recording a command result");
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
        match status {
            Ok(status) => {
                let message = command_status_message(&status);
                host.record_command_status(invocation, &message, status.success());
            }
            Err(error) => host.record_error(invocation, &error.to_string()),
        }
    }

    fn delete_backward(&mut self) -> Result<bool> {
        let returns_to_parent = self.views.len() > 1
            && self
                .views
                .last()
                .is_some_and(|entry| entry.input.raw.is_empty() && entry.input.cursor == 0);
        if returns_to_parent {
            self.pop_current(None)?;
            return Ok(false);
        }
        Ok(self
            .views
            .last_mut()
            .context("session has no active view")?
            .input
            .delete_backward())
    }

    fn mark_input_changed(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        entry.input_dirty = true;
        entry.input.rejected = false;
        self.active_error = None;
        self.active_error_deadline = None;
        Ok(())
    }

    fn dispatch_input_ready(&mut self) -> Result<ViewEffect> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if entry
            .input_deadline
            .is_none_or(|deadline| Instant::now() < deadline)
        {
            return Ok(ViewEffect::Continue);
        }
        entry.input_deadline = None;
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
        entry.instance.input_ready(&mut host)
    }

    fn apply_input_edit(&mut self, edit: InputEdit) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        match edit {
            InputEdit::SetBuffer { raw, cursor } => {
                entry.input.raw = raw;
                entry.input.set_cursor(cursor);
            }
        }
        entry.input_dirty = true;
        entry.input.rejected = false;
        self.active_error = None;
        self.active_error_deadline = None;
        Ok(())
    }

    fn apply_inactive_parent_edit(
        &mut self,
        edit: InputEdit,
        runtime_snapshot: &Value,
    ) -> Result<()> {
        let (input, input_dirty, input_deadline, state) = {
            let entry = self.views.last().context("session has no parent view")?;
            (
                entry.input.clone(),
                entry.input_dirty,
                entry.input_deadline,
                entry.state.clone(),
            )
        };
        let active_error = self.active_error.clone();
        let active_error_deadline = self.active_error_deadline;
        self.runtime.replace(runtime_snapshot.clone());

        let transaction = (|| {
            self.apply_input_edit(edit)?;
            anyhow::ensure!(
                self.reconcile_input()?.is_none(),
                "a parent input edit produced another navigation"
            );
            let entry = self
                .views
                .last_mut()
                .context("session has no parent view")?;
            anyhow::ensure!(!entry.input.rejected, "the parent input edit was rejected");
            entry.input_deadline = None;
            Ok(())
        })();
        if let Err(error) = transaction {
            let entry = self
                .views
                .last_mut()
                .context("session has no parent view during rollback")?;
            entry.input = input;
            entry.input_dirty = input_dirty;
            entry.input_deadline = input_deadline;
            entry.state = state;
            self.active_error = active_error;
            self.active_error_deadline = active_error_deadline;
            self.runtime.replace(runtime_snapshot.clone());
            let activation = self.activate_current();
            let restoration = self.restore_current_input();
            let mut rollback_errors = Vec::new();
            if let Err(activation_error) = activation {
                rollback_errors.push(format!("activate failed: {activation_error:#}"));
            }
            if let Err(restoration_error) = restoration {
                rollback_errors.push(format!("input restore failed: {restoration_error:#}"));
            }
            return if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(error.context(format!(
                    "could not fully restore the parent View after its input edit failed: {}",
                    rollback_errors.join("; ")
                )))
            };
        }
        Ok(())
    }

    fn current_binding_hints(&self) -> (Vec<ChromeHint>, Option<ChromeHint>) {
        let Some(context) = self.active_input_context().ok() else {
            return (Vec::new(), None);
        };
        let snapshot = self.input_router.snapshot(context.id);
        debug_assert_eq!(snapshot.context(), context.id);
        let mut commands = Vec::new();
        let mut overflow = None;
        for (key, id) in snapshot.bindings() {
            let Some(record) = self.input_router.record(id) else {
                continue;
            };
            if record.state == BindingState::Disabled {
                continue;
            }
            let Some(hint) = &record.target.hint else {
                continue;
            };
            let Some(key) = key.binding_name() else {
                continue;
            };
            let value = (key, hint.label.clone());
            match hint.visibility {
                CommandBindingVisibility::Always => commands.push(value),
                CommandBindingVisibility::Overflow => overflow = Some(value),
                CommandBindingVisibility::Hidden => {}
            }
        }
        commands.sort_by(|left, right| crate::engine::command::compare_bindings(&left.0, &right.0));
        (commands, overflow)
    }

    fn current_chrome(&mut self, width: usize) -> Result<crate::chrome::ChromeFrame> {
        self.refresh_active_bindings()?;
        let (binding_commands, binding_overflow) = self.current_binding_hints();
        let route_completion_available = self.route_input_available();
        let route_completion_active = self.route_completion.is_some();
        let route_completion_opens_on_tab =
            route_completion_available && self.resolve_key_binding(Key::Tab).is_none();
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let route = (self.config.default_view.as_deref() != Some(entry.view_ref.as_str()))
            .then(|| self.router.display(&entry.view_ref));
        let error = self
            .active_error
            .as_ref()
            .map(|record| record.label.clone());
        let input_focus = entry.instance.input_focus();
        let input_text = match input_focus {
            InputFocus::Focused => entry.input.raw.clone(),
            InputFocus::Unfocused => entry.input.params.clone(),
        };
        let input_cursor = match input_focus {
            InputFocus::Focused => entry.input.cursor,
            InputFocus::Unfocused => input_text.len(),
        };
        let host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let mut engine_chrome = entry.instance.chrome(&host);
        engine_chrome.commands = binding_commands;
        engine_chrome.overflow_command = binding_overflow;
        if route_completion_available
            && let Some(end) = self
                .router
                .recognized_prefix_end(&entry.view_ref, &entry.input.raw)
        {
            engine_chrome.presentation =
                engine_chrome.presentation.with_recognized_input_prefix(end);
        }
        if let Some(completion) = &self.route_completion {
            engine_chrome.status = Some(format!(
                "{} / {} views",
                usize::from(!completion.candidates.is_empty()).saturating_add(completion.selected),
                completion.candidates.len()
            ));
        }
        if route_completion_active {
            engine_chrome.overflow_command = None;
        } else if route_completion_opens_on_tab {
            engine_chrome.commands.retain(|(key, _)| key != "tab");
        }
        if input_focus == InputFocus::Unfocused {
            engine_chrome.presentation = engine_chrome.presentation.with_unfocused_input();
        }
        Ok(crate::chrome::ChromeFrame::compose_with_cursor(
            width,
            route.as_ref(),
            &input_text,
            input_cursor,
            engine_chrome,
            error.as_deref(),
        ))
    }

    fn render(&mut self, terminal: &mut Terminal) -> Result<()> {
        let chrome = self.current_chrome(terminal.size().0 as usize)?;
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let host = EngineHost {
            config: self.config,
            theme: self.theme,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let route_completion = self.route_completion.clone();
        terminal.draw(|frame| {
            let content_area = chrome.render_chrome(frame, &host.theme);
            if let Some(completion) = &route_completion {
                render_route_completion(frame, content_area, completion, &host.theme);
            } else {
                entry.instance.render(&host, frame, content_area);
            }
            if entry.instance.input_focus() == InputFocus::Focused {
                chrome.set_input_cursor(frame);
            }
        })
    }

    fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
        let route_input_available = self.route_input_available();
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if !entry.input_dirty {
            return Ok(None);
        }
        entry.input_dirty = false;
        let current_view = entry.view_ref.clone();
        let raw_input = entry.input.raw.clone();
        let cursor = entry.input.cursor;

        if !route_input_available {
            self.commit_query_input(raw_input)?;
            return Ok(None);
        }

        match self.router.resolve(&current_view, &raw_input) {
            crate::router::RouteResolution::Navigate { target, query } => {
                let query_cursor = cursor
                    .saturating_sub(raw_input.len().saturating_sub(query.len()))
                    .min(query.len());
                Ok(Some(ViewEffect::Navigate {
                    request: NavigationRequest::routed(target, query, query_cursor),
                    mode: NavigationMode::Push,
                    parent_edit: Some(InputEdit::SetBuffer {
                        raw: String::new(),
                        cursor: 0,
                    }),
                }))
            }
            crate::router::RouteResolution::Current { query } => {
                let query_cursor = cursor
                    .saturating_sub(raw_input.len().saturating_sub(query.len()))
                    .min(query.len());
                let entry = self
                    .views
                    .last_mut()
                    .context("session has no active view")?;
                entry.input.raw = query.clone();
                entry.input.set_cursor(query_cursor);
                self.commit_query_input(query)?;
                Ok(None)
            }
            crate::router::RouteResolution::NotMatched => {
                self.commit_query_input(raw_input)?;
                Ok(None)
            }
        }
    }

    fn commit_query_input(&mut self, params: String) -> Result<()> {
        let mut candidate_state = self
            .views
            .last()
            .context("session has no active view")?
            .state
            .clone();
        if let Err(error) = self
            .config
            .update_query_input(&mut candidate_state, &params)
        {
            self.reject_input(&error.to_string())?;
            return Ok(());
        }

        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        entry.input.params = params;
        entry.input.rejected = false;
        entry.state = candidate_state;
        publish_active_input(&mut self.runtime, &entry.input, &entry.state)?;

        let refresh_policy = entry.instance.input_refresh_policy();
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
        entry.instance.input_committed(&mut host)?;
        entry.input_deadline = match refresh_policy {
            InputRefreshPolicy::None => None,
            InputRefreshPolicy::Debounced(duration) => Some(Instant::now() + duration),
        };
        Ok(())
    }

    fn reject_input(&mut self, message: &str) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        entry.input.rejected = true;
        entry.input_deadline = None;
        let view_ref = entry.view_ref.clone();
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
        host.record_error_message(Some(&view_ref), None, message);
        entry.instance.input_rejected(&mut host)
    }

    fn apply_navigation(
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
        let view = self.engines.create_view(ViewContext {
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

    fn apply_call(&mut self, call: CallRequest) -> Result<()> {
        let suppresses_session_bindings = self
            .current_view_input_context()
            .is_ok_and(|context| context.grammar == InputGrammar::Raw)
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

    fn apply_return(&mut self, returned: ViewReturn) -> Result<ReturnTransition> {
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
        let returned_value = crate::engine::command::return_value(&returned);
        let cancellation = self.cancellation.clone();
        let action = crate::engine::command::prepare_continuation(
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

    fn pop_current(&mut self, edit: Option<InputEdit>) -> Result<Option<SessionOutcome>> {
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

    fn restore_current_input(&mut self) -> Result<()> {
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

    fn publish_current_location(&mut self) -> Result<()> {
        let entry = self.views.last().context("session has no active view")?;
        publish_location(
            &mut self.runtime,
            self.config,
            &entry.view_ref,
            &entry.input,
            &entry.state,
        )
    }

    fn activate_current(&mut self) -> Result<()> {
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

    fn surface_runtime_log_warning(&mut self) {
        if let Some(record) = self.runtime_log.take_warning_record() {
            self.runtime_warning = Some(record.message.clone());
            self.active_error = Some(record);
            self.active_error_deadline =
                Some(Instant::now() + crate::engine::host::ERROR_DISPLAY_DURATION);
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

fn route_completion_edit(input: &InputBuffer, completion: &RouteCompletion) -> Option<InputEdit> {
    let candidate = completion.candidates.get(completion.selected)?;
    let old_length = completion.selector_end;
    let mut replacement = candidate.view_ref.clone();
    if completion.selector_end == input.raw.len() {
        replacement.push(' ');
    }
    let mut raw = input.raw.clone();
    raw.replace_range(..completion.selector_end, &replacement);
    let cursor = if input.cursor > completion.selector_end {
        if replacement.len() >= old_length {
            input.cursor + replacement.len() - old_length
        } else {
            input.cursor.saturating_sub(old_length - replacement.len())
        }
    } else {
        replacement.len()
    };
    Some(InputEdit::SetBuffer { raw, cursor })
}

fn render_route_completion(
    frame: &mut Frame,
    area: Rect,
    completion: &RouteCompletion,
    theme: &crate::theme::Theme,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }
    if completion.candidates.is_empty() {
        frame.render_widget(
            Paragraph::new("(no matching views)").style(theme.picker.muted),
            area,
        );
        return;
    }

    let start = if completion.selected >= height {
        completion.selected + 1 - height
    } else {
        0
    };
    let lines = completion
        .candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(index, candidate)| {
            let reference = candidate
                .alias
                .as_ref()
                .map(|_| format!("  {}", candidate.view_ref))
                .unwrap_or_default();
            let metadata = format!("  [{} / {}]", candidate.plugin_name, candidate.engine_type);
            let label = format!("  {}", candidate.primary_label());
            let text = crate::chrome::clip(&format!("{label}{reference}{metadata}"), width);
            if index == completion.selected {
                Line::from(vec![
                    Span::styled("▌", theme.picker.marker),
                    Span::styled(
                        text.strip_prefix(' ').unwrap_or(&text).to_string(),
                        theme.picker.selected,
                    ),
                ])
            } else {
                let label_end = label.len().min(text.len());
                Line::from(vec![
                    Span::styled(text[..label_end].to_string(), theme.picker.text),
                    Span::styled(text[label_end..].to_string(), theme.picker.muted),
                ])
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(theme.picker.text), area);
}

fn command_status_message(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "finished successfully".to_string(),
        Some(code) => format!("finished with exit code {}", code),
        None => "terminated by signal".to_string(),
    }
}

fn initialize_navigation_input(
    config: &Config,
    state: &mut StateInstance,
    seed: Option<&InputSeed>,
    query: Option<&serde_json::Value>,
) -> Result<InputBuffer> {
    if let Some(query) = query {
        config.update_query_value(state, query)?;
        let input = sanitize_terminal_text(&config.render_query_input(state)?);
        return Ok(InputBuffer::with_params(input.clone(), input));
    }
    let Some(seed) = seed else {
        let input = sanitize_terminal_text(&config.render_query_input(state)?);
        return Ok(InputBuffer::with_params(input.clone(), input));
    };
    config.update_query_input(state, &seed.params)?;
    Ok(input_buffer_from_seed(seed))
}

fn input_buffer_from_seed(seed: &InputSeed) -> InputBuffer {
    let mut input = InputBuffer::with_params(seed.raw.clone(), seed.params.clone());
    input.set_cursor(seed.cursor);
    input
}

fn input_timeout_until(deadline: Instant) -> i32 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .max(1)
        .min(i32::MAX as u128) as i32
}

fn apply_editor_key(input: &mut InputBuffer, key: Key) -> Option<bool> {
    match key {
        Key::Char(character) if !character.is_control() => {
            input.insert(character);
            Some(true)
        }
        Key::Left => {
            input.move_left();
            Some(false)
        }
        Key::Right => {
            input.move_right();
            Some(false)
        }
        Key::Home => {
            input.move_home();
            Some(false)
        }
        Key::End => {
            input.move_end();
            Some(false)
        }
        Key::Delete => Some(input.delete_forward()),
        _ => None,
    }
}

fn publish_active_input(
    runtime: &mut super::RuntimeStore,
    input: &InputBuffer,
    state: &StateInstance,
) -> Result<()> {
    runtime.set_many([
        ("/view/current/state_revision", json!(state.revision())),
        ("/view/current/input", json!(input.params)),
        ("/view/current/raw_input", json!(input.raw)),
        ("/view/current/query", json!(input.params)),
        (
            "/session/input",
            json!({"raw": input.raw, "params": input.params, "cursor": input.cursor}),
        ),
    ])?;
    Ok(())
}

fn publish_location(
    runtime: &mut super::RuntimeStore,
    config: &Config,
    view_ref: &str,
    input: &InputBuffer,
    state: &StateInstance,
) -> Result<()> {
    let commands = crate::engine::command::collect_page_owner_commands(config, view_ref, None)?
        .into_values()
        .collect::<Vec<_>>();
    let view = json!({
        "current": {
            "ref": view_ref,
            "state_revision": state.revision(),
            "input": input.params,
            "raw_input": input.raw,
            "query": input.params,
            "cursor": input.cursor,
            "selected_item": Value::Null,
            "items": [],
            "command": commands,
            "command_owner": view_ref,
        }
    });
    let input = json!({
        "raw": input.raw,
        "params": input.params,
        "cursor": input.cursor,
    });
    if runtime.snapshot().get("session").is_some() {
        runtime.set_many([("/view", view), ("/session/input", input)])?;
    } else {
        runtime.set_many([("/view", view), ("/session", json!({"input": input}))])?;
    }
    Ok(())
}

fn publish_view_catalog(runtime: &mut super::RuntimeStore, config: &Config) -> Result<()> {
    let views = config
        .views
        .iter()
        .map(|(view_ref, view)| {
            json!({
                "label": view.alias.as_deref().unwrap_or(view_ref),
                "value": view_ref,
                "metadata": {
                    "alias": view.alias,
                    "engine": view.selected_engine_type(),
                    "plugin": view_ref.split_once(':').map(|(plugin, _)| plugin).unwrap_or(view_ref),
                },
            })
        })
        .collect::<Vec<_>>();
    runtime.set_many([("/session/views", json!(views))])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::CommandRef;
    use ratatui::Terminal as RatatuiTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Style};
    use std::sync::{Arc, Mutex};

    struct RecordingView {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingView {
        fn record(&self, event: &str, host: &EngineHost<'_>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("{event}:{}", host.input.raw));
        }
    }

    impl ViewInstance for RecordingView {
        fn activate(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
            self.record("activate", host);
            Ok(())
        }

        fn restore_input(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
            self.record("restore", host);
            Ok(())
        }

        fn input_committed(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
            self.record("committed", host);
            Ok(())
        }

        fn step(
            &mut self,
            _host: &mut EngineHost<'_>,
            _terminal: &mut Terminal,
        ) -> Result<ViewEffect> {
            Ok(ViewEffect::Continue)
        }

        fn render(
            &mut self,
            _host: &EngineHost<'_>,
            _frame: &mut ratatui::Frame,
            _area: ratatui::layout::Rect,
        ) {
        }
    }

    struct ParentTransactionView {
        events: Arc<Mutex<Vec<String>>>,
        fail_commit: bool,
        fail_activate: bool,
    }

    impl ParentTransactionView {
        fn record(&self, event: &str) {
            self.events.lock().unwrap().push(event.to_string());
        }
    }

    impl ViewInstance for ParentTransactionView {
        fn activate(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
            self.record("activate");
            if self.fail_activate {
                anyhow::bail!("parent activation failed");
            }
            Ok(())
        }

        fn restore_input(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
            self.record("restore");
            Ok(())
        }

        fn input_committed(&mut self, host: &mut EngineHost<'_>) -> Result<()> {
            let view_ref = host.runtime.snapshot()["view"]["current"]["ref"]
                .as_str()
                .unwrap_or("missing");
            self.record(&format!("commit:{view_ref}:{}", host.input.raw));
            if self.fail_commit {
                anyhow::bail!("parent commit failed");
            }
            Ok(())
        }

        fn step(
            &mut self,
            _host: &mut EngineHost<'_>,
            _terminal: &mut Terminal,
        ) -> Result<ViewEffect> {
            Ok(ViewEffect::Continue)
        }

        fn render(
            &mut self,
            _host: &EngineHost<'_>,
            _frame: &mut ratatui::Frame,
            _area: ratatui::layout::Rect,
        ) {
        }
    }

    #[test]
    fn default_session_reuses_bound_invocation_state() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.default_view = Some("trans:main".to_string());
        let state = config
            .bind_invocation_state("trans:main", &["--source=bound".to_string()])
            .unwrap();
        config.set_invocation(Value::Null, state);
        let session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        assert_eq!(
            config.query_value(&session.views[0].state).unwrap()["source"],
            "bound"
        );
        assert_eq!(session.views[0].input.raw, "bound '' ''");
    }

    #[test]
    fn routed_navigation_preserves_the_active_cursor() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
        assert_eq!(session.views[0].input.params, "");
        let input = &session.views.last().unwrap().input;
        assert_eq!(input.raw, "query");
        assert_eq!(input.cursor, "que".len());
        assert_eq!(
            session.current_chrome(80).unwrap().input_line(),
            "app query"
        );
    }

    #[test]
    fn routed_parent_commit_observes_a_coherent_parent_runtime() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.instance = Box::new(ParentTransactionView {
            events: Arc::clone(&events),
            fail_commit: false,
            fail_activate: false,
        });
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
        assert_eq!(*events.lock().unwrap(), ["commit:core:default:"]);
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "apps:main"
        );
    }

    #[test]
    fn routed_parent_commit_failure_restores_the_parent() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.instance = Box::new(ParentTransactionView {
            events: Arc::clone(&events),
            fail_commit: true,
            fail_activate: false,
        });
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

        let error = session
            .apply_navigation(request, mode, None, parent_edit)
            .unwrap_err();
        assert!(error.to_string().contains("parent commit failed"));
        assert_eq!(
            *events.lock().unwrap(),
            ["commit:core:default:", "activate", "restore"]
        );
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert_eq!(session.views[0].input.raw, "app ");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "core:default"
        );
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "app "
        );
    }

    #[test]
    fn routed_parent_rollback_restores_input_after_activation_failure() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let entry = session.views.last_mut().unwrap();
        entry.instance = Box::new(ParentTransactionView {
            events: Arc::clone(&events),
            fail_commit: true,
            fail_activate: true,
        });
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

        let error = session
            .apply_navigation(request, mode, None, parent_edit)
            .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("parent commit failed"), "error: {error}");
        assert!(error.contains("parent activation failed"), "error: {error}");
        assert_eq!(
            *events.lock().unwrap(),
            ["commit:core:default:", "activate", "restore"]
        );
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].input.raw, "app ");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["ref"],
            "core:default"
        );
    }

    #[test]
    fn routed_navigation_clears_the_default_before_replace_and_return() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
        assert_eq!(session.views[0].input.params, "");

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
        assert_eq!(input.params, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn nondefault_root_shows_its_prefix_without_enabling_routes() {
        let mut config = crate::config::load_test_fixture().unwrap();
        let state = config.bind_invocation_state("apps:main", &[]).unwrap();
        config.set_invocation(Value::Null, state);
        let cancellation = CancellationToken::new();
        let mut session = AppSession::single_root_with_theme(
            &config,
            crate::theme::ResolvedTheme::terminal(),
            crate::runtime_log::RuntimeLog::disabled(),
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
        assert_eq!(session.views[0].input.params, "sys query");
    }

    #[test]
    fn route_prefixes_are_only_resolved_by_the_default_root() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
        assert_eq!(session.views.last().unwrap().input.params, "sys nested");
    }

    #[test]
    fn empty_child_input_backspace_returns_to_its_parent() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let parent = session.views.last_mut().unwrap();
        parent.input.raw = "app old".to_string();
        parent.input.cursor = parent.input.raw.len();
        parent.instance = Box::new(RecordingView {
            events: Arc::clone(&events),
        });

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
    }

    fn call_context(session: &AppSession<'_>) -> CommandContext {
        let entry = session.views.last().unwrap();
        CommandContext {
            page: super::super::CommandOwnerContext {
                view_ref: entry.view_ref.clone(),
                state: entry.state.clone(),
                binding_raw: entry.input.params.clone(),
            },
            selection: None,
            runtime: session.runtime.snapshot().clone(),
            output: None,
        }
    }

    #[test]
    fn return_unwinds_the_nearest_call_branch_and_prepares_the_continuation() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
                output: super::super::ViewOutput::Value {
                    value: serde_json::json!("restored"),
                },
                adapter: None,
            })
            .unwrap();
        let ReturnTransition::Effect(effect) = transition else {
            panic!("return did not produce the caller continuation");
        };
        let ViewEffect::EditInput(InputEdit::SetBuffer { raw, cursor }) = *effect else {
            panic!("return did not produce an input edit");
        };
        assert_eq!(session.views.len(), 1);
        assert_eq!(session.views[0].view_ref, "core:default");
        assert!(session.runtime.snapshot().get("return").is_none());
        assert_eq!(raw, "restored");
        assert_eq!(cursor, raw.len());
    }

    #[test]
    fn nested_return_only_unwinds_the_nearest_call() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
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
                output: super::super::ViewOutput::Value {
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
                output: super::super::ViewOutput::Value {
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
            crate::runtime_log::RuntimeLog::disabled(),
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
                output: super::super::ViewOutput::Value {
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
                crate::runtime_log::RuntimeLog::disabled(),
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
                crate::runtime_log::RuntimeLog::disabled(),
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
            crate::runtime_log::RuntimeLog::disabled(),
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
            Some(InputBinding::Route(RouteAction::Next))
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
    fn configurable_editor_keys_are_not_session_fallbacks() {
        let mut input = InputBuffer::new("word");
        assert_eq!(apply_editor_key(&mut input, Key::Backspace), None);
        assert_eq!(apply_editor_key(&mut input, Key::Ctrl('u')), None);
        assert_eq!(apply_editor_key(&mut input, Key::Ctrl('w')), None);
        assert_eq!(input.raw, "word");
        assert_eq!(apply_editor_key(&mut input, Key::Delete), Some(false));
    }

    #[test]
    fn rejected_query_input_preserves_committed_params_state_and_runtime() {
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
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();

        session.views.last_mut().unwrap().input.raw = "2".to_string();
        session.views.last_mut().unwrap().input.cursor = 1;
        session.mark_input_changed().unwrap();
        assert!(session.reconcile_input().unwrap().is_none());
        let committed_revision = session.views.last().unwrap().state.revision();
        assert_eq!(session.views.last().unwrap().input.params, "2");
        assert_eq!(
            config
                .render_query_input(&session.views.last().unwrap().state)
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
        assert_eq!(entry.input.params, "2");
        assert!(entry.input.rejected);
        assert_eq!(entry.state.revision(), committed_revision);
        assert_eq!(config.render_query_input(&entry.state).unwrap(), "2");
        assert_eq!(session.runtime.snapshot()["view"]["current"]["input"], "2");
        assert_eq!(
            session.runtime.snapshot()["view"]["current"]["raw_input"],
            "2"
        );

        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn route_completion_uses_shared_picker_bindings() {
        let completion = RouteCompletion {
            input_context: InputContextId::default(),
            candidates: vec![
                crate::router::ViewCandidate {
                    view_ref: "apps:main".to_string(),
                    alias: Some("app".to_string()),
                    plugin_name: "Applications".to_string(),
                    engine_type: "picker".to_string(),
                },
                crate::router::ViewCandidate {
                    view_ref: "system:main".to_string(),
                    alias: Some("sys".to_string()),
                    plugin_name: "System".to_string(),
                    engine_type: "capture".to_string(),
                },
            ],
            selected: 1,
            selector_end: 0,
        };
        let mut theme = ResolvedTheme::terminal();
        theme.picker.text = Style::new().fg(Color::Red).bg(Color::Black);
        theme.picker.muted = Style::new().fg(Color::Green).bg(Color::Black);
        theme.picker.selected = Style::new().fg(Color::Yellow).bg(Color::Blue);
        theme.picker.marker = Style::new().fg(Color::Magenta).bg(Color::Black);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(60, 2)).unwrap();

        terminal
            .draw(|frame| {
                render_route_completion(frame, frame.area(), &completion, &theme);
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
        let completion = RouteCompletion {
            input_context: InputContextId::default(),
            candidates: Vec::new(),
            selected: 0,
            selector_end: 0,
        };
        let mut theme = ResolvedTheme::terminal();
        theme.picker.muted = Style::new().fg(Color::Magenta).bg(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(30, 1)).unwrap();

        terminal
            .draw(|frame| {
                render_route_completion(frame, frame.area(), &completion, &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "(");
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn route_completion_replaces_the_selector_and_preserves_the_query_cursor() {
        let input = InputBuffer {
            raw: "app query".to_string(),
            params: "app query".to_string(),
            cursor: 7,
            rejected: false,
        };
        let completion = RouteCompletion {
            input_context: InputContextId::default(),
            candidates: vec![crate::router::ViewCandidate {
                view_ref: "apps:main".to_string(),
                alias: Some("app".to_string()),
                plugin_name: "apps".to_string(),
                engine_type: "picker".to_string(),
            }],
            selected: 0,
            selector_end: 3,
        };

        assert_eq!(
            route_completion_edit(&input, &completion),
            Some(InputEdit::SetBuffer {
                raw: "apps:main query".to_string(),
                cursor: 13,
            })
        );
    }

    #[test]
    fn publishing_a_location_preserves_unrelated_runtime_data() {
        let mut runtime = super::super::RuntimeStore::new();
        runtime
            .set("/invocation_marker", json!({"items": [1, 2]}))
            .unwrap();

        let config = crate::config::load_test_fixture().unwrap();
        let state = config.instantiate_state("core:default").unwrap();
        let input = InputBuffer::new("query");
        publish_location(&mut runtime, &config, "core:default", &input, &state).unwrap();
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
