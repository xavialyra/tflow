use ratatui::layout::{Alignment, Constraint};
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;

/// Semantic visual role for text rendering.
/// Raw styles (colors, modifiers) are prohibited in item data; they must be resolved
/// through the Theme preprocessor to maintain consistency and readable contrast.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum SlotToken {
    Primary,
    Secondary,
    Muted,
    Accent,
    Badge,
    Success,
    Warning,
    Error,
    Custom(String),
}

impl Default for SlotToken {
    fn default() -> Self {
        Self::Primary
    }
}

impl From<String> for SlotToken {
    fn from(s: String) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "primary" => Self::Primary,
            "secondary" => Self::Secondary,
            "muted" => Self::Muted,
            "accent" => Self::Accent,
            "badge" => Self::Badge,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Custom(s),
        }
    }
}

impl From<&str> for SlotToken {
    fn from(s: &str) -> Self {
        Self::from(s.to_string())
    }
}

impl From<SlotToken> for String {
    fn from(token: SlotToken) -> Self {
        token.as_str().to_string()
    }
}

impl SlotToken {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::Muted => "muted",
            Self::Accent => "accent",
            Self::Badge => "badge",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Custom(name) => name.as_str(),
        }
    }
}

/// Internal immutable normalized representation of an item's visual presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedItemDisplay {
    pub rows: Vec<NormalizedRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRow {
    pub constraints: Vec<Constraint>,
    pub cells: Vec<NormalizedCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedCell {
    pub align: Alignment,
    pub spans: Vec<NormalizedSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedSpan {
    pub text: String,
    pub slot: SlotToken,
}

impl NormalizedItemDisplay {
    /// Return the primary plain text representation, used for fallback, accessibility, or debug.
    pub fn plain_text(&self) -> String {
        let mut parts = Vec::new();
        for row in &self.rows {
            for cell in &row.cells {
                for span in &cell.spans {
                    if !span.text.is_empty() {
                        parts.push(span.text.as_str());
                    }
                }
            }
        }
        parts.join(" ")
    }

    /// Total number of rows required by this item layout.
    #[allow(dead_code)]
    pub fn row_count(&self) -> usize {
        self.rows.len().max(1)
    }

    /// Injects a source badge into the top-right corner (Row 0) of the item.
    /// Secondary rows (Row 1..N) are left untouched to preserve description layouts.
    pub fn inject_badge(&mut self, badge_text: &str, slot: SlotToken) {
        if badge_text.is_empty() {
            return;
        }
        if self.rows.is_empty() {
            self.rows.push(NormalizedRow {
                constraints: Vec::new(),
                cells: Vec::new(),
            });
        }
        let row = &mut self.rows[0];
        let badge_len = UnicodeWidthStr::width(badge_text) as u16;
        let badge_cell = NormalizedCell {
            align: Alignment::Right,
            spans: vec![NormalizedSpan {
                text: badge_text.to_string(),
                slot,
            }],
        };

        // Ensure all existing cells in Row 0 have a valid constraint.
        if row.constraints.len() < row.cells.len() {
            row.constraints.resize(row.cells.len(), Constraint::Fill(1));
        } else if row.constraints.is_empty() && !row.cells.is_empty() {
            row.constraints.resize(row.cells.len(), Constraint::Fill(1));
        }

        row.constraints.push(Constraint::Length(badge_len + 1));
        row.cells.push(badge_cell);
    }
}

// ----------------------------------------------------------------------------
// Progressive Desugaring / Shorthand Deserialization Layer
// ----------------------------------------------------------------------------

/// External input contract allowing 3 levels of progressive complexity:
/// - Level 0: Plain text string, e.g. `"Open Settings"`
/// - Level 1: SingleLine shorthand with flat `cells`
/// - Level 2: MultiLine layout with explicit `rows`
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ItemDisplayInput {
    /// Level 0: Plain text string
    Plain(String),

    /// Level 1: Flat cells shorthand for single-line display
    SingleLine {
        #[serde(default)]
        constraints: Vec<ConstraintInput>,
        cells: Vec<CellInput>,
    },

    /// Level 2: Full multi-line structure
    MultiLine { rows: Vec<RowInput> },
}

impl ItemDisplayInput {
    #[allow(dead_code)]
    pub fn plain_text(&self) -> String {
        let normalized: NormalizedItemDisplay = self.clone().into();
        normalized.plain_text()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RowInput {
    #[serde(default)]
    pub constraints: Vec<ConstraintInput>,
    #[serde(default)]
    pub cells: Vec<CellInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ConstraintInput {
    Length(u16),
    Percentage(u16),
    Ratio(u32, u32),
    Min(u16),
    Max(u16),
    Fill(u16),
}

impl From<ConstraintInput> for Constraint {
    fn from(c: ConstraintInput) -> Self {
        match c {
            ConstraintInput::Length(v) => Constraint::Length(v),
            ConstraintInput::Percentage(v) => Constraint::Percentage(v),
            ConstraintInput::Ratio(n, d) => Constraint::Ratio(n, d),
            ConstraintInput::Min(v) => Constraint::Min(v),
            ConstraintInput::Max(v) => Constraint::Max(v),
            ConstraintInput::Fill(v) => Constraint::Fill(v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentInput {
    #[default]
    Left,
    Center,
    Right,
}

impl From<AlignmentInput> for Alignment {
    fn from(a: AlignmentInput) -> Self {
        match a {
            AlignmentInput::Left => Alignment::Left,
            AlignmentInput::Center => Alignment::Center,
            AlignmentInput::Right => Alignment::Right,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum CellInput {
    /// Simple cell shorthand: single text and optional slot/align
    Simple {
        text: String,
        #[serde(default)]
        slot: SlotToken,
        #[serde(default)]
        align: Option<AlignmentInput>,
    },
    /// Rich cell with multiple styled spans
    Rich {
        #[serde(default)]
        align: Option<AlignmentInput>,
        #[serde(default)]
        spans: Vec<SpanInput>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SpanInput {
    /// Plain string span
    Plain(String),
    /// Span with explicit slot token
    Detailed {
        text: String,
        #[serde(default)]
        slot: SlotToken,
    },
}

impl From<ItemDisplayInput> for NormalizedItemDisplay {
    fn from(input: ItemDisplayInput) -> Self {
        match input {
            ItemDisplayInput::Plain(text) => Self {
                rows: vec![NormalizedRow {
                    constraints: vec![Constraint::Fill(1)],
                    cells: vec![NormalizedCell {
                        align: Alignment::Left,
                        spans: vec![NormalizedSpan {
                            text,
                            slot: SlotToken::Primary,
                        }],
                    }],
                }],
            },
            ItemDisplayInput::SingleLine { constraints, cells } => Self {
                rows: vec![normalize_row(constraints, cells)],
            },
            ItemDisplayInput::MultiLine { rows } => {
                let normalized_rows = if rows.is_empty() {
                    vec![NormalizedRow {
                        constraints: vec![Constraint::Fill(1)],
                        cells: vec![NormalizedCell {
                            align: Alignment::Left,
                            spans: vec![NormalizedSpan {
                                text: String::new(),
                                slot: SlotToken::Primary,
                            }],
                        }],
                    }]
                } else {
                    rows.into_iter()
                        .map(|row| normalize_row(row.constraints, row.cells))
                        .collect()
                };
                Self {
                    rows: normalized_rows,
                }
            }
        }
    }
}

fn normalize_row(constraints: Vec<ConstraintInput>, cells: Vec<CellInput>) -> NormalizedRow {
    let cell_count = cells.len();
    let mut resolved_constraints: Vec<Constraint> =
        constraints.into_iter().map(Into::into).collect();

    if resolved_constraints.is_empty() && cell_count > 0 {
        // By default, if no constraints are given:
        // All cells share the available space equally via Fill(1).
        resolved_constraints = vec![Constraint::Fill(1); cell_count];
    } else if resolved_constraints.len() < cell_count {
        // Pad missing constraints with Fill(1)
        resolved_constraints.resize(cell_count, Constraint::Fill(1));
    }

    let normalized_cells = cells
        .into_iter()
        .map(|c| match c {
            CellInput::Simple { text, slot, align } => NormalizedCell {
                align: align.unwrap_or_default().into(),
                spans: vec![NormalizedSpan { text, slot }],
            },
            CellInput::Rich { align, spans } => NormalizedCell {
                align: align.unwrap_or_default().into(),
                spans: spans
                    .into_iter()
                    .map(|s| match s {
                        SpanInput::Plain(text) => NormalizedSpan {
                            text,
                            slot: SlotToken::Primary,
                        },
                        SpanInput::Detailed { text, slot } => NormalizedSpan { text, slot },
                    })
                    .collect(),
            },
        })
        .collect();

    NormalizedRow {
        constraints: resolved_constraints,
        cells: normalized_cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desugaring_plain() {
        let json = r#""Open Settings""#;
        let input: ItemDisplayInput = serde_json::from_str(json).unwrap();
        let normalized = NormalizedItemDisplay::from(input);

        assert_eq!(normalized.rows.len(), 1);
        let row = &normalized.rows[0];
        assert_eq!(row.constraints, vec![Constraint::Fill(1)]);
        assert_eq!(row.cells.len(), 1);
        assert_eq!(row.cells[0].align, Alignment::Left);
        assert_eq!(row.cells[0].spans.len(), 1);
        assert_eq!(row.cells[0].spans[0].text, "Open Settings");
        assert_eq!(row.cells[0].spans[0].slot, SlotToken::Primary);
    }

    #[test]
    fn test_desugaring_single_line_cells() {
        let json = r#"{
            "constraints": [{"Fill": 1}, {"Length": 10}],
            "cells": [
                {"text": "Open Settings"},
                {"text": "Ctrl+,", "slot": "badge", "align": "right"}
            ]
        }"#;
        let input: ItemDisplayInput = serde_json::from_str(json).unwrap();
        let normalized = NormalizedItemDisplay::from(input);

        assert_eq!(normalized.rows.len(), 1);
        let row = &normalized.rows[0];
        assert_eq!(
            row.constraints,
            vec![Constraint::Fill(1), Constraint::Length(10)]
        );
        assert_eq!(row.cells.len(), 2);
        assert_eq!(row.cells[0].align, Alignment::Left);
        assert_eq!(row.cells[0].spans[0].text, "Open Settings");
        assert_eq!(row.cells[0].spans[0].slot, SlotToken::Primary);

        assert_eq!(row.cells[1].align, Alignment::Right);
        assert_eq!(row.cells[1].spans[0].text, "Ctrl+,");
        assert_eq!(row.cells[1].spans[0].slot, SlotToken::Badge);
    }

    #[test]
    fn test_desugaring_multiline() {
        let json = r#"{
            "rows": [
                {
                    "cells": [{"text": "Title"}]
                },
                {
                    "cells": [{"text": "Description", "slot": "muted"}]
                }
            ]
        }"#;
        let input: ItemDisplayInput = serde_json::from_str(json).unwrap();
        let normalized = NormalizedItemDisplay::from(input);

        assert_eq!(normalized.rows.len(), 2);
        assert_eq!(normalized.rows[0].cells[0].spans[0].text, "Title");
        assert_eq!(normalized.rows[1].cells[0].spans[0].text, "Description");
        assert_eq!(normalized.rows[1].cells[0].spans[0].slot, SlotToken::Muted);
    }

    #[test]
    fn test_inject_badge_single_line() {
        let input = ItemDisplayInput::Plain("Termius".to_string());
        let mut display = NormalizedItemDisplay::from(input);
        display.inject_badge("app", SlotToken::Badge);

        assert_eq!(display.rows.len(), 1);
        let row = &display.rows[0];
        assert_eq!(row.cells.len(), 2);
        assert_eq!(row.cells[0].spans[0].text, "Termius");
        assert_eq!(row.cells[0].align, Alignment::Left);
        assert_eq!(row.cells[1].spans[0].text, "app");
        assert_eq!(row.cells[1].spans[0].slot, SlotToken::Badge);
        assert_eq!(row.cells[1].align, Alignment::Right);
        assert_eq!(
            row.constraints,
            vec![Constraint::Fill(1), Constraint::Length(4)]
        );
    }

    #[test]
    fn test_inject_badge_multi_line_anchors_to_row_0() {
        let json = r#"{
            "rows": [
                { "cells": [{"text": "Termius"}] },
                { "cells": [{"text": "SSH Client", "slot": "secondary"}] }
            ]
        }"#;
        let input: ItemDisplayInput = serde_json::from_str(json).unwrap();
        let mut display = NormalizedItemDisplay::from(input);
        display.inject_badge("app", SlotToken::Badge);

        assert_eq!(display.rows.len(), 2);
        // Row 0 has badge injected
        let row0 = &display.rows[0];
        assert_eq!(row0.cells.len(), 2);
        assert_eq!(row0.cells[0].spans[0].text, "Termius");
        assert_eq!(row0.cells[1].spans[0].text, "app");
        assert_eq!(row0.cells[1].align, Alignment::Right);
        assert_eq!(row0.cells[1].spans[0].slot, SlotToken::Badge);
        assert_eq!(
            row0.constraints,
            vec![Constraint::Fill(1), Constraint::Length(4)]
        );

        // Row 1 is untouched
        let row1 = &display.rows[1];
        assert_eq!(row1.cells.len(), 1);
        assert_eq!(row1.cells[0].spans[0].text, "SSH Client");
        assert_eq!(row1.cells[0].spans[0].slot, SlotToken::Secondary);
        assert_eq!(row1.constraints, vec![Constraint::Fill(1)]);
    }

    #[test]
    fn test_slot_token_deserialization_builtin_and_custom() {
        let token_badge: SlotToken = serde_json::from_str(r#""badge""#).unwrap();
        assert_eq!(token_badge, SlotToken::Badge);

        let token_primary: SlotToken = serde_json::from_str(r#""primary""#).unwrap();
        assert_eq!(token_primary, SlotToken::Primary);

        let token_custom: SlotToken = serde_json::from_str(r#""branch""#).unwrap();
        assert_eq!(token_custom, SlotToken::Custom("branch".to_string()));
        assert_eq!(token_custom.as_str(), "branch");

        let serialized = serde_json::to_string(&token_custom).unwrap();
        assert_eq!(serialized, r#""branch""#);
    }
}
