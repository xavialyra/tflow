use crate::router::RouteDisplay;
use crate::terminal::Terminal;
use anyhow::{Context, Result};
use std::env;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineChrome {
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) commands: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FooterContent {
    pub(crate) text: String,
    pub(crate) key_spans: Vec<(usize, usize)>,
}

impl FooterContent {
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            key_spans: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ChromeCursor {
    #[default]
    Hidden,
    Input,
    Content {
        row: usize,
        column: usize,
        visible: bool,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChromeContent {
    pub(crate) lines: Vec<String>,
    pub(crate) selected_row: Option<usize>,
    pub(crate) cursor: ChromeCursor,
}

impl ChromeContent {
    pub(crate) fn new(
        lines: Vec<String>,
        selected_row: Option<usize>,
        cursor: ChromeCursor,
    ) -> Self {
        Self {
            lines,
            selected_row,
            cursor,
        }
    }
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
            padding: Insets::ZERO,
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
    pub(crate) topbar_rows: usize,
    pub(crate) topbar_padding: Insets,
    pub(crate) input: InputLayout,
    pub(crate) footer_divider_rows: usize,
    pub(crate) footer_divider_padding: Insets,
    pub(crate) footer_rows: usize,
    pub(crate) viewport_padding: Insets,
    pub(crate) content_padding: Insets,
    pub(crate) footer_padding: Insets,
}

impl Default for ChromeLayout {
    fn default() -> Self {
        Self {
            topbar_rows: 1,
            topbar_padding: Insets::ZERO,
            input: InputLayout::default(),
            footer_divider_rows: 1,
            footer_divider_padding: Insets::ZERO,
            footer_rows: 1,
            viewport_padding: Insets::new(0, 1, 0, 1),
            content_padding: Insets::ZERO,
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

    pub(crate) fn topbar_row(self) -> usize {
        self.viewport_padding.top
    }

    pub(crate) fn topbar_content_row(self) -> usize {
        self.topbar_row().saturating_add(self.topbar_padding.top)
    }

    pub(crate) fn input_row(self) -> usize {
        self.topbar_row()
            .saturating_add(Self::region_rows(self.topbar_rows, self.topbar_padding))
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

    fn footer_region_row(self, height: usize) -> usize {
        height.saturating_sub(
            self.viewport_padding
                .bottom
                .saturating_add(Self::region_rows(self.footer_rows, self.footer_padding)),
        )
    }

    pub(crate) fn footer_divider_row(self, height: usize) -> usize {
        self.footer_region_row(height)
            .saturating_sub(Self::region_rows(
                self.footer_divider_rows,
                self.footer_divider_padding,
            ))
    }

    pub(crate) fn footer_divider_content_row(self, height: usize) -> usize {
        self.footer_divider_row(height)
            .saturating_add(self.footer_divider_padding.top)
    }

    pub(crate) fn footer_row(self, height: usize) -> usize {
        self.footer_region_row(height)
            .saturating_add(self.footer_padding.top)
    }

    pub(crate) fn content_rows(self, height: usize) -> usize {
        let content_end = self.footer_divider_row(height);
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

    fn topbar_content_line(self, text: &str, width: usize) -> String {
        self.pad_line(text, width, self.topbar_padding)
    }

    pub(crate) fn topbar_line(self, text: &str, width: usize) -> String {
        let left = self.viewport_padding.left.min(width);
        let right = self.viewport_padding.right.min(width.saturating_sub(left));
        let inner =
            self.topbar_content_line(text, width.saturating_sub(left).saturating_sub(right));
        format!("{}{}{}", " ".repeat(left), inner, " ".repeat(right))
    }

    pub(crate) fn system_topbar_line(self, width: usize) -> String {
        let viewport_width = self.viewport_width(width);
        self.topbar_line(
            &topbar_text(viewport_width.saturating_sub(self.topbar_padding.horizontal())),
            width,
        )
    }

    fn footer_content_line(self, text: &str, key_spans: &[(usize, usize)], width: usize) -> String {
        let left = self.footer_padding.left.min(width);
        let right = self.footer_padding.right.min(width.saturating_sub(left));
        let available = width.saturating_sub(left).saturating_sub(right);
        let text = clip(text, available);
        let text = style_footer(&text, key_spans);
        format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
    }

    pub(crate) fn footer_line(
        self,
        text: &str,
        key_spans: &[(usize, usize)],
        width: usize,
    ) -> String {
        let left = self.viewport_padding.left.min(width);
        let right = self.viewport_padding.right.min(width.saturating_sub(left));
        let inner = self.footer_content_line(
            text,
            key_spans,
            width.saturating_sub(left).saturating_sub(right),
        );
        format!("{}{}{}", " ".repeat(left), inner, " ".repeat(right))
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
    input_prefix: String,
    pub(crate) footer: String,
    pub(crate) footer_divider: String,
    footer_keys: Vec<(usize, usize)>,
    layout: ChromeLayout,
}

impl ChromeFrame {
    pub(crate) fn layout(&self) -> ChromeLayout {
        self.layout
    }

    pub(crate) fn footer_line(&self, width: usize) -> String {
        self.layout
            .footer_line(&self.footer, &self.footer_keys, width)
    }

    #[cfg(test)]
    pub(crate) fn input_line(&self) -> String {
        format!("{}{}", self.input_prefix, self.input)
    }

    pub(crate) fn input_line_for_width(&self, width: usize) -> (String, usize) {
        let prefix = self.input_prefix.as_str();
        let prefix_width = UnicodeWidthStr::width(prefix);
        let text_available = width
            .saturating_sub(prefix_width)
            .saturating_sub(self.layout.input.padding.right);
        if text_available == 0 {
            return (clip(prefix, width), width.max(1));
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
        let footer_width = layout
            .chrome_width(width)
            .saturating_sub(layout.footer_padding.horizontal());
        let footer = if let Some(error) = error {
            FooterContent::plain(error)
        } else {
            footer_line(
                footer_width,
                engine.title.as_deref(),
                engine.status.as_deref().unwrap_or(""),
                &engine.commands,
            )
        };
        let route_label = show_route_label.then(|| route.label());
        Self::compose_with_layout(
            width,
            layout,
            " ".repeat(layout.input.padding.left),
            input,
            input_cursor,
            route_label.as_deref().unwrap_or(""),
            footer,
        )
    }

    pub(crate) fn compose_with_layout(
        width: usize,
        layout: ChromeLayout,
        input_prefix: String,
        input: &str,
        input_cursor: usize,
        divider_label: &str,
        footer: FooterContent,
    ) -> Self {
        let width = layout.chrome_width(width);
        let footer_width = width.saturating_sub(layout.footer_padding.horizontal());
        let footer = clip_footer(&footer, footer_width);
        let divider_width = width.saturating_sub(layout.input.divider_padding.horizontal());
        let footer_divider_width = width.saturating_sub(layout.footer_divider_padding.horizontal());
        Self {
            divider: layout.pad_line(
                &divider_line(divider_width, divider_label),
                width,
                layout.input.divider_padding,
            ),
            input: input.to_string(),
            input_cursor,
            input_prefix,
            footer: footer.text,
            footer_divider: layout.pad_line(
                &divider_line(footer_divider_width, ""),
                width,
                layout.footer_divider_padding,
            ),
            footer_keys: footer.key_spans,
            layout,
        }
    }

    pub(crate) fn render(&self, terminal: &Terminal, content: ChromeContent) -> Result<()> {
        let (width, height) = terminal.size();
        let width = width as usize;
        let height = height as usize;
        let layout = self.layout;
        let viewport_width = layout.viewport_width(width);
        let content_start = layout.content_start_row();
        let content_rows = layout.content_rows(height);
        let footer_row = layout.footer_row(height);
        let footer_divider_row = layout.footer_divider_content_row(height);
        let mut output = String::new();

        output.push_str("\x1b[?25l\x1b[H");
        for row in 0..height {
            if row == footer_row {
                output.push_str(&self.footer_line(width));
            } else if row == layout.topbar_content_row() {
                output.push_str("\x1b[2m");
                output.push_str(&clip(&layout.system_topbar_line(width), width));
                output.push_str("\x1b[0m");
            } else if row == layout.input_content_row() {
                let (input, _) = self.input_line_for_width(viewport_width);
                output.push_str(&layout.pad_line(&input, width, layout.viewport_padding));
            } else if row == layout.divider_content_row() {
                output.push_str("\x1b[1;36m");
                output.push_str(&layout.pad_line(&self.divider, width, layout.viewport_padding));
                output.push_str("\x1b[0m");
            } else if row == footer_divider_row {
                output.push_str("\x1b[1;36m");
                output.push_str(&layout.pad_line(
                    &self.footer_divider,
                    width,
                    layout.viewport_padding,
                ));
                output.push_str("\x1b[0m");
            } else if row >= content_start && row < content_start.saturating_add(content_rows) {
                let content_row = row.saturating_sub(content_start);
                let text = content
                    .lines
                    .get(content_row)
                    .map(String::as_str)
                    .unwrap_or("");
                let text = layout.pad_line(text, viewport_width, layout.content_padding);
                let line = layout.pad_line(&text, width, layout.viewport_padding);
                let line = clip(&line, width);
                if content.selected_row == Some(content_row) {
                    let (left, selected, right) = layout.selection_parts(&line, width);
                    output.push_str(&left);
                    output.push_str("\x1b[7m");
                    output.push_str(&selected);
                    output.push_str("\x1b[0m");
                    output.push_str(&right);
                } else {
                    output.push_str(&line);
                }
            }
            output.push_str("\x1b[K");
            if row + 1 < height {
                output.push_str("\r\n");
            }
        }

        match content.cursor {
            ChromeCursor::Hidden => output.push_str("\x1b[?25l"),
            ChromeCursor::Input => {
                let (_, cursor_column) = self.input_line_for_width(viewport_width);
                let row = layout.input_content_row();
                if row < height {
                    let max_column = width.saturating_sub(layout.viewport_padding.right).max(1);
                    let column = layout
                        .viewport_padding
                        .left
                        .saturating_add(cursor_column)
                        .max(1)
                        .min(max_column);
                    output.push_str(&format!("\x1b[{};{}H\x1b[?25h", row + 1, column));
                } else {
                    output.push_str("\x1b[?25l");
                }
            }
            ChromeCursor::Content {
                row,
                column,
                visible,
            } if visible && content_rows > 0 => {
                let row = content_start + row.min(content_rows.saturating_sub(1));
                if row < height {
                    let content_width = layout.content_width(width);
                    let column = layout
                        .viewport_padding
                        .left
                        .saturating_add(layout.content_padding.left)
                        .saturating_add(column.min(content_width.saturating_sub(1)))
                        .saturating_add(1);
                    let max_column = width.saturating_sub(layout.viewport_padding.right).max(1);
                    output.push_str(&format!(
                        "\x1b[{};{}H\x1b[?25h",
                        row + 1,
                        column.min(max_column)
                    ));
                } else {
                    output.push_str("\x1b[?25l");
                }
            }
            ChromeCursor::Content { .. } => output.push_str("\x1b[?25l"),
        }
        terminal
            .write_output(output.as_bytes())
            .context("could not draw chrome")
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
) -> FooterContent {
    if width == 0 {
        return FooterContent::default();
    }
    let left = footer_label(title, status);
    let right = command_footer(commands);
    if right.text.is_empty() {
        return FooterContent::plain(clip(&left, width));
    }
    if left.is_empty() {
        let right = clip_footer(&right, width);
        let padding = width.saturating_sub(UnicodeWidthStr::width(right.text.as_str()));
        let mut result = FooterContent::default();
        append_plain(&mut result, &" ".repeat(padding));
        append_content(&mut result, &right);
        return result;
    }

    let separator = "  ";
    let separator_width = UnicodeWidthStr::width(separator);
    let full_width = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.text.as_str());
    if full_width <= width {
        let padding = width - full_width;
        let mut result = FooterContent::default();
        append_plain(&mut result, &left);
        append_plain(&mut result, &" ".repeat(padding));
        append_plain(&mut result, separator);
        append_content(&mut result, &right);
        return result;
    }
    if separator_width >= width {
        return clip_footer(&right, width);
    }

    let right_budget = (width * 3 / 5).max(1);
    let right = clip_footer(&right, right_budget);
    let left_budget =
        width.saturating_sub(UnicodeWidthStr::width(right.text.as_str()) + separator_width);
    if left_budget == 0 {
        return clip_footer(&right, width);
    }
    let left = clip(&left, left_budget);
    let used = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.text.as_str());
    let padding = width.saturating_sub(used);
    let mut result = FooterContent::default();
    append_plain(&mut result, &left);
    append_plain(&mut result, &" ".repeat(padding));
    append_plain(&mut result, separator);
    append_content(&mut result, &right);
    result
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

pub(crate) fn command_footer(commands: &[(String, String)]) -> FooterContent {
    let mut result = FooterContent::default();
    for (index, (key, label)) in commands.iter().enumerate() {
        if index > 0 {
            append_plain(&mut result, " | ");
        }
        let key = display_binding(key);
        let start = result.text.len();
        append_plain(&mut result, &key);
        let end = result.text.len();
        result.key_spans.push((start, end));
        append_plain(&mut result, " ");
        append_plain(&mut result, label);
    }
    result
}

fn append_plain(target: &mut FooterContent, text: &str) {
    target.text.push_str(text);
}

fn append_content(target: &mut FooterContent, content: &FooterContent) {
    let offset = target.text.len();
    target.text.push_str(&content.text);
    target.key_spans.extend(
        content
            .key_spans
            .iter()
            .map(|(start, end)| (offset + start, offset + end)),
    );
}

fn clip_footer(content: &FooterContent, width: usize) -> FooterContent {
    let text = clip(&content.text, width);
    let visible_end = text.len();
    FooterContent {
        text,
        key_spans: content
            .key_spans
            .iter()
            .filter_map(|(start, end)| {
                (*start < visible_end).then_some((*start, (*end).min(visible_end)))
            })
            .collect(),
    }
}

fn style_footer(text: &str, key_spans: &[(usize, usize)]) -> String {
    const KEY_START: &str = "\x1b[48;2;220;224;230m\x1b[38;2;25;30;35m";
    const KEY_END: &str = "\x1b[0m";
    let mut output = String::new();
    let mut cursor = 0;
    for &(start, end) in key_spans {
        let start = start.max(cursor).min(text.len());
        let end = end.max(start).min(text.len());
        if start > cursor {
            output.push_str(&text[cursor..start]);
        }
        if end > start {
            output.push_str(KEY_START);
            output.push_str(&text[start..end]);
            output.push_str(KEY_END);
        }
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    output
}

fn topbar_text(width: usize) -> String {
    let left = format!(
        "{}@{}",
        env::var("USER").unwrap_or_else(|_| "user".to_string()),
        hostname(),
    );
    let right = local_time();
    if width == 0 {
        return String::new();
    }

    let right = clip(&right, width);
    let left_budget = width.saturating_sub(UnicodeWidthStr::width(right.as_str()) + 1);
    let left = clip(&left, left_budget);
    let gap = width.saturating_sub(
        UnicodeWidthStr::width(left.as_str()) + UnicodeWidthStr::width(right.as_str()),
    );
    format!("{}{}{}", left, " ".repeat(gap), right)
}

fn hostname() -> String {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME
        .get_or_init(|| {
            let mut buffer = [0_u8; 256];
            let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
            if result != 0 {
                return "localhost".to_string();
            }
            let length = buffer
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(buffer.len());
            String::from_utf8_lossy(&buffer[..length]).into_owned()
        })
        .clone()
}

fn local_time() -> String {
    let timestamp = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local = MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::localtime_r(&timestamp, local.as_mut_ptr()) }.is_null() {
        return "--:--:--".to_string();
    }
    let local = unsafe { local.assume_init() };
    format!(
        "{:02}:{:02}:{:02}",
        local.tm_hour, local.tm_min, local.tm_sec
    )
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
        assert_eq!(frame.input_line(), "terminal");
        assert!(frame.footer.starts_with("12 results"));
        assert!(frame.footer.ends_with("Enter Open"));
        assert!(!frame.footer.ends_with("| Enter Open"));
        assert_eq!(UnicodeWidthStr::width(frame.footer.as_str()), 78);
        assert_eq!(UnicodeWidthStr::width(frame.footer_divider.as_str()), 78);
        assert!(frame.layout.system_topbar_line(80).contains('@'));
        assert!(frame.layout.system_topbar_line(80).contains(':'));
        assert!(
            frame
                .footer_line(80)
                .contains("\x1b[48;2;220;224;230m\x1b[38;2;25;30;35mEnter\x1b[0m Open")
        );
    }

    #[test]
    fn default_layout_keeps_the_current_chrome_geometry() {
        let layout = ChromeLayout::default();

        assert_eq!(layout.topbar_content_row(), 0);
        assert_eq!(layout.input_content_row(), 1);
        assert_eq!(layout.divider_content_row(), 2);
        assert_eq!(layout.content_start_row(), 3);
        assert_eq!(layout.content_rows(24), 19);
        assert_eq!(layout.footer_divider_content_row(24), 22);
        assert_eq!(layout.footer_row(24), 23);
        assert_eq!(layout.viewport_width(80), 78);
        assert_eq!(layout.input.padding, Insets::ZERO);
        assert_eq!(layout.content_padding, Insets::ZERO);
        assert_eq!(layout.content_width(80), 78);
        assert_eq!(layout.chrome_width(80), 78);
    }

    #[test]
    fn layout_insets_reserve_rows_and_columns() {
        let layout = ChromeLayout {
            topbar_rows: 0,
            topbar_padding: Insets::ZERO,
            input: InputLayout {
                rows: 2,
                divider_rows: 1,
                padding: Insets::new(1, 5, 2, 6),
                divider_padding: Insets::new(0, 1, 1, 2),
            },
            footer_divider_rows: 0,
            footer_divider_padding: Insets::ZERO,
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
            ChromeFrame::compose(12, &route(), "abcdefghijkl", EngineChrome::default(), None);
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
