use crate::router::RouteDisplay;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineChrome {
    pub(crate) title: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) commands: Vec<(String, String)>,
    pub(crate) overflow_command: Option<(String, String)>,
    pub(crate) presentation: ChromePresentation,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChromePresentation {
    layout: ChromeLayout,
    input_muted: bool,
    recognized_input_prefix_end: Option<usize>,
    footer: Option<FooterContent>,
}

impl ChromePresentation {
    pub(crate) fn with_recognized_input_prefix(mut self, end: usize) -> Self {
        self.recognized_input_prefix_end = Some(end);
        self
    }

    pub(crate) fn with_unfocused_input(mut self) -> Self {
        self.input_muted = true;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InputBuffer {
    pub(crate) raw: String,
    pub(crate) params: String,
    pub(crate) cursor: usize,
    pub(crate) rejected: bool,
}

impl InputBuffer {
    #[cfg(test)]
    pub(crate) fn new(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let cursor = raw.len();
        Self {
            params: raw.clone(),
            raw,
            cursor,
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
            rejected: false,
        }
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
            footer_divider_rows: 0,
            footer_divider_padding: Insets::ZERO,
            footer_rows: 1,
            viewport_padding: Insets::new(0, 1, 0, 1),
            content_padding: Insets::ZERO,
            footer_padding: Insets::ZERO,
        }
    }
}

impl ChromeLayout {
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
}

struct ComposedInput {
    text: String,
    cursor: usize,
    prefix: String,
    prefix_highlight: Option<(usize, usize)>,
    muted: bool,
    recognized_prefix_end: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromeFrame {
    pub(crate) divider: String,
    pub(crate) input: String,
    pub(crate) input_cursor: usize,
    input_prefix: String,
    input_prefix_highlight: Option<(usize, usize)>,
    input_muted: bool,
    recognized_input_prefix_end: Option<usize>,
    pub(crate) footer: String,
    footer_is_error: bool,
    pub(crate) footer_divider: String,
    footer_keys: Vec<(usize, usize)>,
    layout: ChromeLayout,
}

impl ChromeFrame {
    #[cfg(test)]
    pub(crate) fn input_line(&self) -> String {
        format!("{}{}", self.input_prefix, self.input)
    }

    pub(crate) fn input_line_for_width(&self, width: usize) -> (String, usize) {
        let (line, cursor, _) = self.input_line_parts_for_width(width);
        (line, cursor)
    }

    fn input_line_parts_for_width(&self, width: usize) -> (String, usize, Option<(usize, usize)>) {
        let prefix = self.input_prefix.as_str();
        let prefix_width = UnicodeWidthStr::width(prefix);
        let text_available = width
            .saturating_sub(prefix_width)
            .saturating_sub(self.layout.input.padding.right);
        if text_available == 0 {
            let text = clip(prefix, width);
            return (
                text.clone(),
                width.max(1),
                self.static_input_prefix_range(text.len()),
            );
        }

        let cursor = previous_char_boundary(&self.input, self.input_cursor);
        let before = &self.input[..cursor];
        let input_width = UnicodeWidthStr::width(self.input.as_str());
        let cursor_width = UnicodeWidthStr::width(before);
        if input_width <= text_available {
            return (
                format!("{}{}", prefix, self.input),
                prefix_width + cursor_width + 1,
                self.highlighted_input_prefix_range(
                    prefix.len(),
                    0,
                    self.input.len(),
                    prefix.len() + self.input.len(),
                ),
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
        let output_end = text.len();
        let local_cursor = UnicodeWidthStr::width(&self.input[start..cursor]);
        (
            text,
            prefix_width + UnicodeWidthStr::width(marker) + local_cursor + 1,
            self.highlighted_input_prefix_range(
                prefix.len().saturating_add(marker.len()),
                start,
                start.saturating_add(visible.len()),
                output_end,
            ),
        )
    }

    fn highlighted_input_prefix_range(
        &self,
        output_start: usize,
        input_start: usize,
        input_end: usize,
        output_end: usize,
    ) -> Option<(usize, usize)> {
        self.static_input_prefix_range(output_end).or_else(|| {
            let end = self.recognized_input_prefix_end?;
            (input_start == 0 && end <= input_end && self.input.is_char_boundary(end))
                .then_some((output_start, output_start.saturating_add(end)))
        })
    }

    fn static_input_prefix_range(&self, output_end: usize) -> Option<(usize, usize)> {
        self.input_prefix_highlight.filter(|(start, end)| {
            *start < *end
                && *end <= output_end
                && self.input_prefix.is_char_boundary(*start)
                && self.input_prefix.is_char_boundary(*end)
        })
    }

    #[cfg(test)]
    pub(crate) fn compose(
        width: usize,
        route: &RouteDisplay,
        input: &str,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        Self::compose_with_cursor(width, Some(route), input, input.len(), engine, error)
    }

    pub(crate) fn compose_with_cursor(
        width: usize,
        route: Option<&RouteDisplay>,
        input: &str,
        input_cursor: usize,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        let EngineChrome {
            title,
            status,
            commands,
            overflow_command,
            presentation,
        } = engine;
        let ChromePresentation {
            layout,
            input_muted,
            recognized_input_prefix_end,
            footer,
        } = presentation;
        let footer_width = layout
            .chrome_width(width)
            .saturating_sub(layout.footer_padding.horizontal());
        let footer_is_error = error.is_some();
        let footer = if let Some(error) = error {
            FooterContent::plain(error)
        } else if let Some(footer) = footer {
            footer
        } else {
            footer_line(
                footer_width,
                title.as_deref(),
                status.as_deref().unwrap_or(""),
                &commands,
                overflow_command.as_ref(),
            )
        };
        let left_padding = " ".repeat(layout.input.padding.left);
        let (input_prefix, input_prefix_highlight) = match route {
            Some(route) => {
                let route_label = route.label();
                let start = left_padding.len();
                let end = start.saturating_add(route_label.len());
                (
                    format!("{}{} ", left_padding, route_label),
                    (start < end).then_some((start, end)),
                )
            }
            None => (left_padding, None),
        };
        Self::compose_with_layout(
            width,
            layout,
            ComposedInput {
                text: input.to_string(),
                cursor: input_cursor,
                prefix: input_prefix,
                prefix_highlight: input_prefix_highlight,
                muted: input_muted,
                recognized_prefix_end: recognized_input_prefix_end,
            },
            "",
            footer,
            footer_is_error,
        )
    }

    fn compose_with_layout(
        width: usize,
        layout: ChromeLayout,
        input: ComposedInput,
        divider_label: &str,
        footer: FooterContent,
        footer_is_error: bool,
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
            input: input.text,
            input_cursor: input.cursor,
            input_prefix: input.prefix,
            input_prefix_highlight: input.prefix_highlight,
            input_muted: input.muted,
            recognized_input_prefix_end: input.recognized_prefix_end,
            footer: footer.text,
            footer_is_error,
            footer_divider: layout.pad_line(
                &divider_line(footer_divider_width, ""),
                width,
                layout.footer_divider_padding,
            ),
            footer_keys: footer.key_spans,
            layout,
        }
    }

    pub(crate) fn render_chrome(&self, frame: &mut Frame, theme: &Theme) -> Rect {
        let content_area = self.content_area(frame.area());
        self.render_frame(frame, theme);
        content_area
    }

    pub(crate) fn content_area(&self, area: Rect) -> Rect {
        let layout = self.layout;
        let width = area.width as usize;
        let height = area.height as usize;
        Rect::new(
            area.x.saturating_add(as_u16(
                layout
                    .viewport_padding
                    .left
                    .saturating_add(layout.content_padding.left),
            )),
            area.y.saturating_add(as_u16(layout.content_start_row())),
            as_u16(layout.content_width(width)),
            as_u16(layout.content_rows(height)),
        )
    }

    fn render_frame(&self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        let width = area.width as usize;
        let height = area.height as usize;
        let layout = self.layout;
        let viewport_width = layout.viewport_width(width);
        let footer_row = layout.footer_row(height);
        let mut lines = Vec::with_capacity(height);

        for row in 0..height {
            let line = if row == footer_row {
                Line::from(self.footer_spans(width, theme))
            } else if row == layout.topbar_content_row() {
                Line::default()
            } else if row == layout.input_content_row() {
                let (input, _, highlighted_prefix) =
                    self.input_line_parts_for_width(viewport_width);
                let input = layout.pad_line(&input, width, layout.viewport_padding);
                let highlighted_prefix = highlighted_prefix.map(|(start, end)| {
                    let offset = layout.viewport_padding.left;
                    (start.saturating_add(offset), end.saturating_add(offset))
                });
                Line::from(input_spans(
                    input,
                    highlighted_prefix,
                    self.input_muted,
                    theme,
                ))
            } else if row == layout.divider_content_row() {
                Line::styled(
                    layout.pad_line(&self.divider, width, layout.viewport_padding),
                    theme.chrome.divider,
                )
            } else {
                Line::default()
            };
            lines.push(line);
        }

        frame.render_widget(Paragraph::new(Text::from(lines)).style(theme.text), area);
    }

    fn footer_spans(&self, width: usize, theme: &Theme) -> Vec<Span<'static>> {
        let layout = self.layout;
        let viewport_left = layout.viewport_padding.left.min(width);
        let footer_width = layout.chrome_width(width);
        let footer_left = layout.footer_padding.left.min(footer_width);
        let footer_style = if self.footer_is_error {
            theme.chrome.error
        } else {
            theme.chrome.footer
        };
        let key_style = theme.chrome.footer_key;
        let mut spans = vec![Span::raw(" ".repeat(viewport_left))];
        spans.push(Span::styled(" ".repeat(footer_left), footer_style));
        let mut cursor = 0;
        let text = clip(
            &self.footer,
            footer_width.saturating_sub(layout.footer_padding.horizontal()),
        );
        for &(start, end) in &self.footer_keys {
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
        let used = footer_left.saturating_add(UnicodeWidthStr::width(text.as_str()));
        spans.push(Span::styled(
            " ".repeat(footer_width.saturating_sub(used)),
            footer_style,
        ));
        let viewport_right = width
            .saturating_sub(viewport_left)
            .saturating_sub(footer_width);
        spans.push(Span::raw(" ".repeat(viewport_right)));
        spans
    }

    pub(crate) fn set_input_cursor(&self, frame: &mut Frame) {
        let area = frame.area();
        let layout = self.layout;
        let width = area.width as usize;
        let height = area.height as usize;
        let viewport_width = layout.viewport_width(width);
        let (_, cursor_column) = self.input_line_for_width(viewport_width);
        let row = layout.input_content_row();
        if row < height {
            let max_column = width.saturating_sub(layout.viewport_padding.right).max(1);
            let column = layout
                .viewport_padding
                .left
                .saturating_add(cursor_column.saturating_sub(1))
                .min(max_column.saturating_sub(1));
            frame.set_cursor_position((
                area.x.saturating_add(as_u16(column)),
                area.y.saturating_add(as_u16(row)),
            ));
        }
    }
}

fn input_spans(
    text: String,
    highlighted_prefix: Option<(usize, usize)>,
    input_muted: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let input_style = input_muted.then_some(theme.muted_text);
    let span = |text: String| match input_style {
        Some(style) => Span::styled(text, style),
        None => Span::raw(text),
    };
    let Some((start, end)) = highlighted_prefix.filter(|(start, end)| {
        *start < *end
            && *end <= text.len()
            && text.is_char_boundary(*start)
            && text.is_char_boundary(*end)
    }) else {
        return vec![span(text)];
    };
    let prefix_style = theme.chrome.input_prefix;
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(span(text[..start].to_string()));
    }
    spans.push(Span::styled(text[start..end].to_string(), prefix_style));
    if end < text.len() {
        spans.push(span(text[end..].to_string()));
    }
    spans
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
    overflow_command: Option<&(String, String)>,
) -> FooterContent {
    if width == 0 {
        return FooterContent::default();
    }
    let left = footer_label(title, status);
    let mut visible_commands = commands.to_vec();
    let complete = command_footer(commands);
    let separator_width = 2;
    let complete_separator = if !left.is_empty() && !complete.text.is_empty() {
        separator_width
    } else {
        0
    };
    let complete_width = UnicodeWidthStr::width(left.as_str())
        + complete_separator
        + UnicodeWidthStr::width(complete.text.as_str());
    if complete_width > width
        && let Some(overflow_command) = overflow_command
    {
        visible_commands.clear();
        visible_commands.push(overflow_command.clone());
        for command in commands {
            let mut candidate = visible_commands.clone();
            candidate.insert(candidate.len() - 1, command.clone());
            let right = command_footer(&candidate);
            let candidate_separator = if left.is_empty() { 0 } else { separator_width };
            let needed = UnicodeWidthStr::width(left.as_str())
                + candidate_separator
                + UnicodeWidthStr::width(right.text.as_str());
            if needed > width {
                break;
            }
            visible_commands = candidate;
        }
    }
    let right = command_footer(&visible_commands);
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

fn as_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
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
        .or_else(|| {
            key.strip_prefix("ctrl+")
                .map(|character| format!("Ctrl-{}", character.to_ascii_uppercase()))
        })
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
    use ratatui::style::{Color, Modifier};

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
                ..EngineChrome::default()
            },
            None,
        );
        assert_eq!(frame.divider, "─".repeat(78));
        assert_eq!(UnicodeWidthStr::width(frame.divider.as_str()), 78);
        assert_eq!(frame.input, "terminal");
        assert_eq!(frame.input_line(), "app terminal");
        assert!(frame.footer.starts_with("12 results"));
        assert!(frame.footer.ends_with("Enter Open"));
        assert!(!frame.footer.ends_with("| Enter Open"));
        assert_eq!(UnicodeWidthStr::width(frame.footer.as_str()), 78);
        assert_eq!(UnicodeWidthStr::width(frame.footer_divider.as_str()), 78);
        assert!(frame.footer.contains("Enter Open"));
    }

    #[test]
    fn router_alias_is_rendered_separately_from_query_input() {
        let frame = ChromeFrame::compose_with_cursor(
            80,
            Some(&route()),
            "query",
            5,
            EngineChrome::default(),
            None,
        );

        assert_eq!(frame.input_line(), "app query");
        assert_eq!(frame.input_prefix_highlight, Some((0, "app".len())));
        assert_eq!(frame.input, "query");
    }

    #[test]
    fn unfocused_input_uses_the_muted_theme_for_the_query() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;

        let mut theme = Theme::terminal();
        theme.muted_text.fg = Some(Color::Gray);

        let frame = ChromeFrame::compose_with_cursor(
            80,
            Some(&route()),
            "Show date",
            "Show date".len(),
            EngineChrome {
                presentation: ChromePresentation::default().with_unfocused_input(),
                ..EngineChrome::default()
            },
            None,
        );
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|draw| frame.render_frame(draw, &theme))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let input_start = ChromeLayout::default().viewport_padding.left;
        let separator = input_start + UnicodeWidthStr::width("app");
        let query_start = separator + 1;
        let tag = buffer.cell((input_start as u16, 1)).unwrap().style();
        let separator = buffer.cell((separator as u16, 1)).unwrap().style();
        let query = buffer.cell((query_start as u16, 1)).unwrap().style();

        assert_eq!(tag.fg, Some(Color::Cyan));
        assert_eq!(tag.bg, Some(Color::Reset));
        assert!(tag.add_modifier.contains(Modifier::BOLD));
        assert!(!tag.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(separator.fg, Some(Color::Gray));
        assert_eq!(separator.bg, Some(Color::Reset));
        assert!(!separator.add_modifier.contains(Modifier::DIM));
        assert_eq!(query.fg, Some(Color::Gray));
        assert_eq!(query.bg, Some(Color::Reset));
        assert!(!query.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn default_layout_keeps_the_current_chrome_geometry() {
        let layout = ChromeLayout::default();

        assert_eq!(layout.topbar_content_row(), 0);
        assert_eq!(layout.input_content_row(), 1);
        assert_eq!(layout.divider_content_row(), 2);
        assert_eq!(layout.content_start_row(), 3);
        assert_eq!(layout.content_rows(24), 20);
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
    fn divider_fills_width_without_a_label() {
        assert_eq!(divider_line(5, ""), "─────");
    }

    #[test]
    fn child_input_shows_the_route_label() {
        let frame = ChromeFrame::compose_with_cursor(
            80,
            Some(&route()),
            "",
            0,
            EngineChrome::default(),
            None,
        );
        assert_eq!(frame.input_line(), "app ");
        assert_eq!(frame.divider, "─".repeat(78));
    }

    #[test]
    fn root_input_hides_the_route_label() {
        let frame = ChromeFrame::compose_with_cursor(
            80,
            None,
            "query",
            "query".len(),
            EngineChrome::default(),
            None,
        );
        assert_eq!(frame.input_line(), "query");
        assert_eq!(frame.input_prefix_highlight, None);
    }

    #[test]
    fn input_buffer_edits_at_the_cursor() {
        let mut input = InputBuffer::new("ac");
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
    fn footer_only_shows_the_overflow_binding_when_commands_do_not_fit() {
        let wide = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome {
                status: Some("1 result".to_string()),
                commands: vec![("enter".to_string(), "Open".to_string())],
                overflow_command: Some(("ctrl+k".to_string(), "Commands".to_string())),
                ..EngineChrome::default()
            },
            None,
        );
        assert!(!wide.footer.contains("Ctrl-K"));

        let narrow = ChromeFrame::compose(
            32,
            &route(),
            "",
            EngineChrome {
                status: Some("1 result".to_string()),
                commands: vec![
                    ("enter".to_string(), "Open".to_string()),
                    ("ctrl+p".to_string(), "Preview".to_string()),
                ],
                overflow_command: Some(("ctrl+k".to_string(), "Commands".to_string())),
                ..EngineChrome::default()
            },
            None,
        );
        assert!(narrow.footer.contains("Ctrl-K"));
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

    #[test]
    fn error_footer_uses_the_error_binding() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;

        let frame = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome::default(),
            Some("operation failed"),
        );
        let mut theme = Theme::terminal();
        theme.chrome.error.fg = Some(Color::Magenta);
        theme.chrome.error.bg = Some(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();

        terminal
            .draw(|draw| frame.render_frame(draw, &theme))
            .unwrap();

        let cell = terminal.backend().buffer().cell((1, 23)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn terminal_theme_footer_keys_use_the_terminal_background() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;

        let frame = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome {
                commands: vec![("enter".to_string(), "Open".to_string())],
                ..EngineChrome::default()
            },
            None,
        );
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::terminal();

        terminal
            .draw(|draw| frame.render_frame(draw, &theme))
            .unwrap();

        let key_x = 1 + frame.footer_keys[0].0;
        let key = terminal
            .backend()
            .buffer()
            .cell((key_x as u16, 23))
            .unwrap()
            .style();
        assert_eq!(key.fg, Some(Color::Cyan));
        assert_eq!(key.bg, Some(Color::Reset));
        assert!(key.add_modifier.contains(Modifier::BOLD));
        assert!(!key.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn configured_theme_reaches_chrome_render_buffer() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;
        use std::path::Path;

        let config_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config");
        let theme = crate::theme::load(
            &config_root.join("config.toml"),
            Some("contrast"),
            &crate::theme::ThemeLoadOptions::default(),
        )
        .unwrap();
        let frame = ChromeFrame::compose(
            80,
            &route(),
            "query",
            EngineChrome {
                commands: vec![("enter".to_string(), "Open".to_string())],
                ..EngineChrome::default()
            },
            None,
        );
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|draw| frame.render_frame(draw, &theme))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let prefix = buffer.cell((1, 1)).unwrap().style();
        let divider = buffer.cell((1, 2)).unwrap().style();
        let footer_key = buffer
            .cell((1 + frame.footer_keys[0].0 as u16, 23))
            .unwrap()
            .style();
        let footer_text = frame.footer.find("Open").unwrap() as u16 + 1;
        let footer_text = buffer.cell((footer_text, 23)).unwrap().style();

        assert_eq!(prefix.fg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(prefix.bg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(divider.fg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(footer_key.fg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(footer_key.bg, Some(Color::Rgb(0, 255, 255)));
        assert_eq!(footer_text.fg, Some(Color::Rgb(255, 255, 255)));
        assert_eq!(footer_text.bg, Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn footer_status_keeps_its_separator() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;

        let frame = ChromeFrame::compose(
            80,
            &route(),
            "",
            EngineChrome {
                status: Some("1/1".to_string()),
                ..EngineChrome::default()
            },
            None,
        );
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::terminal();

        terminal
            .draw(|draw| frame.render_frame(draw, &theme))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((1, 23)).unwrap().symbol(), "1");
        assert_eq!(buffer.cell((2, 23)).unwrap().symbol(), "/");
        assert_eq!(buffer.cell((3, 23)).unwrap().symbol(), "1");
    }

    #[test]
    fn embedded_terminal_keeps_avt_cell_style() {
        use ratatui::Terminal as RatatuiTerminal;
        use ratatui::backend::TestBackend;

        let frame = ChromeFrame::compose(80, &route(), "", EngineChrome::default(), None);
        let mut screen = crate::embedded_terminal::EmbeddedTerminal::new(78, 19);
        screen.feed(b"\x1b[38;5;196mred\x1b[0m");
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::terminal();

        terminal
            .draw(|draw| {
                let area = frame.render_chrome(draw, &theme);
                draw.render_widget(screen.widget(), area);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((1, 3)).unwrap();
        assert_eq!(cell.symbol(), "r");
        assert_eq!(cell.style().fg, Some(Color::Indexed(196)));
    }
}
