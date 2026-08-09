use crate::router::RouteDisplay;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineChrome {
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) commands: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShellInput {
    pub(crate) raw: String,
    pub(crate) params: String,
    pub(crate) cursor: usize,
    pub(crate) changed: bool,
    pub(crate) rejected: bool,
}

impl ShellInput {
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            params: raw.clone(),
            raw,
            cursor,
            changed: false,
            rejected: false,
        }
    }

    pub(crate) fn with_params(raw: impl Into<String>, params: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            raw,
            params: params.into(),
            cursor,
            changed: false,
            rejected: false,
        }
    }

    pub(crate) fn with_cursor(raw: impl Into<String>, cursor: usize) -> Self {
        let mut input = Self::new(raw);
        input.set_cursor(cursor);
        input
    }

    pub(crate) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = previous_char_boundary(&self.raw, cursor);
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.cursor = self.cursor.min(self.raw.len());
        while !self.raw.is_char_boundary(self.cursor) {
            self.cursor = self.cursor.saturating_sub(1);
        }
        self.raw.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn move_left(&mut self) {
        self.cursor = self.cursor.min(self.raw.len());
        if self.cursor == 0 {
            return;
        }
        self.cursor -= self.raw[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
    }

    pub(crate) fn move_right(&mut self) {
        self.cursor = self.cursor.min(self.raw.len());
        if self.cursor >= self.raw.len() {
            self.cursor = self.raw.len();
            return;
        }
        self.cursor += self.raw[self.cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }

    pub(crate) fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.raw.len();
    }

    pub(crate) fn delete_backward(&mut self) -> bool {
        self.cursor = self.cursor.min(self.raw.len());
        if self.cursor == 0 {
            return false;
        }
        let start = self.cursor
            - self.raw[..self.cursor]
                .chars()
                .next_back()
                .map(char::len_utf8)
                .unwrap_or(1);
        self.raw.drain(start..self.cursor);
        self.cursor = start;
        true
    }

    pub(crate) fn delete_forward(&mut self) -> bool {
        self.cursor = self.cursor.min(self.raw.len());
        if self.cursor >= self.raw.len() {
            return false;
        }
        let end = self.cursor
            + self.raw[self.cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        self.raw.drain(self.cursor..end);
        true
    }

    pub(crate) fn delete_word(&mut self) -> bool {
        self.cursor = self.cursor.min(self.raw.len());
        let previous = self.cursor;
        while self.cursor > 0
            && self.raw[..self.cursor]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            self.move_left();
        }
        while self.cursor > 0
            && !self.raw[..self.cursor]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace)
        {
            self.move_left();
        }
        if self.cursor == previous {
            return false;
        }
        self.raw.drain(self.cursor..previous);
        true
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.raw.is_empty() {
            return false;
        }
        self.raw.clear();
        self.cursor = 0;
        true
    }

    pub(crate) fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        self.raw.replace_range(start..end, replacement);
        self.cursor = start + replacement.len();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeFrame {
    pub(crate) divider: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) footer: String,
}

impl ChromeFrame {
    pub(crate) fn input_line(&self) -> String {
        format!("  {}", self.input)
    }

    pub(crate) fn input_line_for_width(&self, width: usize) -> (String, usize) {
        let prefix = "  ";
        let prefix_width = UnicodeWidthStr::width(prefix);
        let available = width.saturating_sub(prefix_width);
        if available == 0 {
            return (clip(prefix, width), width.max(1));
        }
        let text_available = available.saturating_sub(1);

        let cursor = previous_char_boundary(&self.input, self.input_cursor);
        let before = &self.input[..cursor];
        let input_width = UnicodeWidthStr::width(self.input.as_str());
        let cursor_width = UnicodeWidthStr::width(before);
        if input_width <= text_available {
            return (
                format!("{}{}", prefix, self.input),
                prefix_width + cursor_width + 1,
            );
        }

        let needs_left_clip = cursor_width > text_available;
        let marker = if needs_left_clip && text_available >= 4 {
            "..."
        } else {
            ""
        };
        let budget = text_available.saturating_sub(UnicodeWidthStr::width(marker));
        let start_width = if needs_left_clip {
            cursor_width.saturating_sub(budget.saturating_sub(1))
        } else {
            0
        };
        let start = byte_at_width(&self.input, start_width);
        let visible = clip_from(&self.input[start..], budget);
        let text = format!("{}{}{}", prefix, marker, visible);
        let local_cursor = UnicodeWidthStr::width(&self.input[start..cursor]);
        (
            text,
            prefix_width + UnicodeWidthStr::width(marker) + local_cursor + 1,
        )
    }

    #[cfg(test)]
    pub(crate) fn compose(
        width: usize,
        route: &RouteDisplay,
        input: &str,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        Self::compose_with_cursor(width, route, input, input.len(), engine, error)
    }

    pub(crate) fn compose_with_cursor(
        width: usize,
        route: &RouteDisplay,
        input: &str,
        input_cursor: usize,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        let width = width.saturating_sub(1);
        let footer = if let Some(error) = error {
            clip(error, width)
        } else {
            footer_line(
                width,
                engine.title.as_deref(),
                engine.status.as_deref().unwrap_or(""),
                &engine.commands,
            )
        };
        Self {
            divider: divider_line(width, &route.label()),
            input: input.to_string(),
            input_cursor,
            footer,
        }
    }
}

pub(crate) fn divider_line(width: usize, label: &str) -> String {
    if width == 0 {
        return String::new();
    }
    if label.is_empty() {
        return "─".repeat(width);
    }

    let label = clip(label, width);
    let label_width = UnicodeWidthStr::width(label.as_str());
    if label_width >= width {
        return label;
    }
    format!("{} {}", label, "─".repeat(width - label_width - 1))
}

fn footer_line(
    width: usize,
    title: Option<&str>,
    status: &str,
    commands: &[(String, String)],
) -> String {
    if width == 0 {
        return String::new();
    }
    let left = footer_label(title, status);
    let right = command_text(commands);
    if right.is_empty() {
        return clip(&left, width);
    }
    if left.is_empty() {
        return right_aligned(width, &right);
    }

    let separator = " | ";
    let separator_width = UnicodeWidthStr::width(separator);
    let full_width = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.as_str());
    if full_width <= width {
        let padding = width - full_width;
        return format!("{}{}{}{}", left, " ".repeat(padding), separator, right);
    }
    if separator_width >= width {
        return clip(&right, width);
    }

    let right_budget = (width * 3 / 5).max(1);
    let right = clip(&right, right_budget);
    let left_budget =
        width.saturating_sub(UnicodeWidthStr::width(right.as_str()) + separator_width);
    if left_budget == 0 {
        return clip(&right, width);
    }
    let left = clip(&left, left_budget);
    let used = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.as_str());
    let padding = width.saturating_sub(used);
    format!("{}{}{}{}", left, " ".repeat(padding), separator, right)
}

fn footer_label(title: Option<&str>, status: &str) -> String {
    let mut parts = Vec::new();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        parts.push(title.to_string());
    }
    if !status.is_empty() {
        parts.push(status.to_string());
    }
    parts.join(" | ")
}

fn right_aligned(width: usize, text: &str) -> String {
    let text = clip(text, width);
    let padding = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
    format!("{}{}", " ".repeat(padding), text)
}

fn command_text(commands: &[(String, String)]) -> String {
    commands
        .iter()
        .map(|(key, label)| format!("{} {}", display_binding(key), label))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn display_binding(key: &str) -> String {
    match key {
        "enter" => return "Enter".to_string(),
        "tab" => return "Tab".to_string(),
        "shift+tab" => return "Shift-Tab".to_string(),
        "escape" => return "Esc".to_string(),
        _ => {}
    }
    key.strip_prefix("alt+")
        .map(|character| format!("Alt-{}", character.to_ascii_uppercase()))
        .unwrap_or_else(|| key.to_string())
}

pub(crate) fn clip(text: &str, width: usize) -> String {
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

fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn byte_at_width(text: &str, target: usize) -> usize {
    let mut used = 0;
    for (index, character) in text.char_indices() {
        if used >= target {
            return index;
        }
        used += UnicodeWidthChar::width(character).unwrap_or(0);
    }
    text.len()
}

fn clip_from(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RouteDisplay {
        RouteDisplay {
            view_ref: "apps:default".to_string(),
            alias: Some("app".to_string()),
        }
    }

    #[test]
    fn composes_route_engine_status_and_commands() {
        let frame = ChromeFrame::compose(
            80,
            &route(),
            "terminal",
            EngineChrome {
                title: None,
                status: Some("12 results".to_string()),
                commands: vec![("enter".to_string(), "Open".to_string())],
            },
            None,
        );
        assert!(frame.divider.starts_with("apps:default (app) "));
        assert!(frame.divider.contains('─'));
        assert_eq!(UnicodeWidthStr::width(frame.divider.as_str()), 79);
        assert_eq!(frame.input, "terminal");
        assert_eq!(frame.input_line(), "  terminal");
        assert!(frame.footer.starts_with("12 results"));
        assert!(frame.footer.ends_with(" | Enter Open"));
        assert_eq!(UnicodeWidthStr::width(frame.footer.as_str()), 79);
    }

    #[test]
    fn divider_fills_width_without_a_label() {
        assert_eq!(divider_line(5, ""), "─────");
    }

    #[test]
    fn shell_input_edits_at_the_cursor() {
        let mut input = ShellInput::new("ac");
        input.move_left();
        input.insert('b');
        assert_eq!(input.raw, "abc");
        assert_eq!(input.cursor, 2);
        input.delete_backward();
        assert_eq!(input.raw, "ac");
        input.move_home();
        input.delete_forward();
        assert_eq!(input.raw, "c");
    }

    #[test]
    fn input_line_keeps_the_cursor_visible_when_clipped() {
        let mut frame =
            ChromeFrame::compose(12, &route(), "abcdefghij", EngineChrome::default(), None);
        frame.input_cursor = frame.input.len();
        let (line, cursor_column) = frame.input_line_for_width(12);
        assert!(UnicodeWidthStr::width(line.as_str()) <= 12);
        assert!(cursor_column <= 12);
        assert!(line.contains("..."));
    }

    #[test]
    fn error_replaces_the_complete_footer() {
        let frame = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome::default(),
            Some("view alias is ambiguous"),
        );
        assert_eq!(frame.footer, "view alias is ambiguous");
    }
}
