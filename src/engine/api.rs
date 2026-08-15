use super::host::EngineHost;
use super::process::PreparedProcess;
use super::runtime::RuntimeHandle;
use super::task::TaskScheduler;
use crate::chrome::InputBuffer;
use crate::config::{Command, CommandAction, Config, View};
use crate::input::{DecodedInput, Key};
use crate::state::StateInstance;
use crate::terminal::Terminal;
use anyhow::Result;
use ratatui::{Frame, layout::Rect};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
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

    pub(crate) fn routed(query: impl Into<String>, cursor: usize) -> Self {
        let query = query.into();
        Self {
            raw: query.clone(),
            params: query,
            cursor,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavigationRequest {
    pub(crate) view_ref: String,
    pub(crate) input: Option<InputSeed>,
    pub(crate) query: Option<Value>,
    pub(crate) args: Option<Value>,
    pub(crate) route_child: bool,
}

impl NavigationRequest {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::new(input)),
            query: None,
            args: None,
            route_child: false,
        }
    }

    pub(crate) fn with_defaults(view_ref: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: None,
            query: None,
            args: None,
            route_child: false,
        }
    }

    pub(crate) fn routed(
        view_ref: impl Into<String>,
        query: impl Into<String>,
        cursor: usize,
    ) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::routed(query, cursor)),
            query: None,
            args: None,
            route_child: true,
        }
    }

    pub(crate) fn with_query(mut self, query: Value) -> Self {
        self.input = None;
        self.query = Some(query);
        self
    }

    pub(crate) fn with_args(mut self, args: Value) -> Self {
        self.args = Some(args);
        self
    }

    pub(crate) fn reference_value(&self) -> Option<Value> {
        self.args.as_ref().map(|args| json!({"args": args}))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigationMode {
    Push,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputEdit {
    SetBuffer { raw: String, cursor: usize },
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
    Back,
    Exit,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandRef {
    pub(crate) view: String,
    pub(crate) id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddedResultFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmbeddedResultConfig {
    pub(crate) format: EmbeddedResultFormat,
    pub(crate) required: bool,
    pub(crate) max_bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandOrigin {
    View(CommandRef),
    ChromeFooter { view: String, binding: String },
}

impl CommandOrigin {
    pub(crate) fn source_view(&self) -> &str {
        match self {
            Self::View(reference) => &reference.view,
            Self::ChromeFooter { view, .. } => view,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::View(reference) => &reference.id,
            Self::ChromeFooter { binding, .. } => binding,
        }
    }

    pub(crate) fn view_reference(&self) -> Option<&CommandRef> {
        match self {
            Self::View(reference) => Some(reference),
            Self::ChromeFooter { .. } => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    origin: CommandOrigin,
    pub(crate) command: Command,
}

impl CommandInvocation {
    pub(crate) fn from_origin(origin: CommandOrigin, command: Command) -> Self {
        Self { origin, command }
    }

    pub(crate) fn view(reference: CommandRef, command: Command) -> Self {
        Self::from_origin(CommandOrigin::View(reference), command)
    }

    pub(crate) fn chrome_footer(
        view: impl Into<String>,
        binding: impl Into<String>,
        command: Command,
    ) -> Self {
        Self {
            origin: CommandOrigin::ChromeFooter {
                view: view.into(),
                binding: binding.into(),
            },
            command,
        }
    }

    pub(crate) fn origin(&self) -> CommandOrigin {
        self.origin.clone()
    }

    pub(crate) fn view_reference(&self) -> Option<&CommandRef> {
        self.origin.view_reference()
    }

    pub(crate) fn is_chrome_footer(&self) -> bool {
        matches!(self.origin, CommandOrigin::ChromeFooter { .. })
    }

    pub(crate) fn id(&self) -> &str {
        self.origin.id()
    }

    pub(crate) fn source_view(&self) -> &str {
        self.origin.source_view()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CommandOwnerContext {
    pub(crate) view_ref: String,
    pub(crate) state: StateInstance,
    pub(crate) binding_raw: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandSelectionContext {
    pub(crate) owner: CommandOwnerContext,
    pub(crate) item: ViewOutputItem,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandContext {
    pub(crate) page: CommandOwnerContext,
    pub(crate) selection: Option<CommandSelectionContext>,
    pub(crate) runtime: Value,
    pub(crate) request: Option<Value>,
    pub(crate) output: Option<ViewOutput>,
    pub(crate) log_file: Option<PathBuf>,
}

#[derive(Clone)]
pub(crate) struct CommandExecution {
    pub(crate) invocation: CommandInvocation,
    pub(crate) context: CommandContext,
}

pub(crate) struct CallRequest {
    pub(crate) request: NavigationRequest,
    pub(crate) origin: CommandOrigin,
    pub(crate) context: CommandContext,
    pub(crate) then: Option<Box<CommandAction>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReturnAdapter {
    pub(crate) command: CommandRef,
    pub(crate) context: CommandContext,
}

#[derive(Debug, Clone)]
pub(crate) struct ViewReturn {
    pub(crate) source_view: String,
    pub(crate) output: ViewOutput,
    pub(crate) adapter: Option<ReturnAdapter>,
}

pub(crate) enum ViewEffect {
    Continue,
    DispatchCommand(CommandExecution),
    Navigate {
        request: NavigationRequest,
        mode: NavigationMode,
    },
    Call(CallRequest),
    Return(ViewReturn),
    EditInput(InputEdit),
    Back(Option<InputEdit>),
    Exit,
    RunCommand {
        invocation: CommandInvocation,
        prepared: PreparedProcess,
        exit: bool,
    },
    RunEmbedded {
        prepared: PreparedProcess,
        result: Option<EmbeddedResultConfig>,
        escape_cancels: bool,
    },
}

pub(crate) enum LauncherOutcome {
    Continue,
    Effect(Box<ViewEffect>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum ViewOutput {
    Selected {
        item: Option<ViewOutputItem>,
        input: String,
    },
    Value {
        value: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

    fn resolve_view_command(&self, host: &EngineHost<'_>, key: Key) -> Option<CommandInvocation> {
        crate::engine::command::find_command_for_key(host.config, host.state.view_ref(), key)
    }

    fn view_command_output(&self) -> Option<ViewOutput> {
        None
    }

    fn view_command_context(&self, host: &mut EngineHost<'_>) -> Result<CommandContext> {
        Ok(CommandContext {
            page: CommandOwnerContext {
                view_ref: host.state.view_ref().to_string(),
                state: host.state.clone(),
                binding_raw: host.input.params.clone(),
            },
            selection: None,
            runtime: host.runtime.snapshot().clone(),
            request: host.request.clone(),
            output: self.view_command_output(),
            log_file: host.runtime_log.path().map(std::path::Path::to_path_buf),
        })
    }

    fn prepare_view_command(
        &self,
        host: &mut EngineHost<'_>,
        invocation: CommandInvocation,
    ) -> Result<CommandExecution> {
        let context = self.view_command_context(host)?;
        crate::engine::command::resolve_visible_command(
            host.config,
            &context,
            invocation
                .view_reference()
                .expect("View command invocation has a footer origin"),
        )?;
        Ok(CommandExecution {
            invocation,
            context,
        })
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
        _input: DecodedInput,
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

    fn chrome_footer_enabled(&self) -> bool {
        true
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
