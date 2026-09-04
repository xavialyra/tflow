mod cli;
mod invocation;

pub(crate) use cli::run;
pub(crate) use invocation::{InputArtifact, InvocationResult, finish};

use crate::diagnostics::RuntimeLog;
use crate::engine::ProtocolViewFactory;
use crate::input::InputPipeline;
use crate::lifecycle::CancellationToken;
use crate::protocol::ProtocolSession;
use crate::terminal::{InputRead, Terminal};
use crate::theme::ResolvedTheme;
use crate::view::{
    EffectError, EffectExecutor, EffectRequest, EffectResult, HostServices, NavigationRequest,
    ParsedQuery, Router, TerminalSize, ViewContext,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Default application host. Owns exactly one protocol Router and one InputPipeline.
#[derive(Debug, Clone)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(Box<crate::command::ViewReturn>),
}

pub struct App {
    session: ProtocolSession,
    cancellation: CancellationToken,
    root_view: String,
    terminal_size: Arc<Mutex<TerminalSize>>,
    pending_clipboard: Arc<Mutex<Vec<String>>>,
    tasks: crate::task::TaskRuntime,
}

pub(crate) struct LoadedApp {
    pub(crate) config: crate::config::Config,
    pub(crate) theme: ResolvedTheme,
}

struct ProtocolHost {
    tasks: crate::task::TaskRuntime,
}

impl HostServices for ProtocolHost {
    fn task_runtime(&self) -> Option<crate::task::TaskRuntime> {
        Some(self.tasks.clone())
    }
}

struct ProtocolEffects {
    cancellation: CancellationToken,
    pending_clipboard: Arc<Mutex<Vec<String>>>,
}

impl EffectExecutor for ProtocolEffects {
    fn execute(&mut self, effect: EffectRequest, _context: &ViewContext) -> Result<EffectResult> {
        if self.cancellation.is_cancelled() {
            return Ok(EffectResult::Error(EffectError::Failed(
                "operation cancelled".to_string(),
            )));
        }
        match effect {
            EffectRequest::CopyToClipboard(value) => {
                self.pending_clipboard
                    .lock()
                    .expect("protocol clipboard queue lock poisoned")
                    .push(value);
                Ok(EffectResult::Complete)
            }
            EffectRequest::RunPrepared {
                argv,
                environment,
                current_dir,
            } => {
                let Some(program) = argv.first() else {
                    return Ok(EffectResult::Error(EffectError::Failed(
                        "empty prepared command".to_string(),
                    )));
                };
                let mut command = std::process::Command::new(program);
                command.args(&argv[1..]);
                if let Some(directory) = current_dir {
                    command.current_dir(directory);
                }
                command.envs(environment);
                let status = command
                    .status()
                    .context("could not execute configured command")?;
                if status.success() {
                    Ok(EffectResult::Complete)
                } else {
                    Ok(EffectResult::Error(EffectError::Failed(format!(
                        "command exited with {status}"
                    ))))
                }
            }
        }
    }
}

impl App {
    pub(crate) fn with_runtime_log_and_engines(
        config: &crate::config::Config,
        theme: &ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::build(
            config,
            *theme,
            runtime_log,
            engines,
            cancellation.clone(),
            false,
            None,
        )
    }

    pub(crate) fn with_view(
        config: &crate::config::Config,
        theme: &ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Self::build(
            config,
            *theme,
            runtime_log,
            engines,
            cancellation.clone(),
            true,
            Some(view_ref.to_string()),
        )
    }

    fn build(
        config: &crate::config::Config,
        theme: ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: crate::engine::EngineRegistry,
        cancellation: CancellationToken,
        _explicit_view: bool,
        requested_root: Option<String>,
    ) -> Result<Self> {
        let root_view = requested_root
            .or_else(|| config.default_view.clone())
            .context("no default_view configured; specify a target View")?;
        let mut invocation_parameters = config.invocation_parameters.clone();
        config.sanitize_initial_parameter_values(&mut invocation_parameters)?;
        let values = config.parameter_values(&invocation_parameters)?;
        let raw = crate::terminal::sanitize_terminal_text(
            &config.render_parameter_input(&invocation_parameters)?,
        );
        let query = ParsedQuery::new(&root_view, "query", values);
        let mut request = NavigationRequest::new(root_view.clone(), query);
        if !raw.is_empty() {
            request = request.with_input(&raw, raw.len())?;
        }
        let routes = Box::new(crate::view::ConfigRouteCatalog::new(config));
        let factory = Box::new(ProtocolViewFactory::new(
            config,
            theme,
            cancellation.clone(),
            engines,
        ));
        let terminal_size = Arc::new(Mutex::new(TerminalSize::default()));
        let pending_clipboard = Arc::new(Mutex::new(Vec::new()));
        let tasks = crate::task::TaskRuntime::new();
        let host = Box::new(ProtocolHost {
            tasks: tasks.clone(),
        });
        let router = Router::with_host_services(routes, factory, host);
        let mut session = ProtocolSession::configured(
            router,
            Box::new(crate::protocol::ProtocolCommandService::new(
                config,
                cancellation.clone(),
            )),
            Box::new(ProtocolEffects {
                cancellation: cancellation.clone(),
                pending_clipboard: Arc::clone(&pending_clipboard),
            }),
            theme,
            config
                .default_view
                .clone()
                .unwrap_or_else(|| root_view.clone()),
            runtime_log,
        );
        session.start_root(request)?;
        Ok(Self {
            session,
            cancellation,
            root_view,
            terminal_size,
            pending_clipboard,
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
                self.session.resize(size)?;
            }
            let read = pipeline.read_normal(terminal, 50)?;
            if let Some(outcome) = self.tick_session(terminal, &mut pipeline)? {
                return Ok(outcome);
            }
            if pipeline.pending_is_empty() {
                self.draw(terminal)?;
            }
            while let Some(event) = pipeline.pop_input() {
                let eof = matches!(event, crate::input::InputEvent::Eof);
                let active_before = self.session.router().active().map(|view| view.id);
                if let Err(error) = self.session.input(event) {
                    self.handle_protocol_error(error)?;
                }
                let active_after = self.session.router().active().map(|view| view.id);
                if active_after != active_before {
                    pipeline.preserve_decoder_pending();
                }
                self.flush_host_effects(terminal)?;
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
        terminal: &Terminal,
        pipeline: &mut InputPipeline,
    ) -> Result<Option<SessionOutcome>> {
        let active_before = self.session.router().active().map(|view| view.id);
        for event in self.tasks.drain_events() {
            if let Err(error) = self.session.task(event) {
                self.handle_protocol_error(error)?;
            }
        }
        if let Err(error) = self.session.tick() {
            self.handle_protocol_error(error)?;
        }
        let active_after = self.session.router().active().map(|view| view.id);
        if active_after != active_before {
            pipeline.preserve_decoder_pending();
        }
        self.flush_host_effects(terminal)?;
        Ok(self.pending_outcome())
    }

    fn handle_protocol_error(&mut self, error: anyhow::Error) -> Result<()> {
        if error
            .downcast_ref::<crate::view::ViewOperationFailure>()
            .is_some()
        {
            return Err(error);
        }
        self.session.report_error(&error.to_string());
        Ok(())
    }

    fn pending_outcome(&mut self) -> Option<SessionOutcome> {
        if let Some(result) = self.session.take_result() {
            return Some(self.result_outcome(result.value, result.adapter));
        }
        self.session
            .router()
            .stack()
            .is_empty()
            .then_some(SessionOutcome::Exited)
    }

    fn flush_host_effects(&mut self, terminal: &Terminal) -> Result<()> {
        let effects = std::mem::take(
            &mut *self
                .pending_clipboard
                .lock()
                .expect("protocol clipboard queue lock poisoned"),
        );
        for value in effects {
            terminal.copy_to_clipboard(&value)?;
        }
        Ok(())
    }

    fn draw(&mut self, terminal: &mut Terminal) -> Result<()> {
        let mut result = Ok(());
        let image_picker = terminal.image_picker();
        terminal.draw(|frame| {
            let area = frame.area();
            result = self.session.render(frame, area, image_picker).map(|_| ());
        })?;
        result
    }

    fn result_outcome(
        &self,
        value: Value,
        result_adapter: Option<crate::command::ReturnAdapter>,
    ) -> SessionOutcome {
        let output = serde_json::from_value(value.clone())
            .unwrap_or(crate::command::ViewOutput::Value { value });
        SessionOutcome::Completed(Box::new(crate::command::ViewReturn {
            source_view: self.root_view.clone(),
            output,
            adapter: result_adapter,
        }))
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.session.take_runtime_warning()
    }
}
