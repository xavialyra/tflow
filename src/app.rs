use crate::cancellation::CancellationToken;
use crate::engine::{AppSession, EngineRegistry, SessionOutcome};
use crate::runtime_log::RuntimeLog;
use crate::terminal::Terminal;
use crate::theme::ResolvedTheme;
use anyhow::Result;

pub struct App<'a> {
    session: AppSession<'a>,
}

impl<'a> App<'a> {
    pub(crate) fn with_runtime_log_and_engines(
        config: &'a crate::config::Config,
        theme: &'a ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Ok(Self {
            session: AppSession::new_with_theme(
                config,
                *theme,
                runtime_log,
                engines,
                cancellation,
            )?,
        })
    }

    pub(crate) fn with_view(
        config: &'a crate::config::Config,
        theme: &'a ResolvedTheme,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        view_ref: &str,
        cancellation: &CancellationToken,
    ) -> Result<Self> {
        Ok(Self {
            session: AppSession::single_root_with_theme(
                config,
                *theme,
                runtime_log,
                engines,
                view_ref,
                cancellation,
            )?,
        })
    }

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        self.session.run(terminal)
    }

    pub(crate) fn take_runtime_warning(&mut self) -> Option<String> {
        self.session.take_runtime_warning()
    }
}
