#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InputBuffer {
    pub(crate) raw: String,
    pub(crate) params: String,
    pub(crate) cursor: usize,
    pub(crate) rejected: bool,
}

impl InputBuffer {
    #[cfg(test)]
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            params: raw.clone(),
            raw,
            cursor,
            rejected: false,
        }
    }

    pub(crate) fn with_params(raw: impl Into<String>, params: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            raw,
            params: params.into(),
            cursor,
            rejected: false,
        }
    }

    pub(crate) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = previous_char_boundary(&self.raw, cursor);
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.cursor = self.cursor.min(self.raw.len());
        while !self.raw.is_char_boundary(self.cursor) {
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.raw.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn move_left(&mut self) {
        self.cursor = self.cursor.min(self.raw.len());
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
        self.cursor = self.cursor.min(self.raw.len());
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
        self.cursor = self.cursor.min(self.raw.len());
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
        true
    }

    pub(crate) fn delete_forward(&mut self) -> bool {
        self.cursor = self.cursor.min(self.raw.len());
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
        true
    }

    pub(crate) fn delete_word(&mut self) -> bool {
        self.cursor = self.cursor.min(self.raw.len());
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
        true
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.raw.is_empty() {
            return false;
        }
        self.raw.clear();
        self.cursor = 0;
        true
    }
}

pub(crate) fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
