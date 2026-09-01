use super::AppSession;
use super::completion::{
    CompletionCandidate, CompletionRuntime, SelectionDecision, SelectionInput,
    route_completion_edit,
};
use super::decision::{DecisionPolicy, decision_effect, preflight_engine_decision, staged_runtime};
use super::input::RawPipelineEvent;
use super::state::{
    ACTION_PRIORITY, BindingHint, BindingTarget, CompletionState, InputBinding, PublishedBindings,
    RegisteredBinding, RouteAction,
};
use crate::command::{
    EditorAction, InputEdit, LauncherOutcome, NavigationMode, NavigationRequest,
    ResolvedInputAction, ViewEffect, ViewReturn,
};
use crate::config::{
    CommandBinding, CommandBindingVisibility, CommandRequirement, CommandScope, normalize_key,
};
use crate::engine::{
    ActionInvocation, BackgroundEngineTick, EngineActionInput, EngineCommandBinding,
    EngineDecision, EngineTick, EngineTickMode, ExternalTickAction, ExternalTickResult, InputFocus,
    InputRefreshPolicy, TerminalEofPolicy, ViewContextIdentity,
};
use crate::input::EditorBuffer;
use crate::input::keymap::{BindingEntry, BindingRecord, BindingState, LayerId};
use crate::input::{
    CompletionEdit, DecodedInput, InputConsumer, InputContext, InputStrategy, InputTarget, Key,
};
use crate::terminal::{InputRead, Terminal};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::time::Instant;

type ActiveBindingLayerReplacement = (LayerId, Vec<(Key, BindingEntry<RegisteredBinding>)>);

struct PreparedActiveBindingLayers {
    published: PublishedBindings,
    replacements: Vec<ActiveBindingLayerReplacement>,
}

impl AppSession<'_> {
    pub(super) fn current_view_input_context(&self) -> Result<InputContext> {
        self.views
            .last()
            .map(|entry| entry.input_context())
            .context("session has no active view")
    }

    pub(super) fn active_input_context(&self) -> Result<InputContext> {
        if let Some(completion) = &self.route_completion {
            return Ok(completion.host.context);
        }
        self.current_view_input_context()
    }

    fn session_command_bindings(&self) -> BTreeMap<String, CommandBinding> {
        let mut bindings = self.config.commands.bindings.clone();
        if !bindings.contains_key("commands") && self.config.view("selectors:commands").is_some() {
            bindings.insert("commands".to_string(), CommandBinding::builtin_commands());
        }
        bindings
    }

    fn session_command_for_key(
        &self,
        view_ref: &str,
        key: Key,
    ) -> Option<crate::command::CommandInvocation> {
        let name = key.binding_name()?;
        self.session_command_bindings()
            .iter()
            .find_map(|(id, binding)| {
                if binding
                    .key(id)
                    .and_then(|key| normalize_key(key).ok())
                    .as_deref()
                    != Some(name.as_str())
                {
                    return None;
                }
                binding.as_command(id).map(|command| {
                    crate::command::CommandInvocation::session_command(view_ref, id, command)
                })
            })
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
                    && entry.input_focus() == InputFocus::Focused
            })
    }

    pub(super) fn open_route_completion(&mut self) -> Result<()> {
        let parent_context = self.current_view_input_context()?;
        let entry = self.views.last().context("session has no active view")?;
        let input = &entry.input;
        let selector_end = input
            .raw
            .find(char::is_whitespace)
            .unwrap_or(input.raw.len())
            .min(input.cursor);
        let candidates = self
            .router
            .complete_views(&input.raw[..selector_end], &entry.view_ref)
            .into_iter()
            .map(CompletionCandidate::from)
            .collect();
        let input_context_id = self.input_router.create_context();
        let input_layer = self
            .input_router
            .mount_layer(input_context_id, ACTION_PRIORITY);
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
                        owner: InputConsumer::Completion(crate::input::CompletionId(
                            input_context_id.0,
                        )),
                        target: BindingTarget::Completion(action),
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
            self.input_router.remove_context(input_context_id);
            return Err(error);
        }
        let snapshot = crate::input::CompletionSnapshot::from_buffer(
            entry.source_identity(),
            input,
            0..selector_end,
            input.raw[..selector_end].to_string(),
        );
        let mut completion_context = InputContext::decoded(
            input_context_id,
            InputConsumer::Completion(crate::input::CompletionId(input_context_id.0)),
            None,
            InputTarget::Ignore,
        );
        completion_context.replay_context = Some(parent_context.id);
        let completion_host = crate::input::CompletionHost::new(snapshot, completion_context);
        self.route_completion = Some(CompletionState {
            host: completion_host,
            runtime: Box::new(CompletionRuntime::new(candidates)),
        });
        Ok(())
    }

    pub(super) fn close_route_completion(&mut self) -> Option<CompletionState> {
        let completion = self.route_completion.take()?;
        self.input_router.remove_context(completion.host.context.id);
        Some(completion)
    }

    fn handle_route_action(&mut self, action: RouteAction) -> Result<bool> {
        match action {
            RouteAction::Next => {
                let completion = self
                    .route_completion
                    .as_mut()
                    .context("route completion is not active")?;
                let decision = completion.runtime.dispatch(SelectionInput::Next)?;
                anyhow::ensure!(
                    matches!(decision, SelectionDecision::Invalidate),
                    "completion next action returned an unexpected decision"
                );
                Ok(false)
            }
            RouteAction::Previous => {
                let completion = self
                    .route_completion
                    .as_mut()
                    .context("route completion is not active")?;
                let decision = completion.runtime.dispatch(SelectionInput::Previous)?;
                anyhow::ensure!(
                    matches!(decision, SelectionDecision::Invalidate),
                    "completion previous action returned an unexpected decision"
                );
                Ok(false)
            }
            RouteAction::Accept => {
                let completion = self
                    .route_completion
                    .as_mut()
                    .context("route completion is not active")?;
                let decision = completion.runtime.dispatch(SelectionInput::Accept)?;
                let SelectionDecision::Accepted(candidate) = decision else {
                    anyhow::bail!("completion accept action returned an unexpected decision");
                };
                let completion = self
                    .close_route_completion()
                    .context("route completion is not active")?;
                let input = &self
                    .views
                    .last()
                    .context("session has no active view")?
                    .input;
                if let Some(edit) = candidate
                    .as_ref()
                    .and_then(|candidate| route_completion_edit(input, &completion, candidate))
                {
                    return self.apply_completion_edit(edit);
                }
                Ok(false)
            }
            RouteAction::Close => {
                if let Some(completion) = self.route_completion.as_mut() {
                    let decision = completion.runtime.dispatch(SelectionInput::Cancel)?;
                    anyhow::ensure!(
                        matches!(decision, SelectionDecision::Cancelled),
                        "completion cancel action returned an unexpected decision"
                    );
                }
                self.close_route_completion();
                Ok(false)
            }
        }
    }

    fn apply_completion_edit(&mut self, edit: CompletionEdit) -> Result<bool> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if edit.source_identity != entry.source_identity()
            || edit.source_revision != entry.input.revision
        {
            return Ok(false);
        }
        edit.apply(entry.source_identity(), &mut entry.input)
            .map_err(|error| anyhow::anyhow!("completion edit was rejected: {error}"))?;
        entry.mark_input_changed();
        self.clear_input_error();
        Ok(true)
    }

    fn prepare_deferred_effect_and_input_ready(&mut self) -> Result<Option<ViewEffect>> {
        if let Some(effect) = self.deferred_effect.take() {
            return Ok(Some(effect));
        }
        let effect = match self.dispatch_input_ready()? {
            Some(prepared) => match self.commit_input_ready(prepared)? {
                LauncherOutcome::Continue => ViewEffect::Continue,
                LauncherOutcome::Effect(effect) => *effect,
            },
            None => ViewEffect::Continue,
        };
        if self.committed_outcome.is_some() || !matches!(effect, ViewEffect::Continue) {
            return Ok(Some(effect));
        }
        Ok(None)
    }

    fn reconcile_dirty_input_for_step(&mut self) -> Result<Option<ViewEffect>> {
        if self.views.last().is_some_and(|entry| entry.input_dirty)
            && let Some(effect) = self.reconcile_input()?
        {
            return Ok(Some(effect));
        }
        if self.committed_outcome.is_some() {
            return Ok(Some(ViewEffect::Continue));
        }
        Ok(None)
    }

    fn tick_active_engine(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let mode = self.views[index].runtime.tick_mode();
        Ok(match mode {
            EngineTickMode::Prepared => {
                let context = self.current_view_context()?;
                let content_size = self.active_content_size(terminal.size())?;
                let emission = self.views[index].runtime.tick(EngineTick {
                    context,
                    content_size,
                })?;
                match self.commit_engine_emission_at(index, emission)? {
                    LauncherOutcome::Continue => ViewEffect::Continue,
                    LauncherOutcome::Effect(effect) => *effect,
                }
            }
            EngineTickMode::External => {
                let context = self.current_view_context()?;
                let content_size = self.active_content_size(terminal.size())?;
                let result = self.views[index].runtime.drive_tick(EngineTick {
                    context,
                    content_size,
                })?;
                self.apply_external_tick_result(index, result)?
            }
        })
    }

    fn poll_after_foreground_tick(
        &mut self,
        foreground_effect: ViewEffect,
    ) -> Result<Option<ViewEffect>> {
        if self.committed_outcome.is_some() {
            return Ok(Some(ViewEffect::Continue));
        }
        if matches!(foreground_effect, ViewEffect::Continue) {
            self.poll_background_views()?;
        }
        if !matches!(foreground_effect, ViewEffect::Continue) {
            return Ok(Some(foreground_effect));
        }
        if let Some(effect) = self.poll_active_engine_work()? {
            return Ok(Some(effect));
        }
        if self.committed_outcome.is_some() {
            return Ok(Some(ViewEffect::Continue));
        }
        Ok(None)
    }

    fn refresh_bindings_and_retry_pending_command(&mut self) -> Result<Option<ViewEffect>> {
        self.refresh_active_bindings()?;
        if let Some(effect) = self.retry_pending_command()? {
            return Ok(Some(effect));
        }
        if self.committed_outcome.is_some() {
            return Ok(Some(ViewEffect::Continue));
        }
        Ok(None)
    }

    fn process_terminal_input(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let timeout = {
            let entry = self.views.last().context("session has no active view")?;
            input_poll_timeout(
                entry.engine_definition.mount_policy.launcher_input_timeout,
                entry.input_deadline.map(input_timeout_until),
            )
        };
        let input_context = self.active_input_context()?;
        let snapshot = self.input_router.snapshot(input_context.id);
        if input_context.strategy == InputStrategy::RawIntercepted {
            let revision = snapshot.revision();
            let intercepted_keys = snapshot.bindings().map(|(key, _)| key).collect::<Vec<_>>();
            if self.input_pipeline.strategy() != InputStrategy::RawIntercepted {
                self.input_pipeline.preserve_decoder_pending();
                self.input_pipeline
                    .enter_raw_strategy(intercepted_keys.clone());
            }
            self.input_pipeline
                .sync_raw_keys(revision, intercepted_keys);
            return self.step_raw(terminal, timeout);
        }
        if self.input_pipeline.strategy() == InputStrategy::RawIntercepted
            && let Some(pending) = self.input_pipeline.leave_raw_strategy()
        {
            self.input_pipeline.push_event_front(pending);
        }
        if self.input_pipeline.pending_is_empty() {
            match self.input_pipeline.read_normal(terminal, timeout)? {
                InputRead::Eof | InputRead::Data(_) | InputRead::Timeout => {}
            }
        }
        while let Some(queued) = self.input_pipeline.pop_input() {
            match self.dispatch_input_event(queued)? {
                LauncherOutcome::Continue if self.committed_outcome.is_some() => {
                    return Ok(ViewEffect::Continue);
                }
                LauncherOutcome::Continue => {}
                LauncherOutcome::Effect(effect) => return Ok(*effect),
            }
        }

        if self.route_completion.is_some() {
            return Ok(ViewEffect::Continue);
        }
        self.reconcile_input()
            .map(|effect| effect.unwrap_or(ViewEffect::Continue))
    }

    pub(super) fn step(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        if let Some(effect) = self.prepare_deferred_effect_and_input_ready()? {
            return Ok(effect);
        }
        if let Some(effect) = self.reconcile_dirty_input_for_step()? {
            return Ok(effect);
        }
        let foreground_effect = self.tick_active_engine(terminal)?;
        if let Some(effect) = self.poll_after_foreground_tick(foreground_effect)? {
            return Ok(effect);
        }
        if let Some(effect) = self.refresh_bindings_and_retry_pending_command()? {
            return Ok(effect);
        }
        self.process_terminal_input(terminal)
    }

    fn dispatch_input_event(&mut self, event: crate::input::InputEvent) -> Result<LauncherOutcome> {
        match event {
            crate::input::InputEvent::Key(input) => self.dispatch_decoded_input(input),
            crate::input::InputEvent::Raw(bytes)
                if self.active_input_context()?.strategy == InputStrategy::Decoded
                    && matches!(bytes.as_slice(), b"\r" | b"\n") =>
            {
                self.dispatch_decoded_input(DecodedInput {
                    key: Some(Key::Enter),
                    raw: bytes,
                })
            }
            crate::input::InputEvent::Eof if self.route_completion.is_some() => {
                self.dispatch_unmatched_event(crate::input::InputEvent::Eof)
            }
            crate::input::InputEvent::Eof => self.handle_terminal_eof(),
            event => self.dispatch_unmatched_event(event),
        }
    }

    fn dispatch_unmatched_event(
        &mut self,
        event: crate::input::InputEvent,
    ) -> Result<LauncherOutcome> {
        let disposition = if let Some(completion) = &self.route_completion {
            completion.host.unmatched(event.clone())
        } else {
            self.active_input_context()?
                .disposition_for_unmatched(event.clone())
        };
        match disposition {
            crate::input::EventDisposition::CloseAndReplay(parent, replay) => {
                self.close_route_completion();
                self.refresh_active_bindings()?;
                anyhow::ensure!(
                    self.active_input_context()?.id == parent,
                    "completion replay target is no longer active"
                );
                self.dispatch_input_event(replay)
            }
            crate::input::EventDisposition::ForwardRaw(receiver, bytes) => {
                self.dispatch_raw_to_receiver(receiver, &bytes)
            }
            crate::input::EventDisposition::DelegateTo(InputTarget::EditorBuffer) => match event {
                crate::input::InputEvent::Paste(bytes) => self.insert_paste(&bytes),
                crate::input::InputEvent::Raw(_) | crate::input::InputEvent::Key(_) => {
                    Ok(LauncherOutcome::Continue)
                }
                crate::input::InputEvent::Eof => Ok(LauncherOutcome::Continue),
            },
            crate::input::EventDisposition::Ignore
            | crate::input::EventDisposition::DelegateTo(_) => Ok(LauncherOutcome::Continue),
        }
    }

    fn dispatch_raw_to_receiver(
        &mut self,
        receiver: crate::input::ReceiverId,
        bytes: &[u8],
    ) -> Result<LauncherOutcome> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let Some(raw_receiver) = entry.raw_receiver_for(receiver)? else {
            return Ok(LauncherOutcome::Continue);
        };
        raw_receiver.push_input(bytes)?;
        Ok(LauncherOutcome::Continue)
    }

    fn insert_paste(&mut self, bytes: &[u8]) -> Result<LauncherOutcome> {
        let payload = bytes
            .strip_prefix(b"\x1b[200~")
            .and_then(|bytes| bytes.strip_suffix(b"\x1b[201~"))
            .unwrap_or(bytes);
        let Ok(text) = std::str::from_utf8(payload) else {
            return Ok(LauncherOutcome::Continue);
        };
        let changed = self
            .views
            .last_mut()
            .context("session has no active view")?
            .input
            .insert_text(text);
        if changed {
            self.mark_input_changed()?;
        }
        Ok(LauncherOutcome::Continue)
    }

    pub(super) fn dispatch_decoded_input(
        &mut self,
        queued: DecodedInput,
    ) -> Result<LauncherOutcome> {
        self.refresh_active_bindings()?;
        let route_active = self.route_completion.is_some();
        let Some(key) = queued.key else {
            return self.dispatch_unmatched_event(crate::input::InputEvent::Key(queued));
        };
        let binding = self.resolve_key_binding(key);
        if !route_active
            && self.active_input_context()?.strategy == InputStrategy::RawIntercepted
            && binding.is_none()
        {
            return self.dispatch_unmatched_event(crate::input::InputEvent::Key(queued));
        }
        if route_active && binding.is_none() {
            return self.dispatch_unmatched_event(crate::input::InputEvent::Key(queued));
        }
        if binding.is_none() && !route_active && key == Key::Tab && self.route_input_available() {
            if let Some(effect) = self.reconcile_input()? {
                self.input_pipeline.push_front(queued);
                return Ok(LauncherOutcome::Effect(Box::new(effect)));
            }
            self.open_route_completion()?;
            self.refresh_active_bindings()?;
            return Ok(LauncherOutcome::Continue);
        }
        match binding {
            Some(InputBinding::ViewCommand(binding_key)) => {
                self.dispatch_registered_view_binding(binding_key, queued, true)
            }
            Some(InputBinding::EngineCommand {
                invocation,
                pending,
            }) => self.dispatch_registered_engine_binding(invocation, queued, pending, true),
            Some(InputBinding::SessionCommand(binding_key)) => {
                self.dispatch_session_binding(binding_key, queued, true)
            }
            Some(InputBinding::EditorAction(action)) => self.dispatch_editor_action(action),
            Some(InputBinding::EngineAction(id)) => {
                if let Some(effect) = self.reconcile_input()? {
                    self.input_pipeline.push_front(queued);
                    Ok(LauncherOutcome::Effect(Box::new(effect)))
                } else {
                    self.dispatch_engine_invocation(ActionInvocation::new(id))
                }
            }
            Some(InputBinding::Completion(action)) => {
                let applied = self.handle_route_action(action)?;
                if applied && let Some(effect) = self.reconcile_input()? {
                    Ok(LauncherOutcome::Effect(Box::new(effect)))
                } else {
                    Ok(LauncherOutcome::Continue)
                }
            }
            Some(InputBinding::Disabled) => Ok(LauncherOutcome::Continue),
            None => {
                let accepts_editor_input =
                    self.active_input_context()?.buffer_target == Some(InputTarget::EditorBuffer);
                if accepts_editor_input
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

    pub(super) fn poll_active_engine_work(&mut self) -> Result<Option<ViewEffect>> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let mount_id = self.views[index].mount_id;
        let Some(emission) = self.views[index].runtime.poll_work()? else {
            return Ok(None);
        };
        if self.views.last().map(|entry| entry.mount_id) != Some(mount_id) {
            return Ok(None);
        }
        match self.commit_engine_emission_at(index, emission)? {
            LauncherOutcome::Continue => Ok(None),
            LauncherOutcome::Effect(effect) => Ok(Some(*effect)),
        }
    }

    fn apply_background_outcome(
        &mut self,
        index: usize,
        outcome: crate::engine::BackgroundOutcome,
    ) {
        let context_runtime_snapshot = super::restricted_runtime_snapshot(&self.runtime);
        let publication = outcome.publication;
        if let Some(entry) = self.views.get_mut(index) {
            entry.context = entry.context.committed(
                entry.input.snapshot(),
                entry.buffer_generation,
                entry.context.parameter_snapshot().clone(),
                entry.state.input_rejected(),
                context_runtime_snapshot,
                publication.as_ref(),
            );
        } else {
            return;
        }
        let starter = self
            .mount_task_starter(index)
            .expect("background work mount disappeared after commit");
        let runtime_snapshot = self.runtime.snapshot().clone();
        self.views[index]
            .runtime
            .start_prepared_work(&starter, &runtime_snapshot);
        for notice in outcome.notices {
            self.apply_engine_notice(notice);
        }
    }

    pub(super) fn poll_background_views(&mut self) -> Result<()> {
        let count = self.views.len().saturating_sub(1);
        for index in 0..count {
            let tick = BackgroundEngineTick {
                mount_id: self.views[index].mount_id,
                expected: self.views[index].context.identity(),
            };
            match self.views[index].runtime.tick_mode() {
                crate::engine::EngineTickMode::Prepared => {
                    let outcome = self.views[index].runtime.background_tick(tick)?;
                    if self.views.get(index).map(|entry| entry.mount_id) == Some(tick.mount_id) {
                        self.apply_background_outcome(index, outcome);
                    }
                }
                crate::engine::EngineTickMode::External => {
                    self.views[index].runtime.drive_background_tick(tick)?;
                }
            }

            if self.views.get(index).map(|entry| entry.mount_id) != Some(tick.mount_id) {
                break;
            }
            if let Some(outcome) = self.views[index].runtime.poll_background_work()? {
                if self.views.get(index).map(|entry| entry.mount_id) != Some(tick.mount_id) {
                    break;
                }
                self.apply_background_outcome(index, outcome);
            }
        }
        Ok(())
    }

    pub(super) fn refresh_active_bindings(&mut self) -> Result<()> {
        let prepared = match self.prepare_active_binding_layers() {
            Ok(prepared) => prepared,
            Err(error) => {
                self.invalidate_engine_command_layer()?;
                return Err(error);
            }
        };
        if let Err(error) = self.install_active_binding_layers(prepared) {
            self.invalidate_engine_command_layer()?;
            return Err(error);
        }
        Ok(())
    }

    fn prepare_active_binding_layers(&self) -> Result<PreparedActiveBindingLayers> {
        let session_suppressed = self.session_bindings_suppressed();
        let (active_context, projection) = {
            let entry = self.views.last().context("session has no active view")?;
            let context = entry.context.clone();
            let projection = entry.runtime.command_projection(&context)?;
            (context, projection)
        };
        let context_identity = active_context.identity();
        if projection.based_on != context_identity {
            anyhow::bail!("engine command projection is based on a stale ViewContextIdentity");
        }

        let entry = self.views.last().context("session has no active view")?;
        let published = PublishedBindings {
            input_strategy: entry.engine_definition.input.strategy,
            actions: entry.input_bindings.clone(),
            engine_identity: projection.based_on,
            engine_commands: projection.bindings.clone(),
            session_suppressed,
        };
        super::state::validate_engine_input_bindings(&published.actions, &entry.engine_definition)?;
        super::state::validate_engine_command_bindings(&published.engine_commands, self.config)?;

        let layers = entry.input_layers;
        let owner = layers.input_context.owner;
        let item_commands_in_engine_layer =
            entry.engine_definition.kind == crate::config::ENGINE_PICKER;
        let replacements = vec![
            (
                layers.actions,
                Self::prepare_action_layer_entries(&published.actions, owner),
            ),
            (
                layers.view_commands,
                Self::prepare_view_command_layer_entries(
                    &entry.definition,
                    &entry.view_ref,
                    published.input_strategy,
                    item_commands_in_engine_layer,
                    owner,
                )?,
            ),
            (
                layers.engine_commands,
                Self::prepare_engine_command_layer_entries(
                    &published.engine_commands,
                    published.engine_identity,
                    owner,
                ),
            ),
            (
                layers.session_commands,
                self.prepare_session_command_layer_entries(
                    published.input_strategy,
                    published.session_suppressed,
                    owner,
                ),
            ),
        ];
        Ok(PreparedActiveBindingLayers {
            published,
            replacements,
        })
    }

    fn install_active_binding_layers(
        &mut self,
        prepared: PreparedActiveBindingLayers,
    ) -> Result<()> {
        if self
            .views
            .last()
            .is_some_and(|entry| entry.published_bindings.as_ref() == Some(&prepared.published))
        {
            return Ok(());
        }
        self.input_router.replace_layers(prepared.replacements)?;
        self.views
            .last_mut()
            .context("session has no active view")?
            .published_bindings = Some(prepared.published);
        Ok(())
    }

    fn prepare_action_layer_entries(
        actions: &[crate::command::InputActionBinding],
        owner: InputConsumer,
    ) -> Vec<(Key, BindingEntry<RegisteredBinding>)> {
        actions
            .iter()
            .map(|action| {
                let target = match &action.action {
                    ResolvedInputAction::Engine(id) => BindingTarget::EngineAction(id.clone()),
                    ResolvedInputAction::Edit(edit) => BindingTarget::EditorAction(*edit),
                };
                (
                    action.key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            owner,
                            target,
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
            .collect()
    }

    fn prepare_view_command_layer_entries(
        view: &crate::config::ViewDefinition,
        view_ref: &str,
        input_strategy: InputStrategy,
        item_commands_in_engine_layer: bool,
        owner: InputConsumer,
    ) -> Result<Vec<(Key, BindingEntry<RegisteredBinding>)>> {
        let mut entries = Vec::new();
        for (id, command) in view.commands.iter().filter(|(_, command)| {
            (input_strategy == InputStrategy::Decoded || command.passthrough)
                && (!item_commands_in_engine_layer || command.requires != CommandRequirement::Items)
        }) {
            let key = Key::parse_binding(&crate::config::normalize_key(&command.key)?)?;
            let command_ref = crate::command::CommandRef {
                view: view_ref.to_string(),
                id: id.clone(),
            };
            entries.push((
                key,
                BindingEntry::Bind(BindingRecord {
                    target: RegisteredBinding {
                        owner,
                        target: BindingTarget::ViewCommand(command_ref),
                        hint: Some(BindingHint {
                            label: command.label.clone(),
                            visibility: CommandBindingVisibility::Always,
                        }),
                    },
                    state: BindingState::Ready,
                }),
            ));
        }
        Ok(entries)
    }

    fn prepare_engine_command_layer_entries(
        engine_commands: &[EngineCommandBinding],
        engine_identity: ViewContextIdentity,
        owner: InputConsumer,
    ) -> Vec<(Key, BindingEntry<RegisteredBinding>)> {
        engine_commands
            .iter()
            .map(|binding| {
                let invocation = crate::engine::EngineCommandInvocation::new(
                    binding.command.clone(),
                    engine_identity,
                );
                (
                    binding.key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            owner,
                            target: BindingTarget::EngineCommand(invocation),
                            hint: binding.label.clone().map(|label| BindingHint {
                                label,
                                visibility: binding.visibility,
                            }),
                        },
                        state: if binding.enabled {
                            BindingState::Ready
                        } else {
                            BindingState::Pending
                        },
                    }),
                )
            })
            .collect()
    }

    fn prepare_session_command_layer_entries(
        &self,
        input_strategy: InputStrategy,
        session_suppressed: bool,
        owner: InputConsumer,
    ) -> Vec<(Key, BindingEntry<RegisteredBinding>)> {
        let mut entries = Vec::new();
        if !session_suppressed {
            for (id, binding) in self.session_command_bindings() {
                let Some(key) = binding
                    .key(&id)
                    .and_then(|key| crate::config::normalize_key(key).ok())
                    .and_then(|key| Key::parse_binding(key.as_str()).ok())
                else {
                    continue;
                };
                let (Some(label), Some(visibility)) = (binding.label(&id), binding.visibility(&id))
                else {
                    continue;
                };
                if input_strategy == InputStrategy::RawIntercepted && id != "commands" {
                    continue;
                }
                entries.push((
                    key,
                    BindingEntry::Bind(BindingRecord {
                        target: RegisteredBinding {
                            owner,
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
        entries
    }

    pub(super) fn resolve_key_binding(&self, key: Key) -> Option<InputBinding> {
        let context = self.active_input_context().ok()?;
        let record = self.input_router.resolve(context.id, key)?;
        if record.target.owner != context.owner {
            return None;
        }
        if record.state == BindingState::Disabled {
            return Some(InputBinding::Disabled);
        }
        Some(match &record.target.target {
            BindingTarget::EditorAction(action) => InputBinding::EditorAction(*action),
            BindingTarget::EngineAction(id) => InputBinding::EngineAction(id.clone()),
            BindingTarget::EngineCommand(invocation) => InputBinding::EngineCommand {
                invocation: invocation.clone(),
                pending: record.state == BindingState::Pending,
            },
            BindingTarget::ViewCommand(command) => InputBinding::ViewCommand(command.clone()),
            BindingTarget::SessionCommand(key) => InputBinding::SessionCommand(*key),
            BindingTarget::Completion(action) => InputBinding::Completion(*action),
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

    fn command_execution(
        &self,
        invocation: crate::command::CommandInvocation,
    ) -> Result<crate::command::CommandExecution> {
        let context = self.current_view_context()?;
        let page = crate::command::CommandOwnerContext {
            view_ref: context.view_ref().to_string(),
            parameters: context.parameter_snapshot().clone(),
            binding_raw: context.parameter_raw().to_string(),
        };
        let current_fields = self
            .views
            .last()
            .context("session has no active view")?
            .engine_definition
            .current_fields;
        let owner = if invocation.source_view() == page.view_ref {
            page.clone()
        } else {
            self.views
                .last()
                .context("session has no active view")?
                .runtime
                .command_owner_context(&context, invocation.source_view())?
                .with_context(|| {
                    format!(
                        "command owner {:?} is not available in the current ViewContext",
                        invocation.source_view()
                    )
                })?
        };
        Ok(crate::command::CommandExecution {
            invocation,
            context: crate::command::CommandContext {
                page,
                owner,
                current: context.current().clone(),
                current_fields,
                runtime: self.runtime.snapshot().clone(),
            },
        })
    }

    fn dispatch_session_view_command(
        &mut self,
        invocation: crate::command::CommandInvocation,
        pending: bool,
    ) -> Result<LauncherOutcome> {
        let expected_context = self
            .views
            .last()
            .context("session has no active view")?
            .context
            .identity();
        self.dispatch_session_view_command_with_expected(invocation, pending, expected_context)
    }

    fn dispatch_session_view_command_with_expected(
        &mut self,
        invocation: crate::command::CommandInvocation,
        pending: bool,
        expected_context: crate::engine::ViewContextIdentity,
    ) -> Result<LauncherOutcome> {
        let current_context = self
            .views
            .last()
            .context("session has no active view")?
            .context
            .identity();
        if current_context != expected_context {
            self.invalidate_engine_command_layer()?;
            return Ok(LauncherOutcome::Continue);
        }
        if pending {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            if entry.state.input_rejected() {
                entry.clear_pending_command();
                return Ok(LauncherOutcome::Continue);
            }
            let source = entry.source_identity();
            entry.pending_command = Some(super::state::PendingCommand { invocation, source });
            return Ok(LauncherOutcome::Continue);
        }

        let execution = self.command_execution(invocation)?;
        self.apply_engine_decision(EngineDecision::DispatchCommand(Box::new(execution)))
    }

    fn dispatch_registered_engine_binding(
        &mut self,
        binding: crate::engine::EngineCommandInvocation,
        input: DecodedInput,
        mut pending: bool,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile {
            if let Some(effect) = self.reconcile_input()? {
                self.input_pipeline.push_front(input);
                return Ok(LauncherOutcome::Effect(Box::new(effect)));
            }
            self.refresh_active_bindings()?;
            let rebound = input.key.and_then(|key| self.resolve_key_binding(key));
            match rebound {
                Some(InputBinding::EngineCommand {
                    invocation: ref current,
                    pending: rebound_pending,
                }) if *current == binding => {
                    pending = rebound_pending;
                }
                Some(binding) => return self.dispatch_reconciled_binding(binding, input),
                None => {
                    return self.dispatch_unmatched_event(crate::input::InputEvent::Key(input));
                }
            }
        }

        let current_identity = self
            .views
            .last()
            .context("session has no active view")?
            .context
            .identity();
        if current_identity != binding.expected {
            self.invalidate_engine_command_layer()?;
            return Ok(LauncherOutcome::Continue);
        }
        let command_ref = crate::command::CommandRef {
            view: binding.command.owner.clone(),
            id: binding.command.id.clone(),
        };
        let command = self
            .config
            .view(&command_ref.view)
            .and_then(|view| view.commands.get(&command_ref.id))
            .cloned()
            .with_context(|| {
                format!(
                    "dynamic command {:?} for view {:?} disappeared before dispatch",
                    command_ref.id, command_ref.view
                )
            })?;
        anyhow::ensure!(
            command.scope == CommandScope::Selection
                || command.requires == CommandRequirement::Items,
            "dynamic command {}/{} is not an Engine-owned item command",
            command_ref.view,
            command_ref.id
        );
        let invocation = crate::command::CommandInvocation::view(command_ref, command);
        self.dispatch_session_view_command_with_expected(invocation, pending, binding.expected)
    }

    fn invalidate_engine_command_layer(&mut self) -> Result<()> {
        let layer = self
            .views
            .last()
            .context("session has no active view")?
            .input_layers
            .engine_commands;
        self.input_router
            .replace_layers(vec![(layer, Vec::new())])?;
        self.views
            .last_mut()
            .context("session has no active view")?
            .published_bindings = None;
        Ok(())
    }

    fn retry_pending_command(&mut self) -> Result<Option<ViewEffect>> {
        let pending = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            let Some(pending) = entry.pending_command.take() else {
                return Ok(None);
            };
            if pending.source != entry.source_identity() {
                return Ok(None);
            }
            pending
        };

        let current = self
            .views
            .last()
            .context("session has no active view")?
            .context
            .identity();
        let owner = pending.invocation.source_view();
        let id = pending.invocation.id();
        let Some((published_identity, enabled)) = self
            .views
            .last()
            .and_then(|entry| entry.published_bindings.as_ref())
            .and_then(|published| {
                published
                    .engine_commands
                    .iter()
                    .find(|binding| binding.command.owner == owner && binding.command.id == id)
                    .map(|binding| (published.engine_identity, binding.enabled))
            })
        else {
            return Ok(None);
        };
        if !enabled {
            // Keep the pending command until its current projection is ready.
            self.views
                .last_mut()
                .context("session has no active view")?
                .pending_command = Some(pending);
            return Ok(None);
        }
        if published_identity != current {
            self.invalidate_engine_command_layer()?;
            return Ok(None);
        }
        let execution = self.command_execution(pending.invocation)?;
        let dispatch = EngineDecision::DispatchCommand(Box::new(execution));
        match self.apply_engine_decision(dispatch)? {
            LauncherOutcome::Continue => Ok(None),
            LauncherOutcome::Effect(effect) => Ok(Some(*effect)),
        }
    }

    fn dispatch_session_binding(
        &mut self,
        key: Key,
        input: DecodedInput,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile && let Some(effect) = self.reconcile_input()? {
            self.input_pipeline.push_front(input);
            return Ok(LauncherOutcome::Effect(Box::new(effect)));
        }
        let view_ref = self
            .views
            .last()
            .context("session has no active view")?
            .view_ref
            .clone();
        let Some(invocation) = self.session_command_for_key(&view_ref, key) else {
            return Ok(LauncherOutcome::Continue);
        };
        self.dispatch_session_view_command(invocation, false)
    }

    fn dispatch_registered_view_binding(
        &mut self,
        command_ref: crate::command::CommandRef,
        input: DecodedInput,
        reconcile: bool,
    ) -> Result<LauncherOutcome> {
        if reconcile {
            if let Some(effect) = self.reconcile_input()? {
                self.input_pipeline.push_front(input);
                return Ok(LauncherOutcome::Effect(Box::new(effect)));
            }
            self.refresh_active_bindings()?;
            let binding = input.key.and_then(|key| self.resolve_key_binding(key));
            if !matches!(binding, Some(InputBinding::ViewCommand(ref key)) if *key == command_ref) {
                return match binding {
                    Some(binding) => self.dispatch_reconciled_binding(binding, input),
                    None => self.dispatch_unmatched_event(crate::input::InputEvent::Key(input)),
                };
            }
        }
        let invocation = self
            .config
            .view(&command_ref.view)
            .and_then(|view| view.commands.get(&command_ref.id))
            .cloned()
            .with_context(|| {
                format!(
                    "command {:?} for view {:?} disappeared before dispatch",
                    command_ref.id, command_ref.view
                )
            })
            .map(|command| crate::command::CommandInvocation::view(command_ref, command))?;
        self.dispatch_session_view_command(invocation, false)
    }

    fn dispatch_reconciled_binding(
        &mut self,
        binding: InputBinding,
        input: DecodedInput,
    ) -> Result<LauncherOutcome> {
        match binding {
            InputBinding::ViewCommand(key) => {
                self.dispatch_registered_view_binding(key, input, false)
            }
            InputBinding::EngineCommand {
                invocation,
                pending,
            } => self.dispatch_registered_engine_binding(invocation, input, pending, false),
            InputBinding::SessionCommand(key) => self.dispatch_session_binding(key, input, false),
            InputBinding::EditorAction(action) => self.dispatch_editor_action(action),
            InputBinding::EngineAction(id) => {
                self.dispatch_engine_invocation(ActionInvocation::new(id))
            }
            InputBinding::Completion(action) => {
                self.handle_route_action(action)?;
                Ok(LauncherOutcome::Continue)
            }
            InputBinding::Disabled => Ok(LauncherOutcome::Continue),
        }
    }

    fn dispatch_engine_invocation(
        &mut self,
        invocation: ActionInvocation,
    ) -> Result<LauncherOutcome> {
        let context = self.current_view_context()?;
        let action_id = invocation.id.clone();
        let input = EngineActionInput {
            invocation,
            context,
        };
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        {
            let entry = self.views.last().context("session has no active view")?;
            anyhow::ensure!(
                entry.engine_definition.action(&action_id).is_some(),
                "action {:?} is not declared by engine {:?}",
                action_id.as_str(),
                entry.engine_definition.kind
            );
        }
        let emission = self.views[index].runtime.action(input)?;
        self.commit_engine_emission_at(index, emission)
    }

    pub(super) fn commit_engine_emission_at(
        &mut self,
        index: usize,
        emission: crate::engine::EngineEmission,
    ) -> Result<LauncherOutcome> {
        let mut staged_runtime = staged_runtime(&self.runtime);
        let decision = emission.decision_ref().clone();
        let preflight =
            preflight_engine_decision(&decision, &mut staged_runtime, DecisionPolicy::Normal)?;
        let effect = decision_effect(&decision, &self.views[index].view_ref)?;
        let mut source = self.prepared_parent_from_frame(index, false)?;
        source.publication = emission.publication().cloned();
        source.reports = preflight.reports;
        source.work_pending = true;
        let prepared = self.prepare_active_acceptance_at(index, source, effect, &staged_runtime)?;
        self.commit_active_acceptance(prepared)
    }

    /// Drop a pending external completion when cancellation forces Session
    /// shutdown. Forced shutdown does not acknowledge an effect it will never
    /// commit; the runtime is dropped with the Session instead.
    pub(super) fn discard_pending_external_tick(&mut self) {
        self.pending_external_tick.take();
    }

    pub(super) fn acknowledge_external_tick(&mut self) -> Option<crate::engine::EngineNotice> {
        let pending = self.pending_external_tick.take()?;
        if let Some(entry) = self
            .views
            .iter_mut()
            .find(|entry| entry.mount_id == pending.mount_id)
        {
            entry.runtime.commit_external_tick();
        }
        pending.notice
    }

    pub(super) fn commit_external_tick(&mut self) {
        if let Some(notice) = self.acknowledge_external_tick() {
            self.apply_engine_notice(notice);
        }
    }

    pub(super) fn apply_external_tick_result(
        &mut self,
        index: usize,
        result: ExternalTickResult,
    ) -> Result<ViewEffect> {
        anyhow::ensure!(
            !matches!(&result.action, ExternalTickAction::Continue) || result.notice.is_none(),
            "external Continue result cannot carry a notice"
        );
        let source_view = self.views[index].view_ref.clone();
        let action = result.action;
        if !matches!(&action, ExternalTickAction::Continue) {
            let mount_id = self.views[index].mount_id;
            match &mut self.pending_external_tick {
                Some(pending) => {
                    anyhow::ensure!(
                        pending.mount_id == mount_id,
                        "another external completion is pending for mount {:?}",
                        pending.mount_id
                    );
                    anyhow::ensure!(
                        pending.action == action,
                        "external completion changed while its Host effect was pending"
                    );
                    anyhow::ensure!(
                        pending.notice == result.notice,
                        "external completion notice changed while its Host effect was pending"
                    );
                }
                None => {
                    self.pending_external_tick = Some(super::PendingExternalTick {
                        mount_id,
                        action: action.clone(),
                        notice: result.notice,
                    });
                }
            }
        }
        if let ExternalTickAction::Fail(error) = action {
            if let Some(notice) = self.acknowledge_external_tick() {
                self.apply_engine_notice(notice);
            }
            return Err(anyhow::anyhow!(error));
        }
        Ok(match action {
            ExternalTickAction::Continue => ViewEffect::Continue,
            ExternalTickAction::Return(output) => ViewEffect::Return(ViewReturn {
                source_view,
                output,
                adapter: None,
            }),
            ExternalTickAction::Close => ViewEffect::Back(None),
            ExternalTickAction::Fail(_) => unreachable!("external failure was handled above"),
        })
    }

    pub(super) fn apply_engine_decision(
        &mut self,
        decision: EngineDecision,
    ) -> Result<LauncherOutcome> {
        let decision = match decision {
            EngineDecision::ParameterPatch(request) => {
                return self.apply_parameter_patch_request(request);
            }
            decision => decision,
        };
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let source_view = self.views[index].view_ref.clone();
        let mut staged_runtime = staged_runtime(&self.runtime);
        let preflight =
            preflight_engine_decision(&decision, &mut staged_runtime, DecisionPolicy::Normal)?;
        let effect = decision_effect(&decision, &source_view)?;
        let mut source = self.prepared_parent_from_frame(index, false)?;
        source.reports = preflight.reports;
        let prepared = self.prepare_active_acceptance_at(index, source, effect, &staged_runtime)?;
        self.commit_active_acceptance(prepared)
    }

    pub(super) fn apply_engine_notice(&mut self, notice: crate::engine::EngineNotice) {
        match notice {
            crate::engine::EngineNotice::ClearError => {
                self.active_error = None;
                self.active_error_deadline = None;
            }
            crate::engine::EngineNotice::Info { view_ref, message } => {
                self.active_error = None;
                self.active_error_deadline = None;
                self.runtime_log.record(
                    crate::diagnostics::LogLevel::Info,
                    Some(&view_ref),
                    None,
                    &message,
                );
            }
            crate::engine::EngineNotice::Error { view_ref, message } => {
                let record = self.runtime_log.record(
                    crate::diagnostics::LogLevel::Error,
                    Some(&view_ref),
                    None,
                    &message,
                );
                self.active_error = Some(record);
                self.active_error_deadline = Some(Instant::now() + super::ERROR_DISPLAY_DURATION);
            }
        }
    }

    pub(super) fn defer_engine_decision(&mut self, decision: EngineDecision) -> Result<()> {
        if let LauncherOutcome::Effect(effect) = self.apply_engine_decision(decision)? {
            anyhow::ensure!(
                self.deferred_effect.is_none(),
                "multiple Engine decisions were deferred before Session could apply them"
            );
            self.deferred_effect = Some(*effect);
        }
        Ok(())
    }

    fn step_raw(&mut self, terminal: &mut Terminal, timeout: i32) -> Result<ViewEffect> {
        let context_id = self.active_input_context()?.id;
        loop {
            if self.committed_outcome.is_some() {
                return Ok(ViewEffect::Continue);
            }
            if let Some(event) = self.input_pipeline.pop_input() {
                match self.dispatch_input_event(event)? {
                    LauncherOutcome::Continue if self.committed_outcome.is_some() => {
                        return Ok(ViewEffect::Continue);
                    }
                    LauncherOutcome::Continue => {}
                    LauncherOutcome::Effect(effect) => return Ok(*effect),
                }
                if self.active_input_context()?.id != context_id {
                    return Ok(ViewEffect::Continue);
                }
            }
            if let Some(event) = self.input_pipeline.next_raw_event() {
                match self.dispatch_raw_event(event)? {
                    LauncherOutcome::Continue if self.committed_outcome.is_some() => {
                        return Ok(ViewEffect::Continue);
                    }
                    LauncherOutcome::Continue => {}
                    LauncherOutcome::Effect(effect) => return Ok(*effect),
                }
                if self.active_input_context()?.id != context_id {
                    return Ok(ViewEffect::Continue);
                }
                continue;
            }

            match self.input_pipeline.read_raw(terminal, timeout)? {
                InputRead::Data(bytes) => {
                    if bytes.is_empty() {
                        return Ok(ViewEffect::Continue);
                    }
                    continue;
                }
                InputRead::Eof => {
                    let pending = self.input_pipeline.take_raw_pending();
                    if !pending.is_empty()
                        && let LauncherOutcome::Effect(effect) =
                            self.dispatch_raw_event(RawPipelineEvent::Forward(pending))?
                    {
                        self.input_pipeline
                            .push_event_front(crate::input::InputEvent::Eof);
                        return Ok(*effect);
                    }
                    if let LauncherOutcome::Effect(effect) =
                        self.dispatch_input_event(crate::input::InputEvent::Eof)?
                    {
                        return Ok(*effect);
                    }
                    return Ok(ViewEffect::Continue);
                }
                InputRead::Timeout => {
                    if let Some(event) = self.input_pipeline.flush_raw_due()
                        && let LauncherOutcome::Effect(effect) = self.dispatch_raw_event(event)?
                    {
                        return Ok(*effect);
                    }
                    return Ok(ViewEffect::Continue);
                }
            }
        }
    }

    fn dispatch_raw_event(&mut self, event: RawPipelineEvent) -> Result<LauncherOutcome> {
        match event {
            RawPipelineEvent::Forward(bytes) => {
                self.dispatch_unmatched_event(crate::input::InputEvent::Raw(bytes))
            }
            RawPipelineEvent::Switch(input) => self.dispatch_raw_switch(input),
        }
    }

    fn dispatch_raw_switch(&mut self, input: DecodedInput) -> Result<LauncherOutcome> {
        let key = input.key.context("raw input switch has no key")?;
        let context = self.active_input_context()?;
        let receiver = context.unmatched_receiver();
        let Some(record) = self.input_router.resolve(context.id, key).cloned() else {
            return match receiver {
                Some(receiver) => self.dispatch_raw_to_receiver(receiver, &input.raw),
                None => Ok(LauncherOutcome::Continue),
            };
        };
        if record.target.owner != context.owner || record.state == BindingState::Disabled {
            return match receiver {
                Some(receiver) => self.dispatch_raw_to_receiver(receiver, &input.raw),
                None => Ok(LauncherOutcome::Continue),
            };
        }

        match record.target.target {
            BindingTarget::SessionCommand(binding_key) => {
                self.dispatch_session_binding(binding_key, input, false)
            }
            BindingTarget::ViewCommand(binding_key) => {
                self.dispatch_registered_view_binding(binding_key, input, false)
            }
            BindingTarget::EngineCommand(binding) => self.dispatch_registered_engine_binding(
                binding,
                input,
                record.state == BindingState::Pending,
                false,
            ),
            BindingTarget::EditorAction(action) => self.dispatch_editor_action(action),
            BindingTarget::EngineAction(id) => {
                self.dispatch_engine_invocation(ActionInvocation::new(id))
            }
            BindingTarget::Completion(_) => Ok(LauncherOutcome::Continue),
        }
    }

    fn handle_terminal_eof(&mut self) -> Result<LauncherOutcome> {
        let policy = self
            .views
            .last()
            .context("session has no active view")?
            .engine_definition
            .mount_policy
            .terminal_eof;
        let decision = match policy {
            TerminalEofPolicy::Exit => EngineDecision::Exit,
            TerminalEofPolicy::Close => EngineDecision::Close,
        };
        self.apply_engine_decision(decision)
    }
}

impl AppSession<'_> {
    pub(super) fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
        let route_input_available = self.route_input_available();
        let entry = self.views.last().context("session has no active view")?;
        if !entry.input_dirty {
            return Ok(None);
        }
        let current_view = entry.view_ref.clone();
        let raw_input = entry.input.raw.clone();
        let cursor = entry.input.cursor;

        if !route_input_available {
            return self.commit_parameter_input(raw_input, entry.input.clone());
        }

        let resolution = self.router.resolve(&current_view, &raw_input);
        match resolution {
            crate::router::RouteResolution::Navigate {
                target,
                query: parameter_input,
            } => {
                let parameter_cursor = cursor
                    .saturating_sub(raw_input.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                Ok(Some(ViewEffect::Navigate {
                    request: NavigationRequest::routed(target, parameter_input, parameter_cursor),
                    mode: NavigationMode::Push,
                    parent_edit: Some(InputEdit::SetBuffer {
                        raw: String::new(),
                        cursor: 0,
                    }),
                }))
            }
            crate::router::RouteResolution::Current {
                query: parameter_input,
            } => {
                let parameter_cursor = cursor
                    .saturating_sub(raw_input.len().saturating_sub(parameter_input.len()))
                    .min(parameter_input.len());
                let candidate_input = self
                    .views
                    .last()
                    .context("session has no active view")?
                    .input
                    .replaced_all(parameter_input.clone(), parameter_cursor)
                    .map_err(|error| anyhow::anyhow!("candidate input was rejected: {error}"))?;
                self.commit_parameter_input(parameter_input, candidate_input)
            }
            crate::router::RouteResolution::NotMatched => {
                self.commit_parameter_input(raw_input, entry.input.clone())
            }
        }
    }

    fn commit_parameter_input(
        &mut self,
        params: String,
        candidate_input: crate::input::EditorBuffer,
    ) -> Result<Option<ViewEffect>> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let (mut candidate_state, binding, buffer_generation, refresh_policy) = {
            let entry = &self.views[index];
            (
                entry.state.clone(),
                entry.parameter_binding.clone(),
                entry.buffer_generation,
                entry.engine_definition.mount_policy.refresh,
            )
        };
        if let Err(error) = binding.parse_input(&mut candidate_state, &params) {
            return self.reject_input(&error.to_string());
        }
        candidate_state.set_input_rejected(false);

        let mut staged_runtime = super::decision::staged_runtime(&self.runtime);
        super::publication::publish_active_input(
            &mut staged_runtime,
            &candidate_input,
            candidate_state.raw_input(),
            &candidate_state,
        )?;
        let context = self.view_context_at(
            index,
            &candidate_input,
            &candidate_state,
            buffer_generation,
            &staged_runtime,
        )?;
        let committed = self.dispatch_engine_normal_at(index, &mut staged_runtime, |runtime| {
            runtime.input_committed(context.clone())
        })?;
        let mut reports = committed.reports;
        let mut effect = committed.effect;
        let mut publication = committed.publication;

        if effect.is_none() {
            let parameters = context.parameter_snapshot().clone();
            let prepared =
                self.dispatch_engine_normal_at(index, &mut staged_runtime, |runtime| {
                    runtime.parameters(parameters, context.identity())
                })?;
            reports.extend(prepared.reports);
            effect = prepared.effect;
            if prepared.publication.is_some() {
                publication = prepared.publication;
            }
        }

        let mut source = self.prepared_parent_from_frame(index, true)?;
        source.parameters = context.parameter_snapshot().clone();
        source.state = candidate_state;
        source.input = candidate_input;
        source.committed_buffer_projection = source.input.clone();
        source.input_dirty = false;
        source.input_deadline = match refresh_policy {
            InputRefreshPolicy::None => None,
            InputRefreshPolicy::Debounced(duration) => Some(Instant::now() + duration),
        };
        source.reports = reports;
        source.publication = publication;
        source.work_pending = true;
        let prepared = self.prepare_active_commit_at(index, source, effect, &mut staged_runtime)?;
        Ok(match self.commit_active(prepared) {
            LauncherOutcome::Effect(effect) => Some(*effect),
            LauncherOutcome::Continue => None,
        })
    }

    fn reject_input(&mut self, message: &str) -> Result<Option<ViewEffect>> {
        let view_ref = self
            .views
            .last()
            .context("session has no active view")?
            .view_ref
            .clone();
        self.reject_input_for(&view_ref, message)
    }

    pub(super) fn reject_input_for(
        &mut self,
        report_view_ref: &str,
        message: &str,
    ) -> Result<Option<ViewEffect>> {
        let index = self
            .views
            .len()
            .checked_sub(1)
            .context("session has no active view")?;
        let mut candidate_state = self.views[index].state.clone();
        candidate_state.set_input_rejected(true);
        let input = self.views[index].input.clone();
        let mut staged_runtime = super::decision::staged_runtime(&self.runtime);
        let rejected_context = self.view_context_at(
            index,
            &input,
            &candidate_state,
            self.views[index].buffer_generation,
            &staged_runtime,
        )?;
        let prepared = self.dispatch_engine_normal_at(index, &mut staged_runtime, |runtime| {
            runtime.input_rejected(rejected_context.identity())
        })?;
        let reports = prepared.reports;
        let effect = prepared.effect;
        let publication = prepared.publication;
        let mut source = self.prepared_parent_from_frame(index, true)?;
        source.state = candidate_state;
        source.input_dirty = false;
        source.input_deadline = None;
        source.reports.push(crate::engine::EngineNotice::Error {
            view_ref: report_view_ref.to_string(),
            message: message.to_string(),
        });
        source.reports.extend(reports);
        source.publication = publication;
        source.work_pending = true;
        let prepared = self.prepare_active_commit_at(index, source, effect, &mut staged_runtime)?;
        Ok(match self.commit_active(prepared) {
            LauncherOutcome::Effect(effect) => Some(*effect),
            LauncherOutcome::Continue => None,
        })
    }
}

const DEFAULT_INPUT_POLL_MS: i32 = 40;

fn input_poll_timeout(engine: Option<i32>, input: Option<i32>) -> i32 {
    match (engine, input) {
        (Some(engine), Some(input)) => engine.min(input),
        (engine, input) => engine.or(input).unwrap_or(DEFAULT_INPUT_POLL_MS),
    }
}

fn input_timeout_until(deadline: Instant) -> i32 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .max(1)
        .min(i32::MAX as u128) as i32
}

pub(super) fn apply_editor_key(input: &mut EditorBuffer, key: Key) -> Option<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    struct MissingRawRuntime;

    impl crate::engine::EngineRuntime for MissingRawRuntime {
        fn tick_mode(&self) -> crate::engine::EngineTickMode {
            crate::engine::EngineTickMode::Prepared
        }

        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("missing-raw", ())
        }
    }

    #[test]
    fn advertised_raw_receiver_must_remain_available() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            crate::engine::EngineRegistry::new(),
        )
        .unwrap();
        let receiver = crate::input::ReceiverId(session.views[0].mount_id.0);
        session.views[0].raw_receiver = Some(receiver);
        session.views[0].runtime = Box::new(MissingRawRuntime);

        let error = match session.dispatch_raw_to_receiver(receiver, b"input") {
            Ok(_) => panic!("an advertised raw receiver cannot disappear"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("no longer provides"));
        assert!(error.to_string().contains(&format!("{:?}", receiver)));
    }

    #[test]
    fn input_poll_timeout_uses_host_fallback_and_earliest_deadline() {
        assert_eq!(input_poll_timeout(None, None), DEFAULT_INPUT_POLL_MS);
        assert_eq!(input_poll_timeout(Some(80), None), 80);
        assert_eq!(input_poll_timeout(None, Some(12)), 12);
        assert_eq!(input_poll_timeout(Some(80), Some(12)), 12);
    }

    struct AcceptedCandidateRuntime {
        accepted: Option<CompletionCandidate>,
        rendered: CompletionCandidate,
    }

    impl super::super::completion::TransientSelectionRuntime for AcceptedCandidateRuntime {
        fn dispatch(&mut self, input: SelectionInput) -> Result<SelectionDecision> {
            anyhow::ensure!(
                matches!(input, SelectionInput::Accept),
                "test runtime only accepts completion"
            );
            Ok(SelectionDecision::Accepted(self.accepted.clone()))
        }

        fn render_model(&self) -> super::super::completion::SelectionRenderModel {
            super::super::completion::SelectionRenderModel {
                candidates: vec![self.rendered.clone()],
                selected: 0,
            }
        }
    }

    fn completion_candidate(view_ref: &str) -> CompletionCandidate {
        CompletionCandidate {
            view_ref: view_ref.to_string(),
            alias: None,
            plugin_name: "test".to_string(),
            engine_type: "picker".to_string(),
        }
    }

    #[test]
    fn completion_accept_uses_the_candidate_returned_by_the_runtime() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            crate::engine::EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].input = EditorBuffer::new("alias query");
        let source_identity = session.views[0].source_identity();
        let source_revision = session.views[0].input.revision;
        session.route_completion = Some(CompletionState {
            host: crate::input::CompletionHost {
                parent: crate::input::CompletionSnapshot {
                    source_identity,
                    buffer_revision: source_revision,
                    replacement_range: 0..5,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime: Box::new(AcceptedCandidateRuntime {
                accepted: Some(completion_candidate("accepted:main")),
                rendered: completion_candidate("rendered:main"),
            }),
        });

        assert!(session.handle_route_action(RouteAction::Accept).unwrap());
        assert_eq!(session.views[0].input.raw, "accepted:main query");
        assert!(session.route_completion.is_none());
    }

    #[test]
    fn stale_completion_accept_closes_without_overwriting_newer_input() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            crate::engine::EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].input = EditorBuffer::new("alias query");
        let source_identity = session.views[0].source_identity();
        let source_revision = session.views[0].input.revision;
        session.route_completion = Some(CompletionState {
            host: crate::input::CompletionHost {
                parent: crate::input::CompletionSnapshot {
                    source_identity,
                    buffer_revision: source_revision,
                    replacement_range: 0..5,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime: Box::new(AcceptedCandidateRuntime {
                accepted: Some(completion_candidate("accepted:main")),
                rendered: completion_candidate("rendered:main"),
            }),
        });

        session.views[0].input.insert('!');
        let edited_input = session.views[0].input.clone();
        assert_ne!(edited_input.revision, source_revision);

        assert!(!session.handle_route_action(RouteAction::Accept).unwrap());
        assert_eq!(session.views[0].input.raw, edited_input.raw);
        assert_eq!(session.views[0].input.revision, edited_input.revision);
        assert!(session.route_completion.is_none());
    }

    #[test]
    fn completion_accept_without_a_candidate_does_not_edit() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            crate::engine::EngineRegistry::new(),
        )
        .unwrap();
        session.views[0].input = EditorBuffer::new("alias query");
        let source_identity = session.views[0].source_identity();
        let source_revision = session.views[0].input.revision;
        session.route_completion = Some(CompletionState {
            host: crate::input::CompletionHost {
                parent: crate::input::CompletionSnapshot {
                    source_identity,
                    buffer_revision: source_revision,
                    replacement_range: 0..5,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime: Box::new(AcceptedCandidateRuntime {
                accepted: None,
                rendered: completion_candidate("rendered:main"),
            }),
        });

        assert!(!session.handle_route_action(RouteAction::Accept).unwrap());
        assert_eq!(session.views[0].input.raw, "alias query");
        assert!(session.route_completion.is_none());
    }

    #[test]
    fn completion_edit_invalidates_pending_command_identity() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            crate::engine::EngineRegistry::new(),
        )
        .unwrap();
        let command = config
            .commands
            .bindings
            .get("commands")
            .and_then(|binding| binding.as_command("commands"))
            .expect("fixture must define the Commands binding");
        let source = session.views[0].source_identity();
        session.views[0].pending_command = Some(super::super::state::PendingCommand {
            invocation: crate::command::CommandInvocation::session_command(
                "core:default",
                "commands",
                command,
            ),
            source,
        });
        let generation = session.views[0].buffer_generation;
        let edit = CompletionEdit {
            source_identity: source,
            source_revision: session.views[0].input.revision,
            range: 0..0,
            replacement: "x".to_string(),
            cursor: 1,
        };

        assert!(session.apply_completion_edit(edit).unwrap());
        assert_eq!(session.views[0].buffer_generation, generation + 1);
        assert!(session.views[0].pending_command.is_none());
        assert!(session.views[0].input_dirty);
    }
}
