use super::PickerViewServices;
use super::items::{
    FeedId, FeedInstance, FeedRequestIdentity, Item, ItemsEvent, ItemsRequest, ItemsResponse,
    ItemsTaskHandle,
};
use super::preview::{PickerPreview, PickerPreviewConfig};
#[cfg(test)]
use crate::engine::ActionInvocation;
use crate::engine::{
    BackgroundOutcome, EngineActionInput, EngineCommandBinding, EngineCommandProjection,
    EngineDecision, EngineEmission, EngineNotice, EngineRuntime, EngineRuntimeSnapshot, EngineTick,
    QualifiedCommandId, RenderModel, ViewContext, ViewContextPublication,
};
use crate::task::TaskCompletion;
use crate::workflow::command::CommandOwnerContext;
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
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
        identity: FeedRequestIdentity,
        started_at: std::time::Instant,
    },
}

impl ItemsTaskState {
    fn running(identity: FeedRequestIdentity) -> Self {
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

    fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
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
    feed_instances: Arc<BTreeMap<FeedId, FeedInstance>>,
    active: bool,
    started: bool,
    items_task_state: ItemsTaskState,
    preview_visible: bool,
    initial_load_completed: bool,
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
                feed_instances: Arc::new(BTreeMap::new()),
                active: true,
                started: false,
                items_task_state: ItemsTaskState::Idle,
                preview_visible: false,
                initial_load_completed: false,
            },
            services,
            items_task: None,
            items_completion: None,
            preview: PickerPreview::new(preview),
            preview_content_size: None,
        }
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

    fn requested_identity(&self) -> Option<&FeedRequestIdentity> {
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
        let request = item.and_then(|item| {
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
            let parameters = if owner == self.frame.view {
                self.parameter_snapshot.as_ref().map(|p| p.values().clone()).unwrap_or(Value::Null)
            } else {
                self.feed_instances.get(&FeedId(owner.clone())).map(|f| f.parameters.values().clone()).unwrap_or(Value::Null)
            };
            let state = serde_json::json!({"input": self.requested_input(), "item": super::preview::item_value(&item)});
            let request = crate::protocol::preview_request(&parameters, &self.services.launch_input, &state);
            let identity = serde_json::json!({"owner": owner, "source_view": item.source_view, "request": request}).to_string();
            let root = self.services.workflow_root(&owner).map(std::path::Path::to_path_buf);
            Some(super::preview::PreviewRequest { identity, owner, request, root, source })
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
    pub(super) fn running_items_task_identity(&self) -> Option<FeedRequestIdentity> {
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

    fn selected_item_owner(&self) -> Option<&str> {
        self.frame
            .selection
            .selected_item()
            .map(|item| item.source_view.as_str())
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

    fn running_items_task_matches_request(&self, identity: &FeedRequestIdentity) -> bool {
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
        let identity = FeedRequestIdentity::new(
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
            ItemsRequest::new(view.to_string(), identity)?.with_engine_state(engine_state);
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
                self.feed_instances = Arc::new(result.contexts);
                self.frame.selection.replace(result.items);
                self.frame.results = ResultsState::Ready(input);
                let pending_selection = std::mem::take(&mut self.frame.pending_selection);
                self.move_selection(pending_selection);
                (errors, None)
            }
            Err(error) => {
                self.frame.query = query;
                Arc::make_mut(&mut self.feed_instances).clear();
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
        Arc::make_mut(&mut self.feed_instances).clear();
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
        let (item, text, value, metadata) = match selected {
            Some(item) => (
                serde_json::json!({
                    "text": item.text,
                    "value": item.value,
                    "metadata": item.metadata,
                }),
                serde_json::json!(item.text),
                serde_json::json!(item.value),
                item.metadata.clone(),
            ),
            None => (Value::Null, Value::Null, Value::Null, Value::Null),
        };
        ViewContextPublication::new(serde_json::json!({
            "input": self.frame.query,
            "item": item,
            "text": text,
            "value": value,
            "metadata": metadata,
            "selected_index": selected_index,
        }))
        .with_ready(results_ready)
    }

    fn page_item_command_bindings(
        &self,
        page: &str,
        items_ready: bool,
        loading: bool,
        has_selected_item: bool,
    ) -> Result<Vec<EngineCommandBinding>> {
        if !items_ready && !loading {
            return Ok(Vec::new());
        }
        let mut seen_keys = HashSet::new();
        let mut bindings = Vec::new();
        for command in self
            .services
            .page_item_commands(page)
            .iter()
            .filter(|command| !self.services.is_non_selection_command(page, &command.id))
        {
            ensure!(
                seen_keys.insert(command.key.binding_identity()),
                "picker page command owner {:?} contains duplicate physical key {:?}",
                page,
                command.key.binding_name()
            );
            let mut binding = EngineCommandBinding::new(
                QualifiedCommandId::new(page, command.id.clone()),
                command.key,
            );
            binding.label = Some(command.label.clone());
            binding.enabled = if command.requires_items {
                items_ready && has_selected_item
            } else {
                items_ready
            };
            binding.visibility = command.visibility;
            bindings.push(binding);
        }
        Ok(bindings)
    }

    fn command_projection_bindings(
        &self,
        context: &ViewContext,
    ) -> Result<Vec<EngineCommandBinding>> {
        if context.input_rejected() {
            return Ok(Vec::new());
        }

        let items_ready = self.results_current_for_context(context);
        let loading = self.is_loading();
        let retain_stale = loading && !self.frame.selection.items.is_empty();
        let effective_ready = items_ready || retain_stale;
        let has_selected_item = effective_ready && self.frame.selection.selected_item().is_some();
        let mut bindings = self.page_item_command_bindings(
            context.view_ref(),
            effective_ready,
            loading,
            has_selected_item,
        )?;
        if effective_ready
            && let Some(owner) = self.selected_item_owner()
            && owner != context.view_ref()
        {
            for binding in self.dynamic_command_bindings(owner)? {
                bindings.retain(|existing| {
                    existing.key.binding_identity() != binding.key.binding_identity()
                });
                bindings.push(binding);
            }
        }
        Ok(bindings)
    }

    fn dynamic_command_bindings(&self, owner: &str) -> Result<Vec<EngineCommandBinding>> {
        let mut seen_keys = HashSet::new();
        let mut bindings = Vec::new();
        for command in self.services.owner_commands(owner) {
            ensure!(
                seen_keys.insert(command.key.binding_identity()),
                "picker owner command owner {:?} contains duplicate physical key {:?}",
                owner,
                command.key.binding_name()
            );
            let mut binding = EngineCommandBinding::new(
                QualifiedCommandId::new(owner, command.id.clone()),
                command.key,
            );
            binding.label = Some(command.label.clone());
            binding.enabled = true;
            binding.visibility = command.visibility;
            bindings.push(binding);
        }
        Ok(bindings)
    }

    fn command_owner_context(
        &self,
        context: &ViewContext,
        owner: &str,
    ) -> Result<Option<CommandOwnerContext>> {
        if owner == context.view_ref() {
            return Ok(Some(CommandOwnerContext {
                view_ref: context.view_ref().to_string(),
                parameters: context.parameter_snapshot().clone(),
                binding_raw: context.parameter_raw().to_string(),
            }));
        }

        let Some(item) = self
            .results_current_for_context(context)
            .then(|| self.frame.selection.selected_item())
            .flatten()
        else {
            return Ok(None);
        };
        if item.source_view != owner {
            return Ok(None);
        }

        let feed_id = FeedId(owner.to_string());
        let feed = self
            .feed_instances
            .get(&feed_id)
            .with_context(|| format!("picker feed owner {:?} is unavailable", owner))?;
        ensure!(
            feed.definition.owner_view == owner,
            "picker feed owner {:?} does not match selected item provenance",
            owner
        );
        Ok(Some(CommandOwnerContext {
            view_ref: owner.to_string(),
            parameters: feed.parameters.clone(),
            binding_raw: feed.parameters.raw_input().to_string(),
        }))
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
    ) -> Result<Option<(FeedRequestIdentity, ItemsCompletion)>> {
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

    fn command_projection(&self, context: &ViewContext) -> Result<EngineCommandProjection> {
        let mut projection = EngineCommandProjection::new(context.identity());
        projection.bindings = self.command_projection_bindings(context)?;
        Ok(projection)
    }

    fn command_owner_context(
        &self,
        context: &ViewContext,
        owner: &str,
    ) -> Result<Option<CommandOwnerContext>> {
        self.command_owner_context(context, owner)
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
mod tests {
    use super::*;
    use crate::engine::picker::items::FeedDefinition;
    use crate::input::EditorBuffer;
    use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
    use std::sync::Arc;

    fn test_picker(mount_id: u64) -> (Arc<crate::workflow::config::CompiledConfig>, PickerView) {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(mount_id)),
            ),
            "core:default",
        )
        .view_services();
        let picker = PickerView::new("core:default", services);
        (config, picker)
    }

    fn test_parameters(
        input: &str,
        source: crate::input::InputSourceIdentity,
        revision: u64,
    ) -> ParameterSnapshot {
        ParameterSnapshot::from_parts(
            serde_json::json!({"query": input}),
            input.to_string(),
            source,
            revision,
        )
    }

    fn test_context(input: &str, parameters: ParameterSnapshot) -> ViewContext {
        ViewContext::for_test(
            crate::engine::ViewIdentity::new(
                "core:default",
                crate::workflow::config::ENGINE_PICKER,
            ),
            EditorBuffer::new(input).snapshot(),
            parameters,
            false,
            EngineRuntimeSnapshot::default(),
        )
    }

    fn test_item(text: &str) -> Item {
        Item {
            text: text.to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain(text.to_string()).into(),
            value: Some(text.to_string()),
            metadata: Value::Null,
            source_view: "core:default".to_string(),
        }
    }

    fn test_feed_instance(
        config: &crate::workflow::config::CompiledConfig,
        page_view: &str,
        owner_view: &str,
        parameters: ParameterSnapshot,
        _binding_raw: &str,
    ) -> FeedInstance {
        let projection = Arc::new(
            crate::workflow::config::PickerItemsProjection::from_config(
                config,
                &Value::Null,
                page_view,
            )
            .expect("test feed projection must compile"),
        );
        let definitions = FeedDefinition::collection(projection, page_view)
            .expect("test feed definitions must compile");
        let definition = definitions
            .iter()
            .find(|definition| definition.owner_view == owner_view)
            .cloned()
            .or_else(|| definitions.first().cloned())
            .expect("test picker must have a feed definition");
        FeedInstance {
            definition,
            parameters,
        }
    }

    #[test]
    fn preview_scroll_invalidates_rendering_without_publishing_selection_or_readiness() {
        let (_config, mut picker) = test_picker(110);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(110),
            generation: 0,
        };
        let context = test_context("", test_parameters("", source, 1));
        for id in ["picker.preview_scroll_up", "picker.preview_scroll_down"] {
            let emission = picker
                .action(EngineActionInput {
                    invocation: ActionInvocation::new(crate::engine::ActionId::new(id)),
                    context: context.clone(),
                })
                .unwrap();
            assert!(emission.publication().is_none());
            assert!(matches!(
                emission.decision_ref(),
                EngineDecision::Invalidate
            ));
        }
    }

    #[test]
    fn command_projection_includes_selected_feed_commands() {
        let (config, mut picker) = test_picker(109);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(109),
            generation: 0,
        };
        picker.frame.query.clear();
        picker.frame.results = ResultsState::Ready(String::new());
        let parameters = test_parameters("", source, 1);
        picker
            .request_items("core:default", "", "", parameters.clone())
            .unwrap();
        picker.items_task_state = ItemsTaskState::Idle;
        picker.parameter_snapshot = Some(parameters);
        picker.frame.selection.replace(vec![Item {
            text: "Application".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("Application".to_string())
                .into(),
            value: Some("application".to_string()),
            metadata: Value::Null,
            source_view: "apps:main".to_string(),
        }]);
        Arc::make_mut(&mut picker.feed_instances).insert(
            FeedId("apps:main".to_string()),
            test_feed_instance(
                &config,
                "core:default",
                "apps:main",
                test_parameters("", source, 1),
                "",
            ),
        );
        let context = test_context("", test_parameters("", source, 1));

        let projection = picker.command_projection(&context).unwrap();
        assert_eq!(projection.based_on, context.identity());
        let mut projected_commands = projection
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.command.owner.as_str(),
                    binding.command.id.as_str(),
                    binding.key,
                    binding.label.as_deref(),
                    binding.enabled,
                )
            })
            .collect::<Vec<_>>();
        projected_commands.sort_by_key(|(_, id, ..)| *id);
        assert_eq!(
            projected_commands,
            vec![
                (
                    "apps:main",
                    "open",
                    crate::input::Key::Enter,
                    Some("Open"),
                    true,
                ),
                (
                    "apps:main",
                    "weight",
                    crate::input::Key::Ctrl('w'),
                    Some("Set weight"),
                    true,
                ),
            ]
        );
        let publication = picker.current_publication();
        let current = publication.current();
        assert_eq!(current["item"]["value"], "application");
        assert_eq!(current["value"], "application");
        assert!(current["metadata"].is_null());
        assert!(current["item"].get("owner_view").is_none());
    }

    #[test]
    fn background_items_completion_publishes_mount_current_without_runtime_update() {
        let (_config, mut picker) = test_picker(115);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(115),
            generation: 0,
        };
        let parameters = test_parameters("background", source, 1);
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items(
                "core:default",
                "background",
                "background",
                parameters.clone(),
            )
            .unwrap();
        let identity = picker
            .requested_identity()
            .expect("background request should have an identity")
            .clone();
        picker.items_task_state = ItemsTaskState::running(identity.clone());
        let response = response_value(
            &parameters,
            identity.generation,
            "background",
            Ok(crate::engine::picker::items::ItemsResult {
                items: vec![test_item("background")],
                contexts: BTreeMap::new(),
                errors: Vec::new(),
            }),
        );
        let tasks = TaskRuntime::new();
        picker.items_task = Some(tasks.spawn(move |_context| Ok(response)));

        let mut outcome = None;
        for _ in 0..200 {
            if let Some(next) = picker.poll_background_work().unwrap() {
                outcome = Some(next);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let outcome = outcome.expect("background items completion should be polled");
        assert_eq!(
            outcome.publication.unwrap().current()["item"]["text"],
            "background"
        );
        assert_eq!(
            picker.current_publication().current()["item"]["text"],
            "background"
        );
        assert!(picker.poll_background_work().unwrap().is_none());
        tasks.shutdown_and_wait();
    }

    #[test]
    fn stale_polled_completion_applies_idle_retry_state() {
        let (_config, mut picker) = test_picker(119);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(119),
            generation: 0,
        };
        let parameters = test_parameters("stale", source, 1);
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items("core:default", "stale", "stale", parameters.clone())
            .unwrap();
        let identity = picker.requested_identity().unwrap().clone();
        picker.items_task_state = ItemsTaskState::running(identity.clone());
        let response = response_value(
            &parameters,
            identity.generation,
            "stale",
            Ok(crate::engine::picker::items::ItemsResult::default()),
        );
        let tasks = TaskRuntime::new();
        picker.items_task = Some(tasks.spawn(move |_context| Ok(response)));

        let current_parameters = test_parameters("stale", source, 2);
        picker.parameter_snapshot = Some(current_parameters);
        let mut emission = None;
        for _ in 0..200 {
            if let Some(next) = picker.poll_work().unwrap() {
                emission = Some(next);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let emission = emission.expect("stale completion should retire the task");
        assert!(matches!(emission.decision_ref(), EngineDecision::Continue));
        assert_eq!(picker.items_task_state, ItemsTaskState::Idle);
        assert!(picker.frame.input_refresh.is_retry_requested());
        assert!(picker.items_task.is_none());
        assert!(picker.items_completion.is_none());
        tasks.shutdown_and_wait();
    }

    #[test]
    fn failed_or_cancelled_polled_tasks_reset_the_handle_and_prepare_retry_work() {
        for cancelled in [false, true] {
            let (_config, mut picker) = test_picker(if cancelled { 121 } else { 120 });
            let source = crate::input::InputSourceIdentity {
                frame: crate::input::ViewMountId(if cancelled { 121 } else { 120 }),
                generation: 0,
            };
            let parameters = test_parameters("retry", source, 1);
            picker.parameter_snapshot = Some(parameters.clone());
            picker
                .request_items("core:default", "retry", "retry", parameters.clone())
                .unwrap();
            let identity = picker.requested_identity().unwrap().clone();
            picker.items_task_state = ItemsTaskState::running(identity);
            let tasks = TaskRuntime::new();
            picker.items_task = Some(if cancelled {
                tasks.spawn(|context| {
                    while !context.cancellation.is_cancelled() {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err("cancelled".to_string())
                })
            } else {
                tasks.spawn(|_| Err("items failed".to_string()))
            });
            if cancelled {
                tasks.cancel_all();
            }

            let mut emission = None;
            for _ in 0..200 {
                if let Some(next) = picker.poll_work().unwrap() {
                    emission = Some(next);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(emission.is_some(), "task completion should be consumed");
            assert_eq!(picker.items_task_state, ItemsTaskState::Idle);
            assert!(picker.items_task.is_none());
            assert!(picker.items_completion.is_none());
            assert!(picker.frame.input_refresh.is_retry_requested());

            picker.started = true;
            let emission = picker
                .tick(EngineTick {
                    context: test_context("retry", parameters),
                    content_size: (80, 24),
                })
                .unwrap();
            assert!(matches!(
                emission.decision_ref(),
                EngineDecision::RuntimeUpdate(_)
            ));
            assert!(matches!(
                picker.items_task_state,
                ItemsTaskState::Prepared(_)
            ));
            let starter = crate::task::MountTaskStarter::from_lease(
                &tasks,
                crate::task::MountTaskLease::new(source.frame),
            );
            assert!(picker.start_prepared_work(&starter));
            assert!(picker.items_task.is_some());
            tasks.shutdown_and_wait();
        }
    }

    #[test]
    fn item_commands_are_disabled_when_results_have_no_selected_item() {
        let (_config, mut picker) = test_picker(116);
        picker.services.page_commands.insert(
            "core:default".to_string(),
            BTreeMap::from([
                (
                    "enter".to_string(),
                    serde_json::json!({
                        "ref": {"view": "core:default", "id": "requires_items"},
                        "label": "Requires item"
                    }),
                ),
                (
                    "tab".to_string(),
                    serde_json::json!({
                        "ref": {"view": "core:default", "id": "selection_scope"},
                        "label": "Selection scope"
                    }),
                ),
            ]),
        );
        picker.services.page_item_commands.insert(
            "core:default".to_string(),
            vec![
                super::super::PickerSelectionCommand {
                    id: "requires_items".to_string(),
                    key: crate::input::Key::Enter,
                    label: "Requires item".to_string(),
                    requires_items: true,
                    visibility: crate::workflow::config::CommandBindingVisibility::Always,
                },
                super::super::PickerSelectionCommand {
                    id: "selection_scope".to_string(),
                    key: crate::input::Key::Tab,
                    label: "Selection scope".to_string(),
                    requires_items: false,
                    visibility: crate::workflow::config::CommandBindingVisibility::Always,
                },
            ],
        );
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(116),
            generation: 0,
        };
        let parameters = test_parameters("", source, 1);
        picker
            .request_items("core:default", "", "", parameters.clone())
            .unwrap();
        picker.items_task_state = ItemsTaskState::Idle;
        picker.parameter_snapshot = Some(parameters.clone());
        picker.frame.results = ResultsState::Ready(String::new());

        let projection = picker
            .command_projection(&test_context("", parameters))
            .unwrap();
        assert_eq!(projection.bindings.len(), 2);
        assert!(!projection.bindings[0].enabled);
        assert!(projection.bindings[1].enabled);
        let runtime = picker
            .runtime_update(&EngineRuntimeSnapshot::default(), "")
            .unwrap();
        assert_eq!(
            runtime.value["command"],
            serde_json::json!([
                {
                    "ref": {"view": "core:default", "id": "requires_items"},
                    "label": "Requires item"
                },
                {
                    "ref": {"view": "core:default", "id": "selection_scope"},
                    "label": "Selection scope"
                }
            ])
        );
    }

    #[test]
    fn failed_feed_does_not_publish_pending_dynamic_commands() {
        let (_config, mut picker) = test_picker(110);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(110),
            generation: 0,
        };
        let context = test_context("", test_parameters("", source, 1));
        picker.frame.input_refresh = InputRefreshState::RetryRequested;
        picker.items_task_state = ItemsTaskState::Idle;

        let projection = picker.command_projection(&context).unwrap();
        assert!(projection.bindings.is_empty());
    }

    #[test]
    fn task_failure_clears_previous_items_and_feed_instances() {
        let (config, mut picker) = test_picker(111);
        picker.frame.results = ResultsState::Ready(String::new());
        picker.frame.query.clear();
        picker.frame.selection.replace(vec![test_item("stale")]);
        Arc::make_mut(&mut picker.feed_instances).insert(
            FeedId("core:default".to_string()),
            test_feed_instance(
                &config,
                "core:default",
                "apps:main",
                test_parameters(
                    "",
                    crate::input::InputSourceIdentity {
                        frame: crate::input::ViewMountId(111),
                        generation: 0,
                    },
                    1,
                ),
                "",
            ),
        );

        picker
            .handle_task_failure("source failed".to_string())
            .unwrap();

        assert!(picker.frame.selection.items.is_empty());
        assert!(picker.feed_instances.is_empty());
        assert!(matches!(picker.frame.results, ResultsState::Invalid));
        assert!(picker.frame.input_refresh.is_retry_requested());
    }

    fn response_value(
        parameters: &ParameterSnapshot,
        generation: u64,
        input: &str,
        result: std::result::Result<crate::engine::picker::items::ItemsResult, String>,
    ) -> ItemsResponse {
        let identity = FeedRequestIdentity::new(
            parameters.source().frame,
            parameters.source(),
            generation,
            input.to_string(),
            parameters.revision(),
            parameters.raw_input().to_string(),
            parameters.clone(),
        )
        .unwrap();
        ItemsResponse {
            view: "core:default".to_string(),
            identity,
            result,
        }
    }

    #[test]
    fn state_identity_advances_generation_when_view_and_raw_input_match() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(101)),
            ),
            "core:default",
        )
        .view_services();
        let mut picker = PickerView::new("core:default", services);
        let mut first = config.instantiate_parameters("core:default").unwrap();
        let mut second = config.instantiate_parameters("core:default").unwrap();
        config
            .parameter_binding(first.view_ref())
            .unwrap()
            .parse_input(&mut first, "first")
            .unwrap();
        config
            .parameter_binding(second.view_ref())
            .unwrap()
            .parse_input(&mut second, "second")
            .unwrap();
        assert_eq!(first.revision(), second.revision());
        let first_parameters = config
            .parameter_snapshot(&first, crate::input::InputSourceIdentity::default())
            .unwrap();
        let second_parameters = config
            .parameter_snapshot(&second, crate::input::InputSourceIdentity::default())
            .unwrap();

        picker
            .request_items("core:default", "same", "same", first_parameters)
            .unwrap();
        let first_generation = picker.requested_generation();
        picker
            .request_items("core:default", "same", "same", second_parameters)
            .unwrap();

        assert_eq!(picker.requested_generation(), first_generation + 1);
    }

    #[test]
    fn activate_stays_inactive_until_foreground_tick() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(102)),
            ),
            "core:default",
        )
        .view_services();
        let mut picker = PickerView::new("core:default", services);
        let state = config.instantiate_parameters("core:default").unwrap();
        let parameters = config
            .parameter_snapshot(
                &state,
                crate::input::InputSourceIdentity {
                    frame: crate::input::ViewMountId(102),
                    generation: 0,
                },
            )
            .unwrap();
        let context = ViewContext::for_test(
            crate::engine::ViewIdentity::new(
                "core:default",
                crate::workflow::config::ENGINE_PICKER,
            ),
            EditorBuffer::new("").snapshot(),
            parameters,
            state.input_rejected(),
            EngineRuntimeSnapshot::new(serde_json::json!({"ref": "foreground"})),
        );
        picker.deactivate();
        let emission = picker.activate(context.clone()).unwrap();
        assert!(matches!(
            emission.decision_ref(),
            EngineDecision::RuntimeUpdate(_)
        ));
        assert!(!picker.active, "Activate does not resume foreground work");
        picker.schedule_retry();

        picker
            .tick(EngineTick {
                context,
                content_size: (80, 24),
            })
            .unwrap();
        assert!(picker.active, "the first foreground tick resumes Picker");
    }

    #[test]
    fn input_round_trip_waits_for_ready_and_requests_the_latest_snapshot() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let runtime = serde_json::json!({
            "view": {"current": {"ref": "core:default"}},
            "session": {"input": {"raw": "A", "params": "A"}}
        });
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(103)),
            ),
            "core:default",
        )
        .view_services();
        let mut picker = PickerView::new("core:default", services);
        let mut state = config.instantiate_parameters("core:default").unwrap();
        config
            .parameter_binding(state.view_ref())
            .unwrap()
            .parse_input(&mut state, "A")
            .unwrap();
        picker.frame.results = ResultsState::Ready("A".to_string());
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(1),
            generation: 0,
        };
        let initial_parameters = config.parameter_snapshot(&state, source).unwrap();
        picker
            .request_items("core:default", "A", "A", initial_parameters)
            .unwrap();
        let first_generation = picker.requested_generation();
        config
            .parameter_binding(state.view_ref())
            .unwrap()
            .parse_input(&mut state, "AB")
            .unwrap();
        let mut input = EditorBuffer::new("AB");
        let context =
            |parameters: &ParameterSnapshot, input: &EditorBuffer, input_rejected: bool| {
                ViewContext::for_test(
                    crate::engine::ViewIdentity::new(
                        "core:default",
                        crate::workflow::config::ENGINE_PICKER,
                    ),
                    input.snapshot(),
                    parameters.clone(),
                    input_rejected,
                    EngineRuntimeSnapshot::new(
                        runtime
                            .pointer("/view/current")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                )
            };
        let parameters = config.parameter_snapshot(&state, source).unwrap();
        picker
            .handle_input_committed(&context(&parameters, &input, state.input_rejected()))
            .unwrap();

        config
            .parameter_binding(state.view_ref())
            .unwrap()
            .parse_input(&mut state, "A")
            .unwrap();
        input = EditorBuffer::new("A");
        let parameters = config.parameter_snapshot(&state, source).unwrap();
        picker
            .handle_input_committed(&context(&parameters, &input, state.input_rejected()))
            .unwrap();

        assert_eq!(state.revision(), 3);
        assert_eq!(picker.requested_generation(), first_generation);
        assert!(matches!(&picker.items_task_state, ItemsTaskState::Idle));
        assert!(picker.frame.input_refresh.is_awaiting_ready());
        assert!(!picker.frame.input_refresh.is_retry_requested());
        assert!(matches!(picker.frame.results, ResultsState::Invalid));

        let parameters = config.parameter_snapshot(&state, source).unwrap();
        let effect = picker
            .handle_input_ready(&context(&parameters, &input, state.input_rejected()))
            .unwrap();

        assert!(matches!(effect, EngineDecision::RuntimeUpdate(_)));
        assert!(matches!(
            &picker.items_task_state,
            ItemsTaskState::Prepared(_)
        ));
        assert!(picker.items_task_state.is_loading());
        assert_eq!(picker.requested_generation(), first_generation + 1);
        assert_eq!(
            picker
                .requested_identity()
                .map(|identity| identity.parameter_revision),
            Some(state.revision())
        );
        assert_eq!(
            picker
                .requested_identity()
                .map(|identity| identity.page_parameters.revision()),
            Some(state.revision())
        );
    }

    fn picker_request_state(picker: &PickerView) -> Value {
        serde_json::json!({
            "query": picker.frame.query,
            "results": format!("{:?}", picker.frame.results),
            "request_generation": picker.requested_request_ref().map(|request| request.identity.generation),
            "selected": picker.frame.selection.selected,
            "items": picker.frame.selection.items.iter().map(|item| item.text.clone()).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn stale_results_have_no_effect_on_the_current_request() {
        let (_config, mut picker) = test_picker(107);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(107),
            generation: 4,
        };
        let parameters = test_parameters("current", source, 8);
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items("core:default", "current", "current", parameters.clone())
            .unwrap();
        picker.items_task_state = ItemsTaskState::Idle;
        picker
            .request_items("core:default", "current", "current", parameters.clone())
            .unwrap();
        let current_generation = picker.requested_generation();
        picker.frame.query = "preserved query".to_string();
        picker.frame.results = ResultsState::Ready("previous".to_string());
        let expected = picker_request_state(&picker);

        let decision = picker
            .handle_task_result(response_value(
                &parameters,
                current_generation - 1,
                "current",
                Ok(crate::engine::picker::items::ItemsResult::default()),
            ))
            .unwrap();

        assert!(matches!(decision, EngineDecision::Continue));
        assert_eq!(picker_request_state(&picker), expected);
    }

    #[test]
    fn rejected_input_discards_completion_from_old_items_task() {
        let (_config, mut picker) = test_picker(118);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(118),
            generation: 0,
        };
        let parameters = test_parameters("current", source, 1);
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items("core:default", "current", "current", parameters.clone())
            .unwrap();
        let identity = picker
            .requested_identity()
            .expect("current request should have an identity")
            .clone();
        picker.items_task_state = ItemsTaskState::running(identity.clone());
        picker.frame.query = "current".to_string();
        picker.frame.results = ResultsState::Ready("current".to_string());
        picker.frame.selection.replace(vec![test_item("old")]);
        let response = response_value(
            &parameters,
            identity.generation,
            "current",
            Ok(crate::engine::picker::items::ItemsResult {
                items: vec![test_item("stale completion")],
                contexts: BTreeMap::new(),
                errors: Vec::new(),
            }),
        );
        let tasks = TaskRuntime::new();
        picker.items_task = Some(tasks.spawn(move |_context| Ok(response)));
        std::thread::sleep(std::time::Duration::from_millis(5));

        let expected = test_context("current", parameters).identity();
        picker.input_rejected(expected).unwrap();
        let rejected_state = picker_request_state(&picker);
        let rejected_publication = picker.current_publication();

        assert!(picker.poll_work().unwrap().is_none());
        assert_eq!(picker_request_state(&picker), rejected_state);
        assert_eq!(picker.current_publication(), rejected_publication);
        assert!(picker.items_task.is_none());
        tasks.shutdown_and_wait();
    }

    #[test]
    fn preview_decode_starts_only_during_prepared_auxiliary_work_start() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(107)),
            ),
            "core:default",
        )
        .view_services();
        let preview = super::super::preview::parse(
            0.35,
            24,
            Some(serde_json::json!({
                "producer": "declared", "document": {"type": "image", "path": "/missing.png"}
            })),
        )
        .unwrap();
        let mut picker = PickerView::new_with_preview("core:default", services, preview);
        picker.frame.selection.replace(vec![Item {
            text: "item".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("item".to_string()).into(),
            value: None,
            metadata: serde_json::json!({"image": "/missing.png"}),
            source_view: "core:default".to_string(),
        }]);

        assert!(!picker.preview.has_pending_task());
        let tasks = TaskRuntime::new();
        let starter = crate::task::MountTaskStarter::from_lease(
            &tasks,
            crate::task::MountTaskLease::new(crate::input::ViewMountId(107)),
        );
        picker.start_prepared_work(&starter);
        picker.start_prepared_auxiliary_work(&starter);
        assert!(!picker.preview.has_pending_task());
        assert!(picker.preview.prepared_request().is_none());
        picker
            .dispatch_action(crate::engine::ActionId::new("picker.toggle_preview"), None)
            .unwrap();
        assert!(!picker.preview.has_pending_task());
        picker.start_prepared_auxiliary_work(&starter);
        assert!(picker.preview.has_pending_task());
        picker.deactivate();
        picker.start_prepared_auxiliary_work(&starter);
        assert!(!picker.preview.has_pending_task());
        tasks.shutdown_and_wait();
    }

    #[test]
    fn input_retry_waits_for_ready_before_requesting_a_retry() {
        let (_config, mut picker) = test_picker(113);
        picker.frame.input_refresh = InputRefreshState::AwaitingReady;

        picker.schedule_retry();

        assert_eq!(picker.frame.input_refresh, InputRefreshState::AwaitingReady);
        assert!(!picker.frame.input_refresh.is_retry_requested());
    }

    #[test]
    fn retry_selection_keeps_retry_request_for_the_next_tick() {
        let (_config, mut picker) = test_picker(119);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(119),
            generation: 0,
        };
        let parameters = test_parameters("retry", source, 1);
        picker.parameter_snapshot = Some(parameters.clone());
        picker.frame.query = "retry".to_string();
        picker.started = true;
        picker
            .handle_task_failure("items failed".to_string())
            .unwrap();
        assert!(picker.frame.input_refresh.is_retry_requested());

        let context = test_context("retry", parameters);
        let selection = picker
            .action(EngineActionInput {
                invocation: ActionInvocation::new("picker.select_next"),
                context: context.clone(),
            })
            .unwrap();
        assert!(matches!(
            selection.decision_ref(),
            EngineDecision::Invalidate
        ));
        assert!(picker.frame.input_refresh.is_retry_requested());
        assert_eq!(picker.frame.pending_selection, 1);

        let emission = picker
            .tick(EngineTick {
                context,
                content_size: (80, 24),
            })
            .unwrap();
        assert!(matches!(
            emission.decision_ref(),
            EngineDecision::RuntimeUpdate(_)
        ));
        assert!(matches!(
            picker.items_task_state,
            ItemsTaskState::Prepared(_)
        ));
        assert_eq!(picker.frame.pending_selection, 1);
    }

    #[test]
    fn prepared_items_become_running_only_after_prepared_work_start() {
        let (_config, mut picker) = test_picker(114);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(114),
            generation: 0,
        };
        let parameters = test_parameters("query", source, 1);
        picker
            .request_items("core:default", "query", "query", parameters)
            .unwrap();
        let identity = picker
            .requested_identity()
            .expect("request should have an identity")
            .clone();
        assert!(matches!(
            &picker.items_task_state,
            ItemsTaskState::Prepared(_)
        ));
        assert!(picker.items_task.is_none());

        let tasks = TaskRuntime::new();
        let starter = crate::task::MountTaskStarter::from_lease(
            &tasks,
            crate::task::MountTaskLease::new(crate::input::ViewMountId(114)),
        );
        picker.start_prepared_work(&starter);

        assert_eq!(picker.running_items_task_identity(), Some(identity));
        assert!(picker.items_task.is_some());
        tasks.shutdown_and_wait();
    }

    #[test]
    fn selection_count_uses_zero_for_an_empty_list() {
        assert_eq!(selection_count(0, 0), "0 of 0");
        assert_eq!(selection_count(1, 3), "2 of 3");
    }
}

#[cfg(test)]
mod preview_provider_tests {
    use super::*;
    use crate::engine::ActionId;
    use crate::task::{MountTaskLease, MountTaskStarter, TaskRuntime};
    use serde_json::json;

    #[test]
    fn preview_uses_raw_route_input_with_object_parameters_and_stays_suspended_in_background() {
        let config = crate::workflow::config::CompiledConfig::load_unvalidated(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/preview/config.toml"),
        )
        .unwrap()
        .compile()
        .unwrap();
        let tasks = TaskRuntime::new();
        let mount = crate::input::ViewMountId(1001);
        let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(mount));
        let page = "browser:override";
        let services =
            super::super::mount_data(&config, &Value::Null, page, MountTaskLease::new(mount))
                .unwrap();
        let preview_config = super::super::preview::parse(
            0.35,
            24,
            Some(
                crate::workflow::config::toml_to_json(
                    config.view(page).unwrap().engine_field("preview").unwrap(),
                )
                .unwrap(),
            ),
        )
        .unwrap();
        let mut picker = PickerView::new_with_preview(page, services, preview_config);
        picker
            .dispatch_action(ActionId::new("picker.toggle_preview"), None)
            .unwrap();
        let raw_input = "preview needle";
        let binding_raw = "needle";
        let values = json!({"search":"needle", "owner":"browser"});
        let parameters = ParameterSnapshot::from_parts(
            values.clone(),
            binding_raw.into(),
            crate::input::InputSourceIdentity {
                frame: mount,
                generation: 3,
            },
            4,
        );
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items(page, raw_input, binding_raw, parameters)
            .unwrap();
        let identity = picker.requested_identity().unwrap().clone();
        let response = ItemsResponse {
            view: page.into(),
            identity: identity.clone(),
            result: Ok(super::super::items::ItemsResult {
                items: vec![Item {
                    text: "Needle".into(),
                    display: super::super::ItemDisplayInput::Plain("Needle".into()).into(),
                    value: Some("needle".into()),
                    metadata: json!({"summary":"route item"}),
                    source_view: "library:main".into(),
                }],
                contexts: BTreeMap::new(),
                errors: Vec::new(),
            }),
        };
        picker.collect_items(response.clone());
        assert_eq!(picker.frame.query, binding_raw);
        assert!(picker.results_current_snapshot(raw_input));
        assert!(!picker.results_current_snapshot(binding_raw));
        picker.sync_preview();
        let prepared = picker.preview.prepared_request().unwrap();
        assert_eq!(prepared.request["context"]["parameters"], values);
        assert_eq!(
            prepared.request["context"]["engine"]["state"]["input"],
            raw_input
        );
        assert_eq!(
            prepared.request["context"]["engine"]["state"]["item"]["value"],
            "needle"
        );

        picker.suspend_auxiliary_work();
        picker.items_task_state = ItemsTaskState::running(identity);
        picker.items_task = Some(tasks.spawn(move |_| Ok(response)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if picker.poll_background_work().unwrap().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background items did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!picker.active);
        assert_eq!(
            picker.frame.selection.items[0].value.as_deref(),
            Some("needle")
        );
        assert!(picker.preview.prepared_request().is_none());
        assert!(picker.start_prepared_auxiliary_work(&starter).is_empty());
        assert!(picker.preview.prepared_request().is_none());
        picker.deactivate();
        tasks.shutdown_and_wait();
    }

    #[test]
    fn preview_feed_owner_and_page_override_keep_parameters_input_and_paths_in_owner_workflow() {
        let config = crate::workflow::config::CompiledConfig::load_unvalidated(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/preview/config.toml"),
        )
        .unwrap()
        .compile()
        .unwrap();
        let engines = crate::engine::EngineRegistry::new();
        config.validate_with_engines(&engines).unwrap();
        let tasks = TaskRuntime::new();
        for (page, owner, expected_parameter) in [
            ("browser:main", "library:main", "library"),
            ("browser:override", "browser:override", "browser"),
        ] {
            let mount = crate::input::ViewMountId(999);
            let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(mount));
            let input = json!({"stdin":{"path":"launch-input","length":9,"is_tty":false}});
            let services =
                super::super::mount_data(&config, &input, page, MountTaskLease::new(mount))
                    .unwrap();
            assert!(services.workflow_root(page).unwrap().ends_with("browser"));
            if page == "browser:main" {
                assert!(
                    services
                        .workflow_root("library:main")
                        .unwrap()
                        .ends_with("library")
                );
            }
            let view = config.view(page).unwrap();
            let preview_ratio = view
                .engine_field("preview_ratio")
                .map(crate::workflow::config::toml_to_json)
                .transpose()
                .unwrap();
            let preview_min_width = view
                .engine_field("preview_min_width")
                .map(crate::workflow::config::toml_to_json)
                .transpose()
                .unwrap();
            let (preview_ratio, preview_min_width, _) = super::super::preview_options(
                preview_ratio.as_ref(),
                preview_min_width.as_ref(),
                None,
            )
            .unwrap();
            let preview_config = super::super::preview::parse(
                preview_ratio,
                preview_min_width,
                view.engine_field("preview")
                    .map(crate::workflow::config::toml_to_json)
                    .transpose()
                    .unwrap(),
            )
            .unwrap();
            let mut picker = PickerView::new_with_preview(page, services, preview_config);
            picker
                .dispatch_action(ActionId::new("picker.toggle_preview"), None)
                .unwrap();
            let parameters = ParameterSnapshot::from_parts(
                json!({"search":"","owner":"browser"}),
                String::new(),
                crate::input::InputSourceIdentity {
                    frame: mount,
                    generation: 1,
                },
                1,
            );
            picker.parameter_snapshot = Some(parameters.clone());
            picker.request_items(page, "", "", parameters).unwrap();
            picker.start_prepared_work(&starter);
            for _ in 0..200 {
                if picker.poll_work().unwrap().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            picker.sync_preview();
            let request = picker.preview.prepared_request().unwrap();
            assert_eq!(request.owner, owner);
            assert_eq!(
                request.request["context"]["parameters"]["owner"],
                expected_parameter
            );
            assert_eq!(request.request["context"]["input"], input);
            assert_eq!(
                request.request["context"]["engine"]["state"]["item"]["value"],
                "mixed"
            );
            assert!(
                request.request["context"]["engine"]["state"]["item"]
                    .get("source_view")
                    .is_none()
            );
            assert_eq!(request.root.as_deref(), config.workflow_root(owner));
            let identity = request.identity.clone();
            // Same public value, new metadata must invalidate the prepared preview.
            Arc::make_mut(&mut picker.frame.selection.items)[0].metadata["summary"] =
                json!("updated metadata");
            picker.sync_preview();
            assert_ne!(
                picker.preview.prepared_request().unwrap().identity,
                identity
            );
            let identity = picker.preview.prepared_request().unwrap().identity.clone();
            Arc::make_mut(&mut picker.frame.selection.items)[0].source_view =
                "browser:override".into();
            picker.sync_preview();
            assert_ne!(
                picker.preview.prepared_request().unwrap().identity,
                identity
            );
            picker.preview_content_size = Some((1, 1));
            picker.sync_preview();
            assert!(picker.preview.prepared_request().is_none());
            picker.preview_content_size = Some((80, 24));
            picker.sync_preview();
            assert!(picker.preview.prepared_request().is_some());
            // Real session synchronization controls visibility, starts and authoritative scrolling.
            picker.preview = super::super::preview::PickerPreview::new(
                super::super::preview::parse(0.35, 24, Some(json!({"producer":"declared", "document":(0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")}))).unwrap()
            );
            for size in [(40, 0), (23, 3)] {
                picker.set_auxiliary_content_size(size);
                assert!(picker.start_prepared_auxiliary_work(&starter).is_empty());
                let preview = &picker.preview;
                assert!(preview.prepared_request().is_none());
                assert!(!preview.render_state().visible);
                assert!(!preview.document_scroll_state().0);
            }
            picker.set_auxiliary_content_size((40, 5));
            picker.start_prepared_auxiliary_work(&starter);
            assert!(picker.preview.document_scroll_state().0);
            for _ in 0..100 {
                picker
                    .dispatch_action(ActionId::new("picker.preview_scroll_down"), None)
                    .unwrap();
            }
            assert_eq!(picker.preview.document_scroll_state().1, 5);
            picker
                .dispatch_action(ActionId::new("picker.preview_scroll_up"), None)
                .unwrap();
            assert_eq!(picker.preview.document_scroll_state().1, 2);
            picker.set_auxiliary_content_size((40, 10));
            assert_eq!(picker.preview.document_scroll_state().1, 0);
            picker.set_auxiliary_content_size((40, 5));
            assert_eq!(picker.preview.document_scroll_state().1, 0);
            picker
                .dispatch_action(ActionId::new("picker.preview_scroll_up"), None)
                .unwrap();
            assert_eq!(picker.preview.document_scroll_state().1, 0);
            picker.deactivate();
            assert!(picker.preview.prepared_request().is_none());
        }
        tasks.shutdown_and_wait();
    }
}
