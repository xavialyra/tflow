use super::PickerViewServices;
use super::items::{
    Item, ItemsEvent, ItemsRequest, ItemsRequestIdentity, ItemsResponse, ItemsTaskHandle,
};
use super::preview::{PickerPreview, PickerPreviewConfig};
#[cfg(test)]
use crate::engine::ActionInvocation;
use crate::engine::{
    BackgroundOutcome, EngineActionInput, EngineDecision, EngineEmission, EngineNotice,
    EngineRuntime, EngineRuntimeSnapshot, EngineTick, RenderModel, ViewContext,
    ViewContextPublication,
};
use crate::task::TaskCompletion;
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Result, bail};
use serde_json::Value;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

const SEARCH_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_millis(80);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PickerOptions {
    pub(super) show_input: bool,
    pub(super) show_divider: bool,
}

impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            show_input: true,
            show_divider: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InputRefreshState {
    #[default]
    Stable,
    AwaitingReady,
    RetryRequested,
}

impl InputRefreshState {
    fn is_awaiting_ready(self) -> bool {
        matches!(self, Self::AwaitingReady)
    }

    fn is_retry_requested(self) -> bool {
        matches!(self, Self::RetryRequested)
    }

    fn is_loading(self) -> bool {
        !matches!(self, Self::Stable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum ResultsState {
    #[default]
    Invalid,
    Ready(String),
}

impl ResultsState {
    fn is_current(&self, input: &str) -> bool {
        matches!(self, Self::Ready(results_input) if results_input == input)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
enum ItemsTaskState {
    #[default]
    Idle,
    Prepared(ItemsRequest),
    Running {
        identity: ItemsRequestIdentity,
        started_at: std::time::Instant,
    },
}

impl ItemsTaskState {
    fn running(identity: ItemsRequestIdentity) -> Self {
        Self::Running {
            identity,
            started_at: std::time::Instant::now(),
        }
    }

    fn is_loading(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

#[derive(Clone)]
pub(crate) struct PickerSelection {
    pub(crate) items: Arc<Vec<Item>>,
    pub(crate) selected: usize,
}

impl Default for PickerSelection {
    fn default() -> Self {
        Self {
            items: Arc::new(Vec::new()),
            selected: 0,
        }
    }
}

impl PickerSelection {
    fn replace(&mut self, items: Vec<Item>) {
        self.items = Arc::new(items);
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    fn clear(&mut self) {
        self.items = Arc::new(Vec::new());
        self.selected = 0;
    }

    fn move_by(&mut self, direction: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.items.len() - 1;
        self.selected = self.selected.saturating_add_signed(direction).min(last);
    }
}

#[derive(Clone)]
pub(crate) struct PickerFrame {
    pub(crate) view: String,
    pub(crate) selection: PickerSelection,
    pub(crate) query: String,
    input_refresh: InputRefreshState,
    results: ResultsState,
    pub(crate) pending_selection: isize,
}

impl PickerFrame {
    pub(crate) fn new(view: &str) -> Self {
        Self {
            view: view.to_string(),
            selection: PickerSelection::default(),
            query: String::new(),
            input_refresh: InputRefreshState::Stable,
            results: ResultsState::Invalid,
            pending_selection: 0,
        }
    }

    pub(crate) fn input_is_loading(&self) -> bool {
        self.input_refresh.is_loading()
    }
}

#[derive(Clone)]
pub(crate) struct PickerState {
    frame: PickerFrame,
    requested_request: Option<ItemsRequest>,
    parameter_snapshot: Option<ParameterSnapshot>,
    runtime_snapshot: Arc<EngineRuntimeSnapshot>,
    active: bool,
    started: bool,
    items_task_state: ItemsTaskState,
    preview_visible: bool,
    initial_load_completed: bool,
    initial_focus: Option<String>,
}

#[derive(Clone)]
enum ItemsCompletion {
    Completed(Box<ItemsResponse>),
    Failed(String),
    Cancelled,
}

pub(crate) struct PickerView {
    state: PickerState,
    pub(super) services: PickerViewServices,
    items_task: Option<ItemsTaskHandle>,
    items_completion: Option<ItemsCompletion>,
    preview: PickerPreview,
    preview_content_size: Option<(u16, u16)>,
}

impl Deref for PickerView {
    type Target = PickerState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for PickerView {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl PickerView {
    #[cfg(test)]
    pub(super) fn new(view: &str, services: PickerViewServices) -> Self {
        Self::new_with_preview(
            view,
            services,
            super::preview::parse(0.35, 24, None).expect("default picker preview is valid"),
        )
    }

    pub(super) fn new_with_preview(
        view: &str,
        services: PickerViewServices,
        preview: PickerPreviewConfig,
    ) -> Self {
        Self {
            state: PickerState {
                frame: PickerFrame::new(view),
                requested_request: None,
                parameter_snapshot: None,
                runtime_snapshot: Arc::new(EngineRuntimeSnapshot::default()),
                active: true,
                started: false,
                items_task_state: ItemsTaskState::Idle,
                preview_visible: false,
                initial_load_completed: false,
                initial_focus: None,
            },
            services,
            items_task: None,
            items_completion: None,
            preview: PickerPreview::new(preview),
            preview_content_size: None,
        }
    }

    pub(crate) fn set_initial_focus(&mut self, focus: Option<String>) {
        self.state.initial_focus = focus;
    }

    pub(crate) fn current(&self) -> &PickerFrame {
        &self.frame
    }

    pub(crate) fn preview_visible(&self) -> bool {
        self.preview_visible
    }

    pub(crate) fn set_preview_visible(&mut self, visible: bool) {
        self.preview_visible = visible;
        self.preview.set_visible(visible);
    }

    pub(crate) fn preview_render_state(&self) -> super::preview::PickerPreviewRenderState {
        self.preview.render_state()
    }

    fn requested_request_ref(&self) -> Option<&ItemsRequest> {
        self.requested_request.as_ref()
    }

    fn requested_identity(&self) -> Option<&ItemsRequestIdentity> {
        self.requested_request_ref()
            .map(|request| &request.identity)
    }

    fn requested_view(&self) -> &str {
        self.requested_request_ref()
            .map_or("", |request| request.view.as_str())
    }

    fn requested_input(&self) -> &str {
        self.requested_identity()
            .map_or("", |identity| identity.input.as_str())
    }

    fn requested_generation(&self) -> u64 {
        self.requested_identity()
            .map_or(0, |identity| identity.generation)
    }

    fn requested_matches_context_values(
        &self,
        mount_id: crate::input::ViewMountId,
        view: &str,
        input: &str,
        binding_raw: &str,
        page_parameters: &ParameterSnapshot,
    ) -> bool {
        self.requested_request_ref().is_some_and(|request| {
            request.matches_context(mount_id, view, input, binding_raw, page_parameters)
        })
    }

    fn sync_preview(&mut self) {
        if !self.active {
            return;
        }
        let visible = self.preview_visible
            && self
                .preview_content_size
                .is_none_or(|size| self.preview.fits(size));
        let item = (visible
            && (self.requested_request.is_none()
                || self.results_current_snapshot(self.requested_input())))
        .then(|| {
            self.frame
                .selection
                .items
                .get(self.frame.selection.selected)
                .cloned()
        })
        .flatten();
        let request = item.map(|item| {
            let configured = self.preview.source();
            let (owner, source) = match configured {
                super::preview::PreviewSource::Inherit => (
                    item.source_view.clone(),
                    self.services.preview_sources.get(&item.source_view)
                        .filter(|source| !matches!(source, super::preview::PreviewSource::Inherit))
                        .cloned()
                        .unwrap_or(super::preview::PreviewSource::Details),
                ),
                source => (self.frame.view.clone(), source.clone()),
            };
            let parameters = self
                .parameter_snapshot
                .as_ref()
                .map(|p| p.values().clone())
                .unwrap_or(Value::Null);
            let state = serde_json::json!({"input": self.requested_input(), "item": super::preview::item_value(&item)});
            let request = crate::protocol::preview_request(&parameters, &self.services.launch_input, &state);
            let identity = serde_json::json!({"owner": owner, "source_view": item.source_view, "request": request}).to_string();
            let root = self.services.workflow_root(&owner).map(std::path::Path::to_path_buf);
            super::preview::PreviewRequest { identity, owner, request, root, source }
        });
        let content_size = self.preview_content_size;
        self.preview.set_content_size(content_size);
        self.preview.set_visible(visible);
        self.preview.prepare(request);
    }

    pub(super) fn list_presentation(&self) -> String {
        "(no matches)".to_string()
    }

    pub(crate) fn schedule_retry(&mut self) {
        self.frame.results = ResultsState::Invalid;
        if self.frame.input_refresh.is_awaiting_ready() {
            return;
        }
        self.frame.input_refresh = InputRefreshState::RetryRequested;
        self.frame.pending_selection = 0;
    }

    pub(crate) fn results_current(&self, input: &str) -> bool {
        self.frame.results.is_current(input)
            && !self.frame.input_refresh.is_loading()
            && !self.items_task_state.is_loading()
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.frame.input_is_loading() || self.items_task_state.is_loading()
    }

    pub(crate) fn is_in_grace_period(&self) -> bool {
        match &self.items_task_state {
            ItemsTaskState::Running { started_at, .. } => {
                started_at.elapsed() < SEARCH_GRACE_PERIOD
            }
            ItemsTaskState::Prepared(_) => true,
            ItemsTaskState::Idle => false,
        }
    }

    pub(crate) fn has_completed_initial_load(&self) -> bool {
        self.initial_load_completed
    }

    #[cfg(test)]
    pub(super) fn running_items_task_identity(&self) -> Option<ItemsRequestIdentity> {
        match &self.items_task_state {
            ItemsTaskState::Running { identity, .. } => Some(identity.clone()),
            _ => None,
        }
    }

    fn results_current_snapshot(&self, input: &str) -> bool {
        let Some(parameters) = self.parameter_snapshot.as_ref() else {
            return false;
        };
        self.results_current(input)
            && self.requested_matches_context_values(
                parameters.source().frame,
                &self.frame.view,
                input,
                parameters.raw_input(),
                parameters,
            )
    }

    pub(crate) fn current_view_ref(&self) -> &str {
        &self.frame.view
    }

    fn request_current(&mut self, context: &ViewContext) -> Result<Option<EngineDecision>> {
        if context.input_rejected() {
            return Ok(None);
        }
        self.remember_context(context);
        self.frame.input_refresh = InputRefreshState::Stable;

        let current_view = self.frame.view.clone();
        let current_input = context.input_raw().to_string();
        // Committed params are the feed default binding raw (successful parse).
        let binding_raw = context.parameter_raw().to_string();
        let page_parameters = context.parameter_snapshot().clone();
        let runtime_update = self.runtime_update(context.runtime_snapshot(), &current_input)?;
        self.request_items(&current_view, &current_input, &binding_raw, page_parameters)?;
        Ok(Some(EngineDecision::RuntimeUpdate(runtime_update)))
    }

    fn results_current_for_context(&self, context: &ViewContext) -> bool {
        self.results_current(context.input_raw())
            && self.requested_matches_context_values(
                context.mount_id(),
                context.view_ref(),
                context.input_raw(),
                context.parameter_raw(),
                context.parameter_snapshot(),
            )
            && self.parameter_snapshot.as_ref() == Some(context.parameter_snapshot())
    }

    fn response_matches_current_request(&self, response: &ItemsResponse) -> bool {
        let Some(requested_request) = self.requested_request_ref() else {
            return false;
        };
        requested_request.matches_response(&response.view, &response.identity)
            && self
                .parameter_snapshot
                .as_ref()
                .is_none_or(|parameters| parameters == &requested_request.identity.page_parameters)
    }

    fn running_items_task_matches_request(&self, identity: &ItemsRequestIdentity) -> bool {
        matches!(&self.items_task_state, ItemsTaskState::Running { identity: running, .. } if running == identity)
            && self
                .requested_request_ref()
                .is_some_and(|request| request.identity == *identity)
    }

    fn request_items(
        &mut self,
        view: &str,
        input: &str,
        binding_raw: &str,
        page_parameters: ParameterSnapshot,
    ) -> Result<Option<ItemsRequest>> {
        let parameter_revision = page_parameters.revision();
        let requested_generation = self.requested_generation();
        if self.items_task_state.is_loading()
            && self.requested_matches_context_values(
                page_parameters.source().frame,
                view,
                input,
                binding_raw,
                &page_parameters,
            )
        {
            return Ok(None);
        }

        let request_generation = requested_generation.wrapping_add(1);
        let source = page_parameters.source();
        let identity = ItemsRequestIdentity::new(
            source.frame,
            source,
            request_generation,
            input.to_string(),
            parameter_revision,
            binding_raw.to_string(),
            page_parameters,
        )?;
        let mut engine_state = self.current_publication().current;
        if let Some(state) = engine_state.as_object_mut() {
            state.insert("input".to_string(), Value::String(input.to_string()));
        }
        let request =
            ItemsRequest::new(view.to_string(), identity)?.with_engine_state(engine_state)?;
        self.requested_request = Some(request.clone());
        self.items_task_state = ItemsTaskState::Prepared(request.clone());
        if self.frame.input_refresh.is_retry_requested() {
            self.frame.input_refresh = InputRefreshState::Stable;
        }
        Ok(Some(request))
    }

    fn invalidate_items_for_committed_input(&mut self) {
        self.items_task_state = ItemsTaskState::Idle;
        self.frame.input_refresh = InputRefreshState::AwaitingReady;
        self.frame.results = ResultsState::Invalid;
        self.frame.pending_selection = 0;
    }

    fn collect_items(&mut self, response: ItemsResponse) -> Vec<ItemsEvent> {
        let mut events = Vec::new();
        self.frame.input_refresh = InputRefreshState::Stable;
        self.items_task_state = ItemsTaskState::Idle;
        self.initial_load_completed = true;
        let ItemsResponse {
            view,
            identity,
            result,
        } = response;
        let query = identity.binding_raw;
        let input = identity.input;
        let (errors, failure) = match result {
            Ok(result) => {
                let errors = result.errors;
                self.frame.query = query;
                self.frame.selection.replace(result.items);
                if let Some(target) = self.initial_focus.take()
                    && let Some(pos) = self.frame.selection.items.iter().position(|item| {
                        item.value.as_deref() == Some(&target) || item.text == target
                    })
                {
                    self.frame.selection.selected = pos;
                }
                self.frame.results = ResultsState::Ready(input);
                let pending_selection = std::mem::take(&mut self.frame.pending_selection);
                self.move_selection(pending_selection);
                (errors, None)
            }
            Err(error) => {
                self.frame.query = query;
                self.frame.selection.clear();
                self.frame.results = ResultsState::Ready(input);
                self.frame.pending_selection = 0;
                (Vec::new(), Some(error))
            }
        };
        events.push(ItemsEvent {
            view,
            errors,
            failure,
        });
        events
    }

    fn decisions_for_items_events(
        &self,
        events: Vec<ItemsEvent>,
        foreground: bool,
    ) -> Result<Option<EngineDecision>> {
        let items_changed = !events.is_empty();
        let mut decisions = Vec::new();
        for event in events {
            if let Some(error) = event.failure {
                decisions.push(EngineDecision::Report(EngineNotice::Error {
                    view_ref: event.view.clone(),
                    message: error,
                }));
            } else if event.errors.is_empty() {
                decisions.push(EngineDecision::Report(EngineNotice::ClearError));
            } else {
                for error in event.errors {
                    decisions.push(EngineDecision::Report(EngineNotice::Error {
                        view_ref: event.view.clone(),
                        message: error,
                    }));
                }
            }
        }
        if items_changed && foreground {
            decisions.push(EngineDecision::RuntimeUpdate(
                self.runtime_update(&self.runtime_snapshot, self.requested_input())?,
            ));
        }
        Ok(match decisions.len() {
            0 => None,
            1 => decisions.pop(),
            _ => Some(EngineDecision::Batch(decisions)),
        })
    }

    fn clear_items_after_failure(&mut self) {
        self.items_task_state = ItemsTaskState::Idle;
        self.frame.results = ResultsState::Invalid;
        self.frame.pending_selection = 0;
        self.frame.selection.clear();
        self.initial_load_completed = true;
        self.schedule_retry();
    }

    #[cfg(test)]
    fn handle_task_result(&mut self, response: ItemsResponse) -> Result<EngineDecision> {
        self.handle_task_result_for_scope(response, true)
    }

    fn handle_task_result_for_scope(
        &mut self,
        response: ItemsResponse,
        foreground: bool,
    ) -> Result<EngineDecision> {
        if !self.response_matches_current_request(&response) {
            return Ok(EngineDecision::Continue);
        }
        let events = self.collect_items(response);
        self.sync_preview();
        let decision = self
            .decisions_for_items_events(events, foreground)?
            .unwrap_or(EngineDecision::Continue);
        Ok(decision)
    }

    #[cfg(test)]
    fn handle_task_failure(&mut self, error: String) -> Result<EngineDecision> {
        self.handle_task_failure_for_scope(error, true)
    }

    fn handle_task_failure_for_scope(
        &mut self,
        error: String,
        foreground: bool,
    ) -> Result<EngineDecision> {
        self.clear_items_after_failure();
        let events = vec![ItemsEvent {
            view: self.requested_view().to_string(),
            errors: Vec::new(),
            failure: Some(if error.is_empty() {
                "picker items task failed without an error".to_string()
            } else {
                error
            }),
        }];
        Ok(self
            .decisions_for_items_events(events, foreground)?
            .unwrap_or(EngineDecision::Continue))
    }

    fn handle_task_cancelled_for_scope(&mut self, _foreground: bool) -> Result<EngineDecision> {
        self.clear_items_after_failure();
        Ok(EngineDecision::Continue)
    }

    fn move_selection(&mut self, direction: isize) {
        self.frame.selection.move_by(direction);
        self.sync_preview();
    }

    fn current_publication(&self) -> ViewContextPublication {
        let results_ready = self.results_current_snapshot(&self.frame.query);
        let selected = results_ready
            .then(|| {
                self.frame
                    .selection
                    .items
                    .get(self.frame.selection.selected)
            })
            .flatten();
        let selected_index = selected
            .map(|_| self.frame.selection.selected)
            .unwrap_or_default();
        let (item, text, value, metadata, bindings) = match selected {
            Some(item) => (
                serde_json::json!({
                    "text": item.text,
                    "value": item.value,
                    "metadata": item.metadata,
                    "bindings": item.bindings,
                }),
                serde_json::json!(item.text),
                serde_json::json!(item.value),
                item.metadata.clone(),
                serde_json::to_value(&item.bindings).unwrap_or(serde_json::json!({})),
            ),
            None => (
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                serde_json::json!({}),
            ),
        };
        ViewContextPublication::new(serde_json::json!({
            "input": self.frame.query,
            "item": item,
            "text": text,
            "value": value,
            "metadata": metadata,
            "bindings": bindings,
            "selected_index": selected_index,
        }))
        .with_ready(results_ready)
    }
}

impl PickerView {
    fn remember_context(&mut self, context: &ViewContext) {
        self.runtime_snapshot = Arc::new(context.runtime_snapshot().clone());
    }

    fn handle_activate(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.remember_context(context);
        self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        let update = self.runtime_update(context.runtime_snapshot(), context.input_raw())?;
        Ok(EngineDecision::RuntimeUpdate(update))
    }

    fn handle_restore_input(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.remember_context(context);
        self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        self.frame.input_refresh = InputRefreshState::Stable;
        self.items_task_state = ItemsTaskState::Idle;
        self.frame.pending_selection = 0;
        if !self.results_current_for_context(context) {
            return Ok(self
                .request_current(context)?
                .unwrap_or(EngineDecision::Continue));
        }
        Ok(EngineDecision::Continue)
    }

    fn handle_input_committed(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.remember_context(context);
        self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        self.invalidate_items_for_committed_input();
        self.preview.prepare(None);
        Ok(EngineDecision::RuntimeUpdate(self.runtime_update(
            context.runtime_snapshot(),
            context.input_raw(),
        )?))
    }

    fn handle_input_ready(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.remember_context(context);
        self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        self.frame.input_refresh = InputRefreshState::Stable;
        if !self.results_current_for_context(context)
            && let Some(decision) = self.request_current(context)?
        {
            return Ok(decision);
        }
        Ok(EngineDecision::Continue)
    }

    fn handle_input_rejected(&mut self) -> Result<EngineDecision> {
        self.preview.prepare(None);
        self.frame.input_refresh = InputRefreshState::Stable;
        self.items_task_state = ItemsTaskState::Idle;
        self.frame.results = ResultsState::Invalid;
        self.frame.pending_selection = 0;
        Ok(EngineDecision::Continue)
    }

    fn handle_tick(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.active = true;
        self.remember_context(context);
        if !self.started {
            self.started = true;
            if let Some(decision) = self.request_current(context)? {
                return Ok(decision);
            }
        }

        if self.frame.input_refresh.is_retry_requested()
            && let Some(decision) = self.request_current(context)?
        {
            return Ok(decision);
        }
        Ok(EngineDecision::Continue)
    }
}

impl PickerView {
    fn dispatch_action(
        &mut self,
        id: crate::engine::ActionId,
        context: Option<&ViewContext>,
    ) -> Result<EngineDecision> {
        if let Some(context) = context {
            self.remember_context(context);
            self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        }
        match id.as_str() {
            "picker.select_next" => {
                self.select_without_host(1);
                if self.results_current_snapshot(&self.frame.query) {
                    let input = context.map_or_else(|| self.requested_input(), |c| c.input_raw());
                    let snapshot =
                        context.map_or_else(|| &*self.runtime_snapshot, |c| c.runtime_snapshot());
                    Ok(EngineDecision::RuntimeUpdate(
                        self.runtime_update(snapshot, input)?,
                    ))
                } else {
                    Ok(EngineDecision::Invalidate)
                }
            }
            "picker.select_previous" => {
                self.select_without_host(-1);
                if self.results_current_snapshot(&self.frame.query) {
                    let input = context.map_or_else(|| self.requested_input(), |c| c.input_raw());
                    let snapshot =
                        context.map_or_else(|| &*self.runtime_snapshot, |c| c.runtime_snapshot());
                    Ok(EngineDecision::RuntimeUpdate(
                        self.runtime_update(snapshot, input)?,
                    ))
                } else {
                    Ok(EngineDecision::Invalidate)
                }
            }
            "picker.cancel" => Ok(EngineDecision::Close),
            "picker.back" => Ok(EngineDecision::Close),
            "picker.exit" => Ok(EngineDecision::Exit),
            "picker.preview_scroll_up" | "picker.preview_scroll_down" => {
                self.preview
                    .scroll(if id.as_str() == "picker.preview_scroll_up" {
                        -3
                    } else {
                        3
                    });
                Ok(EngineDecision::Continue)
            }
            "picker.toggle_preview" => {
                self.preview_visible = !self.preview_visible;
                self.sync_preview();
                Ok(EngineDecision::Invalidate)
            }
            "picker.retry" => {
                self.schedule_retry();
                Ok(EngineDecision::Invalidate)
            }
            _ => bail!("unknown picker action {:?}", id.as_str()),
        }
    }
}

impl PickerView {
    fn dispatch_action_input(&mut self, input: EngineActionInput) -> Result<EngineDecision> {
        let EngineActionInput {
            invocation,
            context,
        } = input;
        self.dispatch_action(invocation.id, Some(&context))
    }
}

impl PickerView {
    fn receive_items_completion(
        &mut self,
    ) -> Result<Option<(ItemsRequestIdentity, ItemsCompletion)>> {
        let identity = match &self.items_task_state {
            ItemsTaskState::Running { identity, .. } => identity.clone(),
            ItemsTaskState::Idle | ItemsTaskState::Prepared(_) => {
                self.items_task.take();
                self.items_completion = None;
                return Ok(None);
            }
        };
        let completion = if let Some(completion) = self.items_completion.take() {
            completion
        } else {
            let Some(mut task) = self.items_task.take() else {
                return Ok(None);
            };
            match task.try_recv() {
                Ok(TaskCompletion::Completed(response)) => {
                    ItemsCompletion::Completed(Box::new(response))
                }
                Ok(TaskCompletion::Failed(error)) => ItemsCompletion::Failed(error),
                Ok(TaskCompletion::Cancelled) => ItemsCompletion::Cancelled,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    if self.running_items_task_matches_request(&identity) {
                        self.items_task = Some(task);
                    }
                    return Ok(None);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => ItemsCompletion::Failed(
                    "picker items task stopped before producing a result".into(),
                ),
            }
        };
        Ok(Some((identity, completion)))
    }
}

impl EngineRuntime for PickerView {
    fn action(&mut self, input: EngineActionInput) -> Result<EngineEmission> {
        let preview_scroll = matches!(
            input.invocation.id.as_str(),
            "picker.preview_scroll_up" | "picker.preview_scroll_down"
        );
        let decision = self.dispatch_action_input(input)?;
        if preview_scroll {
            return Ok(EngineEmission::decision(EngineDecision::Invalidate));
        }
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn activate(&mut self, context: ViewContext) -> Result<EngineEmission> {
        let decision = self.handle_activate(&context)?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn restore_input(&mut self, context: ViewContext) -> Result<EngineEmission> {
        let decision = self.handle_restore_input(&context)?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn input_committed(&mut self, context: ViewContext) -> Result<EngineEmission> {
        let decision = self.handle_input_committed(&context)?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn input_ready(&mut self, context: ViewContext) -> Result<EngineEmission> {
        let decision = self.handle_input_ready(&context)?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn input_rejected(
        &mut self,
        _expected: crate::engine::ViewContextIdentity,
    ) -> Result<EngineEmission> {
        let decision = self.handle_input_rejected()?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn tick(&mut self, tick: EngineTick) -> Result<EngineEmission> {
        let decision = self.handle_tick(&tick.context)?;
        Ok(EngineEmission::decision(decision).with_publication(self.current_publication()))
    }

    fn poll_work(&mut self) -> Result<Option<EngineEmission>> {
        let Some((identity, completion)) = self.receive_items_completion()? else {
            return Ok(None);
        };
        let running_matches = self.running_items_task_matches_request(&identity);
        let response_matches = matches!(&completion, ItemsCompletion::Completed(response)
            if response.identity == identity && self.response_matches_current_request(response));
        if !running_matches
            || (matches!(&completion, ItemsCompletion::Completed(_)) && !response_matches)
        {
            self.items_task_state = ItemsTaskState::Idle;
            self.schedule_retry();
            return Ok(Some(
                EngineEmission::decision(EngineDecision::Continue)
                    .with_publication(self.current_publication()),
            ));
        }
        let decision = match completion {
            ItemsCompletion::Completed(response) => {
                self.handle_task_result_for_scope(*response, true)?
            }
            ItemsCompletion::Failed(error) => self.handle_task_failure_for_scope(error, true)?,
            ItemsCompletion::Cancelled => self.handle_task_cancelled_for_scope(true)?,
        };
        Ok(Some(
            EngineEmission::decision(decision).with_publication(self.current_publication()),
        ))
    }

    fn poll_background_work(&mut self) -> Result<Option<BackgroundOutcome>> {
        let Some((identity, completion)) = self.receive_items_completion()? else {
            return Ok(None);
        };
        if !self.running_items_task_matches_request(&identity)
            || matches!(&completion, ItemsCompletion::Completed(response)
                if response.identity != identity || !self.response_matches_current_request(response))
        {
            self.items_task_state = ItemsTaskState::Idle;
            self.schedule_retry();
            return Ok(Some(
                BackgroundOutcome::default().with_publication(self.current_publication()),
            ));
        }
        let decision = match completion {
            ItemsCompletion::Completed(response) => {
                self.handle_task_result_for_scope(*response, false)?
            }
            ItemsCompletion::Failed(error) => self.handle_task_failure_for_scope(error, false)?,
            ItemsCompletion::Cancelled => self.handle_task_cancelled_for_scope(false)?,
        };
        let mut outcome = BackgroundOutcome::default().with_publication(self.current_publication());
        fn collect(decision: EngineDecision, notices: &mut Vec<EngineNotice>) {
            match decision {
                EngineDecision::Report(notice) => notices.push(notice),
                EngineDecision::Batch(decisions) => {
                    for decision in decisions {
                        collect(decision, notices);
                    }
                }
                _ => {}
            }
        }
        collect(decision, &mut outcome.notices);
        Ok(Some(outcome))
    }

    fn start_prepared_work(&mut self, starter: &crate::task::MountTaskStarter) -> bool {
        let prepared = std::mem::take(&mut self.items_task_state);
        let started = if let ItemsTaskState::Prepared(request) = prepared {
            self.items_completion = None;
            let identity = request.identity.clone();
            let task = self.services.start_items(starter, request);
            self.items_task_state = ItemsTaskState::running(identity);
            self.items_task = Some(task);
            true
        } else {
            self.items_task_state = prepared;
            false
        };
        self.sync_preview();
        started
    }

    fn set_auxiliary_content_size(&mut self, size: (u16, u16)) {
        self.preview_content_size = Some(size);
        self.sync_preview();
    }

    fn suspend_auxiliary_work(&mut self) {
        self.active = false;
        self.preview.deactivate();
    }

    fn start_prepared_auxiliary_work(
        &mut self,
        starter: &crate::task::MountTaskStarter,
    ) -> Vec<(crate::protocol::contracts::TaskId, u64)> {
        self.sync_preview();
        if !self.active {
            return Vec::new();
        }
        self.preview
            .start(starter)
            .map(|generation| vec![(crate::protocol::contracts::TaskId(2), generation)])
            .unwrap_or_default()
    }

    fn deactivate(&mut self) {
        let was_loading = self.items_task_state.is_loading();
        self.items_task_state = ItemsTaskState::Idle;
        self.items_task.take();
        self.items_completion = None;
        if was_loading {
            self.schedule_retry();
        }
        self.preview.deactivate();
        self.active = false;
    }

    fn render_model(&self) -> RenderModel {
        RenderModel::new("picker", self.render_state())
    }
}

impl PickerView {
    fn select_without_host(&mut self, direction: isize) {
        if self.results_current_snapshot(&self.frame.query) {
            self.move_selection(direction);
            return;
        }
        self.frame.pending_selection = self.frame.pending_selection.saturating_add(direction);
        if !self.frame.input_refresh.is_retry_requested() {
            self.frame.input_refresh = InputRefreshState::AwaitingReady;
        }
        self.frame.results = ResultsState::Invalid;
    }
}

#[cfg(test)]
fn selection_count(selected: usize, total: usize) -> String {
    let current = if total == 0 {
        0
    } else {
        selected.saturating_add(1)
    };
    format!("{current} of {total}")
}

#[cfg(test)]
mod tests;
