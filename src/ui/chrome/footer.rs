//! Host footer rendering component for the Input And Navigation Model.
//!
//! The footer consumes only generic data from the committed Router location
//! and active View metadata. It never renders or inspects a View's editor,
//! cursor, completion, or item list.

use super::{FooterContent, clip_footer, footer_line};
use crate::ui::theme::Theme;
use crate::view::{BindingSet, ViewLocation};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FooterModel {
    pub(crate) location: ViewLocation,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) info: Option<String>,
    pub(crate) bindings: BindingSet,
    pub(crate) overflow_command: Option<(String, String)>,
    pub(crate) has_unbound: bool,
}

impl FooterModel {
    pub(crate) fn notification(&self, theme: &Theme) -> Option<(&str, ratatui::style::Style)> {
        self.error
            .as_deref()
            .map(|text| (text, theme.chrome.error))
            .or_else(|| {
                self.info
                    .as_deref()
                    .map(|text| (text, theme.chrome.footer_status))
            })
    }

    pub(crate) fn commands(&self) -> Vec<(String, String)> {
        let mut commands = self
            .bindings
            .entries()
            .iter()
            .filter_map(|binding| {
                Some((binding.key.binding_name()?, binding.label.as_ref()?.clone()))
            })
            .collect::<Vec<_>>();
        commands
            .sort_by(|left, right| crate::workflow::command::compare_bindings(&left.0, &right.0));
        commands
    }
}

pub(crate) fn spans_from_footer_content(
    content: &FooterContent,
    base_style: ratatui::style::Style,
    key_style: ratatui::style::Style,
    title_style: ratatui::style::Style,
    status_style: ratatui::style::Style,
) -> Vec<Span<'static>> {
    let mut styled_intervals: Vec<((usize, usize), ratatui::style::Style)> = Vec::new();
    if let Some((start, end)) = content.title_span {
        styled_intervals.push(((start, end), title_style));
    }
    if let Some((start, end)) = content.status_span {
        styled_intervals.push(((start, end), status_style));
    }
    for &(start, end) in &content.key_spans {
        styled_intervals.push(((start, end), key_style));
    }
    styled_intervals.sort_by_key(|(range, _)| range.0);

    let mut spans = Vec::new();
    let mut cursor = 0;
    for &((start, end), style) in &styled_intervals {
        let start = start.max(cursor).min(content.text.len());
        let end = end.max(start).min(content.text.len());
        if start > cursor {
            spans.push(Span::styled(
                content.text[cursor..start].to_string(),
                base_style,
            ));
        }
        if end > start {
            spans.push(Span::styled(content.text[start..end].to_string(), style));
        }
        cursor = end;
    }
    if cursor < content.text.len() {
        spans.push(Span::styled(content.text[cursor..].to_string(), base_style));
    }
    spans
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FooterRenderer {
    pub(crate) left_padding: usize,
    pub(crate) right_padding: usize,
}

impl Default for FooterRenderer {
    fn default() -> Self {
        Self {
            left_padding: 1,
            right_padding: 1,
        }
    }
}

impl FooterRenderer {
    pub(crate) fn render(&self, frame: &mut Frame, area: Rect, model: &FooterModel, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let footer_width =
            width.saturating_sub(self.left_padding.saturating_add(self.right_padding));
        let notification = model.notification(theme);
        let footer = if let Some((message, _)) = notification {
            FooterContent::plain(message)
        } else {
            let mut commands = model.commands();
            if let Some((overflow_key, _)) = &model.overflow_command {
                commands.retain(|(key, _)| key != overflow_key);
            }

            let status = model.status.as_deref().unwrap_or("");

            footer_line(
                footer_width,
                Some(model.location.label()),
                status,
                &commands,
                model.overflow_command.as_ref(),
                model.has_unbound,
            )
        };

        let footer = clip_footer(&footer, footer_width);
        let footer_style = notification.map_or(theme.chrome.footer, |(_, style)| style);
        let key_style = theme.chrome.footer_key;

        let mut spans = vec![Span::raw(" ".repeat(self.left_padding.min(width)))];
        let text_spans = spans_from_footer_content(
            &footer,
            footer_style,
            key_style,
            theme.chrome.footer_title,
            theme.chrome.footer_status,
        );
        spans.extend(text_spans);
        let used = UnicodeWidthStr::width(footer.text.as_str());
        spans.push(Span::styled(
            " ".repeat(footer_width.saturating_sub(used)),
            footer_style,
        ));
        let remaining = width
            .saturating_sub(self.left_padding.min(width))
            .saturating_sub(footer_width);
        spans.push(Span::raw(" ".repeat(remaining)));

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    pub(crate) fn render_blank(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let spaces = " ".repeat(width);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(spaces, theme.chrome.footer))),
            area,
        );
    }
}
