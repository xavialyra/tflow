use crate::engine::ActionId;
use crate::input::Key;
use crate::workflow::config::{Command, ViewPresentation};
use crate::workflow::parameter::ParameterSnapshot;
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
}

impl NavigationRequest {
    pub(crate) fn new(view_ref: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: Some(InputSeed::new(input)),
            parameters: None,
            presentation: ViewPresentation::default(),
        }
    }

    pub(crate) fn with_defaults(view_ref: impl Into<String>) -> Self {
        Self {
            view_ref: view_ref.into(),
            input: None,
            parameters: None,
            presentation: ViewPresentation::default(),
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
}

#[derive(Debug, Clone)]
pub(crate) struct CommandContext {
    pub(crate) page: CommandOwnerContext,
    pub(crate) owner: CommandOwnerContext,
    pub(crate) current: Value,
    pub(crate) engine_type: String,
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
    pub(crate) return_processor: Option<crate::workflow::config::ReturnProcessor>,
}
