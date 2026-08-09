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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Insets {
    pub(crate) top: usize,
    pub(crate) right: usize,
    pub(crate) bottom: usize,
    pub(crate) left: usize,
}

impl Insets {
    pub(crate) const ZERO: Self = Self {
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
    };

    pub(crate) const fn new(top: usize, right: usize, bottom: usize, left: usize) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    pub(crate) fn horizontal(self) -> usize {
        self.left.saturating_add(self.right)
    }

    pub(crate) fn vertical(self) -> usize {
        self.top.saturating_add(self.bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputLayout {
    pub(crate) rows: usize,
    pub(crate) divider_rows: usize,
    pub(crate) padding: Insets,
    pub(crate) divider_padding: Insets,
}

impl Default for InputLayout {
    fn default() -> Self {
        Self {
            rows: 1,
            divider_rows: 1,
            padding: Insets::new(0, 0, 0, 2),
            divider_padding: Insets::ZERO,
        }
    }
}

impl InputLayout {
    fn input_region_rows(self) -> usize {
        self.rows.saturating_add(self.padding.vertical())
    }

    fn divider_region_rows(self) -> usize {
        self.divider_rows
            .saturating_add(self.divider_padding.vertical())
    }

    fn total_rows(self) -> usize {
        self.input_region_rows()
            .saturating_add(self.divider_region_rows())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChromeLayout {
    pub(crate) input: InputLayout,
    pub(crate) footer_rows: usize,
    pub(crate) viewport_padding: Insets,
    pub(crate) content_padding: Insets,
    pub(crate) footer_padding: Insets,
}

impl Default for ChromeLayout {
    fn default() -> Self {
        Self {
            input: InputLayout::default(),
            footer_rows: 1,
            viewport_padding: Insets::new(1, 1, 0, 1),
            content_padding: Insets::new(0, 0, 0, 2),
            footer_padding: Insets::ZERO,
        }
    }
}

impl ChromeLayout {
    pub(crate) fn dmenu() -> Self {
        Self {
            input: InputLayout {
                padding: Insets::new(0, 0, 0, 1),
                ..InputLayout::default()
            },
            content_padding: Insets::new(0, 0, 0, 1),
            footer_padding: Insets::new(0, 0, 0, 1),
            ..Self::default()
        }
    }

    fn region_rows(rows: usize, padding: Insets) -> usize {
        rows.saturating_add(padding.vertical())
    }

    pub(crate) fn input_row(self) -> usize {
        self.viewport_padding.top
    }

    pub(crate) fn input_content_row(self) -> usize {
        self.input_row().saturating_add(self.input.padding.top)
    }

    pub(crate) fn divider_row(self) -> usize {
        self.input_row()
            .saturating_add(self.input.input_region_rows())
    }

    pub(crate) fn divider_content_row(self) -> usize {
        self.divider_row()
            .saturating_add(self.input.divider_padding.top)
    }

    pub(crate) fn content_start_row(self) -> usize {
        self.input_row()
            .saturating_add(self.input.total_rows())
            .saturating_add(self.content_padding.top)
    }

    pub(crate) fn footer_row(self, height: usize) -> usize {
        height
            .saturating_sub(
                self.viewport_padding
                    .bottom
                    .saturating_add(Self::region_rows(self.footer_rows, self.footer_padding)),
            )
            .saturating_add(self.footer_padding.top)
    }

    pub(crate) fn content_rows(self, height: usize) -> usize {
        let content_end = self
            .footer_row(height)
            .saturating_sub(self.footer_padding.top);
        content_end.saturating_sub(
            self.content_start_row()
                .saturating_add(self.content_padding.bottom),
        )
    }

    pub(crate) fn viewport_width(self, width: usize) -> usize {
        width.saturating_sub(self.viewport_padding.horizontal())
    }

    pub(crate) fn content_width(self, width: usize) -> usize {
        self.viewport_width(width)
            .saturating_sub(self.content_padding.horizontal())
    }

    pub(crate) fn chrome_width(self, width: usize) -> usize {
        self.viewport_width(width)
    }

    pub(crate) fn pad_line(self, text: &str, width: usize, padding: Insets) -> String {
        let left = padding.left.min(width);
        let right = padding.right.min(width.saturating_sub(left));
        let available = width.saturating_sub(left).saturating_sub(right);
        let text = clip(text, available);
        format!("{}{}{}", " ".repeat(left), text, " ".repeat(right),)
    }

    pub(crate) fn selection_parts(self, text: &str, width: usize) -> (String, String, String) {
        let clipped = clip(text, width);
        let left = self.viewport_padding.left.min(width).min(clipped.len());
        let mut right = self
            .viewport_padding
            .right
            .min(width.saturating_sub(self.viewport_padding.left));
        let mut body_end = clipped.len();
        while right > 0 && body_end > left && clipped.as_bytes().get(body_end - 1) == Some(&b' ') {
            body_end -= 1;
            right -= 1;
        }
        (
            clipped[..left].to_string(),
            clipped[left..body_end].to_string(),
            clipped[body_end..].to_string(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeFrame {
    pub(crate) divider: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    pub(crate) footer: String,
    layout: ChromeLayout,
}

impl ChromeFrame {
    pub(crate) fn layout(&self) -> ChromeLayout {
        self.layout
    }

    pub(crate) fn input_line(&self) -> String {
        format!(
            "{}{}",
            " ".repeat(self.layout.input.padding.left),
            self.input
        )
    }

    pub(crate) fn input_line_for_width(&self, width: usize) -> (String, usize) {
        let prefix = " ".repeat(self.layout.input.padding.left);
        let prefix_width = UnicodeWidthStr::width(prefix.as_str());
        let text_available = width.saturating_sub(self.layout.input.padding.horizontal());
        if text_available == 0 {
            return (clip(&prefix, width), width.max(1));
        }

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
        Self::compose_with_cursor(width, route, true, input, input.len(), engine, error)
    }

    pub(crate) fn compose_with_cursor(
        width: usize,
        route: &RouteDisplay,
        show_route_label: bool,
        input: &str,
        input_cursor: usize,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        let layout = ChromeLayout::default();
        let width = layout.chrome_width(width);
        let footer_width = width.saturating_sub(layout.footer_padding.horizontal());
        let footer_text = if let Some(error) = error {
            clip(error, footer_width)
        } else {
            footer_line(
                footer_width,
                engine.title.as_deref(),
                engine.status.as_deref().unwrap_or(""),
                &engine.commands,
            )
        };
        let route_label = show_route_label.then(|| route.label());
        let divider_width = width.saturating_sub(layout.input.divider_padding.horizontal());
        Self {
            divider: layout.pad_line(
                &divider_line(divider_width, route_label.as_deref().unwrap_or("")),
                width,
                layout.input.divider_padding,
            ),
            input: input.to_string(),
            input_cursor,
            footer: layout.pad_line(&footer_text, width, layout.footer_padding),
            layout,
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
        assert_eq!(UnicodeWidthStr::width(frame.divider.as_str()), 78);
        assert_eq!(frame.input, "terminal");
        assert_eq!(frame.input_line(), "  terminal");
        assert!(frame.footer.starts_with("12 results"));
        assert!(frame.footer.ends_with(" | Enter Open"));
        assert_eq!(UnicodeWidthStr::width(frame.footer.as_str()), 78);
    }

    #[test]
    fn default_layout_keeps_the_current_chrome_geometry() {
        let layout = ChromeLayout::default();

        assert_eq!(layout.input_content_row(), 1);
        assert_eq!(layout.divider_content_row(), 2);
        assert_eq!(layout.content_start_row(), 3);
        assert_eq!(layout.content_rows(24), 20);
        assert_eq!(layout.footer_row(24), 23);
        assert_eq!(layout.viewport_width(80), 78);
        assert_eq!(layout.content_width(80), 76);
        assert_eq!(layout.chrome_width(80), 78);
    }

    #[test]
    fn layout_insets_reserve_rows_and_columns() {
        let layout = ChromeLayout {
            input: InputLayout {
                rows: 2,
                divider_rows: 1,
                padding: Insets::new(1, 5, 2, 6),
                divider_padding: Insets::new(0, 1, 1, 2),
            },
            footer_rows: 2,
            viewport_padding: Insets::new(1, 2, 3, 4),
            content_padding: Insets::new(2, 3, 4, 5),
            footer_padding: Insets::new(1, 2, 1, 3),
        };

        assert_eq!(layout.input_content_row(), 2);
        assert_eq!(layout.divider_content_row(), 6);
        assert_eq!(layout.content_start_row(), 10);
        assert_eq!(layout.content_rows(40), 19);
        assert_eq!(layout.footer_row(40), 34);
        assert_eq!(layout.viewport_width(80), 74);
        assert_eq!(layout.content_width(80), 66);
        assert_eq!(layout.chrome_width(80), 74);
    }

    #[test]
    fn selection_excludes_viewport_padding() {
        let layout = ChromeLayout::default();
        let line = layout.pad_line("item", 10, layout.viewport_padding);

        assert_eq!(
            layout.selection_parts(&line, 10),
            (" ".into(), "item".into(), " ".into())
        );
    }

    #[test]
    fn divider_fills_width_without_a_label() {
        assert_eq!(divider_line(5, ""), "─────");
    }

    #[test]
    fn compose_hides_route_label_when_requested() {
        let frame = ChromeFrame::compose_with_cursor(
            80,
            &route(),
            false,
            "",
            0,
            EngineChrome::default(),
            None,
        );
        assert_eq!(frame.divider, "─".repeat(78));
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
        let viewport_width = ChromeLayout::default().viewport_width(12);
        let (line, cursor_column) = frame.input_line_for_width(viewport_width);
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
