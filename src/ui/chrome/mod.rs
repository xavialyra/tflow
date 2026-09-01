mod frame;
mod input;
mod layout;

pub(crate) use frame::ChromeFrame;
#[allow(unused_imports)]
pub(crate) use input::EditorBuffer;
use input::previous_char_boundary;
use layout::ChromeLayout;
#[cfg(test)]
use layout::{InputLayout, Insets};

#[cfg(test)]
use crate::router::RouteDisplay;
#[cfg(test)]
use crate::theme::Theme;
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
