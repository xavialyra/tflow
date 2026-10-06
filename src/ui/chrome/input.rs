//! Host-level Input state machine and visual presentations.
//!
//! Input represents the logical text editing state (buffer, cursor, history).
//! Visual presentations (Omnibar, BorderTitle, FloatingPrompt) define how that
//! input is rendered onto the terminal.

use crate::input::EditorBuffer;
use crate::input::{Key, previous_char_boundary};
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Visual display and control mode for host input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum InputPresentationMode {
    /// No input bar displayed. Unbound keys pass directly to the view.
    #[default]
    Hidden,
    /// Top-level full-width Omnibar prompt across the top of the content host.
    Omnibar {
        /// Whether the terminal cursor is visible at the input insertion point.
        show_cursor: bool,
    },
    /// Embedded input field rendered within a popup border.
    BorderTitle,
    /// Floating inline search prompt (e.g. for on-demand search).
    FloatingPrompt,
}

#[allow(dead_code)]
impl InputPresentationMode {
    pub(crate) fn is_visible(&self) -> bool {
        !matches!(self, Self::Hidden)
    }

    pub(crate) fn show_cursor(&self) -> bool {
        match self {
            Self::Omnibar { show_cursor } => *show_cursor,
            _ => false,
        }
    }
}

/// Host-level input state machine holding buffer, cursor, and layout options.
#[derive(Debug, Clone, Default)]
pub(crate) struct HostInputState {
    pub(crate) editor: EditorBuffer,
    pub(crate) mode: InputPresentationMode,
    pub(crate) placeholder: Option<String>,
    pub(crate) left_prefix: Option<String>,
}

#[allow(dead_code)]
impl HostInputState {
    pub(crate) fn for_view(view: &dyn crate::view::View) -> Self {
        let seed = view.initial_input();
        Self {
            editor: EditorBuffer {
                raw: seed.raw,
                cursor: seed.cursor,
                revision: seed.revision,
            },
            mode: view.input_mode(),
            placeholder: view.input_placeholder(),
            left_prefix: view.input_left_prefix(),
        }
    }

    pub(crate) fn edit(&mut self, edit: crate::view::InputEdit) -> bool {
        use crate::view::InputEdit;
        match edit {
            InputEdit::Key(key) => {
                let (changed, moved) = self.apply_key(key);
                changed || moved
            }
            InputEdit::Paste(text) => self.insert_text(&text),
            InputEdit::Clear => self.clear(),
            InputEdit::DeleteWord => self.delete_word(),
        }
    }

    pub(crate) fn new(mode: InputPresentationMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    pub(crate) fn raw(&self) -> &str {
        &self.editor.raw
    }

    pub(crate) fn cursor(&self) -> usize {
        self.editor.cursor
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.editor.raw.is_empty()
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.editor.raw.is_empty() {
            return false;
        }
        self.editor.clear();
        true
    }

    pub(crate) fn insert_text(&mut self, text: &str) -> bool {
        self.editor.insert_text(text)
    }

    pub(crate) fn delete_word(&mut self) -> bool {
        let prev = self.editor.raw.len();
        self.editor.delete_word();
        self.editor.raw.len() != prev
    }

    /// Apply a key to the editor buffer. Returns `(text_changed, cursor_moved)`.
    pub(crate) fn apply_key(&mut self, key: Key) -> (bool, bool) {
        match key {
            Key::Char(c) => {
                self.editor.insert(c);
                (true, true)
            }
            Key::Left => {
                let prev = self.editor.cursor;
                self.editor.move_left();
                (false, self.editor.cursor != prev)
            }
            Key::Right => {
                let prev = self.editor.cursor;
                self.editor.move_right();
                (false, self.editor.cursor != prev)
            }
            Key::Home => {
                let prev = self.editor.cursor;
                self.editor.move_home();
                (false, self.editor.cursor != prev)
            }
            Key::End => {
                let prev = self.editor.cursor;
                self.editor.move_end();
                (false, self.editor.cursor != prev)
            }
            Key::Backspace => {
                let changed = self.editor.delete_backward();
                (changed, changed)
            }
            Key::Delete => {
                let changed = self.editor.delete_forward();
                (changed, false)
            }
            _ => (false, false),
        }
    }
}

/// Sliced and formatted query presentation for the Omnibar.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisibleOmnibarQuery {
    pub(crate) text: String,
    pub(crate) cursor: u16,
    pub(crate) highlight: Option<Range<usize>>,
    pub(crate) placeholder: Option<Range<usize>>,
}

/// Calculate the visible query spans, horizontal scrolling, and cursor positioning for a given width.
pub(crate) fn format_visible_omnibar(
    left_prefix: Option<&str>,
    placeholder: Option<&str>,
    raw: &str,
    cursor: usize,
    width: usize,
) -> VisibleOmnibarQuery {
    let cursor = previous_char_boundary(raw, cursor);
    let left_prefix = left_prefix.filter(|prefix| !prefix.is_empty());
    let prefix = left_prefix
        .map(|prefix| format!("{prefix} "))
        .unwrap_or_default();
    let prefix_width = UnicodeWidthStr::width(prefix.as_str());
    let highlight = |output_end: usize| {
        left_prefix
            .filter(|prefix| prefix.len() <= output_end)
            .map(|prefix| 0..prefix.len())
    };
    if width <= prefix_width {
        let text = crate::ui::chrome::clip(&prefix, width);
        return VisibleOmnibarQuery {
            highlight: highlight(text.len()),
            text,
            cursor: width.saturating_sub(1) as u16,
            placeholder: None,
        };
    }

    let available = width - prefix_width;
    let input_width = UnicodeWidthStr::width(raw);
    let cursor_width = UnicodeWidthStr::width(&raw[..cursor]);
    if input_width <= available {
        if raw.is_empty()
            && let Some(placeholder) = placeholder.filter(|placeholder| !placeholder.is_empty())
        {
            let clipped = crate::ui::chrome::clip(placeholder, available);
            let text = format!("{prefix}{clipped}");
            let placeholder = (!clipped.is_empty()).then_some(prefix.len()..text.len());
            return VisibleOmnibarQuery {
                highlight: highlight(prefix.len()),
                placeholder,
                text,
                cursor: prefix_width as u16,
            };
        }
        let text = format!("{prefix}{raw}");
        return VisibleOmnibarQuery {
            highlight: highlight(text.len()),
            text,
            cursor: (prefix_width + cursor_width).min(width.saturating_sub(1)) as u16,
            placeholder: None,
        };
    }

    let needs_left_clip = cursor_width > available;
    let marker = if needs_left_clip && available >= 4 {
        "..."
    } else {
        ""
    };
    let marker_width = UnicodeWidthStr::width(marker);
    let budget = available.saturating_sub(marker_width);
    let start_width = if needs_left_clip {
        cursor_width.saturating_sub(budget.saturating_sub(1))
    } else {
        0
    };
    let mut start = 0;
    let mut used = 0;
    for (index, grapheme) in raw[..cursor].grapheme_indices(true) {
        if used >= start_width {
            start = index;
            break;
        }
        used += UnicodeWidthStr::width(grapheme);
        start = index + grapheme.len();
    }
    let mut visible = String::new();
    let mut visible_width = 0;
    for grapheme in raw[start..].graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if visible_width + grapheme_width > budget {
            break;
        }
        visible.push_str(grapheme);
        visible_width += grapheme_width;
    }
    let local_cursor = UnicodeWidthStr::width(&raw[start..cursor]);
    let text = format!("{prefix}{marker}{visible}");
    VisibleOmnibarQuery {
        highlight: highlight(text.len()),
        text,
        cursor: (prefix_width + marker_width + local_cursor).min(width.saturating_sub(1)) as u16,
        placeholder: None,
    }
}

/// Render the Omnibar into the designated Rect. Returns the cursor coordinate `(x, y)` if visible.
#[allow(dead_code)]
pub(crate) fn render_omnibar_widget(
    frame: &mut Frame,
    area: Rect,
    input: &HostInputState,
    theme: &Theme,
) -> Option<(u16, u16)> {
    if area.width == 0 || area.height == 0 || !input.mode.is_visible() {
        return None;
    }

    let query = format_visible_omnibar(
        input.left_prefix.as_deref(),
        input.placeholder.as_deref(),
        input.raw(),
        input.cursor(),
        area.width as usize,
    );

    frame.render_widget(
        Paragraph::new(Line::styled(
            " ".repeat(area.width as usize),
            theme.picker.text,
        )),
        area,
    );

    let mut spans = Vec::new();
    let mut offset = 0;
    if let Some(highlight) = query.highlight.clone() {
        if highlight.start > 0 {
            spans.push(Span::styled(
                query.text[..highlight.start].to_string(),
                theme.picker.text,
            ));
        }
        spans.push(Span::styled(
            query.text[highlight.clone()].to_string(),
            theme.picker.input_prefix,
        ));
        offset = highlight.end;
    }
    if let Some(placeholder) = query.placeholder.clone() {
        if placeholder.start > offset {
            spans.push(Span::styled(
                query.text[offset..placeholder.start].to_string(),
                theme.picker.text,
            ));
        }
        spans.push(Span::styled(
            query.text[placeholder.clone()].to_string(),
            theme.picker.placeholder,
        ));
        offset = placeholder.end;
    }
    if offset < query.text.len() {
        spans.push(Span::styled(
            query.text[offset..].to_string(),
            theme.picker.text,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if input.mode.show_cursor() {
        let cursor_x = area
            .x
            .saturating_add(query.cursor)
            .min(area.x.saturating_add(area.width.saturating_sub(1)));
        Some((cursor_x, area.y))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_input_state_apply_key_and_cursor() {
        let mut state = HostInputState::new(InputPresentationMode::Omnibar { show_cursor: true });
        assert_eq!(state.raw(), "");
        assert_eq!(state.cursor(), 0);

        // Type "hello"
        for c in "hello".chars() {
            let (changed, moved) = state.apply_key(Key::Char(c));
            assert!(changed);
            assert!(moved);
        }
        assert_eq!(state.raw(), "hello");
        assert_eq!(state.cursor(), 5);

        // Move left
        let (changed, moved) = state.apply_key(Key::Left);
        assert!(!changed);
        assert!(moved);
        assert_eq!(state.cursor(), 4);

        // Home
        let (changed, moved) = state.apply_key(Key::Home);
        assert!(!changed);
        assert!(moved);
        assert_eq!(state.cursor(), 0);

        // End
        let (changed, moved) = state.apply_key(Key::End);
        assert!(!changed);
        assert!(moved);
        assert_eq!(state.cursor(), 5);

        // Backspace
        let (changed, moved) = state.apply_key(Key::Backspace);
        assert!(changed);
        assert!(moved);
        assert_eq!(state.raw(), "hell");
        assert_eq!(state.cursor(), 4);

        // Clear
        assert!(state.clear());
        assert_eq!(state.raw(), "");
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn clipped_query_preserves_graphemes_and_local_cursor_position() {
        let query = format_visible_omnibar(None, None, "abcdefghijk", 7, 5);
        assert_eq!(query.text, "...gh");
        assert_eq!(query.cursor, 4);

        let query = format_visible_omnibar(None, None, "abcdefghijk", 3, 5);
        assert_eq!(query.text, "abcde");
        assert_eq!(query.cursor, 3);

        let query = format_visible_omnibar(None, None, "A👩‍💻Bxyz", "A👩‍💻".len(), 4);
        assert_eq!(query.text, "A👩‍💻B");
        assert_eq!(query.cursor, 3);
    }

    #[test]
    fn query_cursor_stays_inside_exact_and_tiny_widths() {
        for width in 0..=6 {
            let query = format_visible_omnibar(None, None, "abcdef", 6, width);
            assert!(UnicodeWidthStr::width(query.text.as_str()) <= width);
            assert!(usize::from(query.cursor) < width.max(1));
        }
    }

    #[test]
    fn omnibar_formatting_and_placeholder() {
        let query = format_visible_omnibar(Some("sys"), Some("Type here"), "", 0, 20);
        assert_eq!(query.text, "sys Type here");
        assert!(query.placeholder.is_some());
        assert_eq!(query.cursor, 4);

        let query_typed = format_visible_omnibar(Some("sys"), Some("Type here"), "foo", 3, 20);
        assert_eq!(query_typed.text, "sys foo");
        assert!(query_typed.placeholder.is_none());
        assert_eq!(query_typed.cursor, 7);
    }
}
