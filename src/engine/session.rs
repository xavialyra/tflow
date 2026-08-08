use super::{
    EngineHost, EngineRegistry, NavigationMode, TaskScheduler, ViewEffect, ViewInstance,
    ViewLocation,
};
use crate::config::Config;
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::json;
use std::time::Instant;

struct ViewEntry {
    location: ViewLocation,
    instance: Box<dyn ViewInstance>,
}

pub(crate) struct AppSession<'a> {
    config: &'a Config,
    engines: EngineRegistry,
    views: Vec<ViewEntry>,
    tasks: TaskScheduler,
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
        let mut runtime = super::RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let location = ViewLocation::new(&config.default_view, "");
        publish_location(&mut runtime, &location);
        let root = engines.create_view(
            config,
            &location,
            runtime_log.path(),
            runtime.handle(),
            tasks.clone(),
        )?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewEntry {
                location,
                instance: root,
            }],
            tasks,
            runtime,
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

    fn step(&mut self, terminal: &mut Terminal) -> Result<ViewEffect> {
        let mut host = EngineHost {
            config: self.config,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        self.views
            .last_mut()
            .context("session has no active view")?
            .instance
            .step(&mut host, terminal)
    }

    fn render(&mut self, terminal: &Terminal) -> Result<()> {
        let host = EngineHost {
            config: self.config,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        self.views
            .last()
            .context("session has no active view")?
            .instance
            .render(&host, terminal)
    }

    fn apply(&mut self, effect: ViewEffect) -> Result<bool> {
        match effect {
            ViewEffect::Continue => Ok(false),
            ViewEffect::Exit => Ok(true),
            ViewEffect::Back => {
                if self.views.len() <= 1 {
                    return Ok(true);
                }
                self.views.pop();
                self.activate_current()?;
                Ok(false)
            }
            ViewEffect::Navigate { location, mode } => {
                self.deactivate_current()?;
                publish_location(&mut self.runtime, &location);
                let view = self.engines.create_view(
                    self.config,
                    &location,
                    self.runtime_log.path(),
                    self.runtime.handle(),
                    self.tasks.clone(),
                );
                let view = match view {
                    Ok(view) => view,
                    Err(error) => {
                        self.record_navigation_error(&location.view_ref, &error.to_string());
                        self.activate_current()?;
                        return Ok(false);
                    }
                };
                if mode == NavigationMode::Replace {
                    self.views.pop();
                }
                self.views.push(ViewEntry {
                    location,
                    instance: view,
                });
                Ok(false)
            }
        }
    }

    fn deactivate_current(&mut self) -> Result<()> {
        self.views
            .last_mut()
            .context("session has no active view")?
            .instance
            .deactivate()
    }

    fn activate_current(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        publish_location(&mut self.runtime, &entry.location);
        let mut host = EngineHost {
            config: self.config,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        entry.instance.activate(&mut host)
    }

    fn record_navigation_error(&mut self, view_ref: &str, message: &str) {
        let mut host = EngineHost {
            config: self.config,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        host.record_error_message(Some(view_ref), None, message);
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

fn publish_location(runtime: &mut super::RuntimeStore, location: &ViewLocation) {
    runtime.replace(json!({
        "view": {
            "current": {
                "ref": location.view_ref,
                "input": location.input,
                "query": location.input,
                "request": {
                    "input": location.input,
                    "query": location.input,
                }
            }
        }
    }));
}
