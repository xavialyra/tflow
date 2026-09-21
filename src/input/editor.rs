#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EditorBuffer {
    pub(crate) raw: String,
    pub(crate) cursor: usize,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorSnapshot {
    pub(crate) raw: String,
    pub(crate) cursor: usize,
    pub(crate) revision: u64,
}

impl EditorBuffer {
    #[cfg(test)]
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            raw,
            cursor,
            revision: 0,
        }
    }

    pub(crate) fn from_raw(raw: impl Into<String>, cursor: usize) -> Self {
        let raw = raw.into();
        let mut buffer = Self {
            raw,
            cursor: 0,
            revision: 0,
        };
        buffer.set_cursor(cursor);
        buffer
    }

    pub(crate) fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            raw: self.raw.clone(),
            cursor: self.cursor,
            revision: self.revision,
        }
    }

    pub(crate) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = previous_char_boundary(&self.raw, cursor);
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        self.raw.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.bump_revision();
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        self.raw.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.bump_revision();
        true
    }

    pub(crate) fn move_left(&mut self) {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        if self.cursor == 0 {
            return;
        }
        self.cursor -= self.raw[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
    }

    pub(crate) fn move_right(&mut self) {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        if self.cursor >= self.raw.len() {
            self.cursor = self.raw.len();
            return;
        }
        self.cursor += self.raw[self.cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.raw.len();
    }

    pub(crate) fn delete_backward(&mut self) -> bool {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        if self.cursor == 0 {
            return false;
        }
        let start = self.cursor
            - self.raw[..self.cursor]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(1);
        self.raw.drain(start..self.cursor);
        self.cursor = start;
        self.bump_revision();
        true
    }

    pub(crate) fn delete_forward(&mut self) -> bool {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        if self.cursor >= self.raw.len() {
            return false;
        }
        let end = self.cursor
            + self.raw[self.cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        self.raw.drain(self.cursor..end);
        self.bump_revision();
        true
    }

    pub(crate) fn delete_word(&mut self) -> bool {
        self.cursor = previous_char_boundary(&self.raw, self.cursor);
        let previous = self.cursor;
        while self.cursor > 0
            && self.raw[..self.cursor]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            self.move_left();
        }
        while self.cursor > 0
            && !self.raw[..self.cursor]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            self.move_left();
        }
        if self.cursor == previous {
            return false;
        }
        self.raw.drain(self.cursor..previous);
        self.bump_revision();
        true
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.raw.is_empty() {
            return false;
        }
        self.raw.clear();
        self.cursor = 0;
        self.bump_revision();
        true
    }

    pub(crate) fn replace_all(&mut self, raw: String, cursor: usize) {
        self.raw = raw;
        self.set_cursor(cursor);
        self.bump_revision();
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

pub(crate) fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_tracks_text_edits_but_not_cursor_motion() {
        let mut buffer = EditorBuffer::new("ab");
        assert_eq!(buffer.revision, 0);
        buffer.move_left();
        assert_eq!(buffer.revision, 0);
        buffer.insert('x');
        assert_eq!(buffer.raw, "axb");
        assert_eq!(buffer.revision, 1);
        assert!(buffer.delete_backward());
        assert_eq!(buffer.revision, 2);
    }
}
