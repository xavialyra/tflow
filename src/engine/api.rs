use super::host::EngineHost;
use super::process::PreparedProcess;
use super::runtime::RuntimeHandle;
use super::task::TaskScheduler;
use crate::chrome::InputBuffer;
use crate::config::{Command, Config, View};
use crate::input::Key;
use crate::state::StateInstance;
use crate::terminal::Terminal;
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputSeed {
    pub(crate) raw: String,
    pub(crate) params: String,
    pub(crate) cursor: usize,
}

impl InputSeed {
    pub(crate) fn new(input: impl Into<String>) -> Self {
        let input = input.into();
        let cursor = input.len();
        Self {
            raw: input.clone(),
            params: input,
            cursor,
        }
    }

    pub(crate) fn routed(raw: impl Into<String>, params: impl Into<String>, cursor: usize) -> Self {
        Self {
            raw: raw.into(),
            params: params.into(),
            cursor,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandPickerItem {
    pub(crate) prefix: String,
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) source_view: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandPickerOwnerContext {
    pub(crate) view_ref: String,
    pub(crate) state: StateInstance,
    pub(crate) binding_raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CommandPickerContext {
    pub(crate) page_view: String,
    pub(crate) page_state: StateInstance,
    pub(crate) page_binding_raw: String,
    pub(crate) page_runtime: Value,
    pub(crate) owner: Option<CommandPickerOwnerContext>,
    pub(crate) parent_item: Option<CommandPickerItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavigationRequest {
    pub(crate) view_ref: String,
    pub(crate) input: Option<InputSeed>,
    pub(crate) query: Option<Value>,
    pub(crate) route_child: bool,
    pub(crate) command_picker: Option<Box<CommandPickerContext>>,
}

impl NavigationRequest {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::new(input)),
            query: None,
            route_child: false,
            command_picker: None,
        }
    }

    pub(crate) fn with_defaults(view_ref: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: None,
            query: None,
            route_child: false,
            command_picker: None,
        }
    }

    pub(crate) fn routed(
        view_ref: impl Into<String>,
        raw: impl Into<String>,
        params: impl Into<String>,
        cursor: usize,
    ) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::routed(raw, params, cursor)),
            query: None,
            route_child: true,
            command_picker: None,
        }
    }

    pub(crate) fn with_query(mut self, query: Value) -> Self {
        self.input = None;
        self.query = Some(query);
        self
    }

    pub(crate) fn with_command_picker(mut self, context: CommandPickerContext) -> Self {
        self.command_picker = Some(Box::new(context));
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigationMode {
    Push,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputEdit {
    ReplaceRange {
        start: usize,
        end: usize,
        replacement: String,
        cursor: usize,
    },
    SetBuffer {
        raw: String,
        cursor: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorAction {
    DeleteBackward,
    ClearInput,
    DeleteWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LauncherAction {
    Activate,
    MoveNext,
    MovePrevious,
    OpenCompletion,
    CycleCompletionNext,
    CycleCompletionPrevious,
    AcceptCompletion,
    CloseCompletion,
    DismissCompletion,
    Back,
    Exit,
    OpenCommandView,
    TogglePreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolvedLauncherAction {
    Edit(EditorAction),
    View(LauncherAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputRefreshPolicy {
    None,
    Debounced(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFocus {
    Focused,
    Unfocused,
}

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    pub(crate) id: String,
    pub(crate) source_view: String,
    pub(crate) command: Command,
}

pub(crate) enum ViewEffect {
    Continue,
    Navigate {
        request: NavigationRequest,
        mode: NavigationMode,
    },
    Back(Option<InputEdit>),
    Exit,
    Complete(CompletionRequest),
    RunCommand {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
        return_to_parent: bool,
    },
    RunEmbedded {
        prepared: PreparedProcess,
    },
}

pub(crate) enum LauncherOutcome {
    Continue,
    Effect(Box<ViewEffect>),
    EditInput(InputEdit),
    ReplayKey(Key),
}

#[derive(Debug, Clone)]
pub(crate) struct CompletionRequest {
    pub(crate) source_view: String,
    pub(crate) command_id: String,
    pub(crate) state: StateInstance,
    pub(crate) binding_raw: String,
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

    fn input_committed(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn input_refresh_policy(&self) -> InputRefreshPolicy {
        InputRefreshPolicy::None
    }

    fn input_ready(&mut self, _host: &mut EngineHost<'_>) -> Result<ViewEffect> {
        Ok(ViewEffect::Continue)
    }

    fn input_rejected(&mut self, _host: &mut EngineHost<'_>) -> Result<()> {
        Ok(())
    }

    fn step(&mut self, host: &mut EngineHost<'_>, terminal: &mut Terminal) -> Result<ViewEffect>;

    fn launcher_input_timeout(&self, _host: &EngineHost<'_>) -> Option<i32> {
        None
    }

    fn captures_editor_input(&self) -> bool {
        false
    }

    fn resolve_launcher_action(
        &self,
        _host: &EngineHost<'_>,
        _key: Key,
    ) -> Option<ResolvedLauncherAction> {
        None
    }

    fn handle_launcher_action(
        &mut self,
        _host: &mut EngineHost<'_>,
        _action: LauncherAction,
        _key: Key,
    ) -> Result<LauncherOutcome> {
        Ok(LauncherOutcome::Continue)
    }

    fn chrome(&self, _host: &EngineHost<'_>) -> crate::chrome::EngineChrome {
        crate::chrome::EngineChrome::default()
    }

    fn render(&mut self, host: &EngineHost<'_>, frame: &mut Frame, area: Rect);

    fn input_focus(&self) -> InputFocus {
        InputFocus::Focused
    }
}

pub(crate) struct ViewContext<'a> {
    pub(crate) config: &'a Config,
    pub(crate) request: &'a NavigationRequest,
    pub(crate) input: &'a InputBuffer,
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
