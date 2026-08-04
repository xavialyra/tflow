use unicode_width::UnicodeWidthChar;

#[derive(Clone)]
struct ScreenState {
    cells: Vec<Vec<char>>,
    cursor_x: usize,
    cursor_y: usize,
    saved_x: usize,
    saved_y: usize,
}

impl ScreenState {
    fn new(columns: usize, rows: usize) -> Self {
        Self {
            cells: vec![vec![' '; columns]; rows],
            cursor_x: 0,
            cursor_y: 0,
            saved_x: 0,
            saved_y: 0,
        }
    }

    fn resize(&mut self, columns: usize, rows: usize) {
        let mut next = vec![vec![' '; columns]; rows];
        for (target, source) in next.iter_mut().zip(&self.cells) {
            for (target_cell, source_cell) in target.iter_mut().zip(source) {
                *target_cell = *source_cell;
            }
        }
        self.cells = next;
        self.cursor_x = self.cursor_x.min(columns.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(rows.saturating_sub(1));
        self.saved_x = self.saved_x.min(columns.saturating_sub(1));
        self.saved_y = self.saved_y.min(rows.saturating_sub(1));
    }

    fn columns(&self) -> usize {
        self.cells.first().map_or(0, Vec::len)
    }

    fn rows(&self) -> usize {
        self.cells.len()
    }
}

#[derive(Clone)]
enum ParserState {
    Ground,
    Escape,
    Csi(Vec<u8>),
    Osc(bool),
}

pub struct VirtualTerminal {
    screen: ScreenState,
    normal_screen: Option<ScreenState>,
    parser: ParserState,
    utf8_pending: Vec<u8>,
    cursor_visible: bool,
}

impl VirtualTerminal {
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            screen: ScreenState::new(columns as usize, rows as usize),
            normal_screen: None,
            parser: ParserState::Ground,
            utf8_pending: Vec::new(),
            cursor_visible: true,
        }
    }

    pub fn resize(&mut self, columns: u16, rows: u16) {
        let columns = columns as usize;
        let rows = rows as usize;
        self.screen.resize(columns, rows);
        if let Some(normal) = &mut self.normal_screen {
            normal.resize(columns, rows);
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    pub fn row_text(&self, row: usize) -> String {
        self.screen
            .cells
            .get(row)
            .map(|cells| cells.iter().collect())
            .unwrap_or_default()
    }

    pub fn cursor(&self) -> (usize, usize, bool) {
        (
            self.screen.cursor_x,
            self.screen.cursor_y,
            self.cursor_visible,
        )
    }

    fn feed_byte(&mut self, byte: u8) {
        if !self.utf8_pending.is_empty() {
            if byte >= 0x80 && byte != 0x1b {
                self.utf8_pending.push(byte);
                let expected = utf8_width(self.utf8_pending[0]);
                if self.utf8_pending.len() >= expected {
                    if let Ok(text) = std::str::from_utf8(&self.utf8_pending) {
                        if let Some(character) = text.chars().next() {
                            self.put_char(character);
                        }
                    } else {
                        self.put_char('\u{fffd}');
                    }
                    self.utf8_pending.clear();
                }
                return;
            }
            self.utf8_pending.clear();
        }

        match self.parser.clone() {
            ParserState::Ground => self.feed_ground(byte),
            ParserState::Escape => self.feed_escape(byte),
            ParserState::Csi(mut params) => {
                if byte == 0x1b {
                    self.parser = ParserState::Escape;
                } else if (0x40..=0x7e).contains(&byte) {
                    self.handle_csi(&params, byte);
                    self.parser = ParserState::Ground;
                } else {
                    if params.len() < 64 {
                        params.push(byte);
                    }
                    self.parser = ParserState::Csi(params);
                }
            }
            ParserState::Osc(mut escaped) => {
                if escaped {
                    if byte == b'\\' {
                        self.parser = ParserState::Ground;
                    } else {
                        escaped = false;
                        self.parser = ParserState::Osc(escaped);
                    }
                } else if byte == 0x07 {
                    self.parser = ParserState::Ground;
                } else if byte == 0x1b {
                    self.parser = ParserState::Osc(true);
                }
            }
        }
    }

    fn feed_ground(&mut self, byte: u8) {
        match byte {
            0x07 => {}
            0x08 => self.screen.cursor_x = self.screen.cursor_x.saturating_sub(1),
            0x09 => {
                let next = ((self.screen.cursor_x / 8) + 1) * 8;
                self.screen.cursor_x = next.min(self.screen.columns().saturating_sub(1));
            }
            b'\n' => self.line_feed(),
            b'\r' => self.screen.cursor_x = 0,
            0x1b => self.parser = ParserState::Escape,
            0x20..=0x7e => self.put_char(byte as char),
            0x80..=0xff => {
                if utf8_width(byte) == 1 {
                    self.put_char('\u{fffd}');
                } else {
                    self.utf8_pending.push(byte);
                }
            }
            _ => {}
        }
    }

    fn feed_escape(&mut self, byte: u8) {
        self.parser = match byte {
            b'[' => ParserState::Csi(Vec::new()),
            b']' => ParserState::Osc(false),
            b'7' => {
                self.save_cursor();
                ParserState::Ground
            }
            b'8' => {
                self.restore_cursor();
                ParserState::Ground
            }
            b'D' => {
                self.line_feed();
                ParserState::Ground
            }
            b'M' => {
                self.reverse_line_feed();
                ParserState::Ground
            }
            b'E' => {
                self.screen.cursor_x = 0;
                self.line_feed();
                ParserState::Ground
            }
            b'c' => {
                self.reset();
                ParserState::Ground
            }
            _ => ParserState::Ground,
        };
    }

    fn handle_csi(&mut self, raw: &[u8], final_byte: u8) {
        let private = raw
            .first()
            .is_some_and(|byte| *byte == b'?' || *byte == b'>');
        let params = parse_params(raw);
        let first = param(&params, 0, 1);
        let second = param(&params, 1, 1);

        match final_byte {
            b'A' => self.screen.cursor_y = self.screen.cursor_y.saturating_sub(first),
            b'B' | b'e' => {
                self.screen.cursor_y =
                    (self.screen.cursor_y + first).min(self.screen.rows().saturating_sub(1));
            }
            b'C' | b'a' => {
                self.screen.cursor_x =
                    (self.screen.cursor_x + first).min(self.screen.columns().saturating_sub(1));
            }
            b'D' => self.screen.cursor_x = self.screen.cursor_x.saturating_sub(first),
            b'E' => {
                self.screen.cursor_y =
                    (self.screen.cursor_y + first).min(self.screen.rows().saturating_sub(1));
                self.screen.cursor_x = 0;
            }
            b'F' => {
                self.screen.cursor_y = self.screen.cursor_y.saturating_sub(first);
                self.screen.cursor_x = 0;
            }
            b'G' | b'`' => {
                self.screen.cursor_x = first
                    .saturating_sub(1)
                    .min(self.screen.columns().saturating_sub(1));
            }
            b'd' => {
                self.screen.cursor_y = first
                    .saturating_sub(1)
                    .min(self.screen.rows().saturating_sub(1));
            }
            b'H' | b'f' => {
                self.screen.cursor_y = first
                    .saturating_sub(1)
                    .min(self.screen.rows().saturating_sub(1));
                self.screen.cursor_x = second
                    .saturating_sub(1)
                    .min(self.screen.columns().saturating_sub(1));
            }
            b'J' => self.erase_display(params.first().copied().unwrap_or(0)),
            b'K' => self.erase_line(params.first().copied().unwrap_or(0)),
            b'P' => self.delete_chars(first),
            b'@' => self.insert_chars(first),
            b'X' => self.erase_chars(first),
            b'L' => self.insert_lines(first),
            b'M' => self.delete_lines(first),
            b'S' => self.scroll_up(first),
            b'T' => self.scroll_down(first),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'h' | b'l' if private => self.handle_private_mode(final_byte, &params),
            _ => {}
        }
    }

    fn handle_private_mode(&mut self, final_byte: u8, params: &[usize]) {
        for mode in params {
            match (*mode, final_byte) {
                (25, b'h') => self.cursor_visible = true,
                (25, b'l') => self.cursor_visible = false,
                (47 | 1047 | 1049, b'h') => self.enter_alternate_screen(),
                (47 | 1047 | 1049, b'l') => self.leave_alternate_screen(),
                _ => {}
            }
        }
    }

    fn put_char(&mut self, character: char) {
        if self.screen.columns() == 0 || self.screen.rows() == 0 {
            return;
        }
        let width = UnicodeWidthChar::width(character).unwrap_or(1);
        if width == 0 {
            return;
        }
        if width > 1 && self.screen.cursor_x + width > self.screen.columns() {
            self.line_feed();
            self.screen.cursor_x = 0;
        }
        if self.screen.cursor_y >= self.screen.rows() {
            self.screen.cursor_y = self.screen.rows().saturating_sub(1);
        }
        if self.screen.cursor_x < self.screen.columns() {
            self.screen.cells[self.screen.cursor_y][self.screen.cursor_x] = character;
        }
        for offset in 1..width {
            if self.screen.cursor_x + offset < self.screen.columns() {
                self.screen.cells[self.screen.cursor_y][self.screen.cursor_x + offset] = ' ';
            }
        }
        self.screen.cursor_x += width;
        if self.screen.cursor_x >= self.screen.columns() {
            self.line_feed();
            self.screen.cursor_x = 0;
        }
    }

    fn line_feed(&mut self) {
        if self.screen.cursor_y + 1 >= self.screen.rows() {
            self.scroll_up(1);
        } else {
            self.screen.cursor_y += 1;
        }
    }

    fn reverse_line_feed(&mut self) {
        if self.screen.cursor_y == 0 {
            self.scroll_down(1);
        } else {
            self.screen.cursor_y -= 1;
        }
    }

    fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.max(1) {
            if !self.screen.cells.is_empty() {
                self.screen.cells.remove(0);
                self.screen.cells.push(vec![' '; self.screen.columns()]);
            }
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.max(1) {
            if !self.screen.cells.is_empty() {
                self.screen.cells.pop();
                self.screen
                    .cells
                    .insert(0, vec![' '; self.screen.columns()]);
            }
        }
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_line(0);
                for row in self.screen.cursor_y + 1..self.screen.rows() {
                    self.screen.cells[row].fill(' ');
                }
            }
            1 => {
                self.erase_line(1);
                for row in 0..self.screen.cursor_y {
                    self.screen.cells[row].fill(' ');
                }
            }
            _ => self.clear_screen(),
        }
    }

    fn erase_line(&mut self, mode: usize) {
        let cursor_y = self.screen.cursor_y;
        let cursor_x = self.screen.cursor_x;
        let Some(row) = self.screen.cells.get_mut(cursor_y) else {
            return;
        };
        if row.is_empty() {
            return;
        }
        let cursor_x = cursor_x.min(row.len() - 1);
        match mode {
            0 => row[cursor_x..].fill(' '),
            1 => row[..=cursor_x].fill(' '),
            _ => row.fill(' '),
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let cursor_y = self.screen.cursor_y;
        let cursor_x = self.screen.cursor_x;
        let Some(row) = self.screen.cells.get_mut(cursor_y) else {
            return;
        };
        let start = cursor_x.min(row.len());
        let end = (start + count.max(1)).min(row.len());
        row[start..end].fill(' ');
    }

    fn delete_chars(&mut self, count: usize) {
        let cursor_y = self.screen.cursor_y;
        let cursor_x = self.screen.cursor_x;
        let Some(row) = self.screen.cells.get_mut(cursor_y) else {
            return;
        };
        let start = cursor_x.min(row.len());
        let count = count.max(1).min(row.len().saturating_sub(start));
        row.drain(start..start + count);
        row.extend(std::iter::repeat_n(' ', count));
    }

    fn insert_chars(&mut self, count: usize) {
        let cursor_y = self.screen.cursor_y;
        let cursor_x = self.screen.cursor_x;
        let columns = self.screen.columns();
        let Some(row) = self.screen.cells.get_mut(cursor_y) else {
            return;
        };
        let start = cursor_x.min(row.len());
        let count = count.max(1).min(row.len().saturating_sub(start));
        row.splice(start..start, std::iter::repeat_n(' ', count));
        row.truncate(columns);
    }

    fn insert_lines(&mut self, count: usize) {
        let row = self.screen.cursor_y.min(self.screen.rows());
        for _ in 0..count.max(1) {
            self.screen
                .cells
                .insert(row, vec![' '; self.screen.columns()]);
            self.screen.cells.pop();
        }
    }

    fn delete_lines(&mut self, count: usize) {
        let row = self.screen.cursor_y.min(self.screen.rows());
        for _ in 0..count.max(1) {
            if row < self.screen.cells.len() {
                self.screen.cells.remove(row);
                self.screen.cells.push(vec![' '; self.screen.columns()]);
            }
        }
    }

    fn clear_screen(&mut self) {
        for row in &mut self.screen.cells {
            row.fill(' ');
        }
        self.screen.cursor_x = 0;
        self.screen.cursor_y = 0;
    }

    fn save_cursor(&mut self) {
        self.screen.saved_x = self.screen.cursor_x;
        self.screen.saved_y = self.screen.cursor_y;
    }

    fn restore_cursor(&mut self) {
        self.screen.cursor_x = self
            .screen
            .saved_x
            .min(self.screen.columns().saturating_sub(1));
        self.screen.cursor_y = self
            .screen
            .saved_y
            .min(self.screen.rows().saturating_sub(1));
    }

    fn reset(&mut self) {
        self.normal_screen = None;
        self.screen = ScreenState::new(self.screen.columns(), self.screen.rows());
        self.cursor_visible = true;
    }

    fn enter_alternate_screen(&mut self) {
        if self.normal_screen.is_none() {
            self.normal_screen = Some(self.screen.clone());
            self.screen = ScreenState::new(self.screen.columns(), self.screen.rows());
        }
    }

    fn leave_alternate_screen(&mut self) {
        if let Some(normal) = self.normal_screen.take() {
            self.screen = normal;
        }
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

fn parse_params(raw: &[u8]) -> Vec<usize> {
    let start = usize::from(
        raw.first()
            .is_some_and(|byte| *byte == b'?' || *byte == b'>'),
    );
    String::from_utf8_lossy(&raw[start..])
        .split(';')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn param(params: &[usize], index: usize, default: usize) -> usize {
    match params.get(index).copied() {
        Some(0) | None => default,
        Some(value) => value,
    }
}

#[cfg(test)]
mod tests {
    use super::VirtualTerminal;

    #[test]
    fn renders_basic_text_and_cursor_movement() {
        let mut terminal = VirtualTerminal::new(10, 3);
        terminal.feed(b"hello\x1b[2;3Hworld");
        assert_eq!(&terminal.row_text(0)[..5], "hello");
        assert_eq!(&terminal.row_text(1)[..7], "  world");
    }

    #[test]
    fn handles_clear_and_alternate_screen() {
        let mut terminal = VirtualTerminal::new(10, 3);
        terminal.feed(b"main\x1b[?1049h\x1b[2Jalt\x1b[?1049l");
        assert_eq!(&terminal.row_text(0)[..4], "main");
    }
}
