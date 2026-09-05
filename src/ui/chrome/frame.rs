use super::{
    ChromeLayout, ChromePresentation, EngineChrome, FooterContent, as_u16, clip, clip_footer,
    footer_line,
};
#[cfg(test)]
use super::{byte_at_width, clip_from, divider_line, previous_char_boundary};
#[cfg(test)]
use crate::router::RouteDisplay;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

#[cfg(test)]
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
    #[cfg(test)]
    pub(crate) divider: String,
    #[cfg(test)]
    pub(crate) input: String,
    #[cfg(test)]
    pub(crate) input_cursor: usize,
    #[cfg(test)]
    input_prefix: String,
    #[cfg(test)]
    pub(super) input_prefix_highlight: Option<(usize, usize)>,
    #[cfg(test)]
    input_muted: bool,
    #[cfg(test)]
    recognized_input_prefix_end: Option<usize>,
    pub(crate) footer: String,
    footer_is_error: bool,
    #[cfg(test)]
    pub(crate) footer_divider: String,
    pub(super) footer_keys: Vec<(usize, usize)>,
    layout: ChromeLayout,
}

impl ChromeFrame {
    #[cfg(test)]
    pub(crate) fn input_line(&self) -> String {
        format!("{}{}", self.input_prefix, self.input)
    }

    #[cfg(test)]
    pub(crate) fn input_line_for_width(&self, width: usize) -> (String, usize) {
        let (line, cursor, _) = self.input_line_parts_for_width(width);
        (line, cursor)
    }

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn compose_with_cursor(
        width: usize,
        route: Option<&RouteDisplay>,
        input: &str,
        input_cursor: usize,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        Self::compose_with_cursor_label(
            width,
            route.map(RouteDisplay::label),
            input,
            input_cursor,
            engine,
            error,
        )
    }

    #[cfg(test)]
    pub(crate) fn compose_with_cursor_label(
        width: usize,
        route_label: Option<&str>,
        input: &str,
        input_cursor: usize,
        engine: EngineChrome,
        error: Option<&str>,
    ) -> Self {
        let EngineChrome {
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
                None,
                status.as_deref().unwrap_or(""),
                &commands,
                overflow_command.as_ref(),
                false,
            )
        };
        let left_padding = " ".repeat(layout.input.padding.left);
        let (input_prefix, input_prefix_highlight) = match route_label {
            Some(route_label) => {
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

    #[cfg(test)]
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

    pub(super) fn render_frame(&self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        let width = area.width as usize;
        let height = area.height as usize;
        let layout = self.layout;
        #[cfg(test)]
        let viewport_width = layout.viewport_width(width);
        let footer_row = layout.footer_row(height);
        let mut lines = Vec::with_capacity(height);

        for row in 0..height {
            let line = if row == footer_row {
                Line::from(self.footer_spans(width, theme))
            } else if layout.topbar_rows > 0 && row == layout.topbar_content_row() {
                Line::default()
            } else {
                #[cfg(test)]
                {
                    if layout.input.rows > 0 && row == layout.input_content_row() {
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
                    } else if layout.input.divider_rows > 0 && row == layout.divider_content_row() {
                        Line::styled(
                            layout.pad_line(&self.divider, width, layout.viewport_padding),
                            theme.chrome.divider,
                        )
                    } else {
                        Line::default()
                    }
                }
                #[cfg(not(test))]
                {
                    Line::default()
                }
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
}

#[cfg(test)]
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
    let prefix_style = theme.picker.input_prefix;
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
