use super::input::ViewCompletion;
use super::{Item, PickerView};
use crate::config::Config;
use crate::router::ViewCandidate;
use crate::terminal::Terminal;
use anyhow::Result;
use std::collections::BTreeMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone)]
pub(crate) struct PickerRenderState {
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) searching: bool,
    pub(crate) completion: Option<ViewCompletion>,
}

const PICKER_COLUMN_GAP: usize = 2;

pub(crate) fn picker_content(
    terminal: &Terminal,
    state: &PickerRenderState,
    chrome: &crate::chrome::ChromeFrame,
) -> Result<crate::chrome::ChromeContent> {
    let (width, height) = terminal.size();
    let width = width as usize;
    let height = height as usize;
    let layout = chrome.layout();
    let content_area_width = layout.content_width(width);
    let list_height = layout.content_rows(height);
    let prefix_width = prefix_column_width(&state.items, content_area_width);
    let content_width = content_area_width.saturating_sub(prefix_width + PICKER_COLUMN_GAP);

    let completion_start = state.completion.as_ref().map(|completion| {
        if completion.selected >= list_height && list_height > 0 {
            completion.selected + 1 - list_height
        } else {
            0
        }
    });
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
    } else {
        let start = if state.selected >= list_height && list_height > 0 {
            state.selected + 1 - list_height
        } else {
            0
        };
        if state.items.is_empty() || list_height == 0 {
            None
        } else {
            Some(state.selected.saturating_sub(start))
        }
    };

    let mut content_lines = Vec::new();
    if list_height > 0 {
        if let Some(completion) = &state.completion {
            if completion.candidates.is_empty() {
                content_lines.push("(no matching views)".to_string());
            } else {
                let primary_width =
                    completion_primary_width(&completion.candidates, content_area_width);
                let start = completion_start.unwrap_or(0);
                for candidate in completion.candidates.iter().skip(start).take(list_height) {
                    content_lines.push(format_view_line(
                        candidate,
                        primary_width,
                        content_area_width,
                    ));
                }
            }
        } else if state.items.is_empty() {
            content_lines.push(if state.searching {
                "(searching...)".to_string()
            } else {
                "(no matches)".to_string()
            });
        } else {
            let start = if state.selected >= list_height && list_height > 0 {
                state.selected + 1 - list_height
            } else {
                0
            };
            for item in state.items.iter().skip(start).take(list_height) {
                content_lines.push(format_item_line(
                    &item.prefix,
                    &item.text,
                    prefix_width,
                    content_width,
                ));
            }
        }
    }

    Ok(crate::chrome::ChromeContent::new(
        content_lines,
        selected_row,
        crate::chrome::ChromeCursor::Input,
    ))
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

fn format_item_line(
    prefix: &str,
    content: &str,
    prefix_width: usize,
    content_width: usize,
) -> String {
    let prefix = pad_right(&clip(prefix, prefix_width), prefix_width);
    format!(
        "{}{}{}",
        prefix,
        " ".repeat(PICKER_COLUMN_GAP),
        clip(content, content_width),
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

impl PickerView {
    pub(crate) fn visible_commands(&self, config: &Config) -> Vec<(String, String)> {
        let Some(owner) = self.command_owner() else {
            return Vec::new();
        };
        let mut commands = BTreeMap::new();
        super::command::add_view_commands(config, &mut commands, owner);
        let mut commands = commands.into_iter().collect::<Vec<_>>();
        commands.sort_by(|left, right| super::command::compare_bindings(&left.0, &right.0));
        commands
    }

    pub(crate) fn render_state(&self) -> PickerRenderState {
        let frame = self.current();
        PickerRenderState {
            items: frame.items.clone(),
            selected: frame.selected,
            searching: frame.refresh_deadline.is_some() || frame.items_pending,
            completion: self.completion.clone(),
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
