use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Alt(char),
    Enter,
    Backspace,
    Up,
    Down,
    Escape,
    CtrlC,
    CtrlK,
    CtrlD,
    CtrlU,
    CtrlW,
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
                        let character = self.pending[1] as char;
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
                if self.pending.len() < 3 {
                    self.escape_since.get_or_insert_with(Instant::now);
                    break;
                }
                let code = self.pending[2];
                let key = match code {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    _ => None,
                };
                if let Some(key) = key {
                    self.pending.drain(..3);
                    self.escape_since = None;
                    keys.push(key);
                    continue;
                }
                if self.pending.last() == Some(&b'~') {
                    self.pending.clear();
                    self.escape_since = None;
                    continue;
                }
                if self.pending.len() > 8 {
                    self.pending.remove(0);
                    self.escape_since = None;
                    keys.push(Key::Escape);
                    continue;
                }
                self.escape_since.get_or_insert_with(Instant::now);
                break;
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
        0x7f | 0x08 => Some(Key::Backspace),
        0x03 => Some(Key::CtrlC),
        0x04 => Some(Key::CtrlD),
        0x0b => Some(Key::CtrlK),
        0x15 => Some(Key::CtrlU),
        0x17 => Some(Key::CtrlW),
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
            decoder.feed(b"a\r\x03"),
            vec![Key::Char('a'), Key::Enter, Key::CtrlC]
        );
    }

    #[test]
    fn decodes_navigation_and_alt_keys() {
        let mut decoder = InputDecoder::default();
        assert_eq!(decoder.feed(b"\x1b[A\x1ba"), vec![Key::Up, Key::Alt('a')]);
    }
}
