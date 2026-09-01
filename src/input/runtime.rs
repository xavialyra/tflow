use super::keymap::InputContextId;
use super::{DecodedInput, EditorBuffer, EditorSnapshot};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct ViewMountId(pub(crate) u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct CompletionId(pub(crate) u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct ReceiverId(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InputConsumer {
    Mount(ViewMountId),
    Completion(CompletionId),
    Session,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InputStrategy {
    #[default]
    Decoded,
    RawIntercepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputTarget {
    EditorBuffer,
    Pty(ReceiverId),
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputEvent {
    Key(DecodedInput),
    Raw(Vec<u8>),
    Paste(Vec<u8>),
    Eof,
}

impl InputEvent {
    pub(crate) fn decoded(input: DecodedInput) -> Self {
        if input.raw.starts_with(b"\x1b[200~") {
            Self::Paste(input.raw)
        } else {
            Self::Key(input)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EventDisposition {
    DelegateTo(InputTarget),
    CloseAndReplay(InputContextId, InputEvent),
    ForwardRaw(ReceiverId, Vec<u8>),
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputContext {
    pub(crate) id: InputContextId,
    pub(crate) owner: InputConsumer,
    pub(crate) strategy: InputStrategy,
    pub(crate) buffer_target: Option<InputTarget>,
    pub(crate) unmatched_target: InputTarget,
    pub(crate) replay_context: Option<InputContextId>,
}

impl Default for InputContext {
    fn default() -> Self {
        Self {
            id: InputContextId::default(),
            owner: InputConsumer::Session,
            strategy: InputStrategy::Decoded,
            buffer_target: None,
            unmatched_target: InputTarget::Ignore,
            replay_context: None,
        }
    }
}

impl InputContext {
    pub(crate) fn decoded(
        id: InputContextId,
        owner: InputConsumer,
        buffer_target: Option<InputTarget>,
        unmatched_target: InputTarget,
    ) -> Self {
        Self {
            id,
            owner,
            strategy: InputStrategy::Decoded,
            buffer_target,
            unmatched_target,
            replay_context: None,
        }
    }

    pub(crate) fn raw(
        id: InputContextId,
        owner: InputConsumer,
        unmatched_target: InputTarget,
    ) -> Self {
        Self {
            id,
            owner,
            strategy: InputStrategy::RawIntercepted,
            buffer_target: None,
            unmatched_target,
            replay_context: None,
        }
    }

    pub(crate) fn unmatched_receiver(&self) -> Option<ReceiverId> {
        match self.unmatched_target {
            InputTarget::Pty(receiver) => Some(receiver),
            InputTarget::EditorBuffer | InputTarget::Ignore => None,
        }
    }

    pub(crate) fn disposition_for_unmatched(&self, event: InputEvent) -> EventDisposition {
        if let Some(parent) = self.replay_context {
            return EventDisposition::CloseAndReplay(parent, event);
        }
        match (self.unmatched_target, event) {
            (InputTarget::Pty(receiver), InputEvent::Raw(bytes) | InputEvent::Paste(bytes)) => {
                EventDisposition::ForwardRaw(receiver, bytes)
            }
            (InputTarget::Pty(receiver), InputEvent::Key(input)) => {
                EventDisposition::ForwardRaw(receiver, input.raw)
            }
            (InputTarget::EditorBuffer, _) => {
                EventDisposition::DelegateTo(InputTarget::EditorBuffer)
            }
            (InputTarget::Ignore, _) => EventDisposition::Ignore,
            (_, InputEvent::Eof) => EventDisposition::Ignore,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompletionHost {
    pub(crate) parent: CompletionSnapshot,
    pub(crate) context: InputContext,
}

impl CompletionHost {
    pub(crate) fn new(parent: CompletionSnapshot, context: InputContext) -> Self {
        Self { parent, context }
    }

    pub(crate) fn unmatched(&self, event: InputEvent) -> EventDisposition {
        self.context.disposition_for_unmatched(event)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CompletionSnapshot {
    pub(crate) source_identity: InputSourceIdentity,
    pub(crate) buffer_revision: u64,
    pub(crate) raw: String,
    pub(crate) cursor: usize,
    pub(crate) replacement_range: std::ops::Range<usize>,
    pub(crate) parameter_input: String,
}

impl CompletionSnapshot {
    pub(crate) fn from_buffer(
        source_identity: InputSourceIdentity,
        buffer: &EditorBuffer,
        replacement_range: std::ops::Range<usize>,
        parameter_input: impl Into<String>,
    ) -> Self {
        let snapshot = buffer.snapshot();
        Self::from_editor_snapshot(
            source_identity,
            snapshot,
            replacement_range,
            parameter_input,
        )
    }

    fn from_editor_snapshot(
        source_identity: InputSourceIdentity,
        snapshot: EditorSnapshot,
        replacement_range: std::ops::Range<usize>,
        parameter_input: impl Into<String>,
    ) -> Self {
        Self {
            source_identity,
            buffer_revision: snapshot.revision,
            raw: snapshot.raw,
            cursor: snapshot.cursor,
            replacement_range,
            parameter_input: parameter_input.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct InputSourceIdentity {
    pub(crate) frame: ViewMountId,
    pub(crate) generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionEdit {
    pub(crate) source_identity: InputSourceIdentity,
    pub(crate) source_revision: u64,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) replacement: String,
    pub(crate) cursor: usize,
}

impl CompletionEdit {
    pub(crate) fn apply(
        &self,
        identity: InputSourceIdentity,
        buffer: &mut EditorBuffer,
    ) -> Result<(), super::BufferEditError> {
        if self.source_identity != identity {
            return Err(super::BufferEditError::RevisionMismatch {
                expected: self.source_revision,
                actual: buffer.revision,
            });
        }
        buffer.replace_range(
            self.source_revision,
            self.range.clone(),
            &self.replacement,
            self.cursor,
        )
    }
}
