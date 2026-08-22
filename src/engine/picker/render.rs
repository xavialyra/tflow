use super::{Item, PickerView};
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone)]
pub(crate) struct PickerRenderState {
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) searching: bool,
    pub(crate) show_prefix: bool,
    pub(crate) empty_message: String,
}

const COLUMN_GAP: usize = 2;
const SIDE_PADDING: usize = 1;
const MARKER_WIDTH: usize = 1;
const SCROLLBAR_WIDTH: usize = 1;
const SCROLLBAR_THUMB_HEIGHT: usize = 2;

pub(crate) fn render_picker(
    frame: &mut Frame,
    area: Rect,
    state: &PickerRenderState,
    theme: &Theme,
) {
    let width = area.width as usize;
    let height = area.height as usize;
    let start = if state.selected >= height && height > 0 {
        state.selected + 1 - height
    } else {
        0
    };
    let reserves_scrollbar = state.items.len() > height;
    let show_scrollbar = scrollbar_visible(state.items.len(), height, start);
    let right_padding = SIDE_PADDING + usize::from(reserves_scrollbar) * SCROLLBAR_WIDTH;
    let item_width = width.saturating_sub(MARKER_WIDTH + SIDE_PADDING + right_padding);
    let prefix_width = if state.show_prefix {
        prefix_column_width(&state.items, item_width)
    } else {
        0
    };
    let content_width = if state.show_prefix {
        item_width.saturating_sub(prefix_width + COLUMN_GAP)
    } else {
        item_width
    };

    let mut lines = Vec::new();
    if height > 0 && state.items.is_empty() {
        let text = if state.searching {
            "(searching...)"
        } else {
            &state.empty_message
        };
        lines.push(Line::from(Span::styled(
            text.to_string(),
            theme.picker.muted,
        )));
    } else {
        let thumb_top = scrollbar_thumb_top(start, state.items.len(), height);
        for (visible_row, (index, item)) in state
            .items
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .enumerate()
        {
            let selected = index == state.selected;
            let (content, prefix_range) = if state.show_prefix {
                let text = pad_right(&clip(&item.text, content_width), content_width);
                let prefix = pad_left(&clip(&item.prefix, prefix_width), prefix_width);
                let prefix_start = text.len().saturating_add(COLUMN_GAP);
                let prefix_end = prefix_start.saturating_add(prefix.len());
                (
                    format!("{text}{}{prefix}", " ".repeat(COLUMN_GAP)),
                    Some((prefix_start, prefix_end)),
                )
            } else {
                (clip(&item.text, content_width), None)
            };
            let marker = if selected { "▌" } else { " " };
            let leading_bytes = marker.len().saturating_add(SIDE_PADDING);
            let prefix_range = prefix_range.map(|(start, end)| {
                (
                    start.saturating_add(leading_bytes),
                    end.saturating_add(leading_bytes),
                )
            });
            let mut text = pad_right(
                &clip(
                    &format!("{marker} {content}{}", " ".repeat(right_padding)),
                    width,
                ),
                width,
            );
            if show_scrollbar {
                let thumb_height = SCROLLBAR_THUMB_HEIGHT.min(height);
                if visible_row >= thumb_top
                    && visible_row < thumb_top.saturating_add(thumb_height)
                    && text.ends_with(' ')
                {
                    text.pop();
                    text.push('█');
                }
            }
            lines.push(picker_line(text, selected, prefix_range, theme));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(theme.picker.text),
        area,
    );
}

fn picker_line(
    mut text: String,
    selected: bool,
    prefix_range: Option<(usize, usize)>,
    theme: &Theme,
) -> Line<'static> {
    let marker_style = theme.picker.marker;
    let scrollbar_style = theme.picker.scrollbar;
    let body = if selected {
        theme.picker.selected
    } else {
        theme.picker.text
    };
    let prefix = if selected {
        theme.picker.selected_muted
    } else {
        theme.picker.muted
    };
    let scrollbar = text.ends_with('█');
    if scrollbar {
        text.pop();
    }
    let has_marker = selected && text.starts_with('▌');
    let marker_bytes = if has_marker { '▌'.len_utf8() } else { 0 };
    if has_marker {
        text.remove(0);
    }
    let prefix_range = prefix_range.and_then(|(start, end)| {
        let start = start.checked_sub(marker_bytes)?;
        let end = end.checked_sub(marker_bytes)?;
        (start < end
            && end <= text.len()
            && text.is_char_boundary(start)
            && text.is_char_boundary(end))
        .then_some((start, end))
    });

    let mut spans = Vec::with_capacity(5);
    if has_marker {
        spans.push(Span::styled("▌", marker_style));
    }
    if let Some((start, end)) = prefix_range {
        spans.push(Span::styled(text[..start].to_string(), body));
        spans.push(Span::styled(text[start..end].to_string(), prefix));
        spans.push(Span::styled(text[end..].to_string(), body));
    } else {
        spans.push(Span::styled(text, body));
    }
    if scrollbar {
        spans.push(Span::styled("█", scrollbar_style));
    }
    Line::from(spans)
}

fn prefix_column_width(items: &[Item], width: usize) -> usize {
    let longest = items
        .iter()
        .map(|item| UnicodeWidthStr::width(item.prefix.as_str()))
        .max()
        .unwrap_or(6)
        .clamp(6, 20);
    longest.min(width.saturating_sub(8).max(1))
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

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{}", text, " ".repeat(width.saturating_sub(used)))
}

fn pad_left(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{}", " ".repeat(width.saturating_sub(used)), text)
}

impl PickerView {
    pub(crate) fn render_state(&self) -> PickerRenderState {
        let frame = self.current();
        let (show_prefix, empty_message) = self.list_presentation();
        PickerRenderState {
            items: frame.items.clone(),
            selected: frame.selected,
            searching: frame.input_pending || frame.retry_requested || frame.items_pending,
            show_prefix,
            empty_message,
        }
    }
}

fn clip(text: &str, width: usize) -> String {
    if width == 0 || UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }

    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 3 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push_str("...");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn scrollbar_is_hidden_at_the_top_and_shown_after_scrolling() {
        assert!(!scrollbar_visible(10, 5, 0));
        assert!(scrollbar_visible(10, 5, 1));
        assert!(!scrollbar_visible(5, 5, 1));
    }

    #[test]
    fn picker_markers_and_scrollbars_use_their_bindings() {
        let mut theme = Theme::terminal();
        theme.picker.marker.fg = Some(Color::Magenta);
        theme.picker.scrollbar.fg = Some(Color::Green);

        let selected = picker_line("▌ item  █".to_string(), true, None, &theme);
        assert_eq!(selected.spans.first().unwrap().content, "▌");
        assert_eq!(
            selected.spans.first().unwrap().style.fg,
            Some(Color::Magenta)
        );
        assert_eq!(selected.spans.last().unwrap().content, "█");
        assert_eq!(selected.spans.last().unwrap().style.fg, Some(Color::Green));

        let unselected = picker_line("  item  █".to_string(), false, None, &theme);
        assert_eq!(unselected.spans.last().unwrap().content, "█");
        assert_eq!(
            unselected.spans.last().unwrap().style.fg,
            Some(Color::Green)
        );
    }

    #[test]
    fn picker_prefix_uses_the_selected_container_style() {
        let theme = Theme::terminal();
        let line = picker_line("▌ Item  sys ".to_string(), true, Some((10, 13)), &theme);
        let prefix = line
            .spans
            .iter()
            .find(|span| span.content == "sys")
            .expect("prefix span should be separate");
        assert_eq!(prefix.style.fg, Some(Color::Cyan));
        assert_eq!(prefix.style.bg, Some(Color::Reset));
        assert!(!prefix.style.add_modifier.contains(Modifier::DIM));
        assert!(!prefix.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn selected_prefix_keeps_a_custom_muted_foreground() {
        let mut theme = Theme::terminal();
        theme.picker.muted.fg = Some(Color::Gray);
        theme.picker.selected = Style::new()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD);
        theme.picker.selected_muted = Style::new()
            .fg(Color::Gray)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD);

        let line = picker_line("▌ Item  sys ".to_string(), true, Some((10, 13)), &theme);
        let prefix = line
            .spans
            .iter()
            .find(|span| span.content == "sys")
            .expect("prefix span should be separate");
        assert_eq!(prefix.style.fg, Some(Color::Gray));
        assert_eq!(prefix.style.bg, Some(Color::Green));
        assert!(prefix.style.add_modifier.contains(Modifier::BOLD));

        let body = line
            .spans
            .iter()
            .find(|span| span.content.contains("Item"))
            .expect("selected body span should be present");
        assert_eq!(body.style.fg, Some(Color::Black));
        assert_eq!(body.style.bg, Some(Color::Green));
    }

    #[test]
    fn clips_wide_text_without_exceeding_the_width() {
        assert_eq!(UnicodeWidthStr::width(clip("abcdefgh", 6).as_str()), 6);
        assert!(UnicodeWidthStr::width(clip("終端abcdef", 7).as_str()) <= 7);
    }
}
