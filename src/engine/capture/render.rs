use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

pub(super) fn render_capture(frame: &mut Frame, area: Rect, lines: &[String]) {
    let start = lines.len().saturating_sub(area.height as usize);
    let lines = lines[start..]
        .iter()
        .cloned()
        .map(Line::raw)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}
