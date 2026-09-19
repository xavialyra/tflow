//! Bounded data-only preview documents. The host owns the outer pane and scrolling.
use super::super::display::{
    ConstraintInput, ItemDisplayInput, NormalizedItemDisplay, SlotToken, SpanInput,
};
use super::image_protocol::ImageProtocolKey;
use super::{Direction, ImageProtocolCache, PreviewImageState};
use anyhow::{Result, ensure};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(in crate::engine::picker) enum Document {
    Text(String),
    Node(Node),
}

#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(in crate::engine::picker) enum Node {
    Display {
        display: ItemDisplayInput,
        #[serde(default)]
        border: bool,
        #[serde(default)]
        title: Option<String>,
    },
    Paragraph {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        spans: Option<Vec<SpanInput>>,
        #[serde(default = "yes")]
        wrap: bool,
        #[serde(default)]
        slot: Option<SlotToken>,
        #[serde(default)]
        border: bool,
        #[serde(default)]
        title: Option<String>,
    },
    Image {
        path: String,
        #[serde(default)]
        border: bool,
        #[serde(default)]
        title: Option<String>,
    },
    Separator {},
    Layout {
        direction: Direction,
        #[serde(default)]
        constraints: Vec<ConstraintInput>,
        children: Vec<Document>,
        #[serde(default)]
        border: bool,
        #[serde(default)]
        title: Option<String>,
    },
}
fn yes() -> bool {
    true
}

/// Generic item details use literal text; metadata never selects a renderer or loads files.
pub(super) fn item_details(item: &Value) -> Document {
    fn value_text(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            value => serde_json::to_string_pretty(value).expect("JSON value is serializable"),
        }
    }

    let mut text = item["text"].as_str().unwrap_or_default().to_owned();
    if let Some(value) = item.get("value").filter(|value| !value.is_null()) {
        text.push_str("\n\nValue\n");
        text.push_str(&value_text(value));
    }
    const LIMIT: usize = 256 * 1024;
    const TRUNCATED: &str = "\n… (preview truncated)";
    if text.len() > LIMIT {
        let mut end = LIMIT - TRUNCATED.len();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(TRUNCATED);
    }
    Document::Text(text)
}

pub(super) fn parse(value: Value) -> Result<Option<Document>> {
    if value.is_null() {
        return Ok(None);
    }
    // Bound all JSON structure and strings before normalization or recursive rendering.
    fn budget(v: &Value, depth: usize, nodes: &mut usize, bytes: &mut usize) -> Result<()> {
        *nodes += 1;
        ensure!(
            depth <= 48 && *nodes <= 4096,
            "preview JSON exceeds depth/node limit"
        );
        match v {
            Value::String(s) => {
                *bytes += s.len();
                ensure!(*bytes <= 256 * 1024, "preview text exceeds 256 KiB");
            }
            Value::Array(a) => {
                for v in a {
                    budget(v, depth + 1, nodes, bytes)?;
                }
            }
            Value::Object(o) => {
                for v in o.values() {
                    budget(v, depth + 1, nodes, bytes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    budget(&value, 0, &mut 0, &mut 0)?;
    let doc: Document = serde_json::from_value(value)?;
    doc.validate(0, &mut 0, &mut 0)?;
    Ok(Some(doc))
}
fn constraints(cs: &[ConstraintInput], count: usize) -> Result<()> {
    ensure!(
        cs.is_empty() || cs.len() == count,
        "preview constraints must match children/cells"
    );
    for c in cs {
        ensure!(
            !matches!(c, ConstraintInput::Ratio(_, 0) | ConstraintInput::Fill(0)),
            "preview ratio denominator and fill must be positive"
        );
        if let ConstraintInput::Percentage(p) = c {
            ensure!(*p <= 100, "preview percentage exceeds 100");
        }
    }
    Ok(())
}
impl Document {
    fn validate(&self, depth: usize, nodes: &mut usize, images: &mut usize) -> Result<()> {
        *nodes += 1;
        ensure!(
            depth <= 16 && *nodes <= 128,
            "preview document exceeds depth 16 or 128 nodes"
        );
        match self {
            Self::Node(Node::Layout {
                constraints: cs,
                children,
                ..
            }) => {
                ensure!(!children.is_empty(), "preview layout needs children");
                constraints(cs, children.len())?;
                for child in children {
                    child.validate(depth + 1, nodes, images)?;
                }
            }
            Self::Node(Node::Display { display, .. }) => match display {
                ItemDisplayInput::SingleLine {
                    constraints: cs,
                    cells,
                } => constraints(cs, cells.len())?,
                ItemDisplayInput::MultiLine { rows } => {
                    for row in rows {
                        constraints(&row.constraints, row.cells.len())?;
                    }
                }
                _ => {}
            },
            Self::Node(Node::Paragraph { text, spans, .. }) => ensure!(
                text.is_some() != spans.is_some(),
                "preview paragraph requires exactly one of text or spans"
            ),
            Self::Node(Node::Image { path, .. }) => {
                *images += 1;
                ensure!(
                    *images <= 4 && !path.is_empty() && !path.contains('\0'),
                    "preview supports at most 4 images with nonempty paths"
                );
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn images(&self, paths: &mut Vec<String>) {
        match self {
            Self::Node(Node::Image { path, .. }) => paths.push(path.clone()),
            Self::Node(Node::Layout { children, .. }) => {
                for child in children {
                    child.images(paths);
                }
            }
            _ => {}
        }
    }
    fn decoration(&self) -> (bool, Option<&str>) {
        match self {
            Self::Node(
                Node::Display { border, title, .. }
                | Node::Paragraph { border, title, .. }
                | Node::Image { border, title, .. }
                | Node::Layout { border, title, .. },
            ) => (*border, title.as_deref()),
            _ => (false, None),
        }
    }
    fn text(
        &self,
        theme: Option<&crate::ui::theme::Theme>,
        package: &str,
    ) -> Option<(Text<'static>, bool)> {
        let (text, spans, wrap, slot) = match self {
            Self::Text(s) => (Some(s), None, true, None),
            Self::Node(Node::Paragraph {
                text,
                spans,
                wrap,
                slot,
                ..
            }) => (text.as_ref(), spans.as_ref(), *wrap, slot.clone()),
            _ => return None,
        };
        let mut lines = vec![Line::default()];
        let spans: Vec<(String, Option<SlotToken>)> = match spans {
            Some(spans) => spans
                .iter()
                .map(|span| match span {
                    SpanInput::Plain(text) => (text.clone(), None),
                    SpanInput::Detailed { text, slot } => (text.clone(), Some(slot.clone())),
                })
                .collect(),
            None => vec![(text.cloned().unwrap_or_default(), slot)],
        };
        for (text, slot) in spans {
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    lines.push(Line::default());
                }
                lines.last_mut().unwrap().spans.push(Span::styled(
                    crate::terminal::sanitize_terminal_text(part),
                    theme
                        .map(|theme| {
                            slot.as_ref()
                                .map(|slot| theme.resolve_slot(package, slot, false))
                                .unwrap_or(theme.picker.preview.text)
                        })
                        .unwrap_or_default(),
                ));
            }
        }
        Some((Text::from(lines), wrap))
    }
    fn height(&self, width: u16) -> u16 {
        let geometry = self.inner(Rect::new(0, 0, width, u16::MAX));
        let extra = u16::MAX - geometry.height;
        let w = geometry.width.max(1);
        let height = if let Some((text, wrap)) = self.text(None, "") {
            let mut p = Paragraph::new(text);
            if wrap {
                p = p.wrap(Wrap { trim: false });
            }
            p.line_count(w).min(16384) as u16
        } else {
            match self {
                Self::Node(Node::Display { display, .. }) => {
                    NormalizedItemDisplay::from(display.clone())
                        .rows
                        .len()
                        .min(16384) as u16
                }
                Self::Node(Node::Image { .. }) => 8,
                Self::Node(Node::Layout {
                    direction,
                    constraints,
                    children,
                    ..
                }) => {
                    if *direction == Direction::Vertical {
                        let mut fixed = 0u64;
                        let mut grow = 0u32;
                        let mut unit = 0u32;
                        for (i, child) in children.iter().enumerate() {
                            let height = u32::from(child.height(w));
                            match constraints
                                .get(i)
                                .copied()
                                .unwrap_or(ConstraintInput::Fill(1))
                            {
                                ConstraintInput::Length(n) => fixed += u64::from(n),
                                ConstraintInput::Fill(n) => {
                                    grow = grow.saturating_add(u32::from(n));
                                    unit = unit.max(height.div_ceil(u32::from(n).max(1)));
                                }
                                ConstraintInput::Min(n) => {
                                    fixed += u64::from(height.max(u32::from(n)))
                                }
                                ConstraintInput::Max(n) => {
                                    fixed += u64::from(height.min(u32::from(n)))
                                }
                                ConstraintInput::Percentage(n) => {
                                    fixed += u64::from(height)
                                        .saturating_mul(100)
                                        .checked_div(u64::from(n))
                                        .unwrap_or(0)
                                }
                                ConstraintInput::Ratio(n, d) => {
                                    fixed += u64::from(height)
                                        .saturating_mul(u64::from(d))
                                        .checked_div(u64::from(n))
                                        .unwrap_or(0)
                                }
                            }
                            // Child heights are capped and constraint values are at most u32,
                            // so each contribution fits u64. Cap the running sum as well.
                            fixed = fixed.min(16384);
                        }
                        fixed
                            .saturating_add(u64::from(grow.saturating_mul(unit)))
                            .min(16384) as u16
                    } else {
                        let cs = if constraints.is_empty() {
                            vec![Constraint::Fill(1); children.len()]
                        } else {
                            constraints.iter().copied().map(Into::into).collect()
                        };
                        let areas = Layout::horizontal(cs).split(Rect::new(0, 0, w, 1));
                        children
                            .iter()
                            .zip(areas.iter())
                            .map(|(c, a)| c.height(a.width))
                            .max()
                            .unwrap_or(0)
                    }
                }
                _ => 1,
            }
        };
        height.saturating_add(extra).min(16384)
    }
    pub(super) fn scroll_limit(&self, area: Rect) -> u16 {
        self.height(area.width).saturating_sub(area.height)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &crate::ui::theme::Theme,
        package: &str,
        scroll: u16,
        revision: u64,
        images: &[PreviewImageState],
        picker: Option<crate::terminal::ImagePicker>,
        protocols: &mut ImageProtocolCache,
    ) {
        let height = self.height(area.width).max(area.height);
        let scroll = scroll.min(height.saturating_sub(area.height));
        let virtual_area = Rect::new(0, 0, area.width, height);
        let mut leaves = Vec::new();
        self.place(virtual_area, &mut 0, &mut leaves);
        let viewport = Rect::new(0, scroll, area.width, area.height);
        let projected = |rect: Rect| {
            let r = rect.intersection(viewport);
            Rect::new(
                area.x.saturating_add(r.x),
                area.y.saturating_add(r.y.saturating_sub(scroll)),
                r.width,
                r.height,
            )
        };
        let mut desired = Vec::new();
        for (doc, rect, image_index) in &leaves {
            let visible = projected(*rect);
            if visible.is_empty() {
                continue;
            }
            let (border, title) = doc.decoration();
            let mut block = Block::new().border_style(theme.picker.preview.border);
            let original_inner = doc.inner(*rect);
            let inner = projected(original_inner);
            let top_visible = rect.y >= viewport.y;
            let bottom_visible = rect.bottom() <= viewport.bottom();
            if border {
                let mut borders = Borders::LEFT | Borders::RIGHT;
                if top_visible {
                    borders |= Borders::TOP;
                }
                if bottom_visible {
                    borders |= Borders::BOTTOM;
                }
                block = block.borders(borders);
            }
            if top_visible && let Some(title) = title {
                block = block.title(crate::terminal::sanitize_terminal_text(title));
            }
            frame.render_widget(block, visible);
            if let Some((text, wrap)) = doc.text(Some(theme), package) {
                let mut p = Paragraph::new(text)
                    .style(theme.picker.preview.text)
                    .scroll((scroll.saturating_sub(original_inner.y), 0));
                if wrap {
                    p = p.wrap(Wrap { trim: false });
                }
                frame.render_widget(p, inner);
            } else {
                match doc {
                    Self::Node(Node::Display { display, .. }) => {
                        let display = NormalizedItemDisplay::from(display.clone());
                        for (i, row) in display
                            .rows
                            .iter()
                            .skip(scroll.saturating_sub(original_inner.y) as usize)
                            .take(inner.height as usize)
                            .enumerate()
                        {
                            let cells = Layout::horizontal(row.constraints.clone())
                                .split(Rect::new(inner.x, inner.y + i as u16, inner.width, 1));
                            for (cell, area) in row.cells.iter().zip(cells.iter()) {
                                let spans = cell
                                    .spans
                                    .iter()
                                    .map(|s| {
                                        Span::styled(
                                            crate::terminal::sanitize_terminal_text(&s.text),
                                            theme.resolve_slot(package, &s.slot, false),
                                        )
                                    })
                                    .collect::<Vec<_>>();
                                frame.render_widget(
                                    Paragraph::new(Line::from(spans)).alignment(cell.align),
                                    *area,
                                );
                            }
                        }
                    }
                    Self::Node(Node::Separator {}) => frame.render_widget(
                        Block::new()
                            .borders(Borders::TOP)
                            .border_style(theme.picker.preview.border),
                        inner,
                    ),
                    Self::Node(Node::Image { .. }) => {
                        if let Some(index) = image_index {
                            match images.get(*index) {
                                Some(PreviewImageState {
                                    image: Some(image), ..
                                }) => {
                                    if let Some(picker) = picker.filter(|_| !inner.is_empty()) {
                                        let key = ImageProtocolKey::new(
                                            revision,
                                            *index,
                                            image,
                                            ratatui::layout::Size::new(inner.width, inner.height),
                                            picker,
                                        );
                                        desired.push(super::image_protocol::DesiredImageProtocol {
                                            key,
                                            image: image.clone(),
                                            picker,
                                        });
                                    }
                                }
                                Some(PreviewImageState {
                                    error: Some(error), ..
                                }) => frame.render_widget(
                                    Paragraph::new(error.as_str())
                                        .style(theme.picker.preview.error)
                                        .wrap(Wrap { trim: false }),
                                    inner,
                                ),
                                _ => frame.render_widget(
                                    Paragraph::new("Loading image…")
                                        .style(theme.picker.preview.text),
                                    inner,
                                ),
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        protocols.update(desired);
        // Encoding is asynchronous; draw only protocols ready in the cache.
        for (doc, rect, index) in leaves {
            let Some(index) = index else {
                continue;
            };
            let inner = projected(doc.inner(rect));
            if inner.is_empty() {
                continue;
            }
            if let (
                Some(picker),
                Some(PreviewImageState {
                    image: Some(image), ..
                }),
            ) = (picker, images.get(index))
            {
                let key = ImageProtocolKey::new(
                    revision,
                    index,
                    image,
                    ratatui::layout::Size::new(inner.width, inner.height),
                    picker,
                );
                if let Some(protocol) = protocols.protocol(key) {
                    frame.render_stateful_widget(
                        ratatui_image::StatefulImage::default(),
                        inner,
                        protocol,
                    );
                } else if let Some(error) = protocols.error(key) {
                    frame.render_widget(
                        Paragraph::new(error).style(theme.picker.preview.error),
                        inner,
                    );
                }
            }
        }
    }
    fn inner(&self, area: Rect) -> Rect {
        let (border, title) = self.decoration();
        let mut block = Block::new();
        if border {
            block = block.borders(Borders::ALL);
        }
        if let Some(title) = title {
            block = block.title(title);
        }
        block.inner(area)
    }
    fn place<'a>(
        &'a self,
        area: Rect,
        image: &mut usize,
        leaves: &mut Vec<(&'a Document, Rect, Option<usize>)>,
    ) {
        let index = if matches!(self, Self::Node(Node::Image { .. })) {
            let i = *image;
            *image += 1;
            Some(i)
        } else {
            None
        };
        leaves.push((self, area, index));
        if let Self::Node(Node::Layout {
            direction,
            constraints,
            children,
            border,
            ..
        }) = self
        {
            let _ = border;
            let area = self.inner(area);
            let constraints = if constraints.is_empty() {
                vec![Constraint::Fill(1); children.len()]
            } else {
                constraints.iter().copied().map(Into::into).collect()
            };
            let areas = Layout::default()
                .direction(if *direction == Direction::Horizontal {
                    ratatui::layout::Direction::Horizontal
                } else {
                    ratatui::layout::Direction::Vertical
                })
                .constraints(constraints)
                .split(area);
            for (child, area) in children.iter().zip(areas.iter()) {
                child.place(*area, image, leaves);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn item_details_preserve_multiline_values_and_bound_unicode_text() {
        let doc = item_details(&json!({
            "text": "Item title", "value": "item-value", "source_view": "private:source",
            "metadata": {"body": "first line\nsecond line", "nested": {"count": 2}, "image": "/missing.png"}
        }));
        let Document::Text(text) = &doc else {
            panic!("expected literal details")
        };
        assert!(text.contains("Item title") && text.contains("item-value"));
        assert!(!text.contains("first line\nsecond line"));
        assert!(!text.contains("Metadata"));
        assert!(!text.contains("private:source"));
        let mut images = Vec::new();
        doc.images(&mut images);
        assert!(images.is_empty());
        let Document::Text(text) = item_details(&json!({
            "text": "Large item", "value": "界".repeat(100_000), "metadata": {"body": "ignored"}
        })) else {
            panic!("expected literal details")
        };
        assert!(text.len() <= 256 * 1024);
        assert!(text.ends_with("… (preview truncated)"));
        assert!(parse(json!(text)).is_ok());
        let Document::Text(text) = item_details(&json!({"text": "Title only", "metadata": {}}))
        else {
            panic!("expected literal details")
        };
        assert_eq!(text, "Title only");
    }

    #[test]
    fn rejects_unknown_fields_bad_constraints_and_limits() {
        for value in [
            json!({"type":"paragraph","text":"hi","action":"run"}),
            json!({"type":"paragraph","text":"hi","spans":[]}),
            json!({"type":"display","display":{"cells":[{"text":"hi","color":"red"}]}}),
            json!({"type":"display","display":{"cells":[{"text":"hi"}],"constraints":[{"Ratio":[1,0]}]}}),
            json!({"type":"layout","direction":"vertical","constraints":[{"Percentage":101}],"children":["hi"]}),
            json!({"type":"layout","direction":"vertical","constraints":[{"Fill":1}],"children":["a","b"]}),
            json!({"type":"layout","direction":"vertical","children":(0..5).map(|_| json!({"type":"image","path":"image.png"})).collect::<Vec<_>>()}),
            json!("x".repeat(256 * 1024 + 1)),
        ] {
            assert!(parse(value.clone()).is_err(), "accepted {value}");
        }
        let mut deep = json!("end");
        for _ in 0..18 {
            deep = json!({"type":"layout","direction":"vertical","children":[deep]});
        }
        assert!(parse(deep).is_err());
        assert!(
            parse(json!({"type":"layout","direction":"horizontal","children":vec!["x";128]}))
                .is_err()
        );
    }
    #[test]
    fn styled_nested_content_and_image_coexist_in_a_small_viewport() {
        let document = parse(json!({"type":"layout","direction":"vertical", "constraints":[{"Length":2},{"Length":1},{"Fill":1}], "children":[
            {"type":"display","display":{"rows":[{"cells":[{"text":"Title","slot":"accent"}]},{"cells":[{"spans":["left",{"text":" right","slot":"success"}]}]}]}},
            {"type":"separator"},
            {"type":"layout","direction":"horizontal","constraints":[{"Length":9},{"Fill":1}],"children":[
                {"type":"paragraph","spans":[{"text":"long rich text wraps here","slot":"warning"}]},
                {"type":"image","path":"art.png"}
            ]}
        ]})).unwrap().unwrap();
        let theme = crate::ui::theme::Theme::terminal();
        let images = vec![PreviewImageState {
            image: Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(2, 2))),
            error: None,
        }];
        let mut protocols = ImageProtocolCache::new();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 8)).unwrap();
        terminal
            .draw(|frame| {
                document.render(
                    frame,
                    frame.area(),
                    &theme,
                    "demo",
                    0,
                    1,
                    &images,
                    Some(crate::terminal::ImagePicker::test_halfblocks()),
                    &mut protocols,
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "T");
        assert_eq!(
            buffer[(0, 0)].fg,
            theme
                .resolve_slot("demo", &SlotToken::Accent, false)
                .fg
                .unwrap_or_default()
        );
        assert_eq!(buffer[(0, 3)].symbol(), "l");
        assert_eq!(buffer[(0, 4)].symbol(), "t");
        let mut paths = Vec::new();
        document.images(&mut paths);
        assert_eq!(paths, ["art.png"]);
        // Degenerate panes and very large scroll offsets must remain safe.
        terminal
            .draw(|frame| {
                document.render(
                    frame,
                    Rect::new(0, 0, 0, 0),
                    &theme,
                    "demo",
                    u16::MAX,
                    2,
                    &images,
                    None,
                    &mut protocols,
                )
            })
            .unwrap();
    }
    #[test]
    fn workflow_slots_and_extreme_valid_constraints_render_without_overflow() {
        let mut theme = crate::ui::theme::Theme::terminal();
        let style = crate::ui::theme::RawStyleBinding {
            bold: Some(true),
            ..Default::default()
        };
        theme
            .register_workflow_defaults(
                "library",
                &std::collections::BTreeMap::from([("owner".into(), style)]),
            )
            .unwrap();
        let doc = parse(json!({"type":"paragraph", "spans":[{"text":"Owned", "slot":"owner"}]}))
            .unwrap()
            .unwrap();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(10, 2)).unwrap();
        terminal
            .draw(|frame| {
                doc.render(
                    frame,
                    frame.area(),
                    &theme,
                    "library",
                    0,
                    1,
                    &[],
                    None,
                    &mut ImageProtocolCache::new(),
                )
            })
            .unwrap();
        assert!(
            terminal.backend().buffer()[(0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        terminal
            .draw(|frame| {
                doc.render(
                    frame,
                    frame.area(),
                    &theme,
                    "browser",
                    0,
                    1,
                    &[],
                    None,
                    &mut ImageProtocolCache::new(),
                )
            })
            .unwrap();
        assert!(
            !terminal.backend().buffer()[(0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        let doc = parse(json!({"type":"layout", "direction":"vertical", "constraints":[{"Ratio":[1,4294967295u32]}, {"Ratio":[1,4294967295u32]}], "children":["first\nsecond", "third\nfourth"]})).unwrap().unwrap();
        terminal
            .draw(|frame| {
                doc.render(
                    frame,
                    frame.area(),
                    &theme,
                    "",
                    16384,
                    1,
                    &[],
                    None,
                    &mut ImageProtocolCache::new(),
                )
            })
            .unwrap();
    }

    fn draw_document(
        value: Value,
        width: u16,
        height: u16,
        scroll: u16,
    ) -> ratatui::buffer::Buffer {
        let doc = parse(value).unwrap().unwrap();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                doc.render(
                    frame,
                    frame.area(),
                    &crate::ui::theme::Theme::terminal(),
                    "",
                    scroll,
                    0,
                    &[],
                    None,
                    &mut ImageProtocolCache::new(),
                )
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn scrolling_projects_original_borders_titles_and_inner_content() {
        let doc =
            json!({"type":"paragraph", "text":"a\nb\nc\nd\ne", "border":true, "title":"Title"});
        let top = draw_document(doc.clone(), 10, 3, 0);
        assert_eq!(top[(1, 0)].symbol(), "T");
        assert_eq!(top[(1, 1)].symbol(), "a");
        assert_eq!(top[(1, 2)].symbol(), "b"); // no artificial bottom border
        let middle = draw_document(doc.clone(), 10, 3, 2);
        assert_eq!(middle[(0, 0)].symbol(), "│");
        assert_eq!(middle[(1, 0)].symbol(), "b"); // inner starts one row below original top
        assert_eq!(middle[(1, 2)].symbol(), "d");
        let bottom = draw_document(doc, 10, 3, 99);
        assert_eq!(bottom[(1, 0)].symbol(), "d");
        assert_eq!(bottom[(1, 1)].symbol(), "e");
        assert_eq!(bottom[(0, 2)].symbol(), "└");
        let nested = draw_document(
            json!({"type":"layout", "direction":"vertical", "border":true, "title":"Parent", "children":[{"type":"display", "display":{"rows":[{"cells":[{"text":"a"}]},{"cells":[{"text":"b"}]},{"cells":[{"text":"c"}]},{"cells":[{"text":"d"}]}]}}]}),
            10,
            3,
            1,
        );
        assert_eq!(nested[(0, 0)].symbol(), "│");
        assert_eq!(nested[(1, 0)].symbol(), "a");
        assert_eq!(nested[(1, 2)].symbol(), "c");
        let display = draw_document(
            json!({"type":"display", "border":true, "display":{"rows":[{"cells":[{"text":"a"}]},{"cells":[{"text":"b"}]},{"cells":[{"text":"c"}]}]}}),
            10,
            2,
            1,
        );
        assert_eq!(display[(1, 0)].symbol(), "a");
        assert_eq!(display[(1, 1)].symbol(), "b");
    }

    #[test]
    fn asymmetric_nested_widths_measure_and_scroll_the_complete_narrow_paragraph() {
        let value = json!({
            "type": "layout",
            "direction": "vertical",
            "constraints": [{"Length": 1}, {"Fill": 1}],
            "children": [
                "Heading",
                {
                    "type": "layout",
                    "direction": "horizontal",
                    "constraints": [{"Length": 1}, {"Fill": 1}],
                    "children": [
                        {"type": "layout", "direction": "vertical", "children": [
                            {"type": "paragraph", "text": "abcdefghijklmnopqrstuvwxyz"}
                        ]},
                        "Wide sibling"
                    ]
                }
            ]
        });
        let doc = parse(value.clone()).unwrap().unwrap();
        for width in [10, 40] {
            assert_eq!(doc.height(width), 27);
            assert_eq!(doc.scroll_limit(Rect::new(0, 0, width, 4)), 23);
            let mut placements = Vec::new();
            doc.place(
                Rect::new(0, 0, width, doc.height(width)),
                &mut 0,
                &mut placements,
            );
            let (_, paragraph_area, _) = placements
                .iter()
                .find(|(node, _, _)| matches!(node, Document::Node(Node::Paragraph { .. })))
                .unwrap();
            assert_eq!(*paragraph_area, Rect::new(0, 1, 1, 26));
            let bottom = draw_document(value.clone(), width, 4, u16::MAX);
            let last_lines = (0..4)
                .map(|row| bottom[(0, row)].symbol())
                .collect::<String>();
            assert_eq!(last_lines, "wxyz");
        }
    }

    #[test]
    fn reviewed_ratio_border_and_title_reproductions_remain_renderable() {
        let ratio = json!({
            "type":"layout", "direction":"vertical",
            "constraints":[{"Ratio":[1,4294967295u32]},{"Length":1}],
            "children":["A","B"]
        });
        assert_eq!(parse(ratio.clone()).unwrap().unwrap().height(8), 16384);
        let _ = draw_document(ratio, 8, 3, u16::MAX);

        let bordered = json!({
            "type":"layout", "direction":"vertical", "border":true, "title":"T",
            "children":["A\nB\nC\nD"]
        });
        let top = draw_document(bordered.clone(), 8, 3, 0);
        assert_eq!(top[(1, 0)].symbol(), "T");
        assert_eq!(top[(1, 1)].symbol(), "A");
        assert_eq!(top[(1, 2)].symbol(), "B");
        let middle = draw_document(bordered.clone(), 8, 3, 1);
        assert_eq!(middle[(0, 0)].symbol(), "│");
        assert_eq!(middle[(1, 0)].symbol(), "A");
        assert_eq!(middle[(1, 2)].symbol(), "C");
        let bottom = draw_document(bordered, 8, 3, u16::MAX);
        assert_eq!(bottom[(1, 0)].symbol(), "C");
        assert_eq!(bottom[(1, 1)].symbol(), "D");
        assert_eq!(bottom[(0, 2)].symbol(), "└");

        let titled = json!({"type":"paragraph", "title":"T", "text":"abcd"});
        assert_eq!(
            parse(titled.clone())
                .unwrap()
                .unwrap()
                .scroll_limit(Rect::new(0, 0, 4, 2)),
            0
        );
        let screen = draw_document(titled, 4, 2, u16::MAX);
        assert_eq!(screen[(0, 0)].symbol(), "T");
        assert_eq!(
            (0..4).map(|x| screen[(x, 1)].symbol()).collect::<String>(),
            "abcd"
        );
    }

    #[test]
    fn title_only_height_preserves_full_width() {
        let doc = parse(json!({"type":"paragraph", "text":"12345678", "title":"Title"}))
            .unwrap()
            .unwrap();
        assert_eq!(doc.height(8), 2);
        assert_eq!(doc.inner(Rect::new(0, 0, 8, 2)), Rect::new(0, 1, 8, 1));
    }

    #[test]
    fn plain_text_uses_preview_style_and_sanitizes_controls_preserving_newlines() {
        use ratatui::style::{Color, Style};
        let mut theme = crate::ui::theme::Theme::terminal();
        theme.picker.preview.text = Style::default().fg(Color::Magenta).bg(Color::Blue);
        for value in [
            json!("a\u{1b}[31mb\u{1b}[0m\nnext\u{7}"),
            json!({"type":"paragraph", "text":"ab\nnext"}),
            json!({"type":"paragraph", "spans":["ab\nnext"]}),
            json!({"type":"paragraph", "spans":["a\u{1b}[31mb\u{1b}[0m\n", "next\u{7}"]}),
        ] {
            let doc = parse(value).unwrap().unwrap();
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 3)).unwrap();
            terminal
                .draw(|frame| {
                    doc.render(
                        frame,
                        frame.area(),
                        &theme,
                        "",
                        0,
                        0,
                        &[],
                        None,
                        &mut ImageProtocolCache::new(),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(0, 0)].symbol(), "a");
            assert_eq!(buffer[(1, 0)].symbol(), "b");
            assert_eq!(buffer[(0, 1)].symbol(), "n");
            assert_eq!(buffer[(0, 0)].fg, Color::Magenta);
            assert_eq!(buffer[(7, 2)].bg, Color::Blue);
        }
        let rich = parse(json!({"type":"paragraph", "spans":[
            {"text":"a\t\u{1b}]0;ignored\u{7}b\nnext\u{1b}[31m!\u{1b}[0m", "slot":"accent"}
        ]}))
        .unwrap()
        .unwrap();
        let (text, _) = rich.text(Some(&theme), "").unwrap();
        assert_eq!(text.lines.len(), 2);
        assert_eq!(text.lines[0].spans[0].content, "a b");
        assert_eq!(text.lines[1].spans[0].content, "next!");
        assert_eq!(
            text.lines[0].spans[0].style,
            theme.resolve_slot("", &SlotToken::Accent, false)
        );
        for value in [
            json!({"type":"paragraph", "text":"explicit", "slot":"primary"}),
            json!({"type":"paragraph", "spans":[{"text":"explicit"}]}),
        ] {
            let doc = parse(value).unwrap().unwrap();
            let (text, _) = doc.text(Some(&theme), "").unwrap();
            assert_eq!(
                text.lines[0].spans[0].style,
                theme.resolve_slot("", &SlotToken::Primary, false)
            );
        }
    }

    #[test]
    fn paragraph_scroll_is_clamped_to_the_last_wrapped_line() {
        let doc = parse(json!("first\nsecond\nthird")).unwrap().unwrap();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 1)).unwrap();
        terminal
            .draw(|frame| {
                doc.render(
                    frame,
                    frame.area(),
                    &crate::ui::theme::Theme::terminal(),
                    "",
                    99,
                    0,
                    &[],
                    None,
                    &mut ImageProtocolCache::new(),
                )
            })
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(0, 0)].symbol(), "t");
    }
}
