mod chrome;
mod effects;
mod input_dispatch;
mod navigation;
#[cfg(test)]
use chrome::render_route_completion;
#[cfg(test)]
use input_dispatch::{apply_editor_key, route_completion_edit};
pub(crate) mod input;
mod publication;
mod state;

use publication::{publish_location, publish_view_catalog};
#[cfg(test)]
use state::{InputBinding, ReturnTransition, RouteAction};
use state::{RegisteredBinding, RouteCompletion, ViewEntry, mount_view_input_layers};

use self::input::CommandSession;
#[cfg(test)]
use crate::command::{CallRequest, CommandOrigin};
#[cfg(test)]
use crate::command::{CommandContext, LauncherOutcome};
use crate::command::{InputSeed, NavigationRequest, ViewReturn};
#[cfg(test)]
use crate::command::{NavigationMode, ViewEffect};
use crate::config::{Config, EvaluationSnapshot, InvocationScope, OwnerViewScope, SessionScope};
use crate::diagnostics::{LogRecord, RuntimeLog};
#[cfg(test)]
use crate::engine::InputEdit;
#[cfg(test)]
use crate::engine::{EngineHost, EngineRegistry, EngineTerminal, ViewInstance};
use crate::engine::{ViewContext, ViewFactory};
#[cfg(test)]
use crate::input::DecodedInput;
use crate::input::InputBuffer;
#[cfg(test)]
use crate::input::Key;
#[cfg(test)]
use crate::input::keymap::InputContextId;
use crate::input::keymap::InputRouter;
use crate::lifecycle::CancellationToken;
use crate::state::StateInstance;
use crate::task::TaskRuntime;
use crate::terminal::Terminal;
use crate::terminal::sanitize_terminal_text;
use crate::theme::ResolvedTheme;
use anyhow::{Context, Result};
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    theme: ResolvedTheme,
    view_factory: Box<dyn ViewFactory>,
    views: Vec<ViewEntry>,
    tasks: TaskRuntime,
    runtime: crate::runtime::RuntimeStore,
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

impl<'a> AppSession<'a> {
    #[cfg(test)]
    pub(crate) fn new<V>(config: &'a Config, runtime_log: RuntimeLog, engines: V) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let cancellation = CancellationToken::new();
        Self::new_with_theme(
            config,
            ResolvedTheme::terminal(),
            runtime_log,
            engines,
            &cancellation,
        )
    }

    pub(crate) fn new_with_theme<V>(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: V,
        cancellation: &CancellationToken,
    ) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let view_factory: Box<dyn ViewFactory> = Box::new(engines);
        let mut runtime = crate::runtime::RuntimeStore::new();
        let tasks = TaskRuntime::new(runtime.handle());
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
        let root = match view_factory.create_view(root_context) {
            Ok(root) => root,
            Err(error) => {
                tasks.shutdown_and_wait();
                return Err(error);
            }
        };
        let mut input_router = InputRouter::default();
        let input_layers = mount_view_input_layers(&mut input_router);
        Ok(Self {
            config,
            theme,
            view_factory,
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

    pub(crate) fn single_root_with_theme<V>(
        config: &'a Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: V,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self>
    where
        V: ViewFactory + 'static,
    {
        let view_factory: Box<dyn ViewFactory> = Box::new(engines);
        let mut runtime = crate::runtime::RuntimeStore::new();
        let tasks = TaskRuntime::new(runtime.handle());
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
        let root = match view_factory.create_view(ViewContext {
            config,
            request: &request,
            input: &input,
            state: &state,
            evaluation,
            tasks: tasks.clone(),
            cancellation: cancellation.clone(),
        }) {
            Ok(root) => root,
            Err(error) => {
                tasks.shutdown_and_wait();
                return Err(error);
            }
        };
        let mut input_router = InputRouter::default();
        let input_layers = mount_view_input_layers(&mut input_router);
        Ok(Self {
            config,
            theme,
            view_factory,
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

    fn surface_runtime_log_warning(&mut self) {
        if let Some(record) = self.runtime_log.take_warning_record() {
            self.runtime_warning = Some(record.message.clone());
            self.active_error = Some(record);
            self.active_error_deadline =
                Some(Instant::now() + crate::engine::ERROR_DISPLAY_DURATION);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandRef;
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
                .push(format!("{event}:{}", host.input_raw()));
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

        fn handle_unbound_input(
            &mut self,
            _host: &mut EngineHost<'_>,
            bytes: &[u8],
        ) -> Result<LauncherOutcome> {
            self.events
                .lock()
                .unwrap()
                .push(format!("unbound:{bytes:?}"));
            Ok(LauncherOutcome::Continue)
        }

        fn step(
            &mut self,
            _host: &mut EngineHost<'_>,
            _terminal: &mut dyn EngineTerminal,
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
            self.record(&format!("commit:{view_ref}:{}", host.input_raw()));
            if self.fail_commit {
                anyhow::bail!("parent commit failed");
            }
            Ok(())
        }

        fn step(
            &mut self,
            _host: &mut EngineHost<'_>,
            _terminal: &mut dyn EngineTerminal,
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
            page: crate::engine::CommandOwnerContext {
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
            crate::diagnostics::RuntimeLog::disabled(),
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
                output: crate::engine::ViewOutput::Value {
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
            crate::diagnostics::RuntimeLog::disabled(),
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
                output: crate::engine::ViewOutput::Value {
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
                output: crate::engine::ViewOutput::Value {
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
            crate::diagnostics::RuntimeLog::disabled(),
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
                output: crate::engine::ViewOutput::Value {
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
                crate::diagnostics::RuntimeLog::disabled(),
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
                crate::diagnostics::RuntimeLog::disabled(),
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
            crate::diagnostics::RuntimeLog::disabled(),
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
    fn route_completion_consumes_unbound_control_without_replaying_view_binding() {
        let mut config = crate::config::load_test_fixture().unwrap();
        config.commands.bindings.get_mut("commands").unwrap().key = Some("ctrl+k".to_string());
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.refresh_active_bindings().unwrap();
        session.open_route_completion().unwrap();
        assert!(session.resolve_key_binding(Key::Ctrl('k')).is_none());
        assert!(matches!(
            session
                .dispatch_decoded_input(DecodedInput {
                    key: Some(Key::Ctrl('k')),
                    raw: vec![0x0b],
                })
                .unwrap(),
            LauncherOutcome::Continue
        ));
        assert!(session.route_completion.is_none());
        assert!(matches!(
            session.resolve_key_binding(Key::Ctrl('k')),
            Some(InputBinding::SessionCommand(Key::Ctrl('k')))
        ));
    }

    #[test]
    fn route_completion_hands_editor_actions_to_the_restored_view() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        session.views.last_mut().unwrap().input = InputBuffer::new("two words");

        for (key, raw, expected) in [
            (Key::Backspace, vec![0x7f], "two word"),
            (Key::Ctrl('w'), vec![0x17], "two "),
            (Key::Ctrl('u'), vec![0x15], ""),
        ] {
            session.open_route_completion().unwrap();
            assert!(matches!(
                session
                    .dispatch_decoded_input(DecodedInput {
                        key: Some(key),
                        raw,
                    })
                    .unwrap(),
                LauncherOutcome::Continue
            ));
            assert!(session.route_completion.is_none());
            assert_eq!(session.views.last().unwrap().input.raw, expected);
        }
    }

    #[test]
    fn only_route_completion_hands_opaque_input_to_the_view() {
        let config = crate::config::load_test_fixture().unwrap();
        let mut session = AppSession::new(
            &config,
            crate::diagnostics::RuntimeLog::disabled(),
            EngineRegistry::new(),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        session.views.last_mut().unwrap().instance = Box::new(RecordingView {
            events: events.clone(),
        });
        let opaque = DecodedInput {
            key: None,
            raw: b"\x1b[999~".to_vec(),
        };

        session.open_route_completion().unwrap();
        session.dispatch_decoded_input(opaque.clone()).unwrap();
        assert!(session.route_completion.is_none());
        assert_eq!(
            *events.lock().unwrap(),
            ["unbound:[27, 91, 57, 57, 57, 126]"]
        );

        events.lock().unwrap().clear();
        session.dispatch_decoded_input(opaque).unwrap();
        assert!(events.lock().unwrap().is_empty());
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
            crate::diagnostics::RuntimeLog::disabled(),
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
        let mut runtime = crate::runtime::RuntimeStore::new();
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
