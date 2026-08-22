use crate::config::{CommandBinding, Config, normalize_key};
use crate::engine::CommandInvocation;
use crate::input::{DecodedInput, InputDecoder, Key};
use crate::terminal::{InputRead, Terminal};
use anyhow::Result;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(35);
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PassthroughEvent {
    Forward(Vec<u8>),
    Switch(DecodedInput),
}

#[derive(Debug, Clone)]
struct RawBinding {
    key: Key,
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct PassthroughMatcher {
    bindings: Vec<RawBinding>,
    pending: Vec<u8>,
    started: Option<Instant>,
    in_bracketed_paste: bool,
}

impl PassthroughMatcher {
    fn new(keys: impl IntoIterator<Item = Key>) -> Self {
        let mut matcher = Self::default();
        matcher.replace_keys(keys);
        matcher
    }

    fn replace_keys(&mut self, keys: impl IntoIterator<Item = Key>) {
        self.bindings = keys
            .into_iter()
            .flat_map(|key| {
                key_bytes(key)
                    .into_iter()
                    .map(move |bytes| RawBinding { key, bytes })
            })
            .collect();
        self.bindings
            .sort_by_key(|binding| std::cmp::Reverse(binding.bytes.len()));
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    fn next(&mut self) -> Option<PassthroughEvent> {
        self.next_inner(false)
    }

    fn flush_due(&mut self) -> Option<PassthroughEvent> {
        self.next_inner(true)
    }

    fn take_pending(&mut self) -> Vec<u8> {
        self.started = None;
        self.in_bracketed_paste = false;
        std::mem::take(&mut self.pending)
    }

    fn next_inner(&mut self, force: bool) -> Option<PassthroughEvent> {
        if self.pending.is_empty() {
            self.started = None;
            return None;
        }

        if self.in_bracketed_paste {
            if let Some(end) = self
                .pending
                .windows(BRACKETED_PASTE_END.len())
                .position(|window| window == BRACKETED_PASTE_END)
            {
                let count = end + BRACKETED_PASTE_END.len();
                self.in_bracketed_paste = false;
                return Some(PassthroughEvent::Forward(self.take_prefix(count)));
            }
            let event = self.forward_before_marker(BRACKETED_PASTE_END);
            if event.is_none() && force {
                self.in_bracketed_paste = false;
                return Some(PassthroughEvent::Forward(
                    self.take_prefix(self.pending.len()),
                ));
            }
            return event;
        }

        if self.pending.starts_with(BRACKETED_PASTE_START) {
            self.in_bracketed_paste = true;
            return Some(PassthroughEvent::Forward(
                self.take_prefix(BRACKETED_PASTE_START.len()),
            ));
        }
        if BRACKETED_PASTE_START.starts_with(&self.pending) {
            if self.pending.len() > 1 {
                if force {
                    return Some(PassthroughEvent::Forward(
                        self.take_prefix(self.pending.len()),
                    ));
                }
                return None;
            }
            if !self.has_escape_binding() {
                self.started.get_or_insert_with(Instant::now);
                if !force && !self.escape_expired() {
                    return None;
                }
            }
        }

        if let Some(binding) = self.bindings.iter().find(|binding| {
            self.pending.starts_with(&binding.bytes)
                && !(binding.key == Key::Escape && self.pending.len() > 1)
        }) {
            let binding_len = binding.bytes.len();
            let binding_key = binding.key;
            let exact = self.pending.len() >= binding_len;
            let longer_prefix = self.bindings.iter().any(|candidate| {
                candidate.bytes.len() > binding_len
                    && candidate.bytes.starts_with(&self.pending[..binding_len])
                    && self.pending.len() < candidate.bytes.len()
            });
            let delayed_escape = binding_key == Key::Escape && self.pending.len() == 1;
            if exact
                && (!delayed_escape || force || self.escape_expired())
                && (!longer_prefix || force || self.escape_expired())
            {
                let raw = self.take_prefix(binding_len);
                self.started = None;
                return Some(PassthroughEvent::Switch(DecodedInput {
                    key: Some(binding_key),
                    raw,
                }));
            }
        }

        let is_prefix = self
            .bindings
            .iter()
            .any(|binding| binding.bytes.starts_with(&self.pending));
        if is_prefix {
            self.started.get_or_insert_with(Instant::now);
            if !force && !self.escape_expired() {
                return None;
            }
        }

        let keep = self.longest_prefix_suffix();
        let count = self.pending.len().saturating_sub(keep);
        if count == 0 {
            if force {
                self.started = None;
                return Some(PassthroughEvent::Forward(self.take_prefix(1)));
            }
            return None;
        }
        self.started = None;
        Some(PassthroughEvent::Forward(self.take_prefix(count)))
    }

    fn has_escape_binding(&self) -> bool {
        self.bindings
            .iter()
            .any(|binding| binding.key == Key::Escape)
    }

    fn escape_expired(&self) -> bool {
        self.started
            .is_some_and(|started| started.elapsed() >= ESCAPE_TIMEOUT)
    }

    fn longest_prefix_suffix(&self) -> usize {
        (1..=self.pending.len().min(self.max_binding_len()))
            .rev()
            .find(|length| {
                let suffix = &self.pending[self.pending.len() - *length..];
                self.bindings
                    .iter()
                    .any(|binding| binding.bytes.starts_with(suffix))
            })
            .unwrap_or(0)
    }

    fn max_binding_len(&self) -> usize {
        self.bindings
            .iter()
            .map(|binding| binding.bytes.len())
            .max()
            .unwrap_or(0)
    }

    fn forward_before_marker(&mut self, marker: &[u8]) -> Option<PassthroughEvent> {
        let keep = marker_prefix_suffix_len(&self.pending, marker);
        let count = self.pending.len().saturating_sub(keep);
        (count > 0).then(|| PassthroughEvent::Forward(self.take_prefix(count)))
    }

    fn take_prefix(&mut self, count: usize) -> Vec<u8> {
        self.pending
            .drain(..count.min(self.pending.len()))
            .collect()
    }
}

/// Session-owned input transport and mode state. It never knows about a View;
/// callers decide what a decoded token or a passthrough switch means.
#[derive(Debug, Default)]
pub(crate) struct CommandSession {
    decoder: InputDecoder,
    pending: VecDeque<DecodedInput>,
    passthrough_active: bool,
    passthrough: PassthroughMatcher,
    passthrough_revision: u64,
    session_commands: BTreeMap<String, CommandBinding>,
}

impl CommandSession {
    pub(crate) fn from_config(config: &Config) -> Self {
        let mut session_commands = config.commands.bindings.clone();
        if !session_commands.contains_key("commands") && config.view("selectors:commands").is_some()
        {
            session_commands.insert("commands".to_string(), CommandBinding::builtin_commands());
        }
        Self {
            session_commands,
            ..Self::default()
        }
    }

    pub(crate) fn passthrough_active(&self) -> bool {
        self.passthrough_active
    }

    pub(crate) fn enter_passthrough(&mut self, keys: impl IntoIterator<Item = Key>) {
        self.passthrough = PassthroughMatcher::new(keys);
        self.passthrough_revision = 0;
        self.passthrough_active = true;
    }

    pub(crate) fn sync_passthrough_keys(
        &mut self,
        revision: u64,
        keys: impl IntoIterator<Item = Key>,
    ) {
        if self.passthrough_revision == revision {
            return;
        }
        self.passthrough.replace_keys(keys);
        self.passthrough_revision = revision;
    }

    pub(crate) fn leave_passthrough(&mut self) -> Vec<u8> {
        let pending = self.passthrough.take_pending();
        self.passthrough_revision = 0;
        self.passthrough_active = false;
        pending
    }

    pub(crate) fn session_command_for_key(
        &self,
        view_ref: &str,
        key: Key,
    ) -> Option<CommandInvocation> {
        let name = key.binding_name()?;
        self.session_commands.iter().find_map(|(id, binding)| {
            if binding
                .key(id)
                .and_then(|key| normalize_key(key).ok())
                .as_deref()
                != Some(name.as_str())
            {
                return None;
            }
            binding
                .as_command(id)
                .map(|command| CommandInvocation::session_command(view_ref, id, command))
        })
    }

    pub(crate) fn session_commands(&self) -> impl Iterator<Item = (&String, &CommandBinding)> {
        self.session_commands.iter()
    }

    pub(crate) fn read_normal(
        &mut self,
        terminal: &mut Terminal,
        timeout: i32,
    ) -> Result<InputRead> {
        let read = terminal.read_input(timeout)?;
        match &read {
            InputRead::Data(bytes) => self.feed_normal(bytes),
            InputRead::Timeout => self.flush_normal_due(),
            InputRead::Eof => {}
        }
        Ok(read)
    }

    pub(crate) fn read_passthrough(
        &mut self,
        terminal: &mut Terminal,
        timeout: i32,
    ) -> Result<InputRead> {
        let read = terminal.read_input(timeout)?;
        if let InputRead::Data(bytes) = &read {
            self.feed_passthrough(bytes);
        }
        Ok(read)
    }

    pub(crate) fn feed_normal(&mut self, bytes: &[u8]) {
        self.pending.extend(self.decoder.feed(bytes));
    }

    pub(crate) fn flush_normal_due(&mut self) {
        self.pending.extend(self.decoder.flush_due());
    }

    pub(crate) fn pop_input(&mut self) -> Option<DecodedInput> {
        self.pending.pop_front()
    }

    pub(crate) fn push_front(&mut self, input: DecodedInput) {
        self.pending.push_front(input);
    }

    pub(crate) fn pending_is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub(crate) fn take_all_pending_raw(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Some(input) = self.pending.pop_front() {
            bytes.extend(input.raw);
        }
        bytes.extend(self.decoder.take_pending_raw());
        bytes
    }

    pub(crate) fn feed_passthrough(&mut self, bytes: &[u8]) {
        self.passthrough.feed(bytes);
    }

    pub(crate) fn next_passthrough_event(&mut self) -> Option<PassthroughEvent> {
        self.passthrough.next()
    }

    pub(crate) fn flush_passthrough_due(&mut self) -> Option<PassthroughEvent> {
        self.passthrough.flush_due()
    }

    pub(crate) fn feed_pending_raw_to_normal(&mut self, bytes: &[u8]) {
        self.feed_normal(bytes);
    }
}

fn key_bytes(key: Key) -> Vec<Vec<u8>> {
    match key {
        Key::Char(character) if character.is_ascii() => vec![vec![character as u8]],
        Key::Alt(character) if character.is_ascii() => vec![vec![0x1b, character as u8]],
        Key::Enter => vec![b"\r".to_vec(), b"\n".to_vec()],
        Key::Tab => vec![b"\t".to_vec()],
        Key::BackTab => vec![b"\x1b[Z".to_vec()],
        Key::Backspace => vec![vec![0x7f], vec![0x08]],
        Key::Delete => vec![b"\x1b[3~".to_vec()],
        Key::Left => vec![b"\x1b[D".to_vec()],
        Key::Right => vec![b"\x1b[C".to_vec()],
        Key::Home => vec![b"\x1b[H".to_vec()],
        Key::End => vec![b"\x1b[F".to_vec()],
        Key::Up => vec![b"\x1b[A".to_vec()],
        Key::Down => vec![b"\x1b[B".to_vec()],
        Key::Escape => vec![b"\x1b".to_vec()],
        Key::Ctrl(character) if character.is_ascii_alphabetic() => {
            vec![vec![character.to_ascii_lowercase() as u8 - b'a' + 1]]
        }
        Key::Ctrl(_) | Key::Char(_) | Key::Alt(_) => Vec::new(),
    }
}

fn marker_prefix_suffix_len(bytes: &[u8], marker: &[u8]) -> usize {
    (1..marker.len())
        .rev()
        .find(|length| bytes.ends_with(&marker[..*length]))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_forwards_unmatched_bytes() {
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(b"abc");
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Forward(b"abc".to_vec()))
        );
    }

    #[test]
    fn passthrough_matches_a_split_control_switch() {
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(b"a\x02");
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Forward(b"a".to_vec()))
        );
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Ctrl('b')),
                raw: b"\x02".to_vec(),
            }))
        );
    }

    #[test]
    fn passthrough_matches_canonical_control_key_aliases() {
        let mut matcher = PassthroughMatcher::new([Key::Enter, Key::Backspace]);
        matcher.feed(b"\n\x08");
        assert!(matches!(
            matcher.next(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Enter),
                ..
            }))
        ));
        assert!(matches!(
            matcher.next(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Backspace),
                ..
            }))
        ));
    }

    #[test]
    fn passthrough_flushes_a_bare_escape_switch() {
        let mut matcher = PassthroughMatcher::new([Key::Escape]);
        matcher.feed(b"\x1b");
        assert_eq!(matcher.next(), None);
        assert!(matches!(
            matcher.flush_due(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Escape),
                ..
            }))
        ));
    }

    #[test]
    fn passthrough_waits_for_escape_prefixes() {
        let mut matcher = PassthroughMatcher::new([Key::Escape, Key::Up]);
        matcher.feed(b"\x1b[");
        assert_eq!(matcher.next(), None);
        matcher.feed(b"A");
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Up),
                raw: b"\x1b[A".to_vec(),
            }))
        );
    }

    #[test]
    fn passthrough_forwards_alt_and_unknown_escape_sequences() {
        let mut matcher = PassthroughMatcher::new([Key::Escape]);
        matcher.feed(b"\x1ba\x1b[?25h");
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Forward(b"\x1ba\x1b[?25h".to_vec()))
        );
    }

    #[test]
    fn passthrough_does_not_match_switches_inside_bracketed_paste() {
        let bytes = b"\x1b[200~paste\x02\x1b[201~";
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(bytes);
        let mut forwarded = Vec::new();
        while let Some(event) = matcher.next() {
            match event {
                PassthroughEvent::Forward(bytes) => forwarded.extend(bytes),
                PassthroughEvent::Switch(_) => panic!("paste must not trigger a switch"),
            }
        }
        assert_eq!(forwarded, bytes);
    }

    #[test]
    fn passthrough_keeps_a_split_bracketed_paste_opaque() {
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(b"\x1b");
        assert_eq!(matcher.next(), None);
        matcher.feed(b"[20");
        assert_eq!(matcher.next(), None);
        matcher.feed(b"0~paste\x02\x1b[201~");
        let mut forwarded = Vec::new();
        while let Some(event) = matcher.next() {
            match event {
                PassthroughEvent::Forward(bytes) => forwarded.extend(bytes),
                PassthroughEvent::Switch(_) => panic!("paste must remain opaque"),
            }
        }
        assert_eq!(forwarded, b"\x1b[200~paste\x02\x1b[201~");
    }

    #[test]
    fn passthrough_releases_incomplete_paste_markers_on_timeout() {
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(b"\x1b[20");
        assert_eq!(matcher.next(), None);
        assert_eq!(
            matcher.flush_due(),
            Some(PassthroughEvent::Forward(b"\x1b[20".to_vec()))
        );

        matcher.feed(b"\x1b[200~paste\x1b[20");
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Forward(b"\x1b[200~".to_vec()))
        );
        assert_eq!(
            matcher.next(),
            Some(PassthroughEvent::Forward(b"paste".to_vec()))
        );
        assert_eq!(matcher.next(), None);
        assert_eq!(
            matcher.flush_due(),
            Some(PassthroughEvent::Forward(b"\x1b[20".to_vec()))
        );
    }

    #[test]
    fn passthrough_key_refresh_preserves_pending_input() {
        let mut matcher = PassthroughMatcher::new([Key::Ctrl('b')]);
        matcher.feed(b"\x1b");
        assert_eq!(matcher.next(), None);
        matcher.replace_keys([Key::Escape]);
        assert!(matches!(
            matcher.flush_due(),
            Some(PassthroughEvent::Switch(DecodedInput {
                key: Some(Key::Escape),
                ..
            }))
        ));
    }
}
