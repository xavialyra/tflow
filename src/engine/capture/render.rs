use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

pub(super) fn render_capture(frame: &mut Frame, area: Rect, lines: &[String], theme: &Theme) {
    let start = lines.len().saturating_sub(area.height as usize);
    let lines = lines[start..]
        .iter()
        .cloned()
        .map(Line::raw)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(theme.capture.text),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal as RatatuiTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    #[test]
    fn capture_uses_the_capture_text_binding() {
        let mut theme = Theme::terminal();
        theme.capture.text.fg = Some(Color::Magenta);
        theme.capture.text.bg = Some(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(8, 1)).unwrap();

        terminal
            .draw(|frame| {
                render_capture(frame, frame.area(), &["output".to_string()], &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }
}
