use avt::{Color as AvtColor, Pen, Vt};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

pub(crate) struct EmbeddedTerminal {
    vt: Vt,
    pending_utf8: Vec<u8>,
}

impl EmbeddedTerminal {
    pub(crate) fn new(columns: u16, rows: u16) -> Self {
        Self {
            vt: Vt::new(columns as usize, rows as usize),
            pending_utf8: Vec::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.pending_utf8.extend_from_slice(bytes);
        let text = decode_utf8_prefix(&mut self.pending_utf8);
        if !text.is_empty() {
            self.vt.feed_str(&text);
        }
    }

    pub(crate) fn resize(&mut self, columns: u16, rows: u16) {
        self.vt.resize(columns as usize, rows as usize);
    }

    pub(crate) fn widget(&self) -> EmbeddedTerminalWidget<'_> {
        EmbeddedTerminalWidget { terminal: self }
    }

    pub(crate) fn cursor(&self) -> Option<(usize, usize)> {
        self.vt.cursor().into()
    }
}

pub(crate) struct EmbeddedTerminalWidget<'a> {
    terminal: &'a EmbeddedTerminal,
}

impl Widget for EmbeddedTerminalWidget<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for (row, line) in self
            .terminal
            .vt
            .view()
            .take(area.height as usize)
            .enumerate()
        {
            let y = area.y.saturating_add(row as u16);
            for (column, cell) in line.cells().iter().take(area.width as usize).enumerate() {
                if cell.width() == 0 {
                    continue;
                }

                let x = area.x.saturating_add(column as u16);
                let style = ratatui_style(cell.pen());
                if cell.width() > 1 {
                    buffer.set_stringn(x, y, cell.char().to_string(), cell.width() as usize, style);
                } else {
                    buffer[(x, y)].set_char(cell.char()).set_style(style);
                }
            }
        }
    }
}

fn ratatui_style(pen: &Pen) -> Style {
    let mut style = Style::reset();
    if let Some(foreground) = pen.foreground() {
        style = style.fg(ratatui_color(foreground));
    }
    if let Some(background) = pen.background() {
        style = style.bg(ratatui_color(background));
    }

    let mut modifiers = Modifier::empty();
    if pen.is_bold() {
        modifiers |= Modifier::BOLD;
    }
    if pen.is_faint() {
        modifiers |= Modifier::DIM;
    }
    if pen.is_italic() {
        modifiers |= Modifier::ITALIC;
    }
    if pen.is_underline() {
        modifiers |= Modifier::UNDERLINED;
    }
    if pen.is_strikethrough() {
        modifiers |= Modifier::CROSSED_OUT;
    }
    if pen.is_blink() {
        modifiers |= Modifier::SLOW_BLINK;
    }
    if pen.is_inverse() {
        modifiers |= Modifier::REVERSED;
    }
    style.add_modifier(modifiers)
}

fn ratatui_color(color: AvtColor) -> Color {
    match color {
        AvtColor::Indexed(index) => Color::Indexed(index),
        AvtColor::RGB(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn decode_utf8_prefix(bytes: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                text.push_str(valid);
                bytes.clear();
                return text;
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    // `valid_up_to` is guaranteed to end at a UTF-8 boundary.
                    text.push_str(std::str::from_utf8(&bytes[..valid_up_to]).expect("valid UTF-8"));
                    bytes.drain(..valid_up_to);
                }
                if let Some(invalid_length) = error.error_len() {
                    text.push('\u{fffd}');
                    bytes.drain(..invalid_length);
                } else {
                    return text;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedTerminal, ratatui_style};
    use avt::Pen;
    use ratatui::style::Style;

    #[test]
    fn default_pty_cells_reset_launcher_styles() {
        assert_eq!(ratatui_style(&Pen::default()), Style::reset());
    }

    #[test]
    fn parses_hvp_positioning() {
        let mut terminal = EmbeddedTerminal::new(5, 3);
        terminal.feed(b"\x1b[2;3fX");
        assert_eq!(terminal.vt.line(1).cells()[2].char(), 'X');
    }

    #[test]
    fn preserves_utf8_split_across_pty_reads() {
        let mut terminal = EmbeddedTerminal::new(5, 1);
        terminal.feed(&[0xe7, 0x95]);
        terminal.feed(&[0x8c]);
        assert_eq!(terminal.vt.line(0).cells()[0].char(), '界');
    }
}
