//! Content host component for the Input And Navigation Model.
//!
//! ContentHost owns:
//! - application framing;
//! - inline and popup placement;
//! - content-area calculation;
//! - popup clearing and borders; and
//! - invocation of visible View rendering.
//!
//! It does not know whether the active View is Picker, Embedded,
//! Capture, or another implementation.

use crate::config::{ViewPresentation, ViewPresentationMode};
use crate::theme::Theme;
use crate::view::{RenderContext, RenderResult, ViewInstance};
use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn block_title_bottom_alignment() {
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| {
                let block = Block::bordered()
                    .title(" title ")
                    .title_bottom(Line::from(" left ").alignment(Alignment::Left))
                    .title_bottom(Line::from(" right ").alignment(Alignment::Right));
                frame.render_widget(block, Rect::new(0, 0, 20, 5));
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let top: String = (0..20).map(|x| buffer.cell((x, 0)).unwrap().symbol()).collect();
        let bottom: String = (0..20).map(|x| buffer.cell((x, 4)).unwrap().symbol()).collect();
        assert!(top.contains("title"));
        assert!(bottom.contains("left"));
        assert!(bottom.contains("right"));

        let mut terminal2 = Terminal::new(TestBackend::new(10, 4)).unwrap();
        terminal2
            .draw(|frame| {
                let block = Block::bordered()
                    .title(" child ")
                    .title_bottom(Line::from(" l local ").alignment(Alignment::Right));
                frame.render_widget(block, Rect::new(0, 0, 10, 4));
            })
            .unwrap();
        let buffer2 = terminal2.backend().buffer();
        let top2: String = (0..10).map(|x| buffer2.cell((x, 0)).unwrap().symbol()).collect();
        let bottom2: String = (0..10).map(|x| buffer2.cell((x, 3)).unwrap().symbol()).collect();
        assert!(top2.contains("child"));
        assert!(bottom2.contains("local"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentHost {
    pub(crate) top_padding: u16,
    pub(crate) left_padding: u16,
    pub(crate) right_padding: u16,
    pub(crate) footer_rows: u16,
}

impl Default for ContentHost {
    fn default() -> Self {
        Self {
            top_padding: 1,
            left_padding: 1,
            right_padding: 1,
            footer_rows: 1,
        }
    }
}

impl ContentHost {
    pub(crate) fn content_area(&self, terminal: Rect) -> Rect {
        let x = terminal.x.saturating_add(self.left_padding);
        let y = terminal.y.saturating_add(self.top_padding);
        let width = terminal
            .width
            .saturating_sub(self.left_padding.saturating_add(self.right_padding));
        let height = terminal
            .height
            .saturating_sub(self.top_padding.saturating_add(self.footer_rows));
        Rect::new(x, y, width, height)
    }

    pub(crate) fn footer_area(&self, terminal: Rect) -> Rect {
        let y = terminal
            .y
            .saturating_add(terminal.height.saturating_sub(self.footer_rows));
        let height = self.footer_rows.min(terminal.height);
        Rect::new(terminal.x, y, terminal.width, height)
    }

    pub(crate) fn popup_rect(&self, area: Rect, presentation: &ViewPresentation) -> Rect {
        let width = presentation.width.unwrap_or(72).min(area.width);
        let height = presentation.height.unwrap_or(16).min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y.saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    pub(crate) fn popup_inner(&self, popup: Rect) -> Rect {
        Rect::new(
            popup.x.saturating_add(1),
            popup.y.saturating_add(1),
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        )
    }

    pub(crate) fn visible_base_index(&self, stack: &[ViewInstance], active_index: usize) -> Option<usize> {
        (0..=active_index).rev().find(|&index| {
            stack[index].context.presentation.mode != ViewPresentationMode::Popup
        })
    }

    pub(crate) fn active_content_area(&self, stack: &[ViewInstance], terminal: Rect) -> Rect {
        let mut area = self.content_area(terminal);
        let Some(active_index) = stack.len().checked_sub(1) else {
            return area;
        };
        let first_popup = self
            .visible_base_index(stack, active_index)
            .map(|index| index.saturating_add(1))
            .unwrap_or(0);
        for item in stack.iter().take(active_index + 1).skip(first_popup) {
            area = self.popup_inner(self.popup_rect(area, &item.context.presentation));
        }
        area
    }

    pub(crate) fn render_frame_background(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let height = area.height as usize;
        let lines = vec![Line::default(); height];
        frame.render_widget(Paragraph::new(Text::from(lines)).style(theme.text), area);
    }

    pub(crate) fn render_views<F>(
        &self,
        frame: &mut Frame,
        content_area: Rect,
        stack: &[ViewInstance],
        render_context: &RenderContext,
        mut render_at: F,
    ) -> Result<(RenderResult, Rect, Option<Rect>)>
    where
        F: FnMut(usize, &mut Frame, Rect, &RenderContext) -> Result<RenderResult>,
    {
        let active_index = stack
            .len()
            .checked_sub(1)
            .ok_or_else(|| anyhow::anyhow!("cannot render views without an active View"))?;
        let base_index = self.visible_base_index(stack, active_index);

        let mut render_area = content_area;
        let mut view = None;
        let first_popup = if let Some(base_index) = base_index {
            view = Some(render_at(base_index, frame, render_area, render_context)?);
            base_index.saturating_add(1)
        } else {
            0
        };
        let mut active_popup_rect = None;
        for (index, item) in stack.iter().enumerate().take(active_index + 1).skip(first_popup) {
            let presentation = &item.context.presentation;
            let popup = self.popup_rect(render_area, presentation);
            frame.render_widget(Clear, popup);
            render_area = self.popup_inner(popup);
            let rendered = render_at(index, frame, render_area, render_context)?;
            if index == active_index {
                active_popup_rect = Some(popup);
            } else {
                let mut block = Block::bordered();
                let label = item.context.location.label();
                if !label.is_empty() {
                    let title_budget = (popup.width as usize).saturating_sub(4);
                    let clipped = super::clip(label, title_budget);
                    if !clipped.is_empty() {
                        block = block.title(format!(" {clipped} "));
                    }
                }
                frame.render_widget(block, popup);
            }
            view = Some(rendered);
        }
        let view = view.expect("a non-empty Router stack must render an active View");
        Ok((view, render_area, active_popup_rect))
    }

    pub(crate) fn render_active_popup_border(
        &self,
        frame: &mut Frame,
        popup: Rect,
        model: &super::FooterModel,
        theme: &Theme,
    ) {
        if popup.width == 0 || popup.height == 0 {
            return;
        }
        let mut block = Block::bordered();
        let title = model
            .title
            .as_deref()
            .unwrap_or_else(|| model.location.label());
        if !title.is_empty() {
            let title_budget = (popup.width as usize).saturating_sub(4);
            let clipped = super::clip(title, title_budget);
            if !clipped.is_empty() {
                block = block.title(format!(" {clipped} "));
            }
        }

        let bottom_width = (popup.width as usize).saturating_sub(2);
        if bottom_width > 0 {
            if let Some(error) = &model.error {
                let budget = bottom_width.saturating_sub(2);
                let clipped = super::clip(error, budget);
                if !clipped.is_empty() {
                    let span = ratatui::text::Span::styled(format!(" {clipped} "), theme.chrome.error);
                    block = block.title_bottom(Line::from(span));
                }
            } else {
                let commands = model.commands();
                let status_text = model.status.as_deref().unwrap_or("");
                let cmd_content = if commands.is_empty() {
                    None
                } else {
                    Some(super::command_footer(&commands))
                };
                let cmd_width = cmd_content
                    .as_ref()
                    .map_or(0, |c| UnicodeWidthStr::width(c.text.as_str()));
                let status_width = if status_text.is_empty() {
                    0
                } else {
                    UnicodeWidthStr::width(status_text)
                };

                let can_show_both = status_width > 0
                    && cmd_width > 0
                    && (status_width + cmd_width + 4 <= bottom_width);

                if can_show_both {
                    let status_span = ratatui::text::Span::styled(
                        format!(" {status_text} "),
                        theme.chrome.footer,
                    );
                    block = block.title_bottom(Line::from(status_span).alignment(Alignment::Left));

                    let cmd_spans = super::spans_from_footer_content(
                        cmd_content.as_ref().unwrap(),
                        theme.chrome.footer,
                        theme.chrome.footer_key,
                    );
                    let mut right_spans = vec![ratatui::text::Span::raw(" ")];
                    right_spans.extend(cmd_spans);
                    right_spans.push(ratatui::text::Span::raw(" "));
                    block = block.title_bottom(Line::from(right_spans).alignment(Alignment::Right));
                } else if cmd_width > 0 {
                    let cmd_spans = if cmd_width <= bottom_width {
                        super::spans_from_footer_content(
                            cmd_content.as_ref().unwrap(),
                            theme.chrome.footer,
                            theme.chrome.footer_key,
                        )
                    } else {
                        let clipped = super::clip_footer(cmd_content.as_ref().unwrap(), bottom_width);
                        super::spans_from_footer_content(
                            &clipped,
                            theme.chrome.footer,
                            theme.chrome.footer_key,
                        )
                    };
                    block = block.title_bottom(Line::from(cmd_spans).alignment(Alignment::Right));
                } else if status_width > 0 {
                    let status_budget = bottom_width.saturating_sub(2);
                    let clipped = super::clip(status_text, status_budget);
                    if !clipped.is_empty() {
                        let status_span = ratatui::text::Span::styled(
                            format!(" {clipped} "),
                            theme.chrome.footer,
                        );
                        block = block.title_bottom(Line::from(status_span).alignment(Alignment::Left));
                    }
                }
            }
        }

        frame.render_widget(block, popup);
    }
}
