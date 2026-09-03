//! Host footer rendering component for the Input And Navigation Model.
//!
//! The footer consumes only generic data from the committed Router location
//! and active View metadata. It never renders or inspects a View's editor,
//! cursor, completion, or item list.

use super::{FooterContent, clip, clip_footer, footer_line};
use crate::theme::Theme;
use crate::view::{BindingSet, ViewLocation};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FooterModel {
    pub(crate) location: ViewLocation,
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) bindings: BindingSet,
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
    pub(crate) fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        model: &FooterModel,
        theme: &Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let footer_width = width.saturating_sub(self.left_padding.saturating_add(self.right_padding));
        let is_error = model.error.is_some();
        let footer = if let Some(error) = &model.error {
            FooterContent::plain(error)
        } else {
            let mut commands = model
                .bindings
                .entries()
                .iter()
                .filter_map(|binding| {
                    Some((binding.key.binding_name()?, binding.label.as_ref()?.clone()))
                })
                .collect::<Vec<_>>();
            commands.sort_by(|left, right| crate::command::compare_bindings(&left.0, &right.0));

            let view_status = match (model.title.as_deref(), model.status.as_deref()) {
                (Some(title), Some(status)) if !title.is_empty() && !status.is_empty() => {
                    Some(format!("{title} | {status}"))
                }
                (Some(title), _) if !title.is_empty() => Some(title.to_string()),
                (_, Some(status)) if !status.is_empty() => Some(status.to_string()),
                _ => None,
            };

            footer_line(
                footer_width,
                Some(model.location.label()),
                view_status.as_deref().unwrap_or(""),
                &commands,
                None,
            )
        };

        let footer = clip_footer(&footer, footer_width);
        let footer_style = if is_error {
            theme.chrome.error
        } else {
            theme.chrome.footer
        };
        let key_style = theme.chrome.footer_key;

        let mut spans = vec![Span::raw(" ".repeat(self.left_padding.min(width)))];
        let mut cursor = 0;
        let text = clip(&footer.text, footer_width);
        for &(start, end) in &footer.key_spans {
            let start = start.max(cursor).min(text.len());
            let end = end.max(start).min(text.len());
            if start > cursor {
                spans.push(Span::styled(text[cursor..start].to_string(), footer_style));
            }
            if end > start {
                spans.push(Span::styled(text[start..end].to_string(), key_style));
            }
            cursor = end;
        }
        if cursor < text.len() {
            spans.push(Span::styled(text[cursor..].to_string(), footer_style));
        }
        let used = UnicodeWidthStr::width(text.as_str());
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
}
