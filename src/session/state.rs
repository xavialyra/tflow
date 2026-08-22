use crate::chrome::InputBuffer;
use crate::config::{CommandAction, CommandBindingVisibility};
use crate::engine::{
    CommandContext, CommandOrigin, InputActionBinding, InputEdit, ResolvedInputAction,
    SelectionBindingState, ViewEffect, ViewInputMode, ViewInstance,
};
use crate::input::Key;
use crate::input::keymap::{InputContextId, InputRouter, LayerId};
use crate::state::StateInstance;
use std::time::Instant;

pub(super) struct CallBoundary {
    pub(super) origin: CommandOrigin,
    pub(super) context: CommandContext,
    pub(super) then: Option<Box<CommandAction>>,
    pub(super) suppresses_session_bindings: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ViewInputLayers {
    pub(super) context: InputContextId,
    pub(super) actions: LayerId,
    pub(super) view_commands: LayerId,
    pub(super) selection_commands: LayerId,
    pub(super) session_commands: LayerId,
}

pub(super) struct ViewEntry {
    pub(super) view_ref: String,
    pub(super) input: InputBuffer,
    pub(super) input_dirty: bool,
    pub(super) input_deadline: Option<Instant>,
    pub(super) state: StateInstance,
    pub(super) input_layers: ViewInputLayers,
    pub(super) published_bindings: Option<PublishedBindings>,
    pub(super) call_boundary: Option<CallBoundary>,
    pub(super) instance: Box<dyn ViewInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublishedBindings {
    pub(super) input_mode: ViewInputMode,
    pub(super) actions: Vec<InputActionBinding>,
    pub(super) selection: SelectionBindingState,
    pub(super) session_suppressed: bool,
}

#[derive(Debug, Clone)]
pub(super) struct RouteCompletion {
    pub(super) input_context: InputContextId,
    pub(super) candidates: Vec<crate::router::ViewCandidate>,
    pub(super) selected: usize,
    pub(super) selector_end: usize,
}

pub(super) enum ReturnTransition {
    Effect(Box<ViewEffect>),
    Outcome(super::SessionOutcome),
}

pub(super) enum InputBinding {
    ViewCommand(Key),
    PendingViewCommand(Key),
    SessionCommand(Key),
    Action(ResolvedInputAction),
    Route(RouteAction),
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteAction {
    Next,
    Previous,
    Accept,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingTarget {
    Action(ResolvedInputAction),
    ViewCommand(Key),
    SessionCommand(Key),
    Route(RouteAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BindingHint {
    pub(super) label: String,
    pub(super) visibility: CommandBindingVisibility,
}

pub(super) type ChromeHint = (String, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RegisteredBinding {
    pub(super) target: BindingTarget,
    pub(super) hint: Option<BindingHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputGrammar {
    Decoded,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InputContext {
    pub(super) id: InputContextId,
    pub(super) grammar: InputGrammar,
}

pub(super) const ACTION_PRIORITY: i16 = 100;
pub(super) const VIEW_COMMAND_PRIORITY: i16 = 200;
pub(super) const SELECTION_COMMAND_PRIORITY: i16 = 210;
pub(super) const SESSION_COMMAND_PRIORITY: i16 = 300;

pub(super) fn mount_view_input_layers(
    router: &mut InputRouter<RegisteredBinding>,
) -> ViewInputLayers {
    let context = router.create_context();
    ViewInputLayers {
        context,
        actions: router.mount_layer(context, ACTION_PRIORITY),
        view_commands: router.mount_layer(context, VIEW_COMMAND_PRIORITY),
        selection_commands: router.mount_layer(context, SELECTION_COMMAND_PRIORITY),
        session_commands: router.mount_layer(context, SESSION_COMMAND_PRIORITY),
    }
}

pub(super) fn prepared_action_effect(action: crate::command::PreparedAction) -> ViewEffect {
    match action {
        crate::command::PreparedAction::Navigate { request, mode } => ViewEffect::Navigate {
            request,
            mode,
            parent_edit: None,
        },
        crate::command::PreparedAction::Call(call) => ViewEffect::Call(call),
        crate::command::PreparedAction::Return(returned) => ViewEffect::Return(returned),
        crate::command::PreparedAction::EditInput { value, cursor } => {
            ViewEffect::EditInput(InputEdit::SetBuffer { raw: value, cursor })
        }
        crate::command::PreparedAction::Invoke(execution) => ViewEffect::DispatchCommand(execution),
        crate::command::PreparedAction::Execute {
            invocation,
            prepared,
            exit,
        } => ViewEffect::RunCommand {
            invocation,
            prepared,
            exit,
        },
    }
}
