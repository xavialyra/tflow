use super::AppSession;
use super::input::PassthroughEvent;
use super::state::{
    ACTION_PRIORITY, BindingHint, BindingTarget, InputBinding, InputContext, InputGrammar,
    PublishedBindings, RegisteredBinding, RouteAction, RouteCompletion,
};
use crate::chrome::InputBuffer;
use crate::command::{
    CommandExecution, EditorAction, InputEdit, LauncherOutcome, NavigationMode, NavigationRequest,
    ResolvedInputAction, SelectionBindingState, ViewEffect, ViewInputMode,
};
use crate::config::{CommandBindingVisibility, CommandRequirement, CommandScope};
use crate::engine::{EngineHost, InputFocus, InputRefreshPolicy};
use crate::input::keymap::{BindingEntry, BindingRecord, BindingState};
use crate::input::{DecodedInput, Key};
use crate::terminal::{InputRead, Terminal};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::time::Instant;

impl AppSession<'_> {
    pub(super) fn current_view_input_context(&self) -> Result<InputContext> {
        let entry = self.views.last().context("session has no active view")?;
        Ok(match entry.instance.input_mode() {
            ViewInputMode::Keymap => InputContext {
                id: entry.input_layers.context,
                grammar: InputGrammar::Decoded,
            },
            ViewInputMode::Passthrough => InputContext {
                id: entry.input_layers.context,
                grammar: InputGrammar::Raw,
            },
        })
    }

    pub(super) fn active_input_context(&self) -> Result<InputContext> {
        if let Some(completion) = &self.route_completion {
            return Ok(InputContext {
                id: completion.input_context,
                grammar: InputGrammar::Decoded,
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

    pub(super) fn route_input_available(&self) -> bool {
        self.route_input
            && self.views.len() == 1
            && self.views.last().is_some_and(|entry| {
                self.config.default_view.as_deref() == Some(entry.view_ref.as_str())
                    && entry.instance.input_focus() == InputFocus::Focused
            })
    }

    pub(super) fn open_route_completion(&mut self) -> Result<()> {
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

    pub(super) fn close_route_completion(&mut self) -> Option<RouteCompletion> {
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

    pub(super) fn step(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
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
                match self.dispatch_decoded_input(queued)? {
                    LauncherOutcome::Continue => {}
                    LauncherOutcome::Effect(effect) => return Ok(*effect),
                }
            }
        }

        Ok(self.reconcile_input()?.unwrap_or(ViewEffect::Continue))
    }

    pub(super) fn dispatch_decoded_input(
        &mut self,
        queued: DecodedInput,
    ) -> Result<LauncherOutcome> {
        self.refresh_active_bindings()?;
        let route_active = self.route_completion.is_some();
        let captures_editor_input = self
            .views
            .last()
            .context("session has no active view")?
            .instance
            .captures_editor_input();
        let Some(key) = queued.key else {
            if !route_active {
                return Ok(LauncherOutcome::Continue);
            }
            self.close_route_completion();
            self.refresh_active_bindings()?;
            return self.dispatch_unbound_input(&queued.raw);
        };
        let binding = self.resolve_key_binding(key);
        if route_active && binding.is_none() {
            self.close_route_completion();
            self.refresh_active_bindings()?;
            return self.dispatch_route_unbound(key, captures_editor_input);
        }
        if binding.is_none() && !route_active && key == Key::Tab && self.route_input_available() {
            self.open_route_completion()?;
            self.refresh_active_bindings()?;
            return Ok(LauncherOutcome::Continue);
        }
        match binding {
            Some(InputBinding::ViewCommand(binding_key)) => {
                self.dispatch_registered_view_binding(binding_key, queued, false, true)
            }
            Some(InputBinding::PendingViewCommand(binding_key)) => {
                self.dispatch_registered_view_binding(binding_key, queued, true, true)
            }
            Some(InputBinding::SessionCommand(binding_key)) => {
                self.dispatch_session_binding(binding_key, queued, true)
            }
            Some(InputBinding::Action(ResolvedInputAction::Edit(action))) => {
                self.dispatch_editor_action(action)
            }
            Some(InputBinding::Action(ResolvedInputAction::View(action))) => {
                self.dispatch_view_action(action, queued, true)
            }
            Some(InputBinding::Route(action)) => {
                self.handle_route_action(action)?;
                if matches!(action, RouteAction::Accept)
                    && let Some(effect) = self.reconcile_input()?
                {
                    Ok(LauncherOutcome::Effect(Box::new(effect)))
                } else {
                    Ok(LauncherOutcome::Continue)
                }
            }
            Some(InputBinding::Disabled) => Ok(LauncherOutcome::Continue),
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
                Ok(LauncherOutcome::Continue)
            }
        }
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

    pub(super) fn refresh_active_bindings(&mut self) -> Result<()> {
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
                    .and_then(|key| Key::parse_binding(key.as_str()).ok())
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

    pub(super) fn resolve_key_binding(&self, key: Key) -> Option<InputBinding> {
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

    fn dispatch_route_unbound(
        &mut self,
        key: Key,
        captures_editor_input: bool,
    ) -> Result<LauncherOutcome> {
        if captures_editor_input {
            return Ok(LauncherOutcome::Continue);
        }
        match self.resolve_key_binding(key) {
            Some(InputBinding::Action(ResolvedInputAction::Edit(action))) => {
                return self.dispatch_editor_action(action);
            }
            Some(_) => return Ok(LauncherOutcome::Continue),
            None => {}
        }
        let edited = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            apply_editor_key(&mut entry.input, key)
        };
        if edited == Some(true) {
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
        action: crate::engine::ViewAction,
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
}

impl AppSession<'_> {
    pub(super) fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
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
        super::publication::publish_active_input(&mut self.runtime, &entry.input, &entry.state)?;

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
}

pub(super) fn route_completion_edit(
    input: &InputBuffer,
    completion: &RouteCompletion,
) -> Option<InputEdit> {
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

fn input_timeout_until(deadline: Instant) -> i32 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .max(1)
        .min(i32::MAX as u128) as i32
}

pub(super) fn apply_editor_key(input: &mut InputBuffer, key: Key) -> Option<bool> {
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
