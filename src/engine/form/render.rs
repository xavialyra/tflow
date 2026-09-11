use super::FormView;
use crate::view::{RelativeCursor, RenderResult};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Preserve the draft verbatim; make controls visible only in the display.
fn visible(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn display_width(grapheme: &str) -> usize {
    if grapheme.chars().any(char::is_control) {
        grapheme
            .chars()
            .map(|c| {
                if c.is_control() {
                    1
                } else {
                    c.width().unwrap_or(0)
                }
            })
            .sum()
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

const MAX_FORM_WIDTH: u16 = 48;
const BOXED_FIELD_HEIGHT: u16 = 3;
const FIELD_ERROR_HEIGHT: u16 = 4;

fn field_error(draft: &super::content::Draft) -> Option<String> {
    match draft.value() {
        Err(error) if error != "Required" => Some(error),
        _ => None,
    }
}

fn visible_page(field_heights: &[u16], focus: usize, height: u16) -> (usize, usize, u16) {
    if field_heights.is_empty() || height == 0 {
        return (0, 0, 0);
    }
    let focus = focus.min(field_heights.len() - 1);
    let mut first = focus;
    let mut last = focus + 1;
    let mut used = field_heights[focus].min(height);

    while first > 0 {
        let next = field_heights[first - 1];
        if used.saturating_add(next) > height {
            break;
        }
        first -= 1;
        used = used.saturating_add(next);
    }
    while last < field_heights.len() {
        let next = field_heights[last];
        if used.saturating_add(next) > height {
            break;
        }
        last += 1;
        used = used.saturating_add(next);
    }
    (first, last, used)
}

fn window(text: &str, cursor: usize, width: u16) -> (String, u16) {
    // Walk backward only as far as the visible prefix and allocate only the
    // displayed window. Long pasted drafts never need repeated prefix copies.
    let width = usize::from(width.max(1));
    let mut start = cursor;
    let mut x = 0;
    for (index, grapheme) in text[..cursor].grapheme_indices(true).rev() {
        let cells = display_width(grapheme);
        if x + cells >= width {
            break;
        }
        start = index;
        x += cells;
    }
    let mut output = String::new();
    let mut cells = 0;
    for grapheme in text[start..].graphemes(true) {
        let next = display_width(grapheme);
        if cells + next > width {
            break;
        }
        cells += next;
        output.extend(
            grapheme
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c }),
        );
    }
    (output, x as u16)
}

impl FormView {
    pub(super) fn render_form(&self, frame: &mut Frame, area: Rect) -> RenderResult {
        let mut result = RenderResult::default();
        if area.width == 0 || area.height == 0 {
            return result;
        }
        let theme = self.theme.form;
        frame.render_widget(Paragraph::new("").style(theme.input), area);
        if let Some(error) = &self.error {
            frame.render_widget(Paragraph::new(visible(error)).style(theme.error), area);
            return result;
        }
        if !self.publication.ready || self.fields.is_empty() {
            let message = if self.publication.ready {
                "No fields"
            } else {
                "Loading form…"
            };
            frame.render_widget(Paragraph::new(message).style(theme.input), area);
            return result;
        }

        // Keep the editor column readable on wide terminals and use the spare
        // height for validation feedback.
        let boxed = area.width >= 3 && area.height >= 3;
        let errors = self.fields.iter().map(field_error).collect::<Vec<_>>();
        let field_heights = if boxed {
            errors
                .iter()
                .map(|error| {
                    if error.is_some() {
                        FIELD_ERROR_HEIGHT
                    } else {
                        BOXED_FIELD_HEIGHT
                    }
                    .min(area.height)
                })
                .collect::<Vec<_>>()
        } else {
            vec![1; self.fields.len()]
        };
        let (first, last, total_height) = if boxed {
            visible_page(&field_heights, self.focus, area.height)
        } else {
            let focus = self.focus.min(self.fields.len() - 1);
            (focus, focus + 1, 1)
        };
        let width = if area.width >= 8 {
            area.width.saturating_sub(4).min(MAX_FORM_WIDTH)
        } else {
            area.width
        };
        let column = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(total_height) / 2,
            width,
            total_height,
        );
        for index in first..last {
            let y = field_heights[first..index].iter().copied().sum::<u16>();
            let field_height = field_heights[index];
            let focused = index == self.focus;
            let bounds = Rect::new(
                column.x,
                column.y + y,
                column.width,
                field_height.min(BOXED_FIELD_HEIGHT),
            );
            let editor = if boxed {
                let draft = &self.fields[index];
                let label = format!(
                    "{}{}{}",
                    if focused { "> " } else { "  " },
                    draft.field.label.as_deref().unwrap_or(&draft.field.name),
                    if draft.field.required { " *" } else { "" }
                );
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(if focused { theme.label } else { theme.input })
                    .title(visible(&label));
                let inner = block.inner(bounds);
                frame.render_widget(block, bounds);
                inner
            } else {
                Rect::new(bounds.x, bounds.y, bounds.width, 1)
            };
            let draft = &self.fields[index];
            let cursor = if focused { draft.buffer.cursor } else { 0 };
            let (text, x) = window(&draft.buffer.raw, cursor, editor.width);
            frame.render_widget(
                Paragraph::new(text).style(if focused { theme.focused } else { theme.input }),
                editor,
            );
            if focused {
                result.cursor = Some(RelativeCursor {
                    x: editor.x - area.x + x,
                    y: editor.y - area.y,
                    visible: self.active,
                });
            }
            if boxed
                && field_height > BOXED_FIELD_HEIGHT
                && let Some(error) = &errors[index]
            {
                frame.render_widget(
                    Paragraph::new(visible(error)).style(theme.error),
                    Rect::new(
                        column.x + 1,
                        column.y + y + BOXED_FIELD_HEIGHT,
                        column.width.saturating_sub(2),
                        1,
                    ),
                );
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_unicode_drafts_render_only_the_visible_window() {
        let text = "界e\u{301}".repeat(100_000);
        for cursor in [0, text.len() / 2, text.len()] {
            let (output, x) = window(&text, cursor, 20);
            assert!(UnicodeWidthStr::width(output.as_str()) <= 20);
            assert!(output.len() <= 60);
            assert!(x < 20);
            assert!(text.contains(&output));
        }
        assert_eq!(window("a\nb", 2, 3), ("a b".into(), 2));
        assert_eq!(window("界界", 6, 1), (String::new(), 0));
    }
}
