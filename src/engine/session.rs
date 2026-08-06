use super::{EngineDriver, EngineHost, EngineRegistry, SessionEffect};
use crate::config::Config;
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::time::Instant;

struct ViewInstance {
    driver: Box<dyn EngineDriver>,
}

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    engines: EngineRegistry,
    views: Vec<ViewInstance>,
    runtime: super::RuntimeStore,
    runtime_log: RuntimeLog,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
}

impl<'a> AppSession<'a> {
    pub(crate) fn new(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
    ) -> Result<Self> {
        let root = engines.create_view(config, &config.default_view, "", runtime_log.path())?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewInstance { driver: root }],
            runtime: super::RuntimeStore::new(),
            runtime_log,
            active_error: None,
            active_error_deadline: None,
        })
    }

    pub(crate) fn run(&mut self, terminal: &mut Terminal) -> Result<()> {
        loop {
            self.clear_expired_error();
            let effect = self.step(terminal)?;
            if self.apply(effect)? {
                return Ok(());
            }
            self.render(terminal)?;
        }
    }

    fn step(&mut self, terminal: &mut Terminal) -> Result<SessionEffect> {
        let config = self.config;
        let runtime = &mut self.runtime;
        let runtime_log = &mut self.runtime_log;
        let active_error = &mut self.active_error;
        let active_error_deadline = &mut self.active_error_deadline;
        let mut host = EngineHost {
            config,
            runtime,
            runtime_log,
            active_error,
            active_error_deadline,
        };
        self.views
            .last_mut()
            .context("session has no active view")?
            .driver
            .step(&mut host, terminal)
    }

    fn render(&mut self, terminal: &Terminal) -> Result<()> {
        let config = self.config;
        let runtime = &mut self.runtime;
        let runtime_log = &mut self.runtime_log;
        let active_error = &mut self.active_error;
        let active_error_deadline = &mut self.active_error_deadline;
        let host = EngineHost {
            config,
            runtime,
            runtime_log,
            active_error,
            active_error_deadline,
        };
        self.views
            .last()
            .context("session has no active view")?
            .driver
            .render(&host, terminal)
    }

    fn apply(&mut self, effect: SessionEffect) -> Result<bool> {
        match effect {
            SessionEffect::Continue => Ok(false),
            SessionEffect::Exit => Ok(true),
            SessionEffect::Back => {
                if self.views.len() <= 1 {
                    return Ok(true);
                }
                self.views.pop();
                Ok(false)
            }
            SessionEffect::Push(driver) => {
                self.views.push(ViewInstance { driver });
                Ok(false)
            }
            SessionEffect::OpenView {
                view_ref,
                input,
                replace_current,
            } => {
                let driver = self.engines.create_view(
                    self.config,
                    &view_ref,
                    &input,
                    self.runtime_log.path(),
                )?;
                if replace_current {
                    self.views.pop();
                }
                self.views.push(ViewInstance { driver });
                Ok(false)
            }
            SessionEffect::RunCommand {
                execution,
                replace_current,
            } => {
                let driver = self.engines.create_command(*execution)?;
                if replace_current {
                    self.views.pop();
                }
                self.views.push(ViewInstance { driver });
                Ok(false)
            }
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
