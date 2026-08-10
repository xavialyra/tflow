use super::host::EngineHost;
use super::runtime::RuntimeHandle;
use super::task::TaskScheduler;
use crate::config::{Config, View};
use crate::state::StateInstance;
use crate::terminal::Terminal;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewLocation {
    pub(crate) view_ref: String,
    pub(crate) input: String,
    pub(crate) shell_input: Option<String>,
    pub(crate) context: Value,
    pub(crate) owner_state: Option<crate::state::StateInstance>,
}

impl ViewLocation {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: input.into(),
            shell_input: None,
            context: Value::Null,
            owner_state: None,
        }
    }

    pub(crate) fn with_shell_input(mut self, input: impl Into<String>) -> Self {
        self.shell_input = Some(input.into());
        self
    }

    pub(crate) fn with_context(mut self, context: Value) -> Self {
        self.context = context;
        self
    }

    pub(crate) fn with_owner_state(mut self, state: crate::state::StateInstance) -> Self {
        self.owner_state = Some(state);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigationMode {
    Push,
    Replace,
}

pub(crate) enum ViewEffect {
    Continue,
    Navigate {
        location: ViewLocation,
        mode: NavigationMode,
    },
    Back,
    BackWithInput {
        input: String,
        cursor: usize,
    },
    Exit,
    Complete(CompletionRequest),
}

#[derive(Debug, Clone)]
pub(crate) struct CompletionRequest {
    pub(crate) source_view: String,
    pub(crate) command_id: String,
    pub(crate) state: StateInstance,
    pub(crate) runtime: Value,
    pub(crate) output: ViewOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ViewOutput {
    Selected {
        item: Option<ViewOutputItem>,
        input: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewOutputItem {
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) source_view: String,
}

pub(crate) trait ViewInstance {
    fn activate(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        Ok(())
    }

    fn restore_input(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn input_changed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect>;

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome::default()
    }

    fn content(
        &mut self,
        host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<crate::chrome::ChromeContent>;
}

pub(crate) struct ViewContext<'a> {
    pub(crate) config: &'a Config,
    pub(crate) location: &'a ViewLocation,
    pub(crate) state: &'a StateInstance,
    pub(crate) log_file: Option<&'a Path>,
    pub(crate) runtime: RuntimeHandle,
    pub(crate) tasks: TaskScheduler,
}

pub(crate) trait Engine {
    fn engine_type(&self) -> &'static str;

    fn validate_config(&self, name: &str, view: &View) -> Result<()>;

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>>;
}
