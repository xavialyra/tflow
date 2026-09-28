mod cli;
mod config_adapters;
mod invocation;
mod protocol_factory;

pub(crate) use cli::run;
pub(crate) use invocation::{InputArtifact, InvocationResult, finish};

use self::config_adapters::CompiledRouteCatalog;
use self::protocol_factory::ProtocolViewFactory;
use crate::diagnostics::RuntimeLog;
use crate::execution::{PreparedProcess, run_command_process};
use crate::input::InputPipeline;
use crate::lifecycle::CancellationToken;
use crate::protocol::ProtocolSession;
use crate::terminal::{InputRead, Terminal};
use crate::ui::theme::ResolvedTheme;
use crate::view::{
    EffectError, EffectExecutor, EffectRequest, EffectResult, HostServices, NavigationRequest,
    ParsedQuery, Router, TerminalSize, ViewContext,
};
use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Default application host. Owns exactly one protocol Router and one InputPipeline.
#[derive(Debug, Clone)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(Value),
}

pub struct App {
    session: ProtocolSession,
    cancellation: CancellationToken,
    terminal_size: Arc<Mutex<TerminalSize>>,
    tasks: crate::task::TaskRuntime,
}

pub(crate) struct LoadedApp {
    pub(crate) config: Arc<crate::workflow::config::CompiledConfig>,
    pub(crate) theme: ResolvedTheme,
}

struct ProtocolHost {
    tasks: crate::task::TaskRuntime,
    registry: std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>,
}

impl HostServices for ProtocolHost {
    fn task_runtime(&self) -> Option<crate::task::TaskRuntime> {
        Some(self.tasks.clone())
    }

    fn command_registry(
        &self,
    ) -> Option<std::sync::Arc<std::sync::RwLock<crate::command::CommandRegistry>>> {
        Some(std::sync::Arc::clone(&self.registry))
    }
}

struct ProtocolEffects<'a> {
    cancellation: &'a CancellationToken,
    pipeline: &'a mut InputPipeline,
    terminal: &'a mut Terminal,
}

impl EffectExecutor for ProtocolEffects<'_> {
    fn execute(&mut self, effect: EffectRequest, _context: &ViewContext) -> Result<EffectResult> {
        match effect {
            EffectRequest::CopyToClipboard(value) => {
                if self.cancellation.is_cancelled() {
                    return Ok(EffectResult::Error(EffectError::Failed(
                        "operation cancelled".to_string(),
                    )));
                }
                self.terminal.copy_to_clipboard(&value)?;
                Ok(EffectResult::Complete)
            }
            EffectRequest::RunPrepared { prepared, .. } => {
                self.pipeline.discard_pending();
                run_prepared_effect(prepared, self.cancellation)
            }
            EffectRequest::ShowFeedback { .. } => Ok(EffectResult::Complete),
        }
    }
}

fn run_prepared_effect(
    prepared: PreparedProcess,
    cancellation: &CancellationToken,
) -> Result<EffectResult> {
    match run_command_process(&prepared, cancellation) {
        Ok(status) if status.success() => Ok(EffectResult::Complete),
        Ok(status) => Ok(EffectResult::Error(EffectError::Failed(format!(
            "command exited with {status}"
        )))),
        Err(error) => Ok(EffectResult::Error(EffectError::Failed(error.to_string()))),
    }
}

impl App {
    pub(crate) fn with_runtime_log_and_engines(
        config: Arc<crate::workflow::config::CompiledConfig>,
        invocation: Arc<crate::workflow::InvocationContext>,
        theme: &ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::build(
            config,
            invocation,
            theme.clone(),
            runtime_log,
            engines,
            cancellation.clone(),
            false,
            None,
        )
    }

    pub(crate) fn with_view(
        config: Arc<crate::workflow::config::CompiledConfig>,
        invocation: Arc<crate::workflow::InvocationContext>,
        theme: &ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::build(
            config,
            invocation,
            theme.clone(),
            runtime_log,
            engines,
            cancellation.clone(),
            true,
            Some(view_ref.to_string()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: Arc<crate::workflow::config::CompiledConfig>,
        invocation: Arc<crate::workflow::InvocationContext>,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        cancellation: CancellationToken,
        _explicit_view: bool,
        requested_root: Option<String>,
    ) -> Result<Self> {
        let root_view = invocation.root_view().to_string();
        if let Some(requested_root) = requested_root {
            anyhow::ensure!(
                requested_root == root_view,
                "requested root View does not match invocation context"
            );
        }
        let values = config.parameter_values(invocation.root_parameters())?;
        let raw = crate::terminal::sanitize_terminal_text(
            &config.render_parameter_input(invocation.root_parameters())?,
        );
        let query = ParsedQuery::new(&root_view, "query", values);
        let mut request = NavigationRequest::new(root_view.clone(), query);
        if !raw.is_empty() {
            request = request.with_input(&raw, raw.len())?;
        }
        let routes = Box::new(CompiledRouteCatalog::new(Arc::clone(&config)));
        let factory = Box::new(ProtocolViewFactory::new(
            Arc::clone(&config),
            Arc::clone(&invocation),
            theme.clone(),
            cancellation.clone(),
            engines,
        ));
        let terminal_size = Arc::new(Mutex::new(TerminalSize::default()));
        let tasks = crate::task::TaskRuntime::new();
        let command_service = crate::protocol::ProtocolCommandService::new(
            Arc::clone(&config),
            Arc::clone(&invocation),
            cancellation.clone(),
        );
        let registry = command_service.registry_handle();
        let host = Box::new(ProtocolHost {
            tasks: tasks.clone(),
            registry,
        });
        let router = Router::with_host_services(routes, factory, host);
        let mut session =
            ProtocolSession::configured(router, Box::new(command_service), theme, runtime_log);
        session.start_root(request)?;
        Ok(Self {
            session,
            cancellation,
            terminal_size,
            tasks,
        })
    }

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        let mut pipeline = InputPipeline::default();
        let mut previous_size = TerminalSize::default();
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(SessionOutcome::Exited);
            }
            let (width, height) = terminal.size();
            let size = TerminalSize { width, height };
            if size != previous_size {
                previous_size = size;
                *self
                    .terminal_size
                    .lock()
                    .expect("protocol terminal size lock poisoned") = size;
                let cancellation = self.cancellation.clone();
                let mut effects = ProtocolEffects {
                    cancellation: &cancellation,
                    pipeline: &mut pipeline,
                    terminal,
                };
                self.session.resize_with_effects(size, &mut effects)?;
            }
            let poll_timeout = if self.tasks.has_active_tasks() || self.tasks.has_pending_events() {
                5
            } else {
                50
            };
            let read = pipeline.read_normal(terminal, poll_timeout)?;
            if let Some(outcome) = self.tick_session(terminal, &mut pipeline)? {
                return Ok(outcome);
            }
            if pipeline.pending_is_empty() {
                self.draw(terminal)?;
            }
            while let Some(event) = pipeline.pop_input() {
                let eof = matches!(event, crate::input::InputEvent::Eof);
                let active_before = self.session.router().active().map(|view| view.id);
                let cancellation = self.cancellation.clone();
                let mut effects = ProtocolEffects {
                    cancellation: &cancellation,
                    pipeline: &mut pipeline,
                    terminal,
                };
                if let Err(error) = self.session.input_with_effects(event, &mut effects) {
                    self.handle_protocol_error(error);
                }
                let active_after = self.session.router().active().map(|view| view.id);
                if active_after != active_before {
                    pipeline.preserve_decoder_pending();
                }
                if eof {
                    return Ok(SessionOutcome::Exited);
                }
                if let Some(outcome) = self.pending_outcome() {
                    return Ok(outcome);
                }
            }
            if matches!(read, InputRead::Eof) {
                return Ok(SessionOutcome::Exited);
            }
            if let Some(outcome) = self.tick_session(terminal, &mut pipeline)? {
                return Ok(outcome);
            }
            self.draw(terminal)?;
        }
    }

    fn tick_session(
        &mut self,
        terminal: &mut Terminal,
        pipeline: &mut InputPipeline,
    ) -> Result<Option<SessionOutcome>> {
        let active_before = self.session.router().active().map(|view| view.id);
        let cancellation = self.cancellation.clone();
        let mut effects = ProtocolEffects {
            cancellation: &cancellation,
            pipeline,
            terminal,
        };
        for event in self.tasks.drain_events() {
            if let Err(error) = self.session.task_with_effects(event, &mut effects) {
                self.handle_protocol_error(error);
            }
        }
        if let Err(error) = self.session.tick_with_effects(&mut effects) {
            self.handle_protocol_error(error);
        }
        let active_after = self.session.router().active().map(|view| view.id);
        if active_after != active_before {
            pipeline.preserve_decoder_pending();
        }
        Ok(self.pending_outcome())
    }

    fn handle_protocol_error(&mut self, error: anyhow::Error) {
        self.session.report_error(&error.to_string());
    }

    fn pending_outcome(&mut self) -> Option<SessionOutcome> {
        if let Some(result) = self.session.take_result() {
            return Some(self.result_outcome(result.value));
        }
        self.session
            .router()
            .stack()
            .is_empty()
            .then_some(SessionOutcome::Exited)
    }

    fn draw(&mut self, terminal: &mut Terminal) -> Result<()> {
        let embedded = self
            .session
            .router()
            .active()
            .is_some_and(|entry| entry.view.is_embedded_terminal());
        terminal.set_cursor_owner(embedded)?;
        let mut result = Ok(());
        let image_picker = terminal.image_picker();
        terminal.draw(|frame| {
            let area = frame.area();
            result = self.session.render(frame, area, image_picker).map(|_| ());
        })?;
        result
    }

    fn result_outcome(&self, value: Value) -> SessionOutcome {
        SessionOutcome::Completed(value)
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.session.take_runtime_warning()
    }
}
