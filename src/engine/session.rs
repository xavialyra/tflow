use super::{
    EngineHost, EngineRegistry, NavigationMode, TaskScheduler, ViewEffect, ViewInstance,
    ViewLocation, ViewOutput,
};
use crate::chrome::ShellInput;
use crate::config::{Config, ENGINE_PICKER};
use crate::runtime_log::{LogRecord, RuntimeLog};
use crate::state::StateInstance;
use crate::terminal::Terminal;
use crate::text::sanitize_terminal_text;
use anyhow::{Context, Result};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

struct ViewEntry {
    location: ViewLocation,
    shell_input: ShellInput,
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
    input: ShellInput,
    active_error: Option<LogRecord>,
    active_error_deadline: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionOutcome {
    Exited,
    Completed(ViewOutput),
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
        let state = config.instantiate_state(&location.view_ref)?;
        let initial_input = sanitize_terminal_text(&config.render_query_input(&state)?);
        publish_location(config, &mut runtime, &location, &state)?;
        let root = engines.create_view(
            config,
            &location,
            &state,
            runtime_log.path(),
            runtime.handle(),
            tasks.clone(),
        )?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewEntry {
                shell_input: ShellInput::new(initial_input.clone()),
                location,
                state,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: true,
            input: ShellInput::new(initial_input),
            active_error: None,
            active_error_deadline: None,
        })
    }

    pub(crate) fn single_root(
        config: &'a Config,
        runtime_log: RuntimeLog,
        engines: EngineRegistry,
        location: ViewLocation,
    ) -> Result<Self> {
        let mut runtime = super::RuntimeStore::new();
        let tasks = TaskScheduler::new(runtime.handle());
        let router = Arc::new(crate::router::Router::new(config));
        let state = config.invocation_state.clone();
        let state_input = sanitize_terminal_text(&config.render_query_input(&state)?);
        let raw_input = location.shell_input.clone().unwrap_or_else(|| {
            if location.input.is_empty() {
                state_input
            } else {
                location.input.clone()
            }
        });
        let input = ShellInput::new(raw_input);
        publish_location(config, &mut runtime, &location, &state)?;
        let root = engines.create_view(
            config,
            &location,
            &state,
            runtime_log.path(),
            runtime.handle(),
            tasks.clone(),
        )?;
        Ok(Self {
            config,
            engines,
            views: vec![ViewEntry {
                shell_input: input.clone(),
                location,
                state,
                instance: root,
            }],
            tasks,
            runtime,
            runtime_log,
            router,
            route_input: false,
            input,
            active_error: None,
            active_error_deadline: None,
        })
    }

    pub(crate) fn root_state(&self) -> Result<&StateInstance> {
        self.views
            .first()
            .map(|entry| &entry.state)
            .context("session has no root view")
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
        let effect = {
            let entry = self
                .views
                .last_mut()
                .context("session has no active view")?;
            let mut host = EngineHost {
                config: self.config,
                input: &mut self.input,
                state: &mut entry.state,
                runtime: &mut self.runtime,
                runtime_log: &mut self.runtime_log,
                active_error: &mut self.active_error,
                active_error_deadline: &mut self.active_error_deadline,
            };
            entry.instance.step(&mut host, terminal)?
        };
        if matches!(effect, ViewEffect::Continue)
            && let Some(effect) = self.reconcile_input()?
        {
            return Ok(effect);
        }
        Ok(effect)
    }

    fn render(&mut self, terminal: &Terminal) -> Result<()> {
        let show_route_label = self.views.len() > 1;
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
        let shell_cursor = self.input.cursor;
        let host = EngineHost {
            config: self.config,
            input: &mut self.input,
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
            &shell_input,
            shell_cursor,
            engine_chrome,
            error.as_deref(),
        );
        let content = entry.instance.content(&host, terminal, &chrome)?;
        chrome.render(terminal, content)
    }

    // Resolve the shell input before the active engine refreshes its parameter view.
    fn reconcile_input(&mut self) -> Result<Option<ViewEffect>> {
        if !self.input.changed {
            return Ok(None);
        }
        self.input.changed = false;
        if !self.route_input {
            self.input.params = self.input.raw.clone();
            self.sync_query_state()?;
            return Ok(None);
        }

        let entry = self.views.last().context("session has no active view")?;
        let current_view = entry.location.view_ref.clone();
        let route_child = entry.location.shell_input.is_some();
        if current_view == self.config.command_view
            || self.config.engine(&current_view)? != ENGINE_PICKER
        {
            self.input.params = self.input.raw.clone();
            self.sync_query_state()?;
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
                self.sync_query_state()?;
                Ok(None)
            }
            crate::router::RouteResolution::NotMatched if route_child => {
                Ok(Some(ViewEffect::BackWithInput {
                    input: raw_input,
                    cursor: self.input.cursor,
                }))
            }
            crate::router::RouteResolution::NotMatched => {
                self.input.params = raw_input;
                self.sync_query_state()?;
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
                let entry = self
                    .views
                    .last_mut()
                    .context("session has no active view")?;
                let mut host = EngineHost {
                    config: self.config,
                    input: &mut self.input,
                    state: &mut entry.state,
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

    fn sync_query_state(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let view_ref = entry.location.view_ref.clone();
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        host.sync_query_state(&view_ref)?;
        Ok(())
    }

    fn apply(&mut self, effect: ViewEffect) -> Result<Option<SessionOutcome>> {
        match effect {
            ViewEffect::Continue => Ok(None),
            ViewEffect::Exit => Ok(Some(SessionOutcome::Exited)),
            ViewEffect::Complete(output) => Ok(Some(SessionOutcome::Completed(output))),
            ViewEffect::Back => self.pop_current(None),
            ViewEffect::BackWithInput { input, cursor } => self.pop_current(Some((input, cursor))),
            ViewEffect::Navigate { location, mode } => {
                let previous_input = self.input.clone();
                if let Some(entry) = self.views.last_mut() {
                    entry.shell_input = previous_input.clone();
                }
                self.deactivate_current()?;
                let mut state = self.config.instantiate_state(&location.view_ref)?;
                if !location.input.is_empty()
                    && let Err(error) = self.config.update_query_input(&mut state, &location.input)
                {
                    self.input = previous_input;
                    self.record_navigation_error(&location.view_ref, &error.to_string());
                    self.activate_current()?;
                    self.restore_current_input(false)?;
                    return Ok(None);
                }
                publish_location(self.config, &mut self.runtime, &location, &state)?;
                let view = self.engines.create_view(
                    self.config,
                    &location,
                    &state,
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
                        return Ok(None);
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
                    state,
                    instance: view,
                });
                Ok(None)
            }
        }
    }

    fn pop_current(
        &mut self,
        edited_input: Option<(String, usize)>,
    ) -> Result<Option<SessionOutcome>> {
        if self.views.len() <= 1 {
            let Some((input, cursor)) = edited_input else {
                return Ok(Some(SessionOutcome::Exited));
            };
            self.input = ShellInput::with_cursor(input.clone(), cursor);
            if let Some(entry) = self.views.last_mut() {
                entry.shell_input = self.input.clone();
            }
            self.activate_current()?;
            self.restore_current_input(true)?;
            return Ok(None);
        }

        // An edited pop carries the new buffer; Esc restores the saved parent snapshot.
        let changed = edited_input.is_some();
        self.views.pop();
        if let Some((input, cursor)) = edited_input {
            self.input = ShellInput::with_cursor(input, cursor);
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
        Ok(None)
    }

    fn reject_current_input(&mut self) -> Result<()> {
        let entry = self
            .views
            .last_mut()
            .context("session has no active view")?;
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            state: &mut entry.state,
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
            state: &mut entry.state,
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
        publish_location(
            self.config,
            &mut self.runtime,
            &entry.location,
            &entry.state,
        )?;
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            state: &mut entry.state,
            runtime: &mut self.runtime,
            runtime_log: &mut self.runtime_log,
            active_error: &mut self.active_error,
            active_error_deadline: &mut self.active_error_deadline,
        };
        entry.instance.activate(&mut host)
    }

    fn record_navigation_error(&mut self, view_ref: &str, message: &str) {
        let entry = self
            .views
            .last_mut()
            .expect("session has no active view while reporting navigation error");
        let mut host = EngineHost {
            config: self.config,
            input: &mut self.input,
            state: &mut entry.state,
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

fn publish_location(
    _config: &Config,
    runtime: &mut super::RuntimeStore,
    location: &ViewLocation,
    state: &StateInstance,
) -> Result<()> {
    runtime.set(
        "/view",
        json!({
            "active": {
                "ref": location.view_ref,
                "state_revision": state.revision(),
                "input": location.input,
                "query": location.input,
                "request": {
                    "input": location.input,
                    "query": location.input,
                }
            }
        }),
    )?;
    runtime.set(
        "/session",
        json!({
            "input": {
                "raw": location.shell_input.as_deref().unwrap_or(&location.input),
                "params": location.input,
            }
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_a_location_preserves_unrelated_runtime_data() {
        let mut runtime = super::super::RuntimeStore::new();
        runtime
            .set("/invocation_marker", json!({"items": [1, 2]}))
            .unwrap();

        let config = Config::load(std::path::Path::new("config/config.toml")).unwrap();
        let state = config.instantiate_state("core:default").unwrap();
        publish_location(
            &config,
            &mut runtime,
            &ViewLocation::new("core:default", "query"),
            &state,
        )
        .unwrap();

        assert_eq!(
            runtime.snapshot()["invocation_marker"]["items"],
            json!([1, 2])
        );
        assert_eq!(runtime.snapshot()["view"]["active"]["query"], "query");
    }
}
