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

/// A command reference by stable identity and the snapshot revision it was
/// resolved against. This is the wire shape for `invoke-command` and for a
/// command's returned `ref`; the owning View is a dispatch-origin detail and is
/// never part of the reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandRef {
    pub(crate) id: String,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandOrigin {
    View { view: String, reference: CommandRef },
}

impl CommandOrigin {
    pub(crate) fn source_view(&self) -> &str {
        match self {
            Self::View { view, .. } => view,
        }
    }

    pub(crate) fn id(&self) -> &str {
        match self {
            Self::View { reference, .. } => &reference.id,
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

    pub(crate) fn view(view: impl Into<String>, reference: CommandRef, command: Command) -> Self {
        Self::from_origin(
            CommandOrigin::View {
                view: view.into(),
                reference,
            },
            command,
        )
    }

    pub(crate) fn origin(&self) -> CommandOrigin {
        self.origin.clone()
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
    pub(crate) commands: Value,
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
