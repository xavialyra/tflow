use super::{DecodedInput, Key};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct ViewMountId(pub(crate) u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputEvent {
    /// A decoded key together with the exact bytes that produced it.
    Key {
        key: Key,
        raw: Vec<u8>,
    },
    /// A bracketed paste. Invalid UTF-8 is retained in `raw` and exposed as
    /// `None` rather than being replaced or discarded.
    Paste {
        text: Option<String>,
        raw: Vec<u8>,
    },
    /// Bytes that the decoder could not interpret as a key or paste.
    Bytes(Vec<u8>),
    Eof,
}

impl From<DecodedInput> for InputEvent {
    fn from(input: DecodedInput) -> Self {
        Self::decoded(input)
    }
}

impl InputEvent {
    pub(crate) fn decoded(input: DecodedInput) -> Self {
        if input.raw.starts_with(b"\x1b[200~") {
            let start = b"\x1b[200~";
            let end = b"\x1b[201~";
            let text = input
                .raw
                .strip_prefix(start)
                .and_then(|bytes| bytes.strip_suffix(end))
                .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());
            Self::Paste {
                text,
                raw: input.raw,
            }
        } else if let Some(key) = input.key {
            Self::Key {
                key,
                raw: input.raw,
            }
        } else {
            Self::Bytes(input.raw)
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct InputSourceIdentity {
    pub(crate) frame: ViewMountId,
    pub(crate) generation: u64,
}
