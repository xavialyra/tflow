use super::state::CompletionState;
use crate::input::{CompletionEdit, EditorBuffer};
use crate::router::ViewCandidate;
use crate::selection::SelectionCore;
use crate::theme::Theme;
use anyhow::Result;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionCandidate {
    pub(super) view_ref: String,
    pub(super) alias: Option<String>,
    pub(super) plugin_name: String,
    pub(super) engine_type: String,
}

impl From<ViewCandidate> for CompletionCandidate {
    fn from(candidate: ViewCandidate) -> Self {
        Self {
            view_ref: candidate.view_ref,
            alias: candidate.alias,
            plugin_name: candidate.plugin_name,
            engine_type: candidate.engine_type,
        }
    }
}

impl CompletionCandidate {
    fn primary_label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.view_ref)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionInput {
    Next,
    Previous,
    Accept,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SelectionDecision {
    Invalidate,
    Accepted(Option<CompletionCandidate>),
    Cancelled,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SelectionRenderModel {
    pub(super) candidates: Vec<CompletionCandidate>,
    pub(super) selected: usize,
}

pub(super) trait TransientSelectionRuntime {
    fn dispatch(&mut self, input: SelectionInput) -> Result<SelectionDecision>;
    fn render_model(&self) -> SelectionRenderModel;
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompletionRuntime {
    selection: SelectionCore<CompletionCandidate>,
}

impl CompletionRuntime {
    pub(super) fn new(candidates: Vec<CompletionCandidate>) -> Self {
        Self {
            selection: SelectionCore::new(candidates),
        }
    }

    #[cfg(test)]
    pub(super) fn with_selected(mut self, selected: usize) -> Self {
        self.selection.selected = selected.min(self.selection.items.len().saturating_sub(1));
        self
    }
}

impl TransientSelectionRuntime for CompletionRuntime {
    fn dispatch(&mut self, input: SelectionInput) -> Result<SelectionDecision> {
        match input {
            SelectionInput::Next => {
                self.selection.cycle_by(1);
                Ok(SelectionDecision::Invalidate)
            }
            SelectionInput::Previous => {
                self.selection.cycle_by(-1);
                Ok(SelectionDecision::Invalidate)
            }
            SelectionInput::Accept => Ok(SelectionDecision::Accepted(
                self.selection.selected_item().cloned(),
            )),
            SelectionInput::Cancel => Ok(SelectionDecision::Cancelled),
        }
    }

    fn render_model(&self) -> SelectionRenderModel {
        SelectionRenderModel {
            candidates: self.selection.items.clone(),
            selected: self.selection.selected,
        }
    }
}

pub(super) fn route_completion_edit(
    input: &EditorBuffer,
    completion: &CompletionState,
    candidate: &CompletionCandidate,
) -> Option<CompletionEdit> {
    let range = completion.host.parent.replacement_range.clone();
    let old_length = range.end.saturating_sub(range.start);
    let mut replacement = candidate.view_ref.clone();
    if range.end == input.raw.len() {
        replacement.push(' ');
    }
    let cursor = if input.cursor > range.end {
        if replacement.len() >= old_length {
            input.cursor + replacement.len() - old_length
        } else {
            input.cursor.saturating_sub(old_length - replacement.len())
        }
    } else {
        replacement.len()
    };
    Some(CompletionEdit {
        source_identity: completion.host.parent.source_identity,
        source_revision: completion.host.parent.buffer_revision,
        range,
        replacement,
        cursor,
    })
}

pub(super) fn render_completion(
    frame: &mut Frame,
    area: Rect,
    completion: &SelectionRenderModel,
    theme: &Theme,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height == 0 || width == 0 {
        return;
    }
    if completion.candidates.is_empty() {
        frame.render_widget(
            Paragraph::new("(no matching views)").style(theme.picker.muted),
            area,
        );
        return;
    }

    let start = if completion.selected >= height {
        completion.selected + 1 - height
    } else {
        0
    };
    let lines = completion
        .candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(index, candidate)| {
            let reference = candidate
                .alias
                .as_ref()
                .map(|_| format!("  {}", candidate.view_ref))
                .unwrap_or_default();
            let metadata = format!("  [{} / {}]", candidate.plugin_name, candidate.engine_type);
            let label = format!("  {}", candidate.primary_label());
            let text = crate::chrome::clip(&format!("{label}{reference}{metadata}"), width);
            if index == completion.selected {
                Line::from(vec![
                    Span::styled("▌", theme.picker.marker),
                    Span::styled(
                        text.strip_prefix(' ').unwrap_or(&text).to_string(),
                        theme.picker.selected,
                    ),
                ])
            } else {
                let label_end = label.len().min(text.len());
                Line::from(vec![
                    Span::styled(text[..label_end].to_string(), theme.picker.text),
                    Span::styled(text[label_end..].to_string(), theme.picker.muted),
                ])
            }
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(theme.picker.text), area);
}
