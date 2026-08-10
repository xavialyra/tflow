use crate::engine::{AppSession, EngineRegistry, SessionOutcome, ViewLocation};
use crate::runtime_log::RuntimeLog;
use crate::terminal::Terminal;
use anyhow::Result;

pub struct App<'a> {
    session: AppSession<'a>,
}

impl<'a> App<'a> {
    pub(crate) fn with_runtime_log_and_engines(
        config: &'a crate::config::Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
    ) -> Result<Self> {
        Ok(Self {
            session: AppSession::new(config, runtime_log, engines)?,
        })
    }

    pub(crate) fn with_view(
        config: &'a crate::config::Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        view_ref: &str,
    ) -> Result<Self> {
        Ok(Self {
            session: AppSession::single_root(
                config,
                runtime_log,
                engines,
                ViewLocation::new(view_ref, ""),
            )?,
        })
    }

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<SessionOutcome> {
        self.session.run(terminal)
    }
}
