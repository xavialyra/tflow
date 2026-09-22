mod document;
mod image_decode;
mod image_path;
mod image_protocol;

use self::image_decode::ImageDecodeHandle;
pub(super) use self::image_protocol::ImageProtocolCache;
use super::Item;
use crate::ui::theme::Theme;
use anyhow::{Context, Result, bail};
use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PickerPreviewConfig {
    layout: PickerLayout,
    pub(super) source: PreviewSource,
}

const DEBOUNCE_DURATION: std::time::Duration = std::time::Duration::from_millis(80);
const GRACE_PERIOD_DURATION: std::time::Duration = std::time::Duration::from_millis(200);
const DECODE_CACHE_CAPACITY: usize = 32;
const DOCUMENT_CACHE_CAPACITY: usize = 32;

/// Provider-keyed cache of fully rendered preview documents, shared by every
/// Picker View instance of one session. A fresh mount whose preview provider was
/// rendered before paints the cached document immediately instead of flashing
/// `Loading preview…` while the script re-runs. The refresh still happens in the
/// background, so the cached pixels are replaced as soon as new output arrives.
///
/// The key is the provider owner, not the full request identity, because a
/// self-navigation (`replace = true` to the same View) exists precisely to
/// update parameters such as a Picker's selected set; those parameters are part
/// of the request identity, so an identity key would always miss the remount it
/// is meant to cover.
#[derive(Clone, Default)]
pub(crate) struct PreviewDocumentCache {
    inner: Arc<std::sync::Mutex<PreviewDocumentCacheInner>>,
}

#[derive(Default)]
struct PreviewDocumentCacheInner {
    entries: std::collections::HashMap<String, CachedPreview>,
    order: std::collections::VecDeque<String>,
}

#[derive(Clone)]
struct CachedPreview {
    document: Option<document::Document>,
}

impl PreviewDocumentCache {
    fn get(&self, owner: &str) -> Option<CachedPreview> {
        let mut inner = self.lock();
        let cached = inner.entries.get(owner).cloned()?;
        if let Some(position) = inner.order.iter().position(|key| key == owner) {
            inner.order.remove(position);
        }
        inner.order.push_back(owner.to_owned());
        Some(cached)
    }

    fn insert(&self, owner: String, document: Option<document::Document>) {
        let mut inner = self.lock();
        if inner.entries.contains_key(&owner) {
            if let Some(position) = inner.order.iter().position(|key| key == &owner) {
                inner.order.remove(position);
            }
        } else {
            while inner.entries.len() >= DOCUMENT_CACHE_CAPACITY {
                let Some(oldest) = inner.order.pop_front() else {
                    break;
                };
                inner.entries.remove(&oldest);
            }
        }
        inner.order.push_back(owner.clone());
        inner.entries.insert(owner, CachedPreview { document });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PreviewDocumentCacheInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PickerLayout {
    #[serde(default = "default_direction")]
    direction: Direction,
    #[serde(default)]
    gap: u16,
    panes: Vec<PaneConfig>,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
    Horizontal,
    Vertical,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaneConfig {
    slot: String,
    #[serde(default)]
    size: Option<u16>,
    #[serde(default)]
    grow: Option<u16>,
    #[serde(default)]
    min: u16,
}

#[derive(Clone)]
pub(super) enum PreviewSource {
    Inherit,
    Details,
    Declared(Option<document::Document>),
    Script(crate::workflow::config::ResolvedScriptSource),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewSpec {
    #[serde(default)]
    producer: Option<crate::workflow::config::ProducerKind>,
    #[serde(default)]
    inherit: bool,
    #[serde(default)]
    handler: Option<Value>,
    #[serde(default)]
    document: Option<Value>,
}

pub(super) fn parse_source(value: Value, root: Option<&std::path::Path>) -> Result<PreviewSource> {
    let source = parse_source_shape(value)?;
    if let PreviewSource::Script(script) = &source {
        script.validate_target(root)?;
    }
    Ok(source)
}

fn parse_source_shape(value: Value) -> Result<PreviewSource> {
    let spec: PreviewSpec = serde_json::from_value(value).context("picker preview is invalid")?;
    use crate::workflow::config::ProducerKind;
    match (spec.producer, spec.inherit, spec.handler, spec.document) {
        (None, true, None, None) => Ok(PreviewSource::Inherit),
        (Some(ProducerKind::Declared), false, None, Some(value)) => {
            Ok(PreviewSource::Declared(document::parse(value)?))
        }
        (Some(ProducerKind::Script), false, Some(handler), None) => {
            let handler = toml::Value::try_from(&handler)?;
            Ok(PreviewSource::Script(
                crate::workflow::config::parse_producer_script_handler_shape(&handler)?,
            ))
        }
        _ => bail!(
            "preview requires inherit=true, producer='declared' with document, or producer='script' with handler"
        ),
    }
}

#[derive(Clone)]
pub(super) struct PreviewRequest {
    pub(super) identity: String,
    pub(super) owner: String,
    pub(super) request: Value,
    pub(super) root: Option<std::path::PathBuf>,
    pub(super) source: PreviewSource,
}

fn default_direction() -> Direction {
    Direction::Horizontal
}

pub(super) fn parse(
    preview_ratio: f64,
    preview_min_width: u16,
    preview: Option<Value>,
) -> Result<PickerPreviewConfig> {
    anyhow::ensure!(
        (0.0..=1.0).contains(&preview_ratio),
        "picker preview_ratio must be between 0 and 1"
    );
    let layout = serde_json::json!({"gap": 1, "panes": [
        {"slot": "items", "grow": (((1.0 - preview_ratio) * 100.0).round() as u16).max(1)},
        {"slot": "preview", "grow": ((preview_ratio * 100.0).round() as u16).max(1), "min": preview_min_width}]});
    let layout: PickerLayout =
        serde_json::from_value(layout).context("picker layout is invalid")?;
    validate_layout(&layout)?;
    let source = match preview {
        Some(value) => parse_source_shape(value)?,
        None => PreviewSource::Inherit,
    };
    Ok(PickerPreviewConfig { layout, source })
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

#[derive(Clone)]
pub(crate) struct PickerPreviewRenderState {
    pub(crate) config: PickerPreviewConfig,
    pub(crate) visible: bool,
    pub(crate) revision: u64,
    images: Vec<PreviewImageState>,
    document: Option<document::Document>,
    status: Option<String>,
    error: bool,
    package: String,
    scroll: u16,
}

pub(super) struct PickerPreview {
    config: PickerPreviewConfig,
    visible: bool,
    revision: u64,
    selection: Option<String>,
    images: Vec<PreviewImageState>,
    task: Option<ImageTask>,
    pending_images: Vec<(usize, std::path::PathBuf)>,
    prepared: Option<PreviewRequest>,
    script_task: Option<crate::task::TaskHandle<Option<document::Document>>>,
    due: Option<std::time::Instant>,
    grace_due: Option<std::time::Instant>,
    document: Option<document::Document>,
    status: Option<String>,
    error: bool,
    package: String,
    scroll: u16,
    pool: Option<Arc<image_decode::ImageDecodePool>>,
    content_size: Option<(u16, u16)>,
    decode_cache: std::collections::HashMap<std::path::PathBuf, Arc<DynamicImage>>,
    decode_order: std::collections::VecDeque<std::path::PathBuf>,
    preview_cache: PreviewDocumentCache,
}

#[derive(Clone, Default)]
struct PreviewImageState {
    image: Option<Arc<DynamicImage>>,
    error: Option<String>,
}

struct ImageTask {
    handle: ImageDecodeHandle,
}

impl PickerPreviewRenderState {
    pub(super) fn areas(&self, area: Rect) -> (Rect, Option<Rect>) {
        preview_areas(&self.config, self.visible, area)
    }

    pub(super) fn render_separator(
        &self,
        frame: &mut Frame,
        items: Rect,
        preview: Rect,
        theme: &Theme,
    ) {
        render_separator(self.config.layout.direction, frame, items, preview, theme);
    }

    pub(super) fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        picker: Option<crate::terminal::ImagePicker>,
        protocols: &mut ImageProtocolCache,
    ) {
        if let Some(document) = &self.document {
            document.render(
                frame,
                area,
                theme,
                &self.package,
                self.scroll,
                self.revision,
                &self.images,
                picker,
                protocols,
            );
            return;
        }
        if let Some(status) = &self.status {
            protocols.update(Vec::new());
            frame.render_widget(
                Paragraph::new(status.as_str())
                    .style(if self.error {
                        theme.picker.preview.error
                    } else {
                        theme.picker.preview.text
                    })
                    .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        protocols.update(Vec::new());
    }
}

impl PickerPreview {
    pub(super) fn new(config: PickerPreviewConfig, preview_cache: PreviewDocumentCache) -> Self {
        Self {
            config,
            visible: false,
            revision: 0,
            selection: None,
            images: Vec::new(),
            task: None,
            pending_images: Vec::new(),
            prepared: None,
            script_task: None,
            due: None,
            grace_due: None,
            document: None,
            status: None,
            error: false,
            package: String::new(),
            scroll: 0,
            pool: None,
            content_size: None,
            decode_cache: std::collections::HashMap::new(),
            decode_order: std::collections::VecDeque::new(),
            preview_cache,
        }
    }

    pub(super) fn set_visible(&mut self, visible: bool) {
        if self.visible && !visible {
            self.reset_selection();
        }
        self.visible = visible;
    }

    pub(super) fn deactivate(&mut self) {
        self.reset_selection();
        self.pool.take();
        self.decode_cache.clear();
        self.decode_order.clear();
    }

    /// Stop accepting new preview work while the Picker is covered without
    /// discarding what is already rendered. Reactivation resumes any pending
    /// load and keeps the current document, so the return trip does not flash
    /// a loading state.
    pub(super) fn suspend(&mut self) {
        self.decode_cache.clear();
        self.decode_order.clear();
    }

    fn get_cached_image(&mut self, path: &std::path::Path) -> Option<Arc<DynamicImage>> {
        if let Some(image) = self.decode_cache.get(path) {
            let image = Arc::clone(image);
            if let Some(pos) = self.decode_order.iter().position(|p| p == path) {
                self.decode_order.remove(pos);
            }
            self.decode_order.push_back(path.to_path_buf());
            Some(image)
        } else {
            None
        }
    }

    fn cache_decoded_image(&mut self, path: std::path::PathBuf, image: Arc<DynamicImage>) {
        if self.decode_cache.contains_key(&path) {
            if let Some(pos) = self.decode_order.iter().position(|p| p == &path) {
                self.decode_order.remove(pos);
            }
            self.decode_order.push_back(path.clone());
            self.decode_cache.insert(path, image);
            return;
        }
        while self.decode_cache.len() >= DECODE_CACHE_CAPACITY {
            if let Some(oldest) = self.decode_order.pop_front() {
                self.decode_cache.remove(&oldest);
            } else {
                break;
            }
        }
        self.decode_order.push_back(path.clone());
        self.decode_cache.insert(path, image);
    }

    #[cfg(test)]
    pub(super) fn has_pending_task(&self) -> bool {
        self.task.is_some()
    }

    fn reset_selection(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.selection = None;
        self.task.take();
        self.pending_images.clear();
        self.prepared = None;
        self.script_task = None;
        self.due = None;
        self.grace_due = None;
        self.document = None;
        self.status = None;
        self.error = false;
        self.scroll = 0;
        self.images.clear();
    }

    fn cancel_pending(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.task.take();
        self.pending_images.clear();
        self.script_task = None;
        self.due = None;
        self.grace_due = None;
        self.error = false;
    }

    pub(super) fn fits(&self, size: (u16, u16)) -> bool {
        preview_areas(&self.config, true, Rect::new(0, 0, size.0, size.1))
            .1
            .is_some_and(|area| !area.is_empty())
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn document_scroll_state(&self) -> (bool, u16) {
        (self.document.is_some(), self.scroll)
    }

    #[cfg(test)]
    pub(super) fn prepared_request(&self) -> Option<&PreviewRequest> {
        self.prepared.as_ref()
    }

    pub(super) fn source(&self) -> &PreviewSource {
        &self.config.source
    }

    pub(super) fn hold_for_items_refresh(&mut self) {
        if self.selection.is_none() && self.prepared.is_none() && self.grace_due.is_some() {
            return;
        }
        self.cancel_pending();
        self.selection = None;
        self.prepared = None;
        self.grace_due = Some(std::time::Instant::now() + GRACE_PERIOD_DURATION);
        if self.document.is_none() {
            self.status = Some("Loading preview…".into());
        }
    }

    pub(super) fn prepare(&mut self, request: Option<PreviewRequest>) {
        let Some(request) = request else {
            if self.selection.is_none()
                && self.prepared.is_none()
                && self.document.is_none()
                && self.grace_due.is_none()
            {
                return;
            }
            self.reset_selection();
            return;
        };
        if self.selection.as_deref() == Some(request.identity.as_str()) {
            return;
        }
        self.cancel_pending();
        let now = std::time::Instant::now();
        let is_script = matches!(request.source, PreviewSource::Script(_));
        let owner = request.owner.clone();
        self.selection = Some(request.identity.clone());
        self.due = Some(now + DEBOUNCE_DURATION);
        self.prepared = Some(request);
        if self.document.is_none() {
            self.package = owner.split(':').next().unwrap_or("").to_owned();
            // A script document cached for this provider can render immediately,
            // even though the request identity changed with the parameters that
            // triggered the self-navigation. Only script previews need this:
            // declared and inherited documents install synchronously during
            // `start` and never expose the loading status.
            let cached = is_script.then(|| self.preview_cache.get(&owner)).flatten();
            if let Some(cached) = cached {
                // Image decoding still waits for `start`, so an input-path call
                // never starts a task.
                self.install_document_inner(cached.document, false);
            } else {
                self.status = Some("Loading preview…".into());
            }
        } else {
            self.grace_due = Some(now + GRACE_PERIOD_DURATION);
        }
    }

    pub(super) fn set_content_size(&mut self, size: Option<(u16, u16)>) {
        self.content_size = size;
        self.scroll(0);
    }

    pub(super) fn scroll(&mut self, delta: i16) {
        let limit = self
            .content_size
            .and_then(|(width, height)| {
                let area =
                    preview_areas(&self.config, self.visible, Rect::new(0, 0, width, height)).1?;
                Some(self.document.as_ref()?.scroll_limit(area))
            })
            .unwrap_or(0);
        self.scroll = self
            .scroll
            .min(limit)
            .saturating_add_signed(delta)
            .min(limit);
    }

    // Only called with post-commit host authority. Both scripts and image decoding
    // begin here; input callbacks merely replace the prepared immutable request.
    pub(super) fn start(&mut self, starter: &crate::task::MountTaskStarter) -> Option<u64> {
        self.collect();
        if let Some(mut task) = self.script_task.take() {
            use crate::task::TaskCompletion;
            match task.try_recv() {
                Ok(TaskCompletion::Completed(document)) => {
                    self.cache_document(document.as_ref());
                    self.install_document(document)
                }
                Ok(TaskCompletion::Failed(error)) => {
                    self.grace_due = None;
                    self.document = None;
                    self.images.clear();
                    self.status = Some(error);
                    self.error = true;
                }
                Ok(TaskCompletion::Cancelled) => {
                    self.status = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.script_task = Some(task),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.grace_due = None;
                    self.document = None;
                    self.images.clear();
                    self.status = Some("preview worker disconnected".into());
                    self.error = true;
                }
            }
        }
        if !self.pending_images.is_empty() {
            let pending = std::mem::take(&mut self.pending_images);
            self.start_images(pending);
        }
        let request = self.prepared.as_ref()?;
        match &request.source {
            PreviewSource::Script(source) => {
                if self.script_task.is_some()
                    || self.due.is_none_or(|due| std::time::Instant::now() < due)
                {
                    return None;
                }
                self.due = None;
                let source = source.clone();
                let request = request.clone();
                let generation = self.revision;
                self.script_task = Some(
                    starter
                        .for_task(crate::protocol::contracts::TaskId(2), generation)
                        .for_preview()
                        .spawn_latest_tagged(
                            "preview",
                            crate::task::TaskTags::new("picker", "preview"),
                            move |context| {
                                let outcome = crate::protocol::run_script_preview_response(
                                    &request.owner,
                                    request.root.as_deref(),
                                    &source,
                                    &request.request,
                                    &context.cancellation,
                                );
                                if outcome.managed_child_reaped {
                                    context.mark_process_reaped();
                                }
                                outcome
                                    .result
                                    .and_then(document::parse)
                                    .map_err(|error| format!("{error:#}"))
                            },
                        ),
                );
                Some(generation)
            }
            PreviewSource::Declared(document) => {
                if self.due.take().is_some() {
                    let document = document.clone();
                    self.install_document(document);
                }
                None
            }
            PreviewSource::Inherit | PreviewSource::Details => {
                if self.due.take().is_some() {
                    let item = &request.request["context"]["engine"]["state"]["item"];
                    self.install_document(Some(document::item_details(item)));
                }
                None
            }
        }
    }

    fn cache_document(&self, document: Option<&document::Document>) {
        let Some(owner) = self.prepared.as_ref().map(|r| r.owner.clone()) else {
            return;
        };
        self.preview_cache.insert(owner, document.cloned());
    }

    fn install_document(&mut self, document: Option<document::Document>) {
        self.install_document_inner(document, true);
    }

    /// Install a document, either starting image decoding now (post-commit host
    /// authority) or deferring it to the next `start` call.
    fn install_document_inner(
        &mut self,
        document: Option<document::Document>,
        decode_images: bool,
    ) {
        self.grace_due = None;
        self.scroll = 0;
        if let Some(prepared) = &self.prepared {
            self.package = prepared.owner.split(':').next().unwrap_or("").to_owned();
        }
        self.status = document.is_none().then(|| "(no preview)".into());
        self.error = false;
        let mut paths = Vec::new();
        if let Some(document) = &document {
            document.images(&mut paths);
        }
        let root = self.prepared.as_ref().and_then(|r| r.root.as_deref());
        let resolved = paths
            .iter()
            .map(|p| image_path::resolve(root, p))
            .collect::<Vec<_>>();

        self.images = vec![PreviewImageState::default(); resolved.len()];
        let mut uncached = Vec::new();
        for (i, path) in resolved.into_iter().enumerate() {
            if let Some(cached) = self.get_cached_image(&path) {
                self.images[i].image = Some(cached);
                self.images[i].error = None;
            } else {
                uncached.push((i, path));
            }
        }
        self.document = document;
        if decode_images {
            self.pending_images.clear();
            self.start_images(uncached);
        } else {
            self.pending_images = uncached;
        }
    }

    fn start_images(&mut self, images: Vec<(usize, std::path::PathBuf)>) {
        if images.is_empty() {
            return;
        }
        let pool = match &self.pool {
            Some(pool) => Arc::clone(pool),
            None => match image_decode::new_default_pool() {
                Ok(pool) => {
                    self.pool = Some(Arc::clone(&pool));
                    pool
                }
                Err(message) => {
                    for state in &mut self.images {
                        state.error = Some(message.clone());
                    }
                    return;
                }
            },
        };
        self.task = Some(ImageTask {
            handle: pool.submit(self.revision, images),
        });
    }

    fn collect(&mut self) {
        let result = self.task.as_ref().map(|task| task.handle.try_recv());
        match result {
            Some(Ok(batch)) => {
                self.task = None;
                if batch.revision != self.revision {
                    // A cancelled generation must never clear the newer document.
                } else {
                    for decoded in batch.images {
                        match decoded.result {
                            Ok(decoded_img) => {
                                let arc_img = Arc::new(decoded_img);
                                self.cache_decoded_image(decoded.path, Arc::clone(&arc_img));
                                self.images[decoded.block].image = Some(arc_img);
                                self.images[decoded.block].error = None;
                            }
                            Err(message) => {
                                self.images[decoded.block].error = Some(format!(
                                    "could not load {}: {message}",
                                    decoded.path.display()
                                ));
                            }
                        }
                    }
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.reset_selection();
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    pub(super) fn render_state(&self) -> PickerPreviewRenderState {
        let grace_expired = self
            .grace_due
            .is_some_and(|due| std::time::Instant::now() >= due);
        let (document, images, status, package) = if grace_expired {
            let package = self
                .prepared
                .as_ref()
                .map(|r| r.owner.split(':').next().unwrap_or("").to_owned())
                .unwrap_or_default();
            (None, Vec::new(), Some("Loading preview…".into()), package)
        } else {
            (
                self.document.clone(),
                self.images.clone(),
                self.status
                    .clone()
                    .or_else(|| self.selection.is_none().then(|| "(no preview)".into())),
                self.package.clone(),
            )
        };
        PickerPreviewRenderState {
            config: self.config.clone(),
            visible: self.visible,
            revision: self.revision,
            images,
            document,
            status,
            error: self.error,
            package,
            scroll: if grace_expired { 0 } else { self.scroll },
        }
    }
}

fn preview_areas(config: &PickerPreviewConfig, visible: bool, area: Rect) -> (Rect, Option<Rect>) {
    if !visible {
        return (area, None);
    }
    let items = config
        .layout
        .panes
        .iter()
        .find(|pane| pane.slot == "items")
        .expect("validated items pane");
    let preview = config
        .layout
        .panes
        .iter()
        .find(|pane| pane.slot == "preview")
        .expect("validated preview pane");
    let (items_length, preview_length) = match pane_lengths(
        match config.layout.direction {
            Direction::Horizontal => area.width,
            Direction::Vertical => area.height,
        },
        config.layout.gap,
        items,
        preview,
    ) {
        Some(lengths) => lengths,
        None => return (area, None),
    };
    match config.layout.direction {
        Direction::Horizontal => (
            Rect::new(area.x, area.y, items_length, area.height),
            Some(Rect::new(
                area.x
                    .saturating_add(items_length)
                    .saturating_add(config.layout.gap),
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
                    .saturating_add(config.layout.gap),
                area.width,
                preview_length,
            )),
        ),
    }
}

fn render_separator(
    direction: Direction,
    frame: &mut Frame,
    items: Rect,
    preview: Rect,
    theme: &Theme,
) {
    let area = match direction {
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
    let borders = match direction {
        Direction::Horizontal => Borders::LEFT,
        Direction::Vertical => Borders::TOP,
    };
    frame.render_widget(
        Block::new()
            .borders(borders)
            .border_style(theme.picker.preview.border),
        area,
    );
}

pub(super) fn item_value(item: &Item) -> Value {
    serde_json::json!({
        "text": item.text,
        "value": item.value,
        "metadata": item.metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ImageProtocolCache, Item, PickerPreview, PreviewDocumentCache, PreviewImageState,
        PreviewSource, item_value, parse,
    };
    use crate::ui::theme::Theme;
    use ratatui::Terminal as RatatuiTerminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use serde_json::json;

    #[test]
    fn preview_ratio_and_min_width_define_outer_panes() {
        let config = parse(
            0.25,
            24,
            Some(json!({"producer": "declared", "document": "summary"})),
        )
        .unwrap();
        let mut preview = PickerPreview::new(config, PreviewDocumentCache::default());
        preview.set_visible(true);

        let (items, preview_area) = preview.render_state().areas(Rect::new(0, 0, 80, 10));
        let preview_area = preview_area.expect("preview should fit");
        assert_eq!(items.x, 0);
        assert_eq!(items.width + preview_area.width, 79);
        assert!(preview_area.width < items.width);

        let (items, preview_area) = preview.render_state().areas(Rect::new(0, 0, 24, 10));
        assert_eq!(items, Rect::new(0, 0, 24, 10));
        assert!(preview_area.is_none());
    }

    #[test]
    fn preview_item_projection_excludes_internal_provenance() {
        let item = Item {
            text: "Terminal".to_string(),
            display: super::super::display::ItemDisplayInput::Plain("Terminal".to_string()).into(),
            value: Some("terminal".to_string()),
            metadata: json!({"summary": "details"}),
            bindings: std::collections::BTreeMap::new(),
            source_view: "apps:main".to_string(),
        };
        let value = item_value(&item);
        assert_eq!(value["text"], "Terminal");
        assert_eq!(value["metadata"]["summary"], "details");
        assert!(value.get("owner_view").is_none());
        assert!(value.get("source_view").is_none());
    }

    #[test]
    fn deactivation_releases_loaded_preview_state() {
        let config = parse(
            0.35,
            24,
            Some(json!({"producer":"declared", "document":{
                "type":"layout", "direction":"vertical", "children":[
                    "loaded", {"type":"image", "path":"image.png"}
                ]
            }})),
        )
        .unwrap();
        let mut preview = PickerPreview::new(config, PreviewDocumentCache::default());
        preview.selection = Some("selected".to_string());
        let PreviewSource::Declared(document) = preview.source().clone() else {
            unreachable!()
        };
        preview.install_document(document);
        preview.images[0].image = Some(std::sync::Arc::new(image::DynamicImage::new_rgba8(2, 2)));
        let revision = preview.revision;

        preview.deactivate();

        assert!(preview.selection.is_none());
        assert!(preview.task.is_none());
        assert!(preview.images.is_empty());
        assert!(preview.document.is_none());
        assert!(preview.prepared.is_none());
        assert!(preview.script_task.is_none());
        assert!(preview.pool.is_none());
        assert_ne!(preview.revision, revision);
    }

    #[test]
    fn preview_errors_use_the_preview_error_binding() {
        let config = parse(
            0.35,
            24,
            Some(json!({"producer":"declared", "document":{"type":"image", "path":"image.png"}})),
        )
        .unwrap();
        let mut preview = PickerPreview::new(config, PreviewDocumentCache::default());
        preview.set_visible(true);
        let PreviewSource::Declared(document) = preview.source().clone() else {
            unreachable!()
        };
        preview.document = document;
        preview.images = vec![PreviewImageState {
            image: None,
            error: Some("image failed".to_string()),
        }];
        let mut theme = Theme::terminal();
        theme.picker.preview.error.fg = Some(Color::Magenta);
        theme.picker.preview.error.bg = Some(Color::Green);
        let mut terminal = RatatuiTerminal::new(TestBackend::new(12, 1)).unwrap();
        let render_state = preview.render_state();
        let mut protocols = ImageProtocolCache::new();

        terminal
            .draw(|frame| {
                let area = frame.area();
                render_state.render(frame, area, &theme, None, &mut protocols);
            })
            .unwrap();

        let cell = terminal.backend().buffer().cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "i");
        assert_eq!(cell.style().fg, Some(Color::Magenta));
        assert_eq!(cell.style().bg, Some(Color::Green));
    }
}

#[cfg(test)]
mod runtime_tests;
