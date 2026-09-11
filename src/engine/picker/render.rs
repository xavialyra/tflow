use super::preview::{ImageProtocolCache, PickerPreviewRenderState};
use super::{Item, PickerView};
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct PickerRenderState {
    pub(crate) items: Arc<Vec<Item>>,
    pub(crate) selected: usize,
    pub(crate) initial_loading: bool,
    pub(crate) searching: bool,
    pub(crate) preview_visible: bool,
    pub(crate) preview: Option<PickerPreviewRenderState>,
    pub(crate) empty_message: String,
    pub(crate) row_height: usize,
}

const SCROLLBAR_THUMB_HEIGHT: usize = 2;

pub(crate) fn render_picker(
    frame: &mut Frame,
    area: Rect,
    state: &PickerRenderState,
    theme: &Theme,
) {
    let width = area.width as usize;
    let height = area.height as usize;
    if height == 0 || width == 0 {
        return;
    }

    let row_height = state.row_height.max(1);
    let item_height = |item: &Item| item.display.rows.len().max(row_height).min(height);

    if state.items.is_empty() {
        let text = if state.searching {
            "(searching...)"
        } else if state.initial_loading {
            ""
        } else {
            &state.empty_message
        };
        if !text.is_empty() {
            let line = Line::from(Span::styled(text.to_string(), theme.picker.muted));
            frame.render_widget(Paragraph::new(line).style(theme.picker.text), area);
        }
        return;
    }

    let selected_index = state.selected.min(state.items.len().saturating_sub(1));
    let mut start = selected_index;
    let mut used = item_height(&state.items[selected_index]);
    while start > 0 && used < height {
        let next = item_height(&state.items[start - 1]);
        if used.saturating_add(next) > height {
            break;
        }
        start -= 1;
        used += next;
    }

    let mut end = selected_index.saturating_add(1);
    while end < state.items.len() {
        let next = item_height(&state.items[end]);
        if used.saturating_add(next) > height {
            break;
        }
        end += 1;
        used += next;
    }
    let visible_count = end.saturating_sub(start);
    let reserves_scrollbar = state.items.len() > visible_count;
    let show_scrollbar = scrollbar_visible(state.items.len(), visible_count, start);
    let thumb_top = scrollbar_thumb_top(start, state.items.len(), visible_count);
    let thumb_height = SCROLLBAR_THUMB_HEIGHT.min(visible_count);
    let scrollbar_color = if let Some(bg) = theme.picker.scrollbar.bg {
        if Some(bg) != theme.picker.text.bg {
            bg
        } else {
            theme.picker.scrollbar.fg.unwrap_or(bg)
        }
    } else {
        theme
            .picker
            .scrollbar
            .fg
            .unwrap_or(ratatui::style::Color::Reset)
    };
    let scrollbar_style = ratatui::style::Style::default().bg(scrollbar_color);

    for (visible_row, (index, item)) in state
        .items
        .as_slice()
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_count)
        .enumerate()
    {
        let selected = index == state.selected;
        let item_y = area.y
            + state.items[start..index]
                .iter()
                .map(item_height)
                .sum::<usize>() as u16;
        let current_height = item_height(item);
        let item_rect = Rect {
            x: area.x,
            y: item_y,
            width: area.width,
            height: current_height as u16,
        };

        if selected {
            frame.render_widget(
                ratatui::widgets::Block::default().style(theme.picker.selected),
                item_rect,
            );
        }

        let right_padding: u16 = if reserves_scrollbar { 2 } else { 1 };
        let h_chunks = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Length(1),             // Marker
            ratatui::layout::Constraint::Length(1),             // Space after marker
            ratatui::layout::Constraint::Fill(1),               // Content
            ratatui::layout::Constraint::Length(right_padding), // Scrollbar column
        ])
        .split(item_rect);

        if selected {
            frame.render_widget(Paragraph::new("▌").style(theme.picker.marker), h_chunks[0]);
        }

        if show_scrollbar
            && visible_row >= thumb_top
            && visible_row < thumb_top.saturating_add(thumb_height)
        {
            let scrollbar_rect = Rect {
                x: item_rect.x + item_rect.width.saturating_sub(1),
                y: item_rect.y,
                width: 1,
                height: current_height as u16,
            };
            frame.render_widget(ratatui::widgets::Clear, scrollbar_rect);
            frame.render_widget(
                ratatui::widgets::Block::default().style(scrollbar_style),
                scrollbar_rect,
            );
        }

        let content_area = h_chunks[2];
        if content_area.width == 0 || content_area.height == 0 {
            continue;
        }

        let v_chunks =
            ratatui::layout::Layout::vertical(vec![
                ratatui::layout::Constraint::Length(1);
                current_height
            ])
            .split(content_area);

        let rows = &item.display.rows;
        for (r_idx, v_row_rect) in v_chunks.iter().enumerate() {
            if let Some(row) = rows.get(r_idx) {
                let cell_chunks =
                    ratatui::layout::Layout::horizontal(&row.constraints).split(*v_row_rect);

                for (c_idx, cell) in row.cells.iter().enumerate() {
                    if c_idx >= cell_chunks.len() {
                        break;
                    }
                    let cell_area = cell_chunks[c_idx];
                    let package_id = crate::workflow::config::package_id(&item.source_view);
                    let spans: Vec<Span> = cell
                        .spans
                        .iter()
                        .map(|s| {
                            let style = theme.resolve_slot(package_id, &s.slot, selected);
                            Span::styled(s.text.clone(), style)
                        })
                        .collect();

                    let paragraph = Paragraph::new(Line::from(spans)).alignment(cell.align);
                    frame.render_widget(paragraph, cell_area);
                }
            }
        }
    }
}

fn scrollbar_visible(total: usize, visible: usize, start: usize) -> bool {
    total > visible && start > 0
}

fn scrollbar_thumb_top(start: usize, total: usize, visible: usize) -> usize {
    let thumb_height = SCROLLBAR_THUMB_HEIGHT.min(visible);
    let track_height = visible.saturating_sub(thumb_height);
    let scroll_range = total.saturating_sub(visible);
    if track_height == 0 || scroll_range == 0 {
        return 0;
    }
    start.saturating_mul(track_height) / scroll_range
}

impl PickerView {
    pub(crate) fn render_state(&self) -> PickerRenderState {
        let frame = self.current();
        let empty_message = self.list_presentation();
        let results_ready = self.results_current(&frame.query);
        let in_grace_period = self.is_in_grace_period();
        let initial_loading = !self.has_completed_initial_load();
        let searching = self.is_loading() && !in_grace_period;
        let should_retain = results_ready || in_grace_period;

        let (items, selected) = if should_retain || !frame.selection.items.is_empty() {
            (Arc::clone(&frame.selection.items), frame.selection.selected)
        } else {
            (Arc::new(Vec::new()), 0)
        };

        let preview = Some(self.preview_render_state());

        PickerRenderState {
            items,
            selected,
            initial_loading,
            searching,
            preview_visible: self.preview_visible(),
            preview,
            empty_message,
            row_height: 1,
        }
    }
}

pub(crate) struct PickerRenderer {
    image_protocols: Mutex<ImageProtocolCache>,
}

impl PickerRenderer {
    pub(super) fn new() -> Self {
        Self {
            image_protocols: Mutex::new(ImageProtocolCache::new()),
        }
    }

    fn clear_image_protocols(&self) {
        self.image_protocols
            .lock()
            .expect("picker image protocol cache was poisoned")
            .clear();
    }
}

impl crate::engine::ViewRenderer for PickerRenderer {
    fn validate_model(&self, model: &crate::engine::RenderModel) -> anyhow::Result<()> {
        if model.kind() != "picker" || model.downcast_ref::<PickerRenderState>().is_none() {
            anyhow::bail!(
                "picker renderer/model pairing mismatch: renderer=picker model={:?}",
                model
            );
        }
        Ok(())
    }

    fn chrome(&self, model: &crate::engine::RenderModel) -> crate::ui::chrome::EngineChrome {
        let Some(state) = model.downcast_ref::<PickerRenderState>() else {
            return crate::ui::chrome::EngineChrome::default();
        };
        let status = if state.items.is_empty() {
            if state.initial_loading {
                None
            } else {
                Some("0 of 0".to_string())
            }
        } else {
            let current = state.selected.saturating_add(1);
            Some(format!("{current} of {}", state.items.len()))
        };
        crate::ui::chrome::EngineChrome::new(status)
    }

    fn render(
        &self,
        model: &crate::engine::RenderModel,
        context: &crate::engine::RenderContext,
        frame: &mut Frame,
        area: Rect,
    ) {
        let Some(state) = model.downcast_ref::<PickerRenderState>() else {
            return;
        };
        if state.preview_visible
            && let Some(preview) = &state.preview
        {
            let (items_area, preview_area) = preview.areas(area);
            render_picker(frame, items_area, state, &context.theme);
            if let Some(preview_area) = preview_area {
                let mut image_protocols = self
                    .image_protocols
                    .lock()
                    .expect("picker image protocol cache was poisoned");
                preview.render(
                    frame,
                    preview_area,
                    &context.theme,
                    context.image_picker,
                    &mut image_protocols,
                );
                preview.render_separator(frame, items_area, preview_area, &context.theme);
            } else {
                self.clear_image_protocols();
            }
        } else {
            self.clear_image_protocols();
            render_picker(frame, area, state, &context.theme);
        }
    }
}

#[cfg(test)]
fn clip(text: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
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
mod tests {
    use super::*;
    use crate::engine::ViewRenderer;
    use ratatui::style::{Color, Modifier, Style};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn picker_renderer_rejects_an_incompatible_render_model() {
        let renderer = PickerRenderer::new();
        let model = crate::engine::RenderModel::new("capture", ());

        let error = renderer
            .validate_model(&model)
            .expect_err("picker renderer must reject a capture model");
        assert!(error.to_string().contains("pairing mismatch"));
    }

    #[test]
    fn scrollbar_is_hidden_at_the_top_and_shown_after_scrolling() {
        assert!(!scrollbar_visible(10, 5, 0));
        assert!(scrollbar_visible(10, 5, 1));
        assert!(!scrollbar_visible(5, 5, 1));
    }

    #[test]
    fn theme_resolves_slot_styles_correctly() {
        use crate::engine::picker::SlotToken;
        let mut theme = Theme::terminal();
        theme.picker.marker.fg = Some(Color::Magenta);
        theme.picker.scrollbar.fg = Some(Color::Green);
        theme.picker.muted.fg = Some(Color::Gray);
        theme.picker.selected = Style::new()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD);
        theme.picker.selected_muted = Style::new()
            .fg(Color::Gray)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD);

        // Marker & Scrollbar
        assert_eq!(theme.picker.marker.fg, Some(Color::Magenta));
        assert_eq!(theme.picker.scrollbar.fg, Some(Color::Green));

        // Primary text
        let unselected_text = theme.resolve_slot_style(SlotToken::Primary, false);
        assert_eq!(unselected_text, theme.picker.text);
        let selected_text = theme.resolve_slot_style(SlotToken::Primary, true);
        assert_eq!(selected_text, theme.picker.selected);

        // Secondary & Muted slots
        let unselected_sec = theme.resolve_slot_style(SlotToken::Secondary, false);
        assert_eq!(unselected_sec.fg, Some(Color::Gray));
        let selected_sec = theme.resolve_slot_style(SlotToken::Secondary, true);
        assert_eq!(selected_sec.fg, Some(Color::Gray));
        assert_eq!(selected_sec.bg, Some(Color::Green));
    }

    #[test]
    fn clips_wide_text_without_exceeding_the_width() {
        assert_eq!(UnicodeWidthStr::width(clip("abcdefgh", 6).as_str()), 6);
        assert!(UnicodeWidthStr::width(clip("終端abcdef", 7).as_str()) <= 7);
    }

    #[test]
    fn test_render_picker_multicell_and_slots() {
        use crate::engine::picker::ItemDisplayInput;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::terminal();

        let display_input: ItemDisplayInput = serde_json::from_str(
            r#"{
                "constraints": [{"Fill": 1}, {"Length": 10}],
                "cells": [
                    {"text": "Open File"},
                    {"text": "Ctrl+O", "slot": "badge", "align": "right"}
                ]
            }"#,
        )
        .unwrap();

        let item = Item {
            text: "Open File".to_string(),
            display: display_input.into(),
            value: Some("open_file".to_string()),
            metadata: serde_json::Value::Null,
            source_view: "test".to_string(),
        };

        let state = PickerRenderState {
            items: Arc::new(vec![item]),
            selected: 0,
            initial_loading: false,
            searching: false,
            preview_visible: false,
            preview: None,
            empty_message: "(no matches)".to_string(),
            row_height: 1,
        };

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_picker(frame, area, &state, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        // Row 0 should contain marker '▌' and 'Open File'
        let content: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(content.contains('▌'));
        assert!(content.contains("Open File"));
        assert!(content.contains("Ctrl+O"));
    }

    #[test]
    fn mixed_row_heights_keep_selected_item_in_visible_window() {
        use crate::engine::picker::ItemDisplayInput;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use serde_json::json;

        let item = |rows: &[&str]| Item {
            text: rows[0].to_string(),
            display: serde_json::from_value::<ItemDisplayInput>(json!({
                "rows": rows
                    .iter()
                    .map(|text| json!({"cells": [{"text": text}]}))
                    .collect::<Vec<_>>()
            }))
            .unwrap()
            .into(),
            value: None,
            metadata: serde_json::Value::Null,
            source_view: "test".to_string(),
        };
        let state = PickerRenderState {
            items: Arc::new(vec![
                item(&["FIRST-A", "FIRST-B", "FIRST-C"]),
                item(&["SECOND-A", "SECOND-B"]),
                item(&["THIRD"]),
            ]),
            selected: 2,
            initial_loading: false,
            searching: false,
            preview_visible: false,
            preview: None,
            empty_message: "(no matches)".to_string(),
            row_height: 1,
        };
        let mut terminal = Terminal::new(TestBackend::new(32, 4)).unwrap();
        terminal
            .draw(|frame| render_picker(frame, frame.area(), &state, &Theme::terminal()))
            .unwrap();

        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("SECOND-A"));
        assert!(content.contains("THIRD"));
        assert_eq!(
            terminal.backend().buffer().cell((0, 2)).unwrap().symbol(),
            "▌"
        );
    }
}
