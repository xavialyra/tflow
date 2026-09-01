use super::completion::{CompletionRuntime, TransientSelectionRuntime};
use crate::command::{
    CommandContext, CommandOrigin, InputActionBinding, InputEdit, ResolvedInputAction, ViewEffect,
};
use crate::config::{CommandAction, CommandBindingVisibility, Config};
use crate::engine::{
    EngineBufferTarget, EngineCommandBinding, EngineCommandInvocation, EngineRuntime, ViewContext,
    ViewRenderer,
};
use crate::input::keymap::{InputContextId, InstructionTable, LayerId};
use crate::input::{
    CompletionHost, EditorBuffer, InputConsumer, InputContext, InputStrategy, InputTarget, Key,
    ViewMountId,
};
use crate::parameter::{ParameterBinding, ParameterSnapshot, ParameterState};
use anyhow::Result;
use std::collections::HashSet;
use std::time::Instant;

#[derive(Clone)]
pub(super) struct CallBoundary {
    pub(super) origin: CommandOrigin,
    pub(super) context: CommandContext,
    pub(super) then: Option<Box<CommandAction>>,
    pub(super) suppresses_session_bindings: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ViewInputLayers {
    pub(super) context: InputContextId,
    pub(super) input_context: InputContext,
    pub(super) actions: LayerId,
    pub(super) view_commands: LayerId,
    pub(super) engine_commands: LayerId,
    pub(super) session_commands: LayerId,
}

#[derive(Debug, Clone)]
pub(super) struct PendingCommand {
    pub(super) invocation: crate::command::CommandInvocation,
    pub(super) source: crate::input::InputSourceIdentity,
}

pub(super) struct ViewMount {
    pub(super) mount_id: ViewMountId,
    pub(super) context: ViewContext,
    pub(super) raw_receiver: Option<crate::input::ReceiverId>,
    pub(super) buffer_generation: u64,
    pub(super) parameter_binding: ParameterBinding,
    pub(super) definition: crate::config::ViewDefinition,
    pub(super) engine_definition: crate::engine::EngineDefinition,
    pub(super) view_ref: String,
    pub(super) input: EditorBuffer,
    pub(super) committed_buffer_projection: EditorBuffer,
    pub(super) input_dirty: bool,
    pub(super) input_deadline: Option<Instant>,
    pub(super) state: ParameterState,
    pub(super) input_layers: ViewInputLayers,
    pub(super) input_bindings: Vec<InputActionBinding>,
    pub(super) published_bindings: Option<PublishedBindings>,
    pub(super) pending_command: Option<PendingCommand>,
    pub(super) runtime: Box<dyn EngineRuntime>,
    pub(super) renderer: Box<dyn ViewRenderer>,
}

pub(super) struct ViewFrame {
    pub(super) mount: ViewMount,
    pub(super) call_boundary: Option<CallBoundary>,
}

impl std::ops::Deref for ViewFrame {
    type Target = ViewMount;

    fn deref(&self) -> &Self::Target {
        &self.mount
    }
}

impl std::ops::DerefMut for ViewFrame {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.mount
    }
}

pub(super) struct PreparedHostState<'a> {
    pub(super) input: EditorBuffer,
    pub(super) committed_buffer_projection: EditorBuffer,
    pub(super) state: ParameterState,
    pub(super) buffer_generation: u64,
    pub(super) input_dirty: bool,
    pub(super) input_deadline: Option<Instant>,
    pub(super) parameters: ParameterSnapshot,
    pub(super) runtime_snapshot: crate::engine::EngineRuntimeSnapshot,
    pub(super) publication: Option<&'a crate::engine::ViewContextPublication>,
}

impl ViewMount {
    pub(super) fn input_context(&self) -> InputContext {
        self.input_layers.input_context
    }

    pub(super) fn committed_input(&self) -> &str {
        self.state.raw_input()
    }

    pub(super) fn input_focus(&self) -> crate::engine::InputFocus {
        self.engine_definition.input.focus
    }

    pub(super) fn source_identity(&self) -> crate::input::InputSourceIdentity {
        crate::input::InputSourceIdentity {
            frame: self.mount_id,
            generation: self.buffer_generation,
        }
    }

    pub(super) fn clear_pending_command(&mut self) {
        self.pending_command = None;
    }

    pub(super) fn raw_receiver_for(
        &mut self,
        receiver: crate::input::ReceiverId,
    ) -> Result<Option<&mut dyn crate::engine::RawInputReceiver>> {
        if self.raw_receiver != Some(receiver) {
            return Ok(None);
        }
        let mount_id = self.mount_id;
        let raw_receiver = self.runtime.raw_receiver().ok_or_else(|| {
            anyhow::anyhow!(
                "mount {:?} no longer provides advertised raw receiver {:?}",
                mount_id,
                receiver
            )
        })?;
        Ok(Some(raw_receiver))
    }

    pub(super) fn mark_input_changed(&mut self) {
        self.buffer_generation = self.buffer_generation.wrapping_add(1);
        self.input_dirty = true;
        self.input_deadline = None;
        self.state.set_input_rejected(false);
        self.clear_pending_command();
    }

    pub(super) fn replace_input(&mut self, raw: String, cursor: usize) -> Result<()> {
        self.input = self
            .input
            .replaced_all(raw, cursor)
            .map_err(|error| anyhow::anyhow!("input edit was rejected: {error}"))?;
        self.mark_input_changed();
        Ok(())
    }

    pub(super) fn apply_prepared_host_state(&mut self, prepared: PreparedHostState<'_>) {
        self.input = prepared.input;
        self.committed_buffer_projection = prepared.committed_buffer_projection;
        self.state = prepared.state;
        self.buffer_generation = prepared.buffer_generation;
        self.input_dirty = prepared.input_dirty;
        self.input_deadline = prepared.input_deadline;
        self.context = self.context.committed(
            self.input.snapshot(),
            self.buffer_generation,
            prepared.parameters,
            self.state.input_rejected(),
            prepared.runtime_snapshot,
            prepared.publication,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublishedBindings {
    pub(super) input_strategy: InputStrategy,
    pub(super) actions: Vec<InputActionBinding>,
    pub(super) engine_identity: crate::engine::ViewContextIdentity,
    pub(super) engine_commands: Vec<EngineCommandBinding>,
    pub(super) session_suppressed: bool,
}

pub(super) fn validate_engine_input_bindings(
    bindings: &[InputActionBinding],
    definition: &crate::engine::EngineDefinition,
) -> Result<()> {
    let mut seen_keys = HashSet::new();
    for binding in bindings {
        anyhow::ensure!(
            seen_keys.insert(binding.key.binding_identity()),
            "key {:?} is registered more than once in one input layer",
            binding.key.binding_name()
        );
        if let ResolvedInputAction::Engine(action) = &binding.action {
            anyhow::ensure!(
                definition.action(action).is_some(),
                "engine binding {:?} is not declared by engine {:?}",
                action.as_str(),
                definition.kind
            );
        }
    }
    Ok(())
}

pub(super) fn validate_engine_command_bindings(
    bindings: &[EngineCommandBinding],
    config: &Config,
) -> Result<()> {
    let mut seen_keys = HashSet::new();
    for binding in bindings {
        anyhow::ensure!(
            !binding.command.owner.is_empty() && !binding.command.id.is_empty(),
            "engine command binding has an empty qualified command id"
        );
        anyhow::ensure!(
            seen_keys.insert(binding.key.binding_identity()),
            "key {:?} is registered more than once in one engine command layer",
            binding.key.binding_name()
        );
        anyhow::ensure!(
            config
                .view(&binding.command.owner)
                .is_some_and(|view| view.commands.contains_key(&binding.command.id)),
            "engine command {}/{} is not statically configured",
            binding.command.owner,
            binding.command.id
        );
    }
    Ok(())
}

pub(super) struct CompletionState {
    pub(super) host: CompletionHost,
    pub(super) runtime: Box<dyn TransientSelectionRuntime>,
}

impl Default for CompletionState {
    fn default() -> Self {
        Self {
            host: CompletionHost::default(),
            runtime: Box::new(CompletionRuntime::default()),
        }
    }
}

impl CompletionState {
    pub(super) fn render_model(&self) -> super::completion::SelectionRenderModel {
        self.runtime.render_model()
    }
}

pub(super) enum ReturnTransition {
    Effect(Box<ViewEffect>),
    Outcome(super::SessionOutcome),
}

pub(super) enum InputBinding {
    ViewCommand(crate::command::CommandRef),
    EngineCommand {
        invocation: EngineCommandInvocation,
        pending: bool,
    },
    SessionCommand(Key),
    EditorAction(crate::command::EditorAction),
    EngineAction(crate::engine::ActionId),
    Completion(RouteAction),
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteAction {
    Next,
    Previous,
    Accept,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BindingTarget {
    EditorAction(crate::command::EditorAction),
    EngineAction(crate::engine::ActionId),
    EngineCommand(EngineCommandInvocation),
    ViewCommand(crate::command::CommandRef),
    SessionCommand(Key),
    Completion(RouteAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BindingHint {
    pub(super) label: String,
    pub(super) visibility: CommandBindingVisibility,
}

pub(super) type ChromeHint = (String, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RegisteredBinding {
    pub(super) owner: InputConsumer,
    pub(super) target: BindingTarget,
    pub(super) hint: Option<BindingHint>,
}

pub(super) const ACTION_PRIORITY: i16 = 100;
pub(super) const VIEW_COMMAND_PRIORITY: i16 = 200;
pub(super) const ENGINE_COMMAND_PRIORITY: i16 = 210;
pub(super) const SESSION_COMMAND_PRIORITY: i16 = 300;

pub(super) fn mount_view_input_layers(
    router: &mut InstructionTable<RegisteredBinding>,
    owner: ViewMountId,
    strategy: InputStrategy,
    buffer_target: Option<EngineBufferTarget>,
    raw_receiver: Option<crate::input::ReceiverId>,
) -> ViewInputLayers {
    let context = router.create_context();
    let input_buffer_target = buffer_target.map(|_| InputTarget::EditorBuffer);
    let input_context = match strategy {
        InputStrategy::Decoded => InputContext::decoded(
            context,
            InputConsumer::Mount(owner),
            input_buffer_target,
            if input_buffer_target.is_some() {
                InputTarget::EditorBuffer
            } else {
                InputTarget::Ignore
            },
        ),
        InputStrategy::RawIntercepted => InputContext::raw(
            context,
            InputConsumer::Mount(owner),
            raw_receiver
                .map(InputTarget::Pty)
                .unwrap_or(InputTarget::Ignore),
        ),
    };
    ViewInputLayers {
        context,
        input_context,
        actions: router.mount_layer(context, ACTION_PRIORITY),
        view_commands: router.mount_layer(context, VIEW_COMMAND_PRIORITY),
        engine_commands: router.mount_layer(context, ENGINE_COMMAND_PRIORITY),
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
