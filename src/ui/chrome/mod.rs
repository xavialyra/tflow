mod footer;
mod host;
#[cfg(test)]
mod layout;

#[cfg(test)]
mod frame;

pub(crate) use footer::{FooterModel, FooterRenderer, spans_from_footer_content};
pub(crate) use host::ContentHost;

#[cfg(test)]
use crate::input::previous_char_boundary;
#[cfg(test)]
pub(crate) use frame::ChromeFrame;
#[cfg(test)]
use layout::ChromeLayout;
#[cfg(test)]
use layout::{InputLayout, Insets};

#[cfg(test)]
use crate::ui::theme::Theme;
#[cfg(test)]
use crate::workflow::navigation::RouteDisplay;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Default)]
pub(crate) struct EngineChrome {
    pub(crate) status: Option<String>,
    #[cfg(test)]
    pub(crate) commands: Vec<(String, String)>,
    #[cfg(test)]
    pub(crate) presentation: ChromePresentation,
}

impl EngineChrome {
    pub(crate) fn new(status: Option<String>) -> Self {
        #[cfg(test)]
        {
            Self {
                status,
                ..Self::default()
            }
        }
        #[cfg(not(test))]
        {
            Self { status }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FooterContent {
    pub(crate) text: String,
    pub(crate) key_spans: Vec<(usize, usize)>,
    pub(crate) title_span: Option<(usize, usize)>,
    pub(crate) status_span: Option<(usize, usize)>,
}

impl FooterContent {
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            key_spans: Vec::new(),
            title_span: None,
            status_span: None,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChromePresentation {
    layout: ChromeLayout,
    input_muted: bool,
    recognized_input_prefix_end: Option<usize>,
    footer: Option<FooterContent>,
}

#[cfg(test)]
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

pub(super) fn footer_line(
    width: usize,
    title: Option<&str>,
    status: &str,
    commands: &[(String, String)],
) -> FooterContent {
    if width == 0 {
        return FooterContent::default();
    }
    let (left, title_span, status_span) = footer_label_spans(title, status);
    let right = command_footer(commands);
    if right.text.is_empty() {
        let text = clip(&left, width);
        let visible_end = text.len();
        return FooterContent {
            text,
            key_spans: Vec::new(),
            title_span: title_span
                .and_then(|(s, e)| (s < visible_end).then_some((s, e.min(visible_end)))),
            status_span: status_span
                .and_then(|(s, e)| (s < visible_end).then_some((s, e.min(visible_end)))),
        };
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
        result.title_span = title_span;
        result.status_span = status_span;
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
    let visible_end = left.len();
    let used = UnicodeWidthStr::width(left.as_str())
        + separator_width
        + UnicodeWidthStr::width(right.text.as_str());
    let padding = width.saturating_sub(used);
    let mut result = FooterContent::default();
    append_plain(&mut result, &left);
    result.title_span =
        title_span.and_then(|(s, e)| (s < visible_end).then_some((s, e.min(visible_end))));
    result.status_span =
        status_span.and_then(|(s, e)| (s < visible_end).then_some((s, e.min(visible_end))));
    append_plain(&mut result, &" ".repeat(padding));
    append_plain(&mut result, separator);
    append_content(&mut result, &right);
    result
}

fn footer_label_spans(
    title: Option<&str>,
    status: &str,
) -> (String, Option<(usize, usize)>, Option<(usize, usize)>) {
    let mut text = String::new();
    let mut title_span = None;
    let mut status_span = None;

    if let Some(title) = title.filter(|title| !title.is_empty()) {
        let start = text.len();
        text.push_str(title);
        let end = text.len();
        title_span = Some((start, end));
    }

    if !status.is_empty() {
        if !text.is_empty() {
            text.push_str(" | ");
        }
        let start = text.len();
        text.push_str(status);
        let end = text.len();
        status_span = Some((start, end));
    }

    (text, title_span, status_span)
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
        title_span: content.title_span.and_then(|(start, end)| {
            (start < visible_end).then_some((start, end.min(visible_end)))
        }),
        status_span: content.status_span.and_then(|(start, end)| {
            (start < visible_end).then_some((start, end.min(visible_end)))
        }),
    }
}

#[cfg(test)]
pub(super) fn as_u16(value: usize) -> u16 {
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

#[cfg(test)]
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

#[cfg(test)]
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
    use crate::input::EditorBuffer;
    use ratatui::style::Color;

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
        let mut input = EditorBuffer::new("ac");
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
        let mut screen = crate::engine::EmbeddedTerminal::new(78, 19);
        screen.feed(b"\x1b[38;5;196mred\x1b[0m");
        let mut terminal = RatatuiTerminal::new(TestBackend::new(80, 24)).unwrap();
        let theme = Theme::terminal();

        terminal
            .draw(|draw| {
                let area = frame.render_chrome(draw, &theme);
                let snapshot = screen.snapshot();
                draw.render_widget(snapshot.widget(), area);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((1, 3)).unwrap();
        assert_eq!(cell.symbol(), "r");
        assert_eq!(cell.style().fg, Some(Color::Indexed(196)));
    }
}
