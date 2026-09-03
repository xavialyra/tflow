mod editor;
pub(crate) mod keymap;
mod pipeline;
mod runtime;

pub(crate) use editor::{EditorBuffer, EditorSnapshot, previous_char_boundary};
pub(crate) use pipeline::InputPipeline;
pub(crate) use runtime::{InputEvent, InputSourceIdentity, ViewMountId};

use anyhow::{Context, Result, bail};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Char(char),
    Alt(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
    Escape,
    Ctrl(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingKey(Key);

impl BindingKey {
    pub(crate) fn from_key(key: Key) -> Self {
        Self(match key {
            Key::Char(character) if character.is_ascii_alphabetic() => {
                Key::Char(character.to_ascii_lowercase())
            }
            Key::Alt(character) => Key::Alt(character.to_ascii_lowercase()),
            Key::Ctrl(character) => Key::Ctrl(character.to_ascii_lowercase()),
            key => key,
        })
    }

    pub(crate) fn key(self) -> Key {
        self.0
    }
}

impl Key {
    pub(crate) fn binding_identity(self) -> BindingKey {
        BindingKey::from_key(self)
    }

    pub(crate) fn parse_binding(source: &str) -> Result<Self> {
        let normalized = source.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "enter" | "ctrl+j" | "ctrl+m" => Ok(Self::Enter),
            "tab" | "ctrl+i" => Ok(Self::Tab),
            "shift+tab" | "backtab" => Ok(Self::BackTab),
            "backspace" | "ctrl+h" => Ok(Self::Backspace),
            "delete" => Ok(Self::Delete),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "home" => Ok(Self::Home),
            "end" => Ok(Self::End),
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "escape" | "esc" => Ok(Self::Escape),
            "space" => Ok(Self::Char(' ')),
            _ if normalized.chars().count() == 1
                && normalized
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_graphic()) =>
            {
                Ok(Self::Char(
                    normalized.chars().next().expect("one character"),
                ))
            }
            _ => {
                let (modifier, value) = normalized
                    .split_once('+')
                    .with_context(|| format!("unsupported key binding {:?}", source))?;
                let mut characters = value.chars();
                let character = characters
                    .next()
                    .with_context(|| format!("key binding {:?} has no key", source))?;
                if characters.next().is_some() || !character.is_ascii_graphic() {
                    bail!("key binding {:?} must contain one ASCII key", source);
                }
                match modifier {
                    "alt" => Ok(Self::Alt(character)),
                    "ctrl" if character.is_ascii_alphabetic() => Ok(Self::Ctrl(character)),
                    "ctrl" => bail!("key binding {:?} requires a Ctrl letter", source),
                    _ => bail!("unsupported key modifier {:?}", modifier),
                }
            }
        }
    }

    pub(crate) fn binding_name(self) -> Option<String> {
        match self {
            Self::Enter => Some("enter".to_string()),
            Self::Tab => Some("tab".to_string()),
            Self::BackTab => Some("shift+tab".to_string()),
            Self::Backspace => Some("backspace".to_string()),
            Self::Delete => Some("delete".to_string()),
            Self::Left => Some("left".to_string()),
            Self::Right => Some("right".to_string()),
            Self::Home => Some("home".to_string()),
            Self::End => Some("end".to_string()),
            Self::Up => Some("up".to_string()),
            Self::Down => Some("down".to_string()),
            Self::Escape => Some("escape".to_string()),
            Self::Alt(character) => Some(format!("alt+{}", character.to_ascii_lowercase())),
            Self::Ctrl(character) => Some(format!("ctrl+{}", character.to_ascii_lowercase())),
            Self::Char(' ') => Some("space".to_string()),
            Self::Char(character) if character.is_ascii_graphic() => {
                Some(character.to_ascii_lowercase().to_string())
            }
            Self::Char(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedInput {
    pub(crate) key: Option<Key>,
    pub(crate) raw: Vec<u8>,
}

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Default)]
pub(crate) struct InputDecoder {
    pending: Vec<u8>,
    escape_since: Option<Instant>,
}

impl InputDecoder {
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<DecodedInput> {
        let mut expired_escape = Vec::new();
        if self.pending.as_slice() == b"\x1b"
            && (bytes.len() > 1
                || self
                    .escape_since
                    .is_some_and(|started| started.elapsed() >= Duration::from_millis(35)))
        {
            self.escape_since = None;
            expired_escape.push(self.take(1, Some(Key::Escape)));
        }
        self.pending.extend_from_slice(bytes);
        expired_escape.extend(self.parse());
        expired_escape
    }

    pub(crate) fn take_pending_raw(&mut self) -> Vec<u8> {
        self.escape_since = None;
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn flush_due(&mut self) -> Vec<DecodedInput> {
        if self
            .escape_since
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(35))
            && self.pending.as_slice() == b"\x1b"
        {
            self.escape_since = None;
            return vec![self.take(1, Some(Key::Escape))];
        }
        Vec::new()
    }

    fn parse(&mut self) -> Vec<DecodedInput> {
        let mut inputs = Vec::new();
        loop {
            let Some(&first) = self.pending.first() else {
                self.escape_since = None;
                break;
            };

            if first == 0x1b {
                if self.pending.len() == 1 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                self.escape_since = None;

                if self.pending.starts_with(BRACKETED_PASTE_START) {
                    let content = &self.pending[BRACKETED_PASTE_START.len()..];
                    let Some(end) = content
                        .windows(BRACKETED_PASTE_END.len())
                        .position(|window| window == BRACKETED_PASTE_END)
                    else {
                        break;
                    };
                    let count = BRACKETED_PASTE_START.len() + end + BRACKETED_PASTE_END.len();
                    inputs.push(self.take(count, None));
                    continue;
                }
                if BRACKETED_PASTE_START.starts_with(&self.pending) {
                    break;
                }

                if matches!(self.pending[1], b'[' | b'O') {
                    let Some(end) = self.pending[2..]
                        .iter()
                        .position(|byte| (0x40..=0x7e).contains(byte))
                        .map(|position| position + 2)
                    else {
                        break;
                    };
                    let params = &self.pending[2..end];
                    let code = self.pending[end];
                    let key = if self.pending[1] == b'O' {
                        match code {
                            b'A' => Some(Key::Up),
                            b'B' => Some(Key::Down),
                            b'C' => Some(Key::Right),
                            b'D' => Some(Key::Left),
                            b'H' => Some(Key::Home),
                            b'F' => Some(Key::End),
                            _ => None,
                        }
                    } else {
                        match code {
                            b'A' => Some(Key::Up),
                            b'B' => Some(Key::Down),
                            b'C' => Some(Key::Right),
                            b'D' => Some(Key::Left),
                            b'H' => Some(Key::Home),
                            b'F' => Some(Key::End),
                            b'Z' => Some(Key::BackTab),
                            b'~' => match params
                                .split(|byte| *byte == b';')
                                .next()
                                .and_then(|value| std::str::from_utf8(value).ok())
                                .and_then(|value| value.parse::<u8>().ok())
                            {
                                Some(1 | 7) => Some(Key::Home),
                                Some(3) => Some(Key::Delete),
                                Some(4 | 8) => Some(Key::End),
                                _ => None,
                            },
                            _ => None,
                        }
                    };
                    inputs.push(self.take(end + 1, key));
                    continue;
                }

                if self.pending[1].is_ascii_graphic() {
                    let character = (self.pending[1] as char).to_ascii_lowercase();
                    inputs.push(self.take(2, Some(Key::Alt(character))));
                    continue;
                }
                inputs.push(self.take(1, Some(Key::Escape)));
                continue;
            }

            if let Some(key) = control_key(first) {
                inputs.push(self.take(1, Some(key)));
                continue;
            }

            if first < 0x80 {
                inputs.push(self.take(1, Some(Key::Char(first as char))));
                continue;
            }

            let width = utf8_width(first);
            if self.pending.len() < width {
                break;
            }
            let key = std::str::from_utf8(&self.pending[..width])
                .ok()
                .and_then(|text| text.chars().next())
                .map(Key::Char);
            inputs.push(self.take(if key.is_some() { width } else { 1 }, key));
        }
        inputs
    }

    fn take(&mut self, count: usize, key: Option<Key>) -> DecodedInput {
        DecodedInput {
            key,
            raw: self.pending.drain(..count).collect(),
        }
    }
}

fn control_key(byte: u8) -> Option<Key> {
    match byte {
        b'\r' | b'\n' => Some(Key::Enter),
        b'\t' => Some(Key::Tab),
        0x7f | 0x08 => Some(Key::Backspace),
        0x01..=0x1a => Some(Key::Ctrl((b'a' + byte - 1) as char)),
        _ => None,
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(key: Option<Key>, raw: &[u8]) -> DecodedInput {
        DecodedInput {
            key,
            raw: raw.to_vec(),
        }
    }

    #[test]
    fn decodes_ascii_and_controls_without_losing_raw_bytes() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"a\r\x01\x03\x0b\x15\x17"),
            vec![
                decoded(Some(Key::Char('a')), b"a"),
                decoded(Some(Key::Enter), b"\r"),
                decoded(Some(Key::Ctrl('a')), b"\x01"),
                decoded(Some(Key::Ctrl('c')), b"\x03"),
                decoded(Some(Key::Ctrl('k')), b"\x0b"),
                decoded(Some(Key::Ctrl('u')), b"\x15"),
                decoded(Some(Key::Ctrl('w')), b"\x17"),
            ]
        );
    }

    #[test]
    fn conventional_keys_take_precedence_over_ctrl_aliases() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\x08\x0a\x0d"),
            vec![
                decoded(Some(Key::Backspace), b"\x08"),
                decoded(Some(Key::Enter), b"\x0a"),
                decoded(Some(Key::Enter), b"\x0d"),
            ]
        );
    }

    #[test]
    fn decodes_navigation_and_alt_keys_with_exact_sequences() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\x1b[A\x1bOA\x1ba\x1bA"),
            vec![
                decoded(Some(Key::Up), b"\x1b[A"),
                decoded(Some(Key::Up), b"\x1bOA"),
                decoded(Some(Key::Alt('a')), b"\x1ba"),
                decoded(Some(Key::Alt('a')), b"\x1bA"),
            ]
        );
    }

    #[test]
    fn decodes_editor_keys_and_tab() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\t\x1b[C\x1b[D\x1b[H\x1b[F\x1b[3~\x1b[Z"),
            vec![
                decoded(Some(Key::Tab), b"\t"),
                decoded(Some(Key::Right), b"\x1b[C"),
                decoded(Some(Key::Left), b"\x1b[D"),
                decoded(Some(Key::Home), b"\x1b[H"),
                decoded(Some(Key::End), b"\x1b[F"),
                decoded(Some(Key::Delete), b"\x1b[3~"),
                decoded(Some(Key::BackTab), b"\x1b[Z"),
            ]
        );
    }

    #[test]
    fn emits_unknown_complete_csi_and_ss3_sequences_losslessly() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(b"\x1b[?25"), Vec::new());
        assert_eq!(
            decoder.feed(b"h\x1bOP"),
            vec![decoded(None, b"\x1b[?25h"), decoded(None, b"\x1bOP"),]
        );
    }

    #[test]
    fn emits_bracketed_paste_as_one_opaque_token_across_feeds() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(b"\x1b[20"), Vec::new());
        assert_eq!(decoder.feed(b"0~text\x03\r\x1b[A"), Vec::new());
        assert_eq!(
            decoder.feed(b"\x1b[201~z"),
            vec![
                decoded(None, b"\x1b[200~text\x03\r\x1b[A\x1b[201~"),
                decoded(Some(Key::Char('z')), b"z"),
            ]
        );
    }

    #[test]
    fn bare_escape_timeout_preserves_raw_byte() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(b"\x1b"), Vec::new());
        decoder.escape_since = Some(Instant::now() - Duration::from_millis(36));
        assert_eq!(
            decoder.flush_due(),
            vec![decoded(Some(Key::Escape), b"\x1b")]
        );
    }

    #[test]
    fn pending_raw_bytes_can_be_transferred_between_input_consumers() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(&[0xc3]), Vec::new());
        assert_eq!(decoder.take_pending_raw(), vec![0xc3]);
        assert!(decoder.take_pending_raw().is_empty());
        assert_eq!(
            decoder.feed(b"a"),
            vec![decoded(Some(Key::Char('a')), b"a")]
        );
    }

    #[test]
    fn preserves_utf8_and_invalid_input_bytes() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(&[0xc3, 0xa9, 0xff]),
            vec![
                decoded(Some(Key::Char('\u{e9}')), &[0xc3, 0xa9]),
                decoded(None, &[0xff]),
            ]
        );
    }

    #[test]
    fn input_events_keep_paste_and_raw_representations_distinct() {
        let paste = InputEvent::decoded(decoded(None, b"\x1b[200~text\x1b[201~"));
        assert!(
            matches!(paste, InputEvent::Paste { text: Some(text), raw } if text == "text" && raw.starts_with(b"\x1b[200~"))
        );
        let raw = InputEvent::Bytes(vec![0xff]);
        assert!(matches!(raw, InputEvent::Bytes(bytes) if bytes == vec![0xff]));
        let key = InputEvent::decoded(decoded(Some(Key::Char('x')), b"x"));
        assert!(matches!(key, InputEvent::Key { key: Key::Char('x'), raw } if raw == b"x"));
    }

    #[test]
    fn invalid_paste_text_remains_lossless() {
        let event = InputEvent::decoded(decoded(None, b"\x1b[200~\xff\x1b[201~"));
        assert!(
            matches!(event, InputEvent::Paste { text: None, raw } if raw.ends_with(b"\x1b[201~"))
        );
    }

    #[test]
    fn parses_configured_key_bindings() {
        assert_eq!(Key::parse_binding("Ctrl+K").unwrap(), Key::Ctrl('k'));
        assert_eq!(Key::parse_binding("alt+A").unwrap(), Key::Alt('a'));
        assert_eq!(Key::parse_binding("esc").unwrap(), Key::Escape);
        assert_eq!(Key::parse_binding("ctrl+h").unwrap(), Key::Backspace);
        assert_eq!(Key::parse_binding("ctrl+j").unwrap(), Key::Enter);
        assert_eq!(Key::parse_binding("ctrl+m").unwrap(), Key::Enter);
        assert_eq!(Key::parse_binding("tab").unwrap(), Key::Tab);
        assert_eq!(Key::parse_binding("ctrl+i").unwrap(), Key::Tab);
        assert_eq!(Key::parse_binding("shift+tab").unwrap(), Key::BackTab);
        assert_eq!(Key::parse_binding("left").unwrap(), Key::Left);
        assert_eq!(Key::parse_binding("space").unwrap(), Key::Char(' '));
        assert_eq!(Key::parse_binding("x").unwrap(), Key::Char('x'));
        assert_eq!(Key::parse_binding("!").unwrap(), Key::Char('!'));
        assert_eq!(Key::Char(' ').binding_name().as_deref(), Some("space"));
        assert_eq!(Key::Char('X').binding_name().as_deref(), Some("x"));
        assert_eq!(Key::Ctrl('R').binding_name().as_deref(), Some("ctrl+r"));
        assert!(Key::parse_binding("ctrl+1").is_err());
        assert!(Key::parse_binding("plain").is_err());
    }
}
