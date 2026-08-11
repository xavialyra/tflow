use super::{
    CommandInvocation, CompletionRequest, EngineHost, EngineRegistry, InputRefreshPolicy,
    InputSeed, NavigationMode, NavigationRequest, TaskScheduler, ViewContext, ViewEffect,
    ViewInstance,
};
use crate::chrome::InputBuffer;
use crate::config::{Config, ENGINE_PICKER};
use crate::engine::api::{EditorAction, InputEdit, LauncherOutcome, ResolvedLauncherAction};
use crate::input::{InputDecoder, Key};
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::state::StateInstance;
use crate::terminal::Terminal;
use crate::text::sanitize_terminal_text;
use anyhow::{Context, Result};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy)]
struct QueuedKey {
    key: Key,
    replayed: bool,
}

impl QueuedKey {
    fn new(key: Key) -> Self {
        Self {
            key,
            replayed: false,
        }
    }
}

struct ViewEntry {
    view_ref: String,
    route_child: bool,
    input: InputBuffer,
    input_dirty: bool,
    input_deadline: Option<Instant>,
    state: StateInstance,
    instance: Box<dyn ViewInstance>,
}

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    engines: EngineRegistry,
    views: Vec<ViewEntry>,
    tasks: TaskScheduler,
    runtime: super::RuntimeStore,
    runtime_log: RuntimeLog,
    router: Arc<crate::router::Router>,
    route_input: bool,
    decoder: InputDecoder,
    pending_keys: VecDeque<QueuedKey>,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
}

#[derive(Debug, Clone)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(Box<CompletionRequest>),
}

impl<'a> AppSession<'a> {
    pub(crate) fn new(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
    ) -> Result<Self> {
        let mut runtime = super::RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let router = Arc::new(crate::router::Router::new(config));
        let view_ref = config.default_view.clone();
        let state = config.instantiate_state(&view_ref)?;
        let initial_input = sanitize_terminal_text(&config.render_query_input(&state)?);
        let request = NavigationRequest::new(&view_ref, initial_input);
        let input = input_buffer_from_seed(
            request
                .input
                .as_ref()
                .context("root navigation request has no input seed")?,
        );
        publish_location(&mut runtime, &view_ref, &input, &state)?;
        let root = engines.create_view(ViewContext {
            config,
            request: &request,
            input: &input,
            state: &state,
            log_file: runtime_log.path(),
            runtime: runtime.handle(),
            tasks: tasks.clone(),
        })?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewEntry {
                view_ref,
                route_child: false,
                input,
                input_dirty: false,
                input_deadline: None,
                state,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: true,
            decoder: InputDecoder::default(),
            pending_keys: VecDeque::new(),
            active_error: None,
            active_error_deadline: None,
        })
    }

    pub(crate) fn single_root(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        view_ref: &str,
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
        publish_location(&mut runtime, view_ref, &input, &state)?;
        let root = engines.create_view(ViewContext {
            config,
            request: &request,
            input: &input,
            state: &state,
            log_file: runtime_log.path(),
            runtime: runtime.handle(),
            tasks: tasks.clone(),
        })?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewEntry {
                view_ref: view_ref.to_string(),
                route_child: false,
                input,
                input_dirty: false,
                input_deadline: None,
                state,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: false,
            decoder: InputDecoder::default(),
            pending_keys: VecDeque::new(),
            active_error: None,
            active_error_deadline: None,
        })
    }

    pub(crate) fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        loop {
            self.clear_expired_error();
            let effect = self.step(terminal)?;
            if let Some(outcome) = self.apply(effect)? {
                return Ok(outcome);
            }
            self.render(terminal)?;
        }
    }

    fn step(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let effect = self.dispatch_input_ready()?;
        let effect = self.dispatch_view_effect(effect, terminal)?;
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
                input: &mut entry.input,
                state: &mut entry.state,
                runtime: &mut self.runtime,
                runtime_log: &mut self.runtime_log,
                active_error: &mut self.active_error,
                active_error_deadline: &mut self.active_error_deadline,
            };
            entry.instance.step(&mut host, terminal)?
        };
        let effect = self.dispatch_view_effect(effect, terminal)?;
        if !matches!(effect, ViewEffect::Continue) {
            return Ok(effect);
        }

        let timeout = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            let input_timeout = entry.input_deadline.map(input_timeout_until);
            let host = EngineHost {
                config: self.config,
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
        if let Some(timeout) = timeout {
            if self.pending_keys.is_empty() {
                let bytes = terminal.read_input(timeout)?;
                self.pending_keys
                    .extend(self.decoder.feed(&bytes).into_iter().map(QueuedKey::new));
                self.pending_keys
                    .extend(self.decoder.flush_due().into_iter().map(QueuedKey::new));
            }
            while let Some(queued_key) = self.pending_keys.pop_front() {
                let key = queued_key.key;
                let captures_editor_input = self
                    .views
                    .last()
                    .context("session has no active view")?
                    .instance
                    .captures_editor_input();
                if matches!(key, Key::Char(character) if !character.is_control())
                    && !captures_editor_input
                {
                    let changed = apply_editor_key(
                        &mut self
                            .views
                            .last_mut()
                            .context("session has no active view")?
                            .input,
                        key,
                    )
                    .expect("printable characters are editor input");
                    if changed {
                        self.mark_input_changed()?;
                    }
                    continue;
                }

                let action = {
                    let entry = self
                        .views
                        .last_mut()
                        .context("session has no active view")?;
                    let host = EngineHost {
                        config: self.config,
                        input: &mut entry.input,
                        state: &mut entry.state,
                        runtime: &mut self.runtime,
                        runtime_log: &mut self.runtime_log,
                        active_error: &mut self.active_error,
                        active_error_deadline: &mut self.active_error_deadline,
                    };
                    entry.instance.resolve_launcher_action(&host, key)
                };
                let outcome = match action {
                    Some(ResolvedLauncherAction::Edit(action)) => {
                        let entry = self
                            .views
                            .last_mut()
                            .context("session has no active view")?;
                        let edited = match action {
                            EditorAction::DeleteBackward => entry.input.delete_backward(),
                            EditorAction::ClearInput => entry.input.clear(),
                            EditorAction::DeleteWord => entry.input.delete_word(),
                        };
                        if edited {
                            self.mark_input_changed()?;
                        }
                        LauncherOutcome::Continue
                    }
                    Some(ResolvedLauncherAction::View(action)) => {
                        if let Some(effect) = self.reconcile_input()? {
                            self.pending_keys.push_front(queued_key);
                            LauncherOutcome::Effect(Box::new(effect))
                        } else {
                            let entry = self
                                .views
                                .last_mut()
                                .context("session has no active view")?;
                            let mut host = EngineHost {
                                config: self.config,
                                input: &mut entry.input,
                                state: &mut entry.state,
                                runtime: &mut self.runtime,
                                runtime_log: &mut self.runtime_log,
                                active_error: &mut self.active_error,
                                active_error_deadline: &mut self.active_error_deadline,
                            };
                            entry
                                .instance
                                .handle_launcher_action(&mut host, action, key)?
                        }
                    }
                    None => {
                        if let Some(changed) = {
                            let entry = self
                                .views
                                .last_mut()
                                .context("session has no active view")?;
                            apply_editor_key(&mut entry.input, key)
                        } && changed
                        {
                            self.mark_input_changed()?;
                        }
                        LauncherOutcome::Continue
                    }
                };
                match outcome {
                    LauncherOutcome::Continue => {}
                    LauncherOutcome::EditInput(edit) => self.apply_input_edit(edit)?,
                    LauncherOutcome::ReplayKey(key) => {
                        anyhow::ensure!(
                            !queued_key.replayed,
                            "active view replayed launcher key {key:?} more than once"
                        );
                        self.pending_keys.push_front(QueuedKey {
                            key,
                            replayed: true,
                        });
                    }
                    LauncherOutcome::Effect(effect) => {
                        let effect = self.dispatch_view_effect(*effect, terminal)?;
                        if !matches!(effect, ViewEffect::Continue) {
                            return Ok(effect);
                        }
                    }
                }
            }
        }

        Ok(self.reconcile_input()?.unwrap_or(ViewEffect::Continue))
    }

    fn dispatch_view_effect(
        &mut self,
        effect: ViewEffect,
        terminal: &mut Terminal,
    ) -> Result<ViewEffect> {
        let ViewEffect::RunCommand {
            invocation,
            prepared,
            exit,
            return_to_parent,
        } = effect
        else {
            return Ok(effect);
        };

        terminal.leave()?;
        let status = prepared.command().status();
        if !exit {
            terminal.reenter()?;
        }
        self.record_command_result(&invocation, status);
        if exit {
            Ok(ViewEffect::Exit)
        } else if return_to_parent {
            Ok(ViewEffect::Back(None))
        } else {
            Ok(ViewEffect::Continue)
        }
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

    fn active_uses_launcher_input(&mut self) -> Result<bool> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let host = EngineHost {
            config: self.config,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        Ok(entry.instance.launcher_input_timeout(&host).is_some())
    }

    fn dispatch_input_ready(&mut self) -> Result<ViewEffect> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if !entry
            .input_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Ok(ViewEffect::Continue);
        }
        entry.input_deadline = None;
        let mut host = EngineHost {
            config: self.config,
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
            InputEdit::ReplaceRange {
                start,
                end,
                replacement,
                cursor,
            } => {
                anyhow::ensure!(
                    start <= end
                        && end <= entry.input.raw.len()
                        && entry.input.raw.is_char_boundary(start)
                        && entry.input.raw.is_char_boundary(end),
                    "input edit range {start}..{end} is invalid for the active buffer"
                );
                entry.input.replace_range(start, end, &replacement);
                entry.input.set_cursor(cursor);
            }
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

    fn render(&mut self, terminal: &Terminal) -> Result<()> {
        let show_route_label = self.views.len() > 1;
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let route = self.router.display(&entry.view_ref);
        let error = self
            .active_error
            .as_ref()
            .map(|record| record.label.clone());
        let input_text = entry.input.raw.clone();
        let input_cursor = entry.input.cursor;
        let host = EngineHost {
            config: self.config,
            input: &mut entry.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let engine_chrome = entry.instance.chrome(&host);
        let chrome = crate::chrome::ChromeFrame::compose_with_cursor(
            terminal.size().0 as usize,
            &route,
            show_route_label,
            &input_text,
            input_cursor,
            engine_chrome,
            error.as_deref(),
        );
        let content = entry.instance.content(&host, terminal, &chrome)?;
        chrome.render(terminal, content)
    }

    fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        if !entry.input_dirty {
            return Ok(None);
        }
        entry.input_dirty = false;
        let current_view = entry.view_ref.clone();
        let route_child = entry.route_child;
        let raw_input = entry.input.raw.clone();
        let cursor = entry.input.cursor;

        if !self.route_input
            || current_view == self.config.command_view
            || self.config.engine(&current_view)? != ENGINE_PICKER
        {
            self.commit_query_input(raw_input)?;
            return Ok(None);
        }

        match self.router.resolve(&current_view, &raw_input) {
            crate::router::RouteResolution::Navigate { target, query } => {
                Ok(Some(ViewEffect::Navigate {
                    request: NavigationRequest::routed(target, raw_input, query, cursor),
                    mode: if route_child {
                        NavigationMode::Replace
                    } else {
                        NavigationMode::Push
                    },
                }))
            }
            crate::router::RouteResolution::Current { query } => {
                self.commit_query_input(query)?;
                Ok(None)
            }
            crate::router::RouteResolution::NotMatched if route_child => {
                Ok(Some(ViewEffect::Back(Some(InputEdit::SetBuffer {
                    raw: raw_input,
                    cursor,
                }))))
            }
            crate::router::RouteResolution::NotMatched => {
                self.commit_query_input(raw_input)?;
                Ok(None)
            }
            crate::router::RouteResolution::Ambiguous { alias, targets } => {
                let message = format!(
                    "view alias {:?} is ambiguous: {}",
                    alias,
                    targets.join(", ")
                );
                self.reject_input(&message)?;
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

    fn apply(&mut self, effect: ViewEffect) -> Result<Option<SessionOutcome>> {
        match effect {
            ViewEffect::Continue => Ok(None),
            ViewEffect::Exit => Ok(Some(SessionOutcome::Exited)),
            ViewEffect::Complete(completion) => {
                Ok(Some(SessionOutcome::Completed(Box::new(completion))))
            }
            ViewEffect::RunCommand { .. } => {
                anyhow::bail!("run command effect reached the navigation dispatcher")
            }
            ViewEffect::Back(edit) => self.pop_current(edit),
            ViewEffect::Navigate { request, mode } => {
                self.deactivate_current()?;
                let mut state = self.config.instantiate_state(&request.view_ref)?;
                let input = match initialize_navigation_input(
                    self.config,
                    &mut state,
                    request.input.as_ref(),
                ) {
                    Ok(input) => input,
                    Err(error) => {
                        self.activate_current()?;
                        self.reject_navigation_error(&request.view_ref, &error.to_string())?;
                        self.restore_current_input()?;
                        return Ok(None);
                    }
                };
                publish_location(&mut self.runtime, &request.view_ref, &input, &state)?;
                let view = self.engines.create_view(ViewContext {
                    config: self.config,
                    request: &request,
                    input: &input,
                    state: &state,
                    log_file: self.runtime_log.path(),
                    runtime: self.runtime.handle(),
                    tasks: self.tasks.clone(),
                });
                let view = match view {
                    Ok(view) => view,
                    Err(error) => {
                        self.activate_current()?;
                        self.reject_navigation_error(&request.view_ref, &error.to_string())?;
                        self.restore_current_input()?;
                        return Ok(None);
                    }
                };
                if mode == NavigationMode::Replace {
                    self.views.pop();
                }
                self.views.push(ViewEntry {
                    view_ref: request.view_ref,
                    route_child: request.route_child,
                    input,
                    input_dirty: false,
                    input_deadline: None,
                    state,
                    instance: view,
                });
                if !self.active_uses_launcher_input()? {
                    self.pending_keys.clear();
                    self.decoder = InputDecoder::default();
                }
                Ok(None)
            }
        }
    }

    fn pop_current(&mut self, edit: Option<InputEdit>) -> Result<Option<SessionOutcome>> {
        if self.views.len() <= 1 {
            let Some(edit) = edit else {
                return Ok(Some(SessionOutcome::Exited));
            };
            self.apply_input_edit(edit)?;
            return match self.reconcile_input()? {
                Some(effect) => self.apply(effect),
                None => Ok(None),
            };
        }

        let returned_from_route_child = self
            .views
            .last()
            .context("session has no active view")?
            .route_child;
        self.deactivate_current()?;
        self.views.pop();
        let Some(edit) = edit else {
            if returned_from_route_child {
                self.remove_route_separator()?;
            }
            self.activate_current()?;
            self.restore_current_input()?;
            return Ok(None);
        };

        self.apply_input_edit(edit)?;
        self.publish_current_location()?;
        let effect = self.reconcile_input()?;
        self.activate_current()?;
        match effect {
            Some(effect) => self.apply(effect),
            None => Ok(None),
        }
    }

    fn remove_route_separator(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let separator_start = entry.input.raw.trim_end_matches(char::is_whitespace).len();
        entry.input.raw.truncate(separator_start);
        entry
            .input
            .set_cursor(entry.input.cursor.min(separator_start));
        Ok(())
    }

    fn restore_current_input(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let mut host = EngineHost {
            config: self.config,
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
) -> Result<InputBuffer> {
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
        ("/view/active/state_revision", json!(state.revision())),
        ("/view/active/input", json!(input.params)),
        ("/view/active/raw_input", json!(input.raw)),
        ("/view/active/query", json!(input.params)),
        (
            "/view/active/request",
            json!({
                "input": input.params,
                "raw_input": input.raw,
                "query": input.params,
            }),
        ),
        (
            "/session/input",
            json!({"raw": input.raw, "params": input.params}),
        ),
    ])?;
    Ok(())
}

fn publish_location(
    runtime: &mut super::RuntimeStore,
    view_ref: &str,
    input: &InputBuffer,
    state: &StateInstance,
) -> Result<()> {
    runtime.set_many([
        (
            "/view",
            json!({
                "active": {
                    "ref": view_ref,
                    "state_revision": state.revision(),
                    "input": input.params,
                    "raw_input": input.raw,
                    "query": input.params,
                    "request": {
                        "input": input.params,
                        "raw_input": input.raw,
                        "query": input.params,
                    }
                }
            }),
        ),
        (
            "/session",
            json!({
                "input": {
                    "raw": input.raw,
                    "params": input.params,
                }
            }),
        ),
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

        fn content(
            &mut self,
            _host: &EngineHost<'_>,
            _terminal: &Terminal,
            _chrome: &crate::chrome::ChromeFrame,
        ) -> Result<crate::chrome::ChromeContent> {
            Ok(crate::chrome::ChromeContent::default())
        }
    }

    #[test]
    fn routed_navigation_preserves_the_active_cursor() {
        let config = Config::load(std::path::Path::new("config/config.toml")).unwrap();
        let mut session = AppSession::new(
            &config,
            crate::runtime_log::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let entry = session.views.last_mut().unwrap();
        entry.input.raw = "app query".to_string();
        entry.input.cursor = 3;
        session.mark_input_changed().unwrap();

        let effect = session.reconcile_input().unwrap().unwrap();
        let ViewEffect::Navigate { request, .. } = &effect else {
            panic!("route input did not produce navigation");
        };
        assert_eq!(request.input.as_ref().unwrap().cursor, 3);
        assert!(session.apply(effect).unwrap().is_none());
        assert_eq!(session.views.last().unwrap().input.cursor, 3);
    }

    #[test]
    fn returning_from_routed_view_removes_the_route_separator() {
        let config = Config::load(std::path::Path::new("config/config.toml")).unwrap();
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
        assert!(session.apply(effect).unwrap().is_none());
        assert!(session.views.last().unwrap().route_child);
        assert!(session.pop_current(None).unwrap().is_none());

        let input = &session.views.last().unwrap().input;
        assert_eq!(input.raw, "app");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn edited_back_commits_before_activation_without_restoring_stale_input() {
        let config = Config::load(std::path::Path::new("config/config.toml")).unwrap();
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
                .apply(ViewEffect::Navigate {
                    request: NavigationRequest::new("core:messages", ""),
                    mode: NavigationMode::Push,
                })
                .unwrap()
                .is_none()
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
        let path = root.join("config.toml");
        std::fs::write(
            &path,
            r#"
            default_view = "core:default"

            [plugins.core.views.default]
            type = "picker"

            [plugins.core.views.default.query]
            type = "object"
            input_order = ["count"]
            count = '''{{ state("integer", 1) }}'''
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
        assert_eq!(session.runtime.snapshot()["view"]["active"]["input"], "2");

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
        assert_eq!(session.runtime.snapshot()["view"]["active"]["input"], "2");
        assert_eq!(
            session.runtime.snapshot()["view"]["active"]["raw_input"],
            "2"
        );

        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publishing_a_location_preserves_unrelated_runtime_data() {
        let mut runtime = super::super::RuntimeStore::new();
        runtime
            .set("/invocation_marker", json!({"items": [1, 2]}))
            .unwrap();

        let config = Config::load(std::path::Path::new("config/config.toml")).unwrap();
        let state = config.instantiate_state("core:default").unwrap();
        let input = InputBuffer::new("query");
        publish_location(&mut runtime, "core:default", &input, &state).unwrap();

        assert_eq!(
            runtime.snapshot()["invocation_marker"]["items"],
            json!([1, 2])
        );
        assert_eq!(runtime.snapshot()["view"]["active"]["query"], "query");
    }
}
