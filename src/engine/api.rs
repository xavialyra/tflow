use crate::config::{Config, View};
use crate::terminal::Terminal;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

use super::host::EngineHost;
use super::runtime::RuntimeHandle;
use super::task::TaskScheduler;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewLocation {
    pub(crate) view_ref: String,
    pub(crate) input: String,
    pub(crate) context: Value,
}

impl ViewLocation {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: input.into(),
            context: Value::Null,
        }
    }

    pub(crate) fn with_context(mut self, context: Value) -> Self {
        self.context = context;
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
    Exit,
}

pub(crate) trait ViewInstance {
    fn activate(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn deactivate(&mut self) -> Result<()> {
        Ok(())
    }

    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect>;

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome::default()
    }

    fn render(
        &mut self,
        host: &EngineHost<'_>,
        terminal: &Terminal,
        chrome: &crate::chrome::ChromeFrame,
    ) -> Result<()>;
}

pub(crate) struct ViewContext<'a> {
    pub(crate) config: &'a Config,
    pub(crate) location: &'a ViewLocation,
    pub(crate) log_file: Option<&'a Path>,
    pub(crate) runtime: RuntimeHandle,
    pub(crate) tasks: TaskScheduler,
    pub(crate) router: Arc<crate::router::Router>,
}

pub(crate) trait Engine {
    fn engine_type(&self) -> &'static str;

    fn validate_config(&self, name: &str, view: &View) -> Result<()>;

    fn create_view(&self, context: ViewContext<'_>) -> Result<Box<dyn ViewInstance>>;
}
