use crate::config::Command;
use crate::discovery::Item;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LauncherFocus {
    Launcher,
    Command,
    Capture,
    Embedded,
}

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

#[derive(Clone)]
pub(crate) struct LauncherRenderState {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) searching: bool,
    pub(crate) commands: Vec<(String, String)>,
}

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    pub(crate) id: String,
    pub(crate) source_view: String,
    pub(crate) command: Command,
}

pub(crate) struct PreparedCommand {
    pub(crate) argv: Vec<String>,
    pub(crate) environment: Vec<(String, String)>,
    pub(crate) current_dir: Option<PathBuf>,
}

impl PreparedCommand {
    pub(crate) fn process(&self) -> ProcessCommand {
        let mut process = ProcessCommand::new(&self.argv[0]);
        process.args(&self.argv[1..]);
        if let Some(current_dir) = &self.current_dir {
            process.current_dir(current_dir);
        }
        for (key, value) in &self.environment {
            process.env(key, value);
        }
        process
    }
}

pub(crate) struct CommandExecution {
    pub(crate) invocation: CommandInvocation,
    pub(crate) prepared: PreparedCommand,
    pub(crate) engine_type: String,
    pub(crate) exit: bool,
    pub(crate) title: String,
    pub(crate) next_view: Option<String>,
}

pub(crate) enum CommandAction {
    Report {
        invocation: CommandInvocation,
        message: String,
    },
    OpenView {
        target: String,
    },
    Execute(CommandExecution),
}

pub(crate) struct EngineResult {
    pub(crate) exit: bool,
    pub(crate) next_view: Option<String>,
}
