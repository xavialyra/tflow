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
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Clear, Paragraph};

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
    ) -> Result<(RenderResult, Rect)>
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
        for (index, item) in stack.iter().enumerate().take(active_index + 1).skip(first_popup) {
            let presentation = &item.context.presentation;
            let popup = self.popup_rect(render_area, presentation);
            frame.render_widget(Clear, popup);
            frame.render_widget(Block::bordered(), popup);
            render_area = self.popup_inner(popup);
            view = Some(render_at(index, frame, render_area, render_context)?);
        }
        let view = view.expect("a non-empty Router stack must render an active View");
        Ok((view, render_area))
    }
}
