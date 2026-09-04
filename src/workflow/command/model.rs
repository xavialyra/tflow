use crate::config::{Command, CommandAction, ViewPresentation};
use crate::engine::ActionId;
use crate::input::Key;
use crate::parameter::ParameterSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavigationRequest {
    pub(crate) view_ref: String,
    pub(crate) input: Option<InputSeed>,
    pub(crate) parameters: Option<Value>,
    pub(crate) presentation: ViewPresentation,
    pub(crate) engine_options: Option<Value>,
}

impl NavigationRequest {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::new(input)),
            parameters: None,
            presentation: ViewPresentation::default(),
            engine_options: None,
        }
    }

    pub(crate) fn with_defaults(view_ref: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: None,
            parameters: None,
            presentation: ViewPresentation::default(),
            engine_options: None,
        }
    }

    pub(crate) fn with_parameters(mut self, parameters: Value) -> Self {
        self.input = None;
        self.parameters = Some(parameters);
        self
    }

    pub(crate) fn with_presentation(mut self, presentation: ViewPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub(crate) fn with_engine_options(mut self, engine_options: Value) -> Self {
        self.engine_options = Some(engine_options);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NavigationMode {
    Push,
    Replace,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorAction {
    DeleteBackward,
    ClearInput,
    DeleteWord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedInputAction {
    Edit(EditorAction),
    Engine(ActionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputActionBinding {
    pub(crate) key: Key,
    pub(crate) action: ResolvedInputAction,
    pub(crate) label: Option<String>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandRef {
    pub(crate) view: String,
    pub(crate) id: String,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandOrigin {
    View(CommandRef),
    Session {
        view: String,
        command: String,
        definition: Box<Command>,
    },
}

impl CommandOrigin {
    pub(crate) fn source_view(&self) -> &str {
        match self {
            Self::View(reference) => &reference.view,
            Self::Session { view, .. } => view,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::View(reference) => &reference.id,
            Self::Session { command, .. } => command,
        }
    }

    pub(crate) fn view_reference(&self) -> Option<&CommandRef> {
        match self {
            Self::View(reference) => Some(reference),
            Self::Session { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
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

    pub(crate) fn session_command(
        view: impl Into<String>,
        command_id: impl Into<String>,
        command: Command,
    ) -> Self {
        Self {
            origin: CommandOrigin::Session {
                view: view.into(),
                command: command_id.into(),
                definition: Box::new(command.clone()),
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
    pub(crate) parameters: ParameterSnapshot,
    pub(crate) binding_raw: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandContext {
    pub(crate) page: CommandOwnerContext,
    pub(crate) owner: CommandOwnerContext,
    pub(crate) current: Value,
    pub(crate) current_fields: &'static [&'static str],
    pub(crate) runtime: Value,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ViewOutputItem {
    pub(crate) text: String,
    pub(crate) value: Option<String>,
    pub(crate) metadata: Value,
    pub(crate) source_view: String,
}
