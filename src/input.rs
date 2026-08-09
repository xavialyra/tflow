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

impl Key {
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
            Self::Char(_) => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct InputDecoder {
    pending: Vec<u8>,
    escape_since: Option<Instant>,
}

impl InputDecoder {
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.pending.extend_from_slice(bytes);
        self.parse()
    }

    pub(crate) fn flush_due(&mut self) -> Vec<Key> {
        if self
            .escape_since
            .is_some_and(|started| started.elapsed() >= Duration::from_millis(35))
        {
            self.escape_since = None;
            if self.pending.first() == Some(&0x1b) {
                self.pending.remove(0);
                return vec![Key::Escape];
            }
        }
        Vec::new()
    }

    fn parse(&mut self) -> Vec<Key> {
        let mut keys = Vec::new();
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
                if self.pending[1] != b'[' {
                    if self.pending[1].is_ascii_graphic() {
                        let character = (self.pending[1] as char).to_ascii_lowercase();
                        self.pending.drain(..2);
                        self.escape_since = None;
                        keys.push(Key::Alt(character));
                        continue;
                    }
                    self.pending.remove(0);
                    keys.push(Key::Escape);
                    self.escape_since = None;
                    continue;
                }
                let Some(end) = self.pending[2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))
                    .map(|position| position + 2)
                else {
                    if self.pending.len() > 16 {
                        self.pending.remove(0);
                        self.escape_since = None;
                        keys.push(Key::Escape);
                        continue;
                    }
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                };
                let params = &self.pending[2..end];
                let code = self.pending[end];
                let key = match code {
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
                };
                self.pending.drain(..=end);
                self.escape_since = None;
                if let Some(key) = key {
                    keys.push(key);
                }
                continue;
            }

            if let Some(key) = control_key(first) {
                self.pending.remove(0);
                keys.push(key);
                continue;
            }

            if first < 0x80 {
                self.pending.remove(0);
                keys.push(Key::Char(first as char));
                continue;
            }

            let width = utf8_width(first);
            if self.pending.len() < width {
                break;
            }
            match std::str::from_utf8(&self.pending[..width]) {
                Ok(text) => {
                    if let Some(character) = text.chars().next() {
                        keys.push(Key::Char(character));
                    }
                    self.pending.drain(..width);
                }
                Err(_) => {
                    self.pending.remove(0);
                }
            }
        }
        keys
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

    #[test]
    fn decodes_ascii_and_controls() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"a\r\x01\x03\x0b\x15\x17"),
            vec![
                Key::Char('a'),
                Key::Enter,
                Key::Ctrl('a'),
                Key::Ctrl('c'),
                Key::Ctrl('k'),
                Key::Ctrl('u'),
                Key::Ctrl('w'),
            ]
        );
    }

    #[test]
    fn conventional_keys_take_precedence_over_ctrl_aliases() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\x08\x0a\x0d"),
            vec![Key::Backspace, Key::Enter, Key::Enter]
        );
    }

    #[test]
    fn decodes_navigation_and_alt_keys() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\x1b[A\x1ba\x1bA"),
            vec![Key::Up, Key::Alt('a'), Key::Alt('a')]
        );
    }

    #[test]
    fn decodes_editor_keys_and_tab() {
        let mut decoder = InputDecoder::default();
        assert_eq!(
            decoder.feed(b"\t\x1b[C\x1b[D\x1b[H\x1b[F\x1b[3~\x1b[Z"),
            vec![
                Key::Tab,
                Key::Right,
                Key::Left,
                Key::Home,
                Key::End,
                Key::Delete,
                Key::BackTab,
            ]
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
        assert_eq!(Key::Ctrl('R').binding_name().as_deref(), Some("ctrl+r"));
        assert!(Key::parse_binding("ctrl+1").is_err());
        assert!(Key::parse_binding("plain").is_err());
    }
}
