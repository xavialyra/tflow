use super::LauncherDriver;
use crate::config::Config;
use crate::discovery::Item;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone)]
pub(crate) struct LauncherRenderState {
    pub(crate) view: String,
    pub(crate) input: String,
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) searching: bool,
    pub(crate) commands: Vec<(String, String)>,
}

pub(crate) fn render_launcher(
    terminal: &Terminal,
    state: &LauncherRenderState,
    footer_left: &str,
) -> Result<()> {
    let (width, height) = terminal.size();
    let width = width as usize;
    let height = height as usize;
    let footer = footer_line(width, footer_left, &state.commands);
    let list_height = height.saturating_sub(3);
    let mut lines = Vec::with_capacity(height);
    let prefix_width = prefix_column_width(&state.items, width);
    let content_width = width.saturating_sub(prefix_width + 4);

    lines.push(format!(" TUI Launcher  [{}]", state.view));
    lines.push(format!(" > {}", state.input));

    let start = if state.selected >= list_height && list_height > 0 {
        state.selected + 1 - list_height
    } else {
        0
    };
    let selected_row = if state.items.is_empty() || list_height == 0 {
        None
    } else {
        Some(2 + state.selected.saturating_sub(start))
    };

    if list_height > 0 {
        if state.items.is_empty() {
            lines.push(if state.searching {
                "   (searching...)".to_string()
            } else {
                "   (no matches)".to_string()
            });
        } else {
            for (offset, item) in state.items.iter().skip(start).take(list_height).enumerate() {
                let index = start + offset;
                let marker = if index == state.selected { "> " } else { "  " };
                lines.push(format_item_line(
                    marker,
                    &item.prefix,
                    &item.text,
                    prefix_width,
                    content_width,
                ));
            }
        }
    }

    while lines.len() < 2 + list_height {
        lines.push(String::new());
    }
    lines.push(footer);

    let mut stdout = io::stdout().lock();
    stdout.write_all(b"\x1b[H")?;
    for row in 0..height {
        let line = lines.get(row).map(String::as_str).unwrap_or("");
        let clipped = clip(line, width);
        if selected_row == Some(row) && clipped.starts_with("> ") {
            write!(stdout, "\x1b[7m{}\x1b[0m", clipped)?;
        } else if row == 0 {
            write!(stdout, "\x1b[1;36m{}\x1b[0m", clipped)?;
        } else {
            stdout.write_all(clipped.as_bytes())?;
        }
        stdout.write_all(b"\x1b[K")?;
        if row + 1 < height {
            stdout.write_all(b"\r\n")?;
        }
    }
    stdout.flush().context("could not draw launcher")
}

pub(crate) fn footer_line(width: usize, left: &str, commands: &[(String, String)]) -> String {
    // Leave the terminal's last column unused so a full row cannot trigger autowrap.
    let width = width.saturating_sub(1);
    let left_width = UnicodeWidthStr::width(left);
    let right_budget = if left_width + 2 < width {
        width - left_width - 2
    } else {
        (width * 3 / 5).max(1).min(width.saturating_sub(1))
    };
    let right = command_footer_text(commands, right_budget);
    footer_row(left, &right, width)
}

fn display_binding(key: &str) -> String {
    if key == "enter" {
        return "Enter".to_string();
    }
    key.strip_prefix("alt+")
        .map(|character| format!("Alt-{}", character.to_ascii_uppercase()))
        .unwrap_or_else(|| key.to_string())
}

fn command_footer_text(commands: &[(String, String)], width: usize) -> String {
    if commands.is_empty() || width == 0 {
        return String::new();
    }
    let formatted = commands
        .iter()
        .map(|(key, label)| format!("{} {}", display_binding(key), label))
        .collect::<Vec<_>>();
    let full = formatted.join(" | ");
    if UnicodeWidthStr::width(full.as_str()) <= width {
        return full;
    }

    let more = "Ctrl-K commands";
    let more_width = UnicodeWidthStr::width(more);
    let mut visible = Vec::new();
    let mut used = 0;
    for command in formatted {
        let command_width = UnicodeWidthStr::width(command.as_str());
        let separator = if visible.is_empty() { 0 } else { 3 };
        let required = used + separator + command_width + 3 + more_width;
        if required > width {
            break;
        }
        used += separator + command_width;
        visible.push(command);
    }
    visible.push(more.to_string());
    clip(visible.join(" | ").as_str(), width)
}

fn footer_row(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if right.is_empty() {
        return clip(left, width);
    }

    let gap = 2;
    if UnicodeWidthStr::width(left) + gap + UnicodeWidthStr::width(right) <= width {
        let padding = width - UnicodeWidthStr::width(left) - gap - UnicodeWidthStr::width(right);
        return format!("{}{}{}", left, " ".repeat(padding + gap), right);
    }

    let right_budget = (width * 3 / 5).max(1).min(width.saturating_sub(1));
    let right = clip(right, right_budget);
    let left_budget = width.saturating_sub(UnicodeWidthStr::width(right.as_str()) + 1);
    let left = clip(left, left_budget);
    let padding = width.saturating_sub(
        UnicodeWidthStr::width(left.as_str()) + UnicodeWidthStr::width(right.as_str()),
    );
    format!("{}{}{}", left, " ".repeat(padding), right)
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
    marker: &str,
    prefix: &str,
    content: &str,
    prefix_width: usize,
    content_width: usize,
) -> String {
    let prefix = pad_right(&clip(prefix, prefix_width), prefix_width);
    format!("{}{}  {}", marker, prefix, clip(content, content_width))
}

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{}", text, " ".repeat(width.saturating_sub(used)))
}

impl LauncherDriver {
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

    pub(crate) fn render_state(&self, config: &Config) -> LauncherRenderState {
        let frame = self.current();
        LauncherRenderState {
            view: frame.view.clone(),
            input: frame.input.clone(),
            items: frame.items.clone(),
            selected: frame.selected,
            searching: frame.refresh_deadline.is_some() || frame.discovery_pending,
            commands: self.visible_commands(config),
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
