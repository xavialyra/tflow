use super::Item;
use crate::engine::{TaskCompletion, TaskHandle, TaskScheduler};
use crate::terminal::Terminal;
use crate::theme::Theme;
use anyhow::{Context, Result, bail};
use image::{DynamicImage, ImageReader};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui_image::{StatefulImage, protocol::StatefulProtocol};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone)]
pub(super) struct PickerPreviewConfig {
    layout: PickerLayout,
    blocks: Vec<PreviewBlockConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PickerLayout {
    #[serde(default = "default_direction")]
    direction: Direction,
    #[serde(default)]
    gap: u16,
    panes: Vec<PaneConfig>,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaneConfig {
    slot: String,
    #[serde(default)]
    size: Option<u16>,
    #[serde(default)]
    grow: Option<u16>,
    #[serde(default)]
    min: u16,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewSpec {
    blocks: Vec<PreviewBlockConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBlockConfig {
    #[serde(rename = "type")]
    kind: PreviewBlockKind,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    size: Option<u16>,
    #[serde(default)]
    grow: Option<u16>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PreviewBlockKind {
    Image,
    Text,
    Separator,
}

fn default_direction() -> Direction {
    Direction::Horizontal
}

pub(super) fn parse(
    layout: Option<Value>,
    preview: Option<Value>,
) -> Result<Option<PickerPreviewConfig>> {
    let (layout, preview) = match (layout, preview) {
        (None, None) => return Ok(None),
        (Some(layout), Some(preview)) => (layout, preview),
        _ => bail!("picker layout and preview must be configured together"),
    };
    let layout: PickerLayout =
        serde_json::from_value(layout).context("picker layout is invalid")?;
    let preview: PreviewSpec =
        serde_json::from_value(preview).context("picker preview is invalid")?;
    validate_layout(&layout)?;
    validate_blocks(&preview.blocks)?;
    Ok(Some(PickerPreviewConfig {
        layout,
        blocks: preview.blocks,
    }))
}

fn validate_layout(layout: &PickerLayout) -> Result<()> {
    if layout.panes.len() != 2 {
        bail!("picker layout must contain exactly items and preview panes");
    }
    let mut items = 0;
    let mut preview = 0;
    for pane in &layout.panes {
        match pane.slot.as_str() {
            "items" => items += 1,
            "preview" => preview += 1,
            _ => bail!("picker layout pane slot {:?} is unsupported", pane.slot),
        }
        if pane.size.is_some() == pane.grow.is_some() {
            bail!(
                "picker layout pane {:?} needs exactly one of size or grow",
                pane.slot
            );
        }
        if pane.size == Some(0) || pane.grow == Some(0) {
            bail!(
                "picker layout pane {:?} size and grow must be positive",
                pane.slot
            );
        }
    }
    if items != 1 || preview != 1 {
        bail!("picker layout must contain one items pane and one preview pane");
    }
    Ok(())
}

fn pane_lengths(
    length: u16,
    gap: u16,
    items: &PaneConfig,
    preview: &PaneConfig,
) -> Option<(u16, u16)> {
    let available = length.checked_sub(gap)?;
    let items_fixed = items.size.map(|size| size.max(items.min));
    let preview_fixed = preview.size.map(|size| size.max(preview.min));
    let fixed_total = items_fixed
        .unwrap_or(0)
        .saturating_add(preview_fixed.unwrap_or(0));
    let grow_minimum = if items_fixed.is_none() { items.min } else { 0 }.saturating_add(
        if preview_fixed.is_none() {
            preview.min
        } else {
            0
        },
    );
    if fixed_total.saturating_add(grow_minimum) > available {
        return None;
    }

    let remaining = available
        .saturating_sub(fixed_total)
        .saturating_sub(grow_minimum);
    let items_grow = items.grow.unwrap_or(0);
    let preview_grow = preview.grow.unwrap_or(0);
    let grow_total = items_grow.saturating_add(preview_grow).max(1);
    let items_extra = remaining.saturating_mul(items_grow) / grow_total;
    let preview_extra = remaining.saturating_sub(items_extra);
    let items_length = items_fixed.unwrap_or(items.min.saturating_add(items_extra));
    let preview_length = preview_fixed.unwrap_or(preview.min.saturating_add(preview_extra));
    Some((items_length, preview_length))
}

fn validate_blocks(blocks: &[PreviewBlockConfig]) -> Result<()> {
    if blocks.is_empty() {
        bail!("picker preview requires at least one block");
    }
    for block in blocks {
        if matches!(&block.kind, PreviewBlockKind::Separator) {
            if block.source.is_some() || block.grow.is_some() || block.size == Some(0) {
                bail!("picker preview separator accepts only an optional positive size");
            }
            continue;
        }
        let Some(source) = &block.source else {
            bail!("picker preview block requires a JSON Pointer source");
        };
        if !source.starts_with('/') {
            bail!("picker preview source {:?} must be a JSON Pointer", source);
        }
        if block.size.is_some() == block.grow.is_some()
            || block.size == Some(0)
            || block.grow == Some(0)
        {
            bail!("picker preview block needs exactly one positive size or grow");
        }
    }
    Ok(())
}

pub(super) struct PickerPreview {
    config: PickerPreviewConfig,
    visible: bool,
    revision: u64,
    selection: Option<String>,
    blocks: Vec<PreviewBlockState>,
    tasks: Vec<Option<TaskHandle<ImageResponse>>>,
}

enum PreviewBlockState {
    Empty,
    Text(String),
    Image {
        protocol: Option<Box<StatefulProtocol>>,
        error: Option<String>,
    },
}

struct ImageRequest {
    revision: u64,
    block: usize,
    path: PathBuf,
}

struct ImageResponse {
    revision: u64,
    block: usize,
    result: std::result::Result<DynamicImage, String>,
}

impl PickerPreview {
    pub(super) fn new(config: PickerPreviewConfig) -> Self {
        let count = config.blocks.len();
        Self {
            config,
            visible: true,
            revision: 0,
            selection: None,
            blocks: (0..count).map(|_| PreviewBlockState::Empty).collect(),
            tasks: (0..count).map(|_| None).collect(),
        }
    }

    pub(super) fn areas(&self, area: Rect) -> (Rect, Option<Rect>) {
        if !self.visible {
            return (area, None);
        }
        let items = self
            .config
            .layout
            .panes
            .iter()
            .find(|pane| pane.slot == "items")
            .expect("validated items pane");
        let preview = self
            .config
            .layout
            .panes
            .iter()
            .find(|pane| pane.slot == "preview")
            .expect("validated preview pane");
        let (items_length, preview_length) = match pane_lengths(
            match self.config.layout.direction {
                Direction::Horizontal => area.width,
                Direction::Vertical => area.height,
            },
            self.config.layout.gap,
            items,
            preview,
        ) {
            Some(lengths) => lengths,
            None => return (area, None),
        };
        match self.config.layout.direction {
            Direction::Horizontal => (
                Rect::new(area.x, area.y, items_length, area.height),
                Some(Rect::new(
                    area.x
                        .saturating_add(items_length)
                        .saturating_add(self.config.layout.gap),
                    area.y,
                    preview_length,
                    area.height,
                )),
            ),
            Direction::Vertical => (
                Rect::new(area.x, area.y, area.width, items_length),
                Some(Rect::new(
                    area.x,
                    area.y
                        .saturating_add(items_length)
                        .saturating_add(self.config.layout.gap),
                    area.width,
                    preview_length,
                )),
            ),
        }
    }

    pub(super) fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }

    pub(super) fn render_separator(
        &self,
        frame: &mut Frame,
        items: Rect,
        preview: Rect,
        theme: &Theme,
    ) {
        let area = match self.config.layout.direction {
            Direction::Horizontal => Rect::new(
                items.x.saturating_add(items.width),
                items.y,
                preview
                    .x
                    .saturating_sub(items.x.saturating_add(items.width)),
                items.height,
            ),
            Direction::Vertical => Rect::new(
                items.x,
                items.y.saturating_add(items.height),
                items.width,
                preview
                    .y
                    .saturating_sub(items.y.saturating_add(items.height)),
            ),
        };
        if area.width == 0 || area.height == 0 {
            return;
        }
        let borders = match self.config.layout.direction {
            Direction::Horizontal => Borders::LEFT,
            Direction::Vertical => Borders::TOP,
        };
        frame.render_widget(
            Block::new()
                .borders(borders)
                .border_style(theme.preview.border),
            area,
        );
    }

    pub(super) fn update(
        &mut self,
        item: Option<&Item>,
        config: &crate::config::Config,
        tasks: &TaskScheduler,
        terminal: &mut Terminal,
    ) {
        self.collect(terminal);
        let selection = item.and_then(|item| serde_json::to_string(&item_value(item)).ok());
        if selection == self.selection {
            return;
        }
        self.selection = selection;
        self.revision = self.revision.wrapping_add(1);
        self.blocks = (0..self.config.blocks.len())
            .map(|_| PreviewBlockState::Empty)
            .collect();
        self.tasks = (0..self.config.blocks.len()).map(|_| None).collect();
        let Some(item) = item else {
            return;
        };
        let value = item_value(item);
        for (index, block) in self.config.blocks.iter().enumerate() {
            if matches!(&block.kind, PreviewBlockKind::Separator) {
                continue;
            }
            let source = block
                .source
                .as_deref()
                .expect("validated preview block source");
            let Some(value) = value.pointer(source).filter(|value| !value.is_null()) else {
                continue;
            };
            match block.kind {
                PreviewBlockKind::Text => {
                    if let Some(text) = value.as_str() {
                        self.blocks[index] = PreviewBlockState::Text(text.to_string());
                    }
                }
                PreviewBlockKind::Image => {
                    let Some(path) = value.as_str() else {
                        continue;
                    };
                    let path = crate::engine::image_path::resolve(
                        config.plugin_root(&item.source_view),
                        path,
                    );
                    self.blocks[index] = PreviewBlockState::Image {
                        protocol: None,
                        error: None,
                    };
                    let request = ImageRequest {
                        revision: self.revision,
                        block: index,
                        path,
                    };
                    self.tasks[index] = Some(tasks.submit_keyed(
                        request,
                        format!("picker-preview:{}:{}", item.source_view, index),
                        |request, _, _| {
                            let result = ImageReader::open(&request.path)
                                .map_err(|error| error.to_string())
                                .and_then(|reader| {
                                    reader.decode().map_err(|error| error.to_string())
                                })
                                .map_err(|error| {
                                    format!("could not load {}: {}", request.path.display(), error)
                                });
                            ImageResponse {
                                revision: request.revision,
                                block: request.block,
                                result,
                            }
                        },
                    ));
                }
                PreviewBlockKind::Separator => {}
            }
        }
    }

    fn collect(&mut self, terminal: &mut Terminal) {
        for task in &mut self.tasks {
            let Some(mut handle) = task.take() else {
                continue;
            };
            match handle.try_recv() {
                Ok(response) if response.is_current() => match response.into_completion() {
                    TaskCompletion::Completed(response) if response.revision == self.revision => {
                        if let PreviewBlockState::Image { protocol, error } =
                            &mut self.blocks[response.block]
                        {
                            match response.result {
                                Ok(image) => {
                                    *protocol = Some(Box::new(
                                        terminal.image_picker().new_resize_protocol(image),
                                    ))
                                }
                                Err(message) => *error = Some(message),
                            }
                        }
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => *task = Some(handle),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
    }

    pub(super) fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let areas = block_areas(area, &self.config.blocks);
        for ((block, state), area) in self.config.blocks.iter().zip(&mut self.blocks).zip(areas) {
            if matches!(&block.kind, PreviewBlockKind::Separator) {
                frame.render_widget(
                    Block::new()
                        .borders(Borders::TOP)
                        .border_style(theme.preview.border),
                    area,
                );
                continue;
            }
            match state {
                PreviewBlockState::Empty => {}
                PreviewBlockState::Text(text) => frame.render_widget(
                    Paragraph::new(text.as_str())
                        .style(theme.preview.text)
                        .wrap(Wrap { trim: false }),
                    area,
                ),
                PreviewBlockState::Image {
                    protocol: Some(protocol),
                    ..
                } => {
                    frame.render_stateful_widget(StatefulImage::default(), area, protocol.as_mut())
                }
                PreviewBlockState::Image {
                    error: Some(error), ..
                } => frame.render_widget(
                    Paragraph::new(error.as_str())
                        .style(theme.preview.error)
                        .wrap(Wrap { trim: false }),
                    area,
                ),
                PreviewBlockState::Image { .. } => {}
            }
        }
    }
}

fn item_value(item: &Item) -> Value {
    serde_json::json!({
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
        "owner_view": item.source_view,
    })
}

fn block_areas(area: Rect, blocks: &[PreviewBlockConfig]) -> Vec<Rect> {
    let total_grow = blocks
        .iter()
        .map(|block| block.grow.unwrap_or(0))
        .sum::<u16>()
        .max(1);
    let fixed = blocks
        .iter()
        .map(|block| block_size(block).unwrap_or(0))
        .sum::<u16>();
    let remaining = area.height.saturating_sub(fixed);
    let mut y = area.y;
    blocks
        .iter()
        .map(|block| {
            let height = block_size(block)
                .unwrap_or_else(|| remaining.saturating_mul(block.grow.unwrap_or(0)) / total_grow);
            let rect = Rect::new(area.x, y, area.width, height);
            y = y.saturating_add(height);
            rect
        })
        .collect()
}

fn block_size(block: &PreviewBlockConfig) -> Option<u16> {
    if matches!(&block.kind, PreviewBlockKind::Separator) {
        Some(block.size.unwrap_or(1))
    } else {
        block.size
    }
}

#[cfg(test)]
mod tests {
    use super::{PickerPreview, PreviewBlockState, block_areas, parse};
    use crate::theme::Theme;
    use ratatui::Terminal as RatatuiTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use serde_json::json;

    #[test]
    fn requires_items_and_preview_panes() {
        assert!(
            parse(
                Some(json!({"panes": [{"slot": "items", "grow": 1}]})),
                Some(
                    json!({"blocks": [{"type": "text", "source": "/metadata/summary", "grow": 1}]})
                )
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_undocumented_json_blocks() {
        assert!(
            parse(
                Some(json!({
                    "panes": [
                        {"slot": "items", "grow": 1},
                        {"slot": "preview", "grow": 1}
                    ]
                })),
                Some(json!({
                    "blocks": [{"type": "json", "source": "/metadata" , "grow": 1}]
                })),
            )
            .is_err()
        );
    }

    #[test]
    fn separator_has_a_default_height_and_no_source() {
        let config = parse(
            Some(json!({
                "panes": [
                    {"slot": "items", "grow": 1},
                    {"slot": "preview", "grow": 1}
                ]
            })),
            Some(json!({"blocks": [{"type": "separator"}]})),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            block_areas(Rect::new(0, 0, 10, 5), &config.blocks)[0].height,
            1
        );
        assert!(
            parse(
                Some(json!({
                    "panes": [
                        {"slot": "items", "grow": 1},
                        {"slot": "preview", "grow": 1}
                    ]
                })),
                Some(json!({"blocks": [{"type": "separator", "grow": 1}]})),
            )
            .is_err()
        );
    }

    #[test]
    fn preview_text_uses_the_preview_text_binding() {
        let config = parse(
            Some(json!({
                "panes": [
                    {"slot": "items", "grow": 1},
                    {"slot": "preview", "grow": 1}
                ]
            })),
            Some(json!({"blocks": [{"type": "text", "source": "/summary", "grow": 1}]})),
        )
        .unwrap()
        .unwrap();
        let mut preview = PickerPreview::new(config);
        preview.blocks[0] = PreviewBlockState::Text("summary".to_string());
        let mut theme = Theme::terminal();
        theme.preview.text.fg = Some(Color::Magenta);
        theme.preview.text.bg = Some(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(12, 1)).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                preview.render(frame, area, &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn preview_errors_use_the_preview_error_binding() {
        let config = parse(
            Some(json!({
                "panes": [
                    {"slot": "items", "grow": 1},
                    {"slot": "preview", "grow": 1}
                ]
            })),
            Some(json!({"blocks": [{"type": "image", "source": "/image", "grow": 1}]})),
        )
        .unwrap()
        .unwrap();
        let mut preview = PickerPreview::new(config);
        preview.blocks[0] = PreviewBlockState::Image {
            protocol: None,
            error: Some("image failed".to_string()),
        };
        let mut theme = Theme::terminal();
        theme.preview.error.fg = Some(Color::Magenta);
        theme.preview.error.bg = Some(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(12, 1)).unwrap();

        terminal
            .draw(|frame| {
                let area = frame.area();
                preview.render(frame, area, &theme);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }

    #[test]
    fn fixed_items_pane_keeps_its_configured_width() {
        let config = parse(
            Some(json!({
                "gap": 1,
                "panes": [
                    {"slot": "items", "size": 30},
                    {"slot": "preview", "grow": 1}
                ]
            })),
            Some(json!({"blocks": [{"type": "text", "source": "/metadata/summary", "grow": 1}]})),
        )
        .unwrap()
        .unwrap();
        let preview = PickerPreview::new(config);
        let (items, preview_area) = preview.areas(Rect::new(0, 0, 80, 10));
        assert_eq!(items.width, 30);
        assert_eq!(preview_area.unwrap().width, 49);
    }

    #[test]
    fn fixed_panes_keep_both_configured_widths() {
        let config = parse(
            Some(json!({
                "panes": [
                    {"slot": "items", "size": 30},
                    {"slot": "preview", "size": 36}
                ]
            })),
            Some(json!({"blocks": [{"type": "text", "source": "/metadata/summary", "grow": 1}]})),
        )
        .unwrap()
        .unwrap();
        let preview = PickerPreview::new(config);
        let (items, preview_area) = preview.areas(Rect::new(0, 0, 80, 10));
        assert_eq!(items.width, 30);
        assert_eq!(preview_area.unwrap().width, 36);
    }

    #[test]
    fn hides_preview_when_its_minimum_width_does_not_fit() {
        let config = parse(
            Some(json!({
                "direction": "horizontal",
                "gap": 1,
                "panes": [
                    {"slot": "items", "grow": 1, "min": 28},
                    {"slot": "preview", "size": 36, "min": 24}
                ]
            })),
            Some(json!({"blocks": [{"type": "text", "source": "/metadata/summary", "grow": 1}]})),
        )
        .unwrap()
        .unwrap();
        let mut preview = PickerPreview::new(config);

        let (items, hidden) = preview.areas(Rect::new(0, 0, 52, 10));
        assert_eq!(items, Rect::new(0, 0, 52, 10));
        assert!(hidden.is_none());

        let (items, preview_area) = preview.areas(Rect::new(0, 0, 80, 10));
        assert_eq!(items.width, 43);
        assert_eq!(preview_area.unwrap(), Rect::new(44, 0, 36, 10));

        preview.toggle_visibility();
        let (items, hidden) = preview.areas(Rect::new(0, 0, 80, 10));
        assert_eq!(items, Rect::new(0, 0, 80, 10));
        assert!(hidden.is_none());
    }
}
