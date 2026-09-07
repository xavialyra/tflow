use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

pub(super) fn render_capture(frame: &mut Frame, area: Rect, lines: &[String], theme: &Theme) {
    let start = lines.len().saturating_sub(area.height as usize);
    let lines = lines[start..]
        .iter()
        .map(|line| Line::raw(line.as_str()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(theme.capture.text),
        area,
    );
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
        render_capture(frame, area, model.lines.as_ref(), &context.theme);
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
                render_capture(frame, frame.area(), &["output".to_string()], &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }
}
