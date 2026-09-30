use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

const SCROLLBAR_THUMB_HEIGHT: usize = 2;

pub(super) fn render_capture(
    frame: &mut Frame,
    area: Rect,
    lines: &[Line<'static>],
    scroll_offset: usize,
    theme: &Theme,
) {
    let width = area.width as usize;
    let height = area.height as usize;
    if height == 0 || width == 0 {
        return;
    }

    let total = lines.len();
    let visible = height;
    let start = scroll_offset.min(total.saturating_sub(1));
    let end = (start + visible).min(total);
    let visible_lines = lines[start..end].to_vec();

    let reserves_scrollbar = total > visible;
    let show_scrollbar = scrollbar_visible(total, visible, start);
    let thumb_top = scrollbar_thumb_top(start, total, visible);
    let thumb_height = SCROLLBAR_THUMB_HEIGHT.min(visible);

    let scrollbar_color = if let Some(bg) = theme.picker.scrollbar.bg {
        if Some(bg) != theme.capture.text.bg {
            bg
        } else {
            theme.picker.scrollbar.fg.unwrap_or(bg)
        }
    } else {
        theme
            .picker
            .scrollbar
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    };
    let scrollbar_style = ratatui::style::Style::default().bg(scrollbar_color);

    let content_width = if reserves_scrollbar {
        area.width.saturating_sub(1)
    } else {
        area.width
    };

    let content_area = Rect {
        x: area.x,
        y: area.y,
        width: content_width,
        height: area.height,
    };

    frame.render_widget(
        Paragraph::new(Text::from(visible_lines)).style(theme.capture.text),
        content_area,
    );

    if show_scrollbar {
        let scrollbar_area = Rect {
            x: area.x + area.width.saturating_sub(1),
            y: area.y + thumb_top as u16,
            width: 1,
            height: thumb_height as u16,
        };
        frame.render_widget(ratatui::widgets::Clear, scrollbar_area);
        frame.render_widget(
            ratatui::widgets::Block::default().style(scrollbar_style),
            scrollbar_area,
        );
    }
}

fn scrollbar_visible(total: usize, visible: usize, start: usize) -> bool {
    total > visible && start > 0
}

fn scrollbar_thumb_top(start: usize, total: usize, visible: usize) -> usize {
    let thumb_height = SCROLLBAR_THUMB_HEIGHT.min(visible);
    let track_height = visible.saturating_sub(thumb_height);
    let scroll_range = total.saturating_sub(visible);
    if track_height == 0 || scroll_range == 0 {
        return 0;
    }
    start.saturating_mul(track_height) / scroll_range
}

pub(crate) struct CaptureRenderer;

impl crate::engine::ViewRenderer for CaptureRenderer {
    fn validate_model(&self, model: &crate::engine::RenderModel) -> anyhow::Result<()> {
        if model.kind() != "capture" || model.downcast_ref::<super::CaptureRenderModel>().is_none()
        {
            anyhow::bail!(
                "capture renderer/model pairing mismatch: renderer=capture model={:?}",
                model
            );
        }
        Ok(())
    }

    fn chrome(&self, model: &crate::engine::RenderModel) -> crate::ui::chrome::EngineChrome {
        let Some(model) = model.downcast_ref::<super::CaptureRenderModel>() else {
            return crate::ui::chrome::EngineChrome::default();
        };
        let status = if model.status.is_empty() {
            None
        } else {
            Some(model.status.clone())
        };
        crate::ui::chrome::EngineChrome::new(status)
    }

    fn render(
        &self,
        model: &crate::engine::RenderModel,
        context: &crate::engine::RenderContext,
        frame: &mut Frame,
        area: Rect,
    ) {
        let Some(model) = model.downcast_ref::<super::CaptureRenderModel>() else {
            return;
        };
        render_capture(
            frame,
            area,
            model.lines.as_ref(),
            model.scroll_offset,
            &context.theme,
        );
    }
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
                render_capture(frame, frame.area(), &[Line::raw("output")], 0, &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn capture_scrollbar_visibility() {
        assert!(!scrollbar_visible(10, 5, 0));
        assert!(scrollbar_visible(10, 5, 1));
        assert!(!scrollbar_visible(5, 5, 1));
    }

    #[test]
    fn capture_renders_ansi_colors_and_styles() {
        use ansi_to_tui::IntoText;
        let theme = Theme::terminal();
        let ansi_str = "\x1b[31mred\x1b[0m\x1b[32mgreen\x1b[0m";
        let text = ansi_str.into_text().unwrap();
        let mut terminal = RatatuiTerminal::new(TestBackend::new(10, 1)).unwrap();

        terminal
            .draw(|frame| {
                render_capture(frame, frame.area(), &text.lines, 0, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "r");
        assert_eq!(buffer.cell((0, 0)).unwrap().style().fg, Some(Color::Red));
        assert_eq!(buffer.cell((3, 0)).unwrap().symbol(), "g");
        assert_eq!(buffer.cell((3, 0)).unwrap().style().fg, Some(Color::Green));
    }

    #[test]
    fn capture_scroll_renders_view_slice_and_scrollbar() {
        let theme = Theme::terminal();
        let lines = (0..10)
            .map(|i| Line::raw(format!("line{i}")))
            .collect::<Vec<_>>();
        let mut terminal = RatatuiTerminal::new(TestBackend::new(10, 4)).unwrap();

        // When scrolled to offset 2, visible lines should be line2, line3, line4, line5
        // And scrollbar should be visible on column 9
        terminal
            .draw(|frame| {
                render_capture(frame, frame.area(), &lines, 2, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        // Row 0 should start with line2
        assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "l");
        assert_eq!(buffer.cell((4, 0)).unwrap().symbol(), "2");
        // Row 3 should start with line5
        assert_eq!(buffer.cell((4, 3)).unwrap().symbol(), "5");
        // Column 9 should have scrollbar thumb rendered
        assert_ne!(
            buffer.cell((9, 0)).unwrap().style().bg,
            theme.capture.text.bg
        );
    }
}
