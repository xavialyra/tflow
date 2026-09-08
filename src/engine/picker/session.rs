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
use crate::workflow::command::{CommandOwnerContext, ViewOutput, ViewOutputItem};
use crate::workflow::parameter::ParameterSnapshot;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

const SEARCH_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_millis(80);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PickerOptions {
    pub(super) preview_enabled: bool,
    pub(super) show_input: bool,
    pub(super) show_divider: bool,
}

impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            preview_enabled: false,
            show_input: true,
            show_divider: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PendingAccept {
    view_ref: String,
    input: String,
    input_revision: u64,
    parameters: ParameterSnapshot,
    binding_raw: String,
    generation: Option<u64>,
}

impl PendingAccept {
    fn from_context(context: &ViewContext, generation: Option<u64>) -> Self {
        Self {
            view_ref: context.view_ref().to_string(),
            input: context.input_raw().to_string(),
            input_revision: context.input_snapshot().revision,
            parameters: context.parameter_snapshot().clone(),
            binding_raw: context.parameter_raw().to_string(),
            generation,
        }
    }

    fn matches_context(&self, context: &ViewContext) -> bool {
        self.view_ref == context.view_ref()
            && self.input == context.input_raw()
            && self.input_revision == context.input_snapshot().revision
            && self.parameters == *context.parameter_snapshot()
            && self.binding_raw == context.parameter_raw()
    }

    fn matches_request(&self, request: &ItemsRequest) -> bool {
        request.matches_context(
            self.parameters.source().frame,
            &self.view_ref,
            &self.input,
            &self.binding_raw,
            &self.parameters,
        )
    }

    fn matches_response(&self, response: &ItemsResponse) -> bool {
        self.generation == Some(response.identity.generation)
            && response.identity.matches_context(
                self.parameters.source().frame,
                &self.input,
                &self.binding_raw,
                &self.parameters,
            )
            && self.view_ref == response.view
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
    pending_accept: Option<PendingAccept>,
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
    options: PickerOptions,
    items_task: Option<ItemsTaskHandle>,
    items_completion: Option<ItemsCompletion>,
    preview: Option<PickerPreview>,
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
    pub(super) fn new(view: &str, services: PickerViewServices, options: PickerOptions) -> Self {
        Self::new_with_preview(view, services, options, None)
    }

    pub(super) fn new_with_preview(
        view: &str,
        services: PickerViewServices,
        options: PickerOptions,
        preview: Option<PickerPreviewConfig>,
    ) -> Self {
        let preview_visible = options.preview_enabled;
        Self {
            state: PickerState {
                frame: PickerFrame::new(view),
                requested_request: None,
                parameter_snapshot: None,
                runtime_snapshot: Arc::new(EngineRuntimeSnapshot::default()),
                feed_instances: Arc::new(BTreeMap::new()),
                active: true,
                started: false,
                pending_accept: None,
                items_task_state: ItemsTaskState::Idle,
                preview_visible,
                initial_load_completed: false,
            },
            services,
            options,
            items_task: None,
            items_completion: None,
            preview: preview.map(PickerPreview::new),
        }
    }

    pub(crate) fn current(&self) -> &PickerFrame {
        &self.frame
    }

    pub(crate) fn preview_visible(&self) -> bool {
        self.preview_visible
    }

    pub(crate) fn preview_render_state(&self) -> Option<super::preview::PickerPreviewRenderState> {
        self.preview.as_ref().map(PickerPreview::render_state)
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
        let visible = self.preview_visible;
        let item = visible
            .then(|| {
                self.frame
                    .selection
                    .items
                    .get(self.frame.selection.selected)
                    .cloned()
            })
            .flatten();
        let workflow_root = item
            .as_ref()
            .and_then(|item| self.services.workflow_root(&item.source_view))
            .map(std::path::Path::to_path_buf);
        if let Some(preview) = &mut self.preview {
            preview.set_visible(visible);
            preview.update(item.as_ref(), workflow_root.as_deref());
        }
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

    pub(crate) fn preview_is_configured(&self) -> bool {
        self.preview.is_some()
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

    /// Owner of the selected list row, if any (feed owner or single-source page).
    pub(crate) fn selected_item_owner(&self) -> Option<&str> {
        self.frame
            .selection
            .items
            .get(self.frame.selection.selected)
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

    fn request_matches_context(&self, context: &ViewContext) -> bool {
        self.items_task_state.is_loading()
            && self.requested_matches_context_values(
                context.mount_id(),
                context.view_ref(),
                context.input_raw(),
                context.parameter_raw(),
                context.parameter_snapshot(),
            )
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

    fn pending_accept_matches_request(&self, generation: u64) -> bool {
        let Some(requested_request) = self.requested_request_ref() else {
            return false;
        };
        self.pending_accept.as_ref().is_some_and(|pending| {
            pending.generation == Some(generation) && pending.matches_request(requested_request)
        })
    }

    fn selected_output_item(&self) -> Option<ViewOutputItem> {
        self.frame
            .selection
            .selected_item()
            .map(|item| ViewOutputItem {
                text: item.text.clone(),
                value: item.value.clone(),
                metadata: item.metadata.clone(),
                source_view: item.source_view.clone(),
            })
    }

    fn append_final_decision(
        decision: EngineDecision,
        final_decision: EngineDecision,
    ) -> EngineDecision {
        match decision {
            EngineDecision::Continue => final_decision,
            EngineDecision::Batch(mut decisions) => {
                decisions.push(final_decision);
                EngineDecision::Batch(decisions)
            }
            decision => EngineDecision::Batch(vec![decision, final_decision]),
        }
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
            if let Some(pending) = &mut self.pending_accept {
                pending.generation.get_or_insert(requested_generation);
            }
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
        let request = ItemsRequest::new(view.to_string(), identity)?;
        self.requested_request = Some(request.clone());
        self.items_task_state = ItemsTaskState::Prepared(request.clone());
        if self.frame.input_refresh.is_retry_requested() {
            self.frame.input_refresh = InputRefreshState::Stable;
        }
        if let Some(pending) = &mut self.pending_accept {
            if pending.matches_request(&request) {
                pending.generation = Some(request_generation);
            } else {
                self.pending_accept = None;
            }
        }
        Ok(Some(request))
    }

    fn invalidate_items_for_committed_input(&mut self) {
        self.pending_accept = None;
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
                self.pending_accept = None;
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
        self.pending_accept = None;
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
        let response_succeeded = response.result.is_ok();
        let pending_accept = self
            .pending_accept
            .as_ref()
            .filter(|pending| response_succeeded && pending.matches_response(&response))
            .cloned();
        if !response_succeeded && self.pending_accept_matches_request(response.identity.generation)
        {
            self.pending_accept = None;
        }
        let events = self.collect_items(response);
        self.sync_preview();
        let decision = self
            .decisions_for_items_events(events, foreground)?
            .unwrap_or(EngineDecision::Continue);
        let Some(pending_accept) = pending_accept else {
            return Ok(decision);
        };
        self.pending_accept = None;
        if !foreground {
            return Ok(decision);
        }
        Ok(Self::append_final_decision(
            decision,
            EngineDecision::Return(ViewOutput::Selected {
                item: self.selected_output_item(),
                input: pending_accept.input,
            }),
        ))
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

    #[cfg(test)]
    fn handle_task_cancelled(&mut self) -> Result<EngineDecision> {
        self.handle_task_cancelled_for_scope(true)
    }

    fn handle_task_cancelled_for_scope(&mut self, _foreground: bool) -> Result<EngineDecision> {
        self.pending_accept = None;
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
                    "owner_view": item.source_view,
                }),
                serde_json::json!(item.text),
                serde_json::json!(item.value),
                item.metadata.clone(),
            ),
            None => (Value::Null, Value::Null, Value::Null, Value::Null),
        };
        ViewContextPublication::new(serde_json::json!({
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
            .filter(|command| {
                command.requires_items || !self.services.is_non_selection_command(page, &command.id)
            })
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
        let has_selected_item = effective_ready && self.selected_item_owner().is_some();
        let mut bindings = self.page_item_command_bindings(
            context.view_ref(),
            effective_ready,
            loading,
            has_selected_item,
        )?;
        if effective_ready {
            if let Some(owner) = self.selected_item_owner() {
                // A selected owner's command layer has higher precedence than
                // the page command layer, so an identical key keeps the owner command.
                let dynamic = self.dynamic_command_bindings(owner, false)?;
                for binding in dynamic {
                    bindings.retain(|candidate| {
                        candidate.key.binding_identity() != binding.key.binding_identity()
                    });
                    bindings.push(binding);
                }
            }
        } else if loading {
            let mut ambiguous_keys = HashSet::new();
            for owner in self.services.feed_owners(context.view_ref()) {
                let items_only = owner == context.view_ref();
                for mut binding in self.dynamic_command_bindings(owner, items_only)? {
                    let key = binding.key.binding_identity();
                    if ambiguous_keys.contains(&key) {
                        continue;
                    }
                    if let Some(index) = bindings
                        .iter()
                        .position(|candidate| candidate.key.binding_identity() == key)
                    {
                        if bindings[index].command == binding.command {
                            bindings[index].enabled = false;
                        } else {
                            bindings.remove(index);
                            ambiguous_keys.insert(key);
                        }
                        continue;
                    }
                    binding.enabled = false;
                    bindings.push(binding);
                }
            }
            bindings.retain(|binding| !ambiguous_keys.contains(&binding.key.binding_identity()));
        }
        Ok(bindings)
    }

    fn dynamic_command_bindings(
        &self,
        owner: &str,
        items_only: bool,
    ) -> Result<Vec<EngineCommandBinding>> {
        let mut seen_keys = HashSet::new();
        let mut bindings = Vec::new();
        for command in self.services.selection_commands(owner) {
            if items_only && !command.requires_items {
                continue;
            }
            ensure!(
                seen_keys.insert(command.key.binding_identity()),
                "picker dynamic command owner {:?} contains duplicate physical key {:?}",
                owner,
                command.key.binding_name()
            );
            let mut binding = EngineCommandBinding::new(
                QualifiedCommandId::new(owner, command.id.clone()),
                command.key,
            );
            binding.label = Some(command.label.clone());
            binding.visibility = command.visibility;
            bindings.push(binding);
        }
        Ok(bindings)
    }

    fn selected_owner_context(
        &self,
        context: &ViewContext,
        owner: &str,
    ) -> Result<Option<CommandOwnerContext>> {
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
        if owner == context.view_ref() {
            return Ok(Some(CommandOwnerContext {
                view_ref: context.view_ref().to_string(),
                parameters: context.parameter_snapshot().clone(),
                binding_raw: context.parameter_raw().to_string(),
            }));
        }
        let feed_instance = self.feed_instances.get(&item.feed_id).with_context(|| {
            format!(
                "selected item owner {:?} has no matching feed instance",
                item.source_view
            )
        })?;
        ensure!(
            feed_instance.definition.owner_view == owner,
            "selected item owner {:?} does not match feed instance {:?}",
            owner,
            feed_instance.definition.owner_view
        );
        Ok(Some(CommandOwnerContext {
            view_ref: feed_instance.definition.owner_view.clone(),
            parameters: feed_instance.parameters.clone(),
            binding_raw: feed_instance.binding_raw.clone(),
        }))
    }
}

impl PickerView {
    fn remember_context(&mut self, context: &ViewContext) {
        self.runtime_snapshot = Arc::new(context.runtime_snapshot().clone());
        if self
            .pending_accept
            .as_ref()
            .is_some_and(|pending| !pending.matches_context(context))
        {
            self.pending_accept = None;
        }
    }

    fn handle_activate(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.pending_accept = None;
        self.remember_context(context);
        self.parameter_snapshot = Some(context.parameter_snapshot().clone());
        let update = self.runtime_update(context.runtime_snapshot(), context.input_raw())?;
        Ok(EngineDecision::RuntimeUpdate(update))
    }

    fn handle_restore_input(&mut self, context: &ViewContext) -> Result<EngineDecision> {
        self.pending_accept = None;
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
        self.pending_accept = None;
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
            "picker.accept" => self.accept(context),
            "picker.cancel" => Ok(EngineDecision::Close),
            "picker.back" => Ok(EngineDecision::Close),
            "picker.exit" => Ok(EngineDecision::Exit),
            "picker.toggle_preview" => {
                if self.options.preview_enabled {
                    self.preview_visible = !self.preview_visible;
                    self.sync_preview();
                }
                Ok(EngineDecision::Invalidate)
            }
            "picker.retry" => {
                self.schedule_retry();
                Ok(EngineDecision::Invalidate)
            }
            _ => bail!("unknown picker action {:?}", id.as_str()),
        }
    }

    fn accept(&mut self, context: Option<&ViewContext>) -> Result<EngineDecision> {
        let Some(context) = context else {
            if !self.results_current_snapshot(&self.frame.query) {
                return Ok(EngineDecision::Continue);
            }
            return Ok(EngineDecision::Return(ViewOutput::Selected {
                item: self.selected_output_item(),
                input: self.frame.query.clone(),
            }));
        };
        if context.input_rejected() {
            self.pending_accept = None;
            return Ok(EngineDecision::Continue);
        }

        self.remember_context(context);
        if self.results_current_for_context(context) {
            self.pending_accept = None;
            return Ok(EngineDecision::Return(ViewOutput::Selected {
                item: self.selected_output_item(),
                input: context.input_raw().to_string(),
            }));
        }

        let generation = self
            .request_matches_context(context)
            .then_some(self.requested_generation());
        self.pending_accept = Some(PendingAccept::from_context(context, generation));
        if generation.is_none() && !self.frame.input_refresh.is_awaiting_ready() {
            return Ok(self
                .request_current(context)?
                .unwrap_or(EngineDecision::Continue));
        }
        Ok(EngineDecision::Continue)
    }
}

impl PickerView {
    fn dispatch_action_input(&mut self, input: EngineActionInput) -> Result<EngineDecision> {
        let EngineActionInput {
            invocation,
            context,
        } = input;
        if context.input_rejected() && invocation.id.as_str() == "picker.accept" {
            self.pending_accept = None;
            Ok(EngineDecision::Continue)
        } else {
            self.dispatch_action(invocation.id, Some(&context))
        }
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
        let decision = self.dispatch_action_input(input)?;
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
        self.selected_owner_context(context, owner)
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

    fn deactivate(&mut self) {
        self.pending_accept = None;
        let was_loading = self.items_task_state.is_loading();
        self.items_task_state = ItemsTaskState::Idle;
        self.items_task.take();
        self.items_completion = None;
        if was_loading {
            self.schedule_retry();
        }
        if let Some(preview) = &mut self.preview {
            preview.deactivate();
        }
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
        let picker = PickerView::new("core:default", services, PickerOptions::default());
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
            feed_id: FeedId("core:default".to_string()),
        }
    }

    fn test_feed_instance(
        config: &crate::workflow::config::CompiledConfig,
        page_view: &str,
        owner_view: &str,
        parameters: ParameterSnapshot,
        binding_raw: &str,
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
            binding_raw: binding_raw.to_string(),
        }
    }

    #[test]
    fn command_projection_qualifies_selected_feed_commands() {
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
            feed_id: FeedId("apps:main".to_string()),
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
        assert_eq!(projection.bindings.len(), 1);
        let binding = &projection.bindings[0];
        assert_eq!(binding.command.owner, "apps:main");
        assert_eq!(binding.command.id, "open");
        assert_eq!(binding.key, crate::input::Key::Enter);
        assert!(binding.enabled);
        let publication = picker.current_publication();
        let current = publication.current();
        assert_eq!(current["item"]["value"], "application");
        assert_eq!(current["value"], "application");
        assert!(current["metadata"].is_null());
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
            ("core:default".to_string(), None),
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
            serde_json::json!([{
                "ref": {"view": "core:default", "id": "selection_scope"},
                "label": "Selection scope"
            }])
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
        assert!(picker.pending_accept.is_none());
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

    fn returned_output(decision: &EngineDecision) -> Option<&ViewOutput> {
        match decision {
            EngineDecision::Return(output) => Some(output),
            EngineDecision::Batch(decisions) => decisions.iter().find_map(returned_output),
            _ => None,
        }
    }

    fn queue_pending_accept(
        picker: &mut PickerView,
        parameters: ParameterSnapshot,
        input: &str,
    ) -> u64 {
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items(
                "core:default",
                input,
                parameters.raw_input(),
                parameters.clone(),
            )
            .unwrap();
        let generation = picker.requested_generation();
        let decision = picker
            .dispatch_action_input(EngineActionInput {
                invocation: ActionInvocation::new("picker.accept"),
                context: test_context(input, parameters),
            })
            .unwrap();
        assert!(matches!(decision, EngineDecision::Continue));
        generation
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
        let mut picker = PickerView::new("core:default", services, PickerOptions::default());
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
        let mut picker = PickerView::new("core:default", services, PickerOptions::default());
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
        let mut picker = PickerView::new("core:default", services, PickerOptions::default());
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

    #[test]
    fn rejected_input_cannot_accept_stale_selection() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(
                &TaskRuntime::new(),
                MountTaskLease::new(crate::input::ViewMountId(104)),
            ),
            "core:default",
        )
        .view_services();
        let mut picker = PickerView::new("core:default", services, PickerOptions::default());
        picker.frame.selection.replace(vec![Item {
            text: "stale".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("stale".to_string()).into(),
            value: Some("stale".to_string()),
            metadata: serde_json::Value::Null,
            source_view: "core:default".to_string(),
            feed_id: FeedId("core:default".to_string()),
        }]);
        picker.frame.results = ResultsState::Ready(String::new());

        let mut state = config.instantiate_parameters("core:default").unwrap();
        state.set_input_rejected(true);
        let input = EditorBuffer::new("invalid");
        let parameters = config
            .parameter_snapshot(
                &state,
                crate::input::InputSourceIdentity {
                    frame: crate::input::ViewMountId(1),
                    generation: 0,
                },
            )
            .unwrap();
        let decision = picker
            .dispatch_action_input(EngineActionInput {
                invocation: ActionInvocation::new("picker.accept"),
                context: ViewContext::for_test(
                    crate::engine::ViewIdentity::new(
                        state.view_ref(),
                        crate::workflow::config::ENGINE_PICKER,
                    ),
                    input.snapshot(),
                    parameters,
                    state.input_rejected(),
                    EngineRuntimeSnapshot::new(Value::Null),
                ),
            })
            .unwrap();

        assert!(matches!(decision, EngineDecision::Continue));
    }

    fn picker_request_state(picker: &PickerView) -> Value {
        serde_json::json!({
            "frame": {
                "view": picker.frame.view,
                "query": picker.frame.query,
                "input_refresh": format!("{:?}", picker.frame.input_refresh),
                "results": format!("{:?}", picker.frame.results),
                "items_task": format!("{:?}", picker.items_task_state),
                "pending_selection": picker.frame.pending_selection,
                "selected": picker.frame.selection.selected,
                "items": picker.frame.selection.items.iter().map(|item| serde_json::json!({
                    "text": item.text,
                    "value": item.value,
                    "metadata": item.metadata,
                    "source_view": item.source_view,
                    "feed_id": item.feed_id.0,
                })).collect::<Vec<_>>(),
            },
            "request": picker.requested_request_ref().map(|request| serde_json::json!({
                "view": request.view,
                "input": request.identity.input,
                "binding_raw": request.identity.binding_raw,
                "parameter_revision": request.identity.parameter_revision,
                "page_parameters": {
                    "values": request.identity.page_parameters.values(),
                    "raw": request.identity.page_parameters.raw_input(),
                    "source": [request.identity.page_parameters.source().frame.0, request.identity.page_parameters.source().generation],
                    "revision": request.identity.page_parameters.revision(),
                },
                "generation": request.identity.generation,
            })),
            "parameter_snapshot": picker.parameter_snapshot.as_ref().map(|snapshot| serde_json::json!({
                "values": snapshot.values(),
                "raw": snapshot.raw_input(),
                "source": [snapshot.source().frame.0, snapshot.source().generation],
                "revision": snapshot.revision(),
            })),
            "feed_instances": picker.feed_instances.iter().map(|(id, instance)| (
                id.0.clone(),
                serde_json::json!({
                    "owner_view": instance.definition.owner_view,
                    "values": instance.parameters.values(),
                    "raw": instance.parameters.raw_input(),
                    "source": [instance.parameters.source().frame.0, instance.parameters.source().generation],
                    "revision": instance.parameters.revision(),
                    "binding_raw": instance.binding_raw,
                }),
            )).collect::<BTreeMap<_, _>>(),
        })
    }

    #[test]
    fn accept_waits_for_fresh_results_after_input_edit() {
        let (_config, mut picker) = test_picker(108);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(108),
            generation: 0,
        };
        picker.frame.query = "old".to_string();
        picker.frame.results = ResultsState::Ready("old".to_string());
        picker.frame.selection.replace(vec![test_item("stale")]);

        let old_parameters = test_parameters("old", source, 1);
        picker.parameter_snapshot = Some(old_parameters);
        let new_parameters = test_parameters("new", source, 2);
        let context = test_context("new", new_parameters.clone());
        picker.handle_input_committed(&context).unwrap();

        let decision = picker
            .dispatch_action_input(EngineActionInput {
                invocation: ActionInvocation::new("picker.accept"),
                context: context.clone(),
            })
            .unwrap();
        assert!(matches!(decision, EngineDecision::Continue));

        let request = picker.handle_input_ready(&context).unwrap();
        assert!(matches!(request, EngineDecision::RuntimeUpdate(_)));
        let generation = picker.requested_generation();
        let decision = picker
            .handle_task_result(response_value(
                &new_parameters,
                generation,
                "new",
                Ok(crate::engine::picker::items::ItemsResult {
                    items: vec![test_item("fresh")],
                    contexts: BTreeMap::new(),
                    errors: Vec::new(),
                }),
            ))
            .unwrap();

        assert_eq!(
            returned_output(&decision),
            Some(&ViewOutput::Selected {
                item: Some(crate::workflow::command::ViewOutputItem {
                    text: "fresh".to_string(),
                    value: Some("fresh".to_string()),
                    metadata: Value::Null,
                    source_view: "core:default".to_string(),
                }),
                input: "new".to_string(),
            })
        );
    }

    #[test]
    fn accept_after_selection_move_uses_the_new_generation_selection() {
        let (_config, mut picker) = test_picker(109);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(109),
            generation: 0,
        };
        let parameters = test_parameters("new", source, 1);
        let context = test_context("new", parameters.clone());
        let generation = queue_pending_accept(&mut picker, parameters.clone(), "new");
        picker.pending_accept = None;
        picker.frame.pending_selection = 0;
        let move_decision = picker
            .dispatch_action_input(EngineActionInput {
                invocation: ActionInvocation::new("picker.select_next"),
                context: context.clone(),
            })
            .unwrap();
        assert!(matches!(move_decision, EngineDecision::Invalidate));
        let accept_decision = picker
            .dispatch_action_input(EngineActionInput {
                invocation: ActionInvocation::new("picker.accept"),
                context,
            })
            .unwrap();
        assert!(matches!(accept_decision, EngineDecision::Continue));
        assert_eq!(
            picker
                .pending_accept
                .as_ref()
                .and_then(|pending| pending.generation),
            Some(generation)
        );

        let decision = picker
            .handle_task_result(response_value(
                &parameters,
                generation,
                "new",
                Ok(crate::engine::picker::items::ItemsResult {
                    items: vec![test_item("first"), test_item("second")],
                    contexts: BTreeMap::new(),
                    errors: Vec::new(),
                }),
            ))
            .unwrap();
        assert_eq!(
            returned_output(&decision),
            Some(&ViewOutput::Selected {
                item: Some(crate::workflow::command::ViewOutputItem {
                    text: "second".to_string(),
                    value: Some("second".to_string()),
                    metadata: Value::Null,
                    source_view: "core:default".to_string(),
                }),
                input: "new".to_string(),
            })
        );
    }

    #[test]
    fn late_old_generation_cannot_resolve_pending_accept() {
        let (_config, mut picker) = test_picker(110);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(110),
            generation: 0,
        };
        let old_parameters = test_parameters("old", source, 1);
        picker.parameter_snapshot = Some(old_parameters.clone());
        picker
            .request_items("core:default", "old", "old", old_parameters.clone())
            .unwrap();
        let old_generation = picker.requested_generation();
        let new_parameters = test_parameters("new", source, 2);
        let new_generation = queue_pending_accept(&mut picker, new_parameters.clone(), "new");

        let old_decision = picker
            .handle_task_result(response_value(
                &old_parameters,
                old_generation,
                "old",
                Ok(crate::engine::picker::items::ItemsResult {
                    items: vec![test_item("stale")],
                    contexts: BTreeMap::new(),
                    errors: Vec::new(),
                }),
            ))
            .unwrap();
        assert!(returned_output(&old_decision).is_none());
        assert_eq!(picker.requested_generation(), new_generation);

        let new_decision = picker
            .handle_task_result(response_value(
                &new_parameters,
                new_generation,
                "new",
                Ok(crate::engine::picker::items::ItemsResult {
                    items: vec![test_item("fresh")],
                    contexts: BTreeMap::new(),
                    errors: Vec::new(),
                }),
            ))
            .unwrap();
        assert_eq!(
            returned_output(&new_decision),
            Some(&ViewOutput::Selected {
                item: Some(crate::workflow::command::ViewOutputItem {
                    text: "fresh".to_string(),
                    value: Some("fresh".to_string()),
                    metadata: Value::Null,
                    source_view: "core:default".to_string(),
                }),
                input: "new".to_string(),
            })
        );
    }

    #[test]
    fn empty_results_resolve_pending_accept_as_free_input() {
        let (_config, mut picker) = test_picker(111);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(111),
            generation: 0,
        };
        let parameters = test_parameters("free", source, 1);
        let generation = queue_pending_accept(&mut picker, parameters.clone(), "free");
        let decision = picker
            .handle_task_result(response_value(
                &parameters,
                generation,
                "free",
                Ok(crate::engine::picker::items::ItemsResult {
                    items: Vec::new(),
                    contexts: BTreeMap::new(),
                    errors: Vec::new(),
                }),
            ))
            .unwrap();
        assert_eq!(
            returned_output(&decision),
            Some(&ViewOutput::Selected {
                item: None,
                input: "free".to_string(),
            })
        );
    }

    #[test]
    fn failed_or_cancelled_results_never_return_stale_pending_accept() {
        let (_config, mut picker) = test_picker(112);
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(112),
            generation: 0,
        };
        let parameters = test_parameters("failure", source, 1);
        queue_pending_accept(&mut picker, parameters.clone(), "failure");
        let failure = picker
            .handle_task_failure("items failed".to_string())
            .unwrap();
        assert!(returned_output(&failure).is_none());
        assert!(picker.pending_accept.is_none());

        let parameters = test_parameters("cancelled", source, 2);
        queue_pending_accept(&mut picker, parameters, "cancelled");
        let cancelled = picker.handle_task_cancelled().unwrap();
        assert!(returned_output(&cancelled).is_none());
        assert!(picker.pending_accept.is_none());
    }

    #[test]
    fn stale_results_have_no_effect_on_the_current_request() {
        let config = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
        let source = crate::input::InputSourceIdentity {
            frame: crate::input::ViewMountId(107),
            generation: 4,
        };
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&config),
            MountTaskStarter::from_lease(&TaskRuntime::new(), MountTaskLease::new(source.frame)),
            "core:default",
        )
        .view_services();
        let mut picker = PickerView::new("core:default", services, PickerOptions::default());
        let parameters = ParameterSnapshot::from_parts(
            serde_json::json!({"query": "current"}),
            "current".to_string(),
            source,
            8,
        );
        picker.parameter_snapshot = Some(parameters.clone());
        picker
            .request_items("core:default", "current", "current", parameters.clone())
            .unwrap();
        picker
            .request_items("core:default", "new", "new", parameters.clone())
            .unwrap();
        picker.frame.query = "preserved query".to_string();
        picker.frame.results = ResultsState::Ready("previous".to_string());
        picker.frame.pending_selection = 3;
        picker.frame.selection.selected = 0;
        Arc::make_mut(&mut picker.frame.selection.items).push(Item {
            text: "preserved".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("preserved".to_string()).into(),
            value: Some("value".to_string()),
            metadata: serde_json::json!({"key": "value"}),
            source_view: "core:default".to_string(),
            feed_id: FeedId("feed".to_string()),
        });
        Arc::make_mut(&mut picker.feed_instances).insert(
            FeedId("feed".to_string()),
            test_feed_instance(
                &config,
                "core:default",
                "apps:main",
                parameters.clone(),
                "feed binding",
            ),
        );
        picker.started = true;
        let expected = picker_request_state(&picker);
        let stale_generation = picker.requested_generation() - 1;

        let decision = picker
            .handle_task_result(response_value(
                &parameters,
                stale_generation,
                "new",
                Ok(crate::engine::picker::items::ItemsResult::default()),
            ))
            .unwrap();
        assert!(matches!(decision, EngineDecision::Continue));
        assert_eq!(picker_request_state(&picker), expected);

        let context = ViewContext::for_test(
            crate::engine::ViewIdentity::new(
                "core:default",
                crate::workflow::config::ENGINE_PICKER,
            ),
            EditorBuffer::new("new").snapshot(),
            parameters,
            false,
            EngineRuntimeSnapshot::default(),
        );
        let emission = picker
            .tick(EngineTick {
                context,
                content_size: (80, 24),
            })
            .unwrap();
        assert!(matches!(emission.decision_ref(), &EngineDecision::Continue));
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
    fn preview_decode_starts_only_during_prepared_work_start() {
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
            Some(serde_json::json!({
                "panes": [
                    {"slot": "items", "grow": 1},
                    {"slot": "preview", "grow": 1}
                ]
            })),
            Some(serde_json::json!({
                "blocks": [{"type": "image", "source": "/metadata/image", "grow": 1}]
            })),
        )
        .unwrap();
        let mut picker = PickerView::new_with_preview(
            "core:default",
            services,
            PickerOptions {
                preview_enabled: true,
                ..Default::default()
            },
            preview,
        );
        picker.frame.selection.replace(vec![Item {
            text: "item".to_string(),
            display: crate::engine::picker::ItemDisplayInput::Plain("item".to_string()).into(),
            value: None,
            metadata: serde_json::json!({"image": "/missing.png"}),
            source_view: "core:default".to_string(),
            feed_id: FeedId("core:default".to_string()),
        }]);

        assert!(!picker.preview.as_ref().unwrap().has_pending_task());
        let tasks = TaskRuntime::new();
        let starter = crate::task::MountTaskStarter::from_lease(
            &tasks,
            crate::task::MountTaskLease::new(crate::input::ViewMountId(107)),
        );
        picker.start_prepared_work(&starter);
        assert!(picker.preview.as_ref().unwrap().has_pending_task());
        picker.deactivate();
        picker.start_prepared_work(&starter);
        assert!(!picker.preview.as_ref().unwrap().has_pending_task());
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
