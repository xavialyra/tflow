use super::session::ViewCompletion;
use super::{Item, PickerView};
use crate::config::Config;
use crate::engine::command;
use crate::router::ViewCandidate;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use std::collections::BTreeMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone)]
pub(crate) struct PickerRenderState {
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) searching: bool,
    pub(crate) completion: Option<ViewCompletion>,
    pub(crate) show_prefix: bool,
    pub(crate) empty_message: String,
}

const PICKER_COLUMN_GAP: usize = 2;
const PICKER_MARKER_WIDTH: usize = 1;
const PICKER_SIDE_PADDING: usize = 1;
const PICKER_PREFIX_RIGHT_PADDING: usize = 0;
const PICKER_SCROLLBAR_WIDTH: usize = 1;
const PICKER_SCROLLBAR_THUMB_HEIGHT: usize = 2;

pub(crate) fn render_picker(frame: &mut Frame, area: Rect, state: &PickerRenderState) {
    let content_area_width = area.width as usize;
    let list_height = area.height as usize;
    let picker_start =
        if state.completion.is_none() && state.selected >= list_height && list_height > 0 {
            state.selected + 1 - list_height
        } else {
            0
        };
    let completion_start = state.completion.as_ref().map(|completion| {
        if completion.selected >= list_height && list_height > 0 {
            completion.selected + 1 - list_height
        } else {
            0
        }
    });
    let item_count = state
        .completion
        .as_ref()
        .map(|completion| completion.candidates.len())
        .unwrap_or(state.items.len());
    let reserves_scrollbar_gutter = item_count > list_height;
    let scroll_start = completion_start.unwrap_or(picker_start);
    let show_scrollbar = reserves_scrollbar_gutter && scroll_start > 0;
    let right_padding = PICKER_SIDE_PADDING
        .saturating_add(usize::from(state.show_prefix) * PICKER_PREFIX_RIGHT_PADDING)
        .saturating_add(usize::from(reserves_scrollbar_gutter) * PICKER_SCROLLBAR_WIDTH);
    let item_area_width = content_area_width.saturating_sub(
        PICKER_MARKER_WIDTH
            .saturating_add(PICKER_SIDE_PADDING)
            .saturating_add(right_padding),
    );
    let prefix_width = if state.show_prefix {
        prefix_column_width(&state.items, item_area_width)
    } else {
        0
    };
    let content_width = if state.show_prefix {
        item_area_width.saturating_sub(prefix_width + PICKER_COLUMN_GAP)
    } else {
        item_area_width
    };

    let selected_row = if let Some(completion) = &state.completion {
        if completion.candidates.is_empty() || list_height == 0 {
            None
        } else {
            Some(
                completion
                    .selected
                    .saturating_sub(completion_start.unwrap_or(0)),
            )
        }
    } else if state.items.is_empty() || list_height == 0 {
        None
    } else {
        Some(state.selected.saturating_sub(picker_start))
    };

    let mut content_lines = Vec::new();
    let mut muted_suffix_ranges = Vec::new();
    let mut accent_ranges = Vec::new();
    if list_height > 0 {
        if let Some(completion) = &state.completion {
            if completion.candidates.is_empty() {
                content_lines.push("(no matching views)".to_string());
            } else {
                let primary_width =
                    completion_primary_width(&completion.candidates, item_area_width);
                let start = completion_start.unwrap_or(0);
                let scrollbar_thumb_top =
                    scrollbar_thumb_top(start, completion.candidates.len(), list_height);
                for (index, candidate) in completion
                    .candidates
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(list_height)
                {
                    let selected = index == completion.selected;
                    let mut line = format_picker_item_line(
                        &format_view_line(candidate, primary_width, item_area_width),
                        content_area_width,
                        selected,
                        right_padding,
                    );
                    let visible_row = index.saturating_sub(start);
                    let scrollbar_start = if show_scrollbar {
                        let thumb_height = PICKER_SCROLLBAR_THUMB_HEIGHT.min(list_height);
                        let thumb = visible_row >= scrollbar_thumb_top
                            && visible_row < scrollbar_thumb_top.saturating_add(thumb_height);
                        line = with_scrollbar(line, thumb);
                        line.ends_with('█')
                            .then(|| line.len().saturating_sub('█'.len_utf8()))
                    } else {
                        None
                    };
                    muted_suffix_ranges.push(None);
                    accent_ranges.push(
                        scrollbar_start.map(|start| (start, start.saturating_add('█'.len_utf8()))),
                    );
                    content_lines.push(line);
                }
            }
        } else if state.items.is_empty() {
            content_lines.push(if state.searching {
                "(searching...)".to_string()
            } else {
                state.empty_message.clone()
            });
        } else {
            let start = picker_start;
            let scrollbar_thumb_top = scrollbar_thumb_top(start, state.items.len(), list_height);
            for (index, item) in state.items.iter().enumerate().skip(start).take(list_height) {
                let (item, suffix_start) = if state.show_prefix {
                    let (item, suffix_start) =
                        format_item_line(&item.text, &item.prefix, content_width, prefix_width);
                    (item, Some(suffix_start))
                } else {
                    (clip(&item.text, content_width), None)
                };
                let selected = index == state.selected;
                let mut item =
                    format_picker_item_line(&item, content_area_width, selected, right_padding);
                let visible_row = index.saturating_sub(start);
                let scrollbar_start = if show_scrollbar {
                    let thumb_height = PICKER_SCROLLBAR_THUMB_HEIGHT.min(list_height);
                    let thumb = visible_row >= scrollbar_thumb_top
                        && visible_row < scrollbar_thumb_top.saturating_add(thumb_height);
                    item = with_scrollbar(item, thumb);
                    item.ends_with('█')
                        .then(|| item.len().saturating_sub('█'.len_utf8()))
                } else {
                    None
                };
                let leading_width = if selected { "▌".len() } else { 1 } + PICKER_SIDE_PADDING;
                muted_suffix_ranges.push(suffix_start.and_then(|start| {
                    let start = leading_width.saturating_add(start);
                    let end = scrollbar_start.unwrap_or(item.len());
                    (start < end
                        && end <= item.len()
                        && item.is_char_boundary(start)
                        && item.is_char_boundary(end))
                    .then_some((start, end))
                }));
                accent_ranges.push(
                    scrollbar_start.map(|start| (start, start.saturating_add('█'.len_utf8()))),
                );
                content_lines.push(item);
            }
        }
    }

    let lines = content_lines
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            picker_line(
                line,
                selected_row == Some(row),
                muted_suffix_ranges.get(row).copied().flatten(),
                accent_ranges.get(row).copied().flatten(),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn picker_line(
    line: String,
    selected: bool,
    suffix_range: Option<(usize, usize)>,
    accent_range: Option<(usize, usize)>,
) -> Line<'static> {
    let marker_style = Style::new()
        .fg(Color::LightCyan)
        .add_modifier(Modifier::BOLD);
    let text_style = if selected {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let suffix_style = Style::new()
        .fg(Color::Rgb(152, 147, 165))
        .add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let (marker, text, marker_width) = if selected {
        match line.strip_prefix('▌') {
            Some(text) => (Some("▌"), text.to_string(), "▌".len()),
            None => (None, line, 0),
        }
    } else {
        (None, line, 0)
    };
    let adjust_range = |range: Option<(usize, usize)>| {
        range.and_then(|(start, end)| {
            Some((
                start.checked_sub(marker_width)?,
                end.checked_sub(marker_width)?,
            ))
        })
    };
    let suffix_range = adjust_range(suffix_range);
    let accent_range = adjust_range(accent_range);
    let mut spans = Vec::new();
    if let Some(marker) = marker {
        spans.push(Span::styled(marker, marker_style));
    }
    spans.extend(picker_spans(
        text,
        text_style,
        suffix_range,
        suffix_style,
        accent_range,
        marker_style,
    ));
    Line::from(spans)
}

fn picker_spans(
    text: String,
    text_style: Style,
    suffix_range: Option<(usize, usize)>,
    suffix_style: Style,
    accent_range: Option<(usize, usize)>,
    accent_style: Style,
) -> Vec<Span<'static>> {
    let valid_range = |range: (usize, usize)| {
        range.0 < range.1
            && range.1 <= text.len()
            && text.is_char_boundary(range.0)
            && text.is_char_boundary(range.1)
    };
    let accent_range = accent_range.filter(|range| valid_range(*range));
    let suffix_range = suffix_range
        .filter(|range| valid_range(*range))
        .filter(|(_, end)| accent_range.map(|(start, _)| *end <= start).unwrap_or(true));
    let mut spans = Vec::new();
    let mut cursor = 0;
    if let Some((start, end)) = suffix_range {
        if start > cursor {
            spans.push(Span::styled(text[cursor..start].to_string(), text_style));
        }
        spans.push(Span::styled(text[start..end].to_string(), suffix_style));
        cursor = end;
    }
    if let Some((start, end)) = accent_range {
        if start > cursor {
            spans.push(Span::styled(text[cursor..start].to_string(), text_style));
        }
        spans.push(Span::styled(text[start..end].to_string(), accent_style));
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::styled(text[cursor..].to_string(), text_style));
    }
    spans
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

fn format_picker_item_line(
    content: &str,
    width: usize,
    selected: bool,
    right_padding: usize,
) -> String {
    if width == 0 {
        return String::new();
    }
    let marker = if selected { "▌" } else { " " };
    let line = clip(
        &format!(
            "{marker}{}{content}{}",
            " ".repeat(PICKER_SIDE_PADDING),
            " ".repeat(right_padding)
        ),
        width,
    );
    pad_right(&line, width)
}

fn scrollbar_thumb_top(start: usize, total: usize, visible: usize) -> usize {
    let thumb_height = PICKER_SCROLLBAR_THUMB_HEIGHT.min(visible);
    let track_height = visible.saturating_sub(thumb_height);
    let scroll_range = total.saturating_sub(visible);
    if track_height == 0 || scroll_range == 0 {
        return 0;
    }
    start.saturating_mul(track_height) / scroll_range
}

fn with_scrollbar(mut line: String, thumb: bool) -> String {
    if !thumb || !line.ends_with(' ') {
        return line;
    }
    line.pop();
    line.push('█');
    line
}

fn format_item_line(
    content: &str,
    prefix: &str,
    content_width: usize,
    prefix_width: usize,
) -> (String, usize) {
    if content_width == 0 {
        return (clip(prefix, prefix_width), 0);
    }
    let content = pad_right(&clip(content, content_width), content_width);
    let suffix_start = content.len().saturating_add(PICKER_COLUMN_GAP);
    let prefix = pad_left(&clip(prefix, prefix_width), prefix_width);
    (
        format!("{}{}{}", content, " ".repeat(PICKER_COLUMN_GAP), prefix,),
        suffix_start,
    )
}

fn completion_primary_width(candidates: &[ViewCandidate], width: usize) -> usize {
    let maximum = width.saturating_sub(20).max(1);
    candidates
        .iter()
        .map(|candidate| UnicodeWidthStr::width(candidate.primary_label()))
        .max()
        .unwrap_or(6)
        .min(maximum)
        .max(1)
}

fn format_view_line(candidate: &ViewCandidate, primary_width: usize, width: usize) -> String {
    let primary = pad_right(
        &clip(candidate.primary_label(), primary_width),
        primary_width,
    );
    let reference = if candidate.alias.is_some() {
        clip(
            candidate.secondary_label(),
            width.saturating_sub(primary_width + PICKER_COLUMN_GAP),
        )
    } else {
        String::new()
    };
    let used = primary_width
        + if reference.is_empty() {
            0
        } else {
            PICKER_COLUMN_GAP + UnicodeWidthStr::width(reference.as_str()) + PICKER_COLUMN_GAP
        };
    let remaining = width.saturating_sub(used);
    let description = if remaining > PICKER_COLUMN_GAP {
        let description = format!("{} [{}]", candidate.plugin_name, candidate.engine_type);
        format!(
            "{}{}",
            " ".repeat(PICKER_COLUMN_GAP),
            clip(&description, remaining.saturating_sub(PICKER_COLUMN_GAP),)
        )
    } else {
        String::new()
    };
    format!(
        "{}{}{}",
        primary,
        if reference.is_empty() {
            String::new()
        } else {
            format!("{}{}", " ".repeat(PICKER_COLUMN_GAP), reference)
        },
        description,
    )
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
    pub(crate) fn visible_commands(&self, config: &Config) -> Vec<(String, String)> {
        let Some(owner) = self.command_owner() else {
            return Vec::new();
        };
        let mut commands = BTreeMap::new();
        command::add_view_commands(config, &mut commands, owner);
        let mut commands = commands.into_iter().collect::<Vec<_>>();
        commands.sort_by(|left, right| command::compare_bindings(&left.0, &right.0));
        commands
    }

    pub(crate) fn render_state(&self) -> PickerRenderState {
        let frame = self.current();
        let (show_prefix, empty_message) = self.list_presentation();
        PickerRenderState {
            items: frame.items.clone(),
            selected: frame.selected,
            searching: frame.input_pending || frame.retry_requested || frame.items_pending,
            completion: self.completion.clone(),
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn completion_selection_uses_the_picker_marker() {
        let state = PickerRenderState {
            items: Vec::new(),
            selected: 0,
            searching: false,
            completion: Some(ViewCompletion {
                candidates: vec![ViewCandidate {
                    view_ref: "apps:default".to_string(),
                    alias: Some("app".to_string()),
                    plugin_name: "apps".to_string(),
                    engine_type: "picker".to_string(),
                }],
                selected: 0,
                selector_start: 0,
                selector_end: 0,
            }),
            show_prefix: false,
            empty_message: "(no matches)".to_string(),
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 4)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, frame.area(), &state))
            .unwrap();

        let selected = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(selected.symbol(), "▌");
        assert_eq!(selected.style().fg, Some(Color::LightCyan));
        assert!(selected.style().add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            terminal.backend().buffer().cell((1, 0)).unwrap().symbol(),
            " "
        );
    }
}
