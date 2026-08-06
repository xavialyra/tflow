use crate::engine::{AppSession, EngineRegistry};
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

    pub fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        self.session.run(terminal)
    }
}
