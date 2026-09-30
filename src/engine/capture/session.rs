use ansi_to_tui::IntoText;
use ratatui::text::{Line, Text};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct CaptureSession {
    clean_output: String,
    lines: Arc<[Line<'static>]>,
    scroll_offset: usize,
    viewport_height: usize,
}

impl CaptureSession {
    pub(crate) fn new(output: &str) -> Self {
        let lines = capture_lines(output);
        let clean_output = strip_ansi_multiline(output);
        Self {
            clean_output,
            lines: lines.into(),
            scroll_offset: 0,
            viewport_height: 1,
        }
    }

    pub(crate) fn output(&self) -> &str {
        &self.clean_output
    }

    pub(crate) fn shared_lines(&self) -> Arc<[Line<'static>]> {
        Arc::clone(&self.lines)
    }

    pub(crate) fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub(crate) fn viewport_height(&self) -> usize {
        self.viewport_height
    }

    pub(crate) fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
        self.clamp_scroll();
    }

    pub(crate) fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub(crate) fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max_offset = self.lines.len().saturating_sub(self.viewport_height);
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }
    }
}

fn capture_lines(output: &str) -> Vec<Line<'static>> {
    if output.is_empty() {
        return vec![Line::raw("(no output)")];
    }
    match output.into_text() {
        Ok(text) => text.lines,
        Err(_) => Text::raw(output.to_string()).lines,
    }
}

fn strip_ansi_multiline(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }
    output
        .lines()
        .map(crate::terminal::sanitize_terminal_text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn capture_lines_keeps_a_visible_line_for_empty_output() {
        let session = CaptureSession::new("");
        assert_eq!(session.shared_lines().len(), 1);
        assert_eq!(session.shared_lines()[0].to_string(), "(no output)");
    }

    #[test]
    fn capture_lines_parses_ansi_colors() {
        let session = CaptureSession::new("one\x1b[31mtwo\x1b[0m");
        let lines = session.shared_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "onetwo");
        // Verify red color in span
        let spans = &lines[0].spans;
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "one");
        assert_eq!(spans[1].content, "two");
        assert_eq!(spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn capture_session_strips_ansi_for_clean_output() {
        let session = CaptureSession::new("line 1\x1b[32m colored\x1b[0m\nline 2");
        assert_eq!(session.output(), "line 1 colored\nline 2");
    }

    #[test]
    fn capture_session_scrolling() {
        let output = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut session = CaptureSession::new(&output);
        session.set_viewport_height(5);
        assert_eq!(session.scroll_offset(), 0);

        session.scroll_down(3);
        assert_eq!(session.scroll_offset(), 3);

        session.scroll_down(50);
        // max offset: 20 - 5 = 15
        assert_eq!(session.scroll_offset(), 15);

        session.scroll_up(4);
        assert_eq!(session.scroll_offset(), 11);

        session.scroll_up(20);
        assert_eq!(session.scroll_offset(), 0);
    }
}
