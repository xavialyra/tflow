use crate::config::{Config, EngineDefinition};
use crate::terminal::Terminal;
use anyhow::Result;
use std::path::Path;

use super::host::EngineHost;
use super::launcher::CommandExecution;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Alt(char),
    Enter,
    Backspace,
    Up,
    Down,
    Escape,
    CtrlC,
    CtrlK,
    CtrlD,
    CtrlU,
    CtrlW,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineKeyAction {
    Continue,
    Refresh,
    ClearError,
    Command(Key),
    OpenCommandView,
    PopView,
    Exit,
}

pub(crate) enum SessionEffect {
    Continue,
    OpenView {
        view_ref: String,
        input: String,
        replace_current: bool,
    },
    Push(Box<dyn EngineDriver>),
    RunCommand {
        execution: Box<CommandExecution>,
        replace_current: bool,
    },
    Back,
    Exit,
}

pub(crate) trait EngineDriver {
    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal)
    -> Result<SessionEffect>;

    fn render(&self, host: &EngineHost<'_>, terminal: &Terminal) -> Result<()>;
}

pub(crate) trait Engine {
    fn engine_type(&self) -> &'static str;

    fn validate_config(&self, name: &str, definition: &EngineDefinition) -> Result<()>;

    fn create_view(
        &self,
        config: &Config,
        view_ref: &str,
        input: &str,
        log_file: Option<&Path>,
    ) -> Result<Box<dyn EngineDriver>>;

    fn create_command(&self, execution: CommandExecution) -> Result<Box<dyn EngineDriver>>;
}
