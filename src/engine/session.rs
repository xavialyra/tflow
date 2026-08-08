use super::{
    EngineHost, EngineRegistry, NavigationMode, TaskScheduler, ViewEffect, ViewInstance,
    ViewLocation,
};
use crate::chrome::ShellInput;
use crate::config::{Config, ENGINE_PICKER};
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

struct ViewEntry {
    location: ViewLocation,
    shell_input: ShellInput,
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
    input: ShellInput,
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
        let router = Arc::new(crate::router::Router::new(config));
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
                shell_input: ShellInput::new(location.input.clone()),
                location,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            input: ShellInput::new(""),
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
        let effect = {
            let mut host = EngineHost {
                config: self.config,
                input: &mut self.input,
                runtime: &mut self.runtime,
                runtime_log: &mut self.runtime_log,
                active_error: &mut self.active_error,
                active_error_deadline: &mut self.active_error_deadline,
            };
            self.views
                .last_mut()
                .context("session has no active view")?
                .instance
                .step(&mut host, terminal)?
        };
        if matches!(effect, ViewEffect::Continue)
            && let Some(effect) = self.reconcile_input()?
        {
            return Ok(effect);
        }
        Ok(effect)
    }

    fn render(&mut self, terminal: &Terminal) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let route = self.router.display(&entry.location.view_ref);
        let error = self
            .active_error
            .as_ref()
            .map(|record| record.label.clone());
        let shell_input = self.input.raw.clone();
        let host = EngineHost {
            config: self.config,
            input: &mut self.input,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        let engine_chrome = entry.instance.chrome(&host);
        let chrome = crate::chrome::ChromeFrame::compose(
            terminal.size().0 as usize,
            &route,
            &shell_input,
            engine_chrome,
            error.as_deref(),
        );
        entry.instance.render(&host, terminal, &chrome)
    }

    // Resolve the shell input before the active engine refreshes its parameter view.
    fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
        if !self.input.changed {
            return Ok(None);
        }
        self.input.changed = false;

        let entry = self.views.last().context("session has no active view")?;
        let current_view = entry.location.view_ref.clone();
        let route_child = entry.location.shell_input.is_some();
        if current_view == self.config.command_view
            || self.config.engine(&current_view)? != ENGINE_PICKER
        {
            self.input.params = self.input.raw.clone();
            return Ok(None);
        }

        let raw_input = self.input.raw.clone();
        match self.router.resolve(&current_view, &raw_input) {
            crate::router::RouteResolution::Navigate { target, query } => {
                self.input.params = query.clone();
                Ok(Some(ViewEffect::Navigate {
                    location: ViewLocation::new(target, query).with_shell_input(raw_input),
                    mode: if route_child {
                        NavigationMode::Replace
                    } else {
                        NavigationMode::Push
                    },
                }))
            }
            crate::router::RouteResolution::Current { query } => {
                self.input.params = query;
                Ok(None)
            }
            crate::router::RouteResolution::NotMatched if route_child => {
                Ok(Some(ViewEffect::BackWithInput(raw_input)))
            }
            crate::router::RouteResolution::NotMatched => {
                self.input.params = raw_input;
                Ok(None)
            }
            crate::router::RouteResolution::Ambiguous { alias, targets } => {
                self.input.params = raw_input;
                self.input.rejected = true;
                self.reject_current_input()?;
                let message = format!(
                    "view alias {:?} is ambiguous: {}",
                    alias,
                    targets.join(", ")
                );
                let mut host = EngineHost {
                    config: self.config,
                    input: &mut self.input,
                    runtime: &mut self.runtime,
                    runtime_log: &mut self.runtime_log,
                    active_error: &mut self.active_error,
                    active_error_deadline: &mut self.active_error_deadline,
                };
                host.record_error_message(Some(&current_view), None, &message);
                Ok(None)
            }
        }
    }

    fn apply(&mut self, effect: ViewEffect) -> Result<bool> {
        match effect {
            ViewEffect::Continue => Ok(false),
            ViewEffect::Exit => Ok(true),
            ViewEffect::Back => self.pop_current(None),
            ViewEffect::BackWithInput(input) => self.pop_current(Some(input)),
            ViewEffect::Navigate { location, mode } => {
                let previous_input = self.input.clone();
                if let Some(entry) = self.views.last_mut() {
                    entry.shell_input = previous_input.clone();
                }
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
                        self.input = previous_input;
                        self.record_navigation_error(&location.view_ref, &error.to_string());
                        self.activate_current()?;
                        self.restore_current_input(false)?;
                        return Ok(false);
                    }
                };
                let shell_input = location
                    .shell_input
                    .clone()
                    .unwrap_or_else(|| location.input.clone());
                self.input = ShellInput::with_params(shell_input.clone(), location.input.clone());
                if mode == NavigationMode::Replace {
                    self.views.pop();
                }
                self.views.push(ViewEntry {
                    location,
                    shell_input: self.input.clone(),
                    instance: view,
                });
                Ok(false)
            }
        }
    }

    fn pop_current(&mut self, edited_input: Option<String>) -> Result<bool> {
        if self.views.len() <= 1 {
            let Some(input) = edited_input else {
                return Ok(true);
            };
            self.input = ShellInput::new(input.clone());
            if let Some(entry) = self.views.last_mut() {
                entry.shell_input = self.input.clone();
            }
            self.activate_current()?;
            self.restore_current_input(true)?;
            return Ok(false);
        }

        // An edited pop carries the new buffer; Esc restores the saved parent snapshot.
        let changed = edited_input.is_some();
        self.views.pop();
        if let Some(input) = edited_input {
            self.input = ShellInput::new(input);
            if let Some(entry) = self.views.last_mut() {
                entry.shell_input = self.input.clone();
            }
        } else {
            self.input = self
                .views
                .last()
                .context("session has no active view")?
                .shell_input
                .clone();
        }
        self.activate_current()?;
        self.restore_current_input(changed)?;
        Ok(false)
    }

    fn reject_current_input(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        entry.instance.input_rejected(&mut host)
    }

    fn restore_current_input(&mut self, changed: bool) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        if changed {
            entry.instance.input_changed(&mut host)
        } else {
            entry.instance.restore_input(&mut host)
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
            input: &mut self.input,
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
            input: &mut self.input,
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
