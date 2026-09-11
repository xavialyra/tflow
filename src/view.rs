//! Engine-neutral input, View, and navigation contracts.
//!

use crate::input::{InputEvent, Key};
use crate::workflow::config::{ViewPresentation, ViewPresentationMode};
use anyhow::{Context, Result, bail};
use ratatui::{Frame, layout::Rect};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::protocol::contracts::{TaskEvent, TaskId, ViewInstanceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewLocation {
    pub(crate) target: String,
    pub(crate) alias: Option<String>,
}

impl ViewLocation {
    pub(crate) fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            alias: None,
        }
    }

    pub(crate) fn label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.target)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewPublication {
    pub(crate) current: Value,
    pub(crate) ready: bool,
}

impl ViewPublication {
    pub(crate) fn new(current: Value, ready: bool) -> Self {
        Self { current, ready }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewCommandSnapshot {
    pub(crate) engine_type: String,
    pub(crate) parameters: Value,
    pub(crate) raw_input: String,
    pub(crate) runtime: Value,
    pub(crate) publication: Option<ViewPublication>,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewContext {
    pub(crate) instance: ViewInstanceId,
    pub(crate) location: ViewLocation,
    pub(crate) has_parent: bool,
    pub(crate) presentation: ViewPresentation,
    pub(crate) query: ParsedQuery,
}

impl ViewContext {
    pub(crate) fn new(instance: ViewInstanceId, target: impl Into<String>) -> Self {
        let target = target.into();
        Self {
            instance,
            location: ViewLocation::new(target.clone()),
            has_parent: false,
            presentation: ViewPresentation::default(),
            query: ParsedQuery::new(target, "query", Value::Null),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleEvent {
    Mounted,
    Activated,
    Covered,
    Closing,
    Closed,
    TransitionCommitted {
        target: ViewInstanceId,
    },
    TransitionRejected {
        target: ViewInstanceId,
        error: String,
    },
}

/// View-owned authority for accepting host-delivered task completions.
///
/// This deliberately only records correlation tuples. It does not allocate
/// task identifiers or interact with task scheduling or cancellation.
#[derive(Debug)]
pub(crate) struct ViewTaskRegistry {
    owner: ViewInstanceId,
    generations: BTreeMap<TaskId, u64>,
}

impl ViewTaskRegistry {
    pub(crate) fn new(owner: ViewInstanceId) -> Self {
        Self {
            owner,
            generations: BTreeMap::new(),
        }
    }

    pub(crate) fn register(&mut self, task: TaskId, generation: u64) {
        self.generations.insert(task, generation);
    }

    pub(crate) fn accepts(&self, event: &TaskEvent) -> bool {
        event.instance == self.owner && self.generations.get(&event.task) == Some(&event.generation)
    }

    pub(crate) fn invalidate(&mut self, task: TaskId) {
        self.generations.remove(&task);
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.generations.clear();
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TerminalSize {
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewEvent {
    Lifecycle(LifecycleEvent),
    Input(InputEvent),
    Command(CommandResult),
    Task(TaskEvent),
    Tick,
    Resize(TerminalSize),
}

#[derive(Debug, Clone)]
pub(crate) struct CommandRequest {
    pub(crate) invocation: crate::workflow::command::CommandInvocation,
    pub(crate) owner: Option<crate::workflow::command::CommandOwnerContext>,
}

impl PartialEq for CommandRequest {
    fn eq(&self, other: &Self) -> bool {
        self.invocation.source_view() == other.invocation.source_view()
            && self.invocation.id() == other.invocation.id()
            && self
                .owner
                .as_ref()
                .map(|owner| (&owner.view_ref, &owner.parameters, &owner.binding_raw))
                == other
                    .owner
                    .as_ref()
                    .map(|owner| (&owner.view_ref, &owner.parameters, &owner.binding_raw))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandResult {
    EditInput { value: String, cursor: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) key: Key,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BindingSet {
    entries: Vec<Binding>,
}

impl BindingSet {
    pub(crate) fn new(entries: impl IntoIterator<Item = Binding>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    pub(crate) fn entries(&self) -> &[Binding] {
        &self.entries
    }

    pub(crate) fn contains(&self, key: Key) -> bool {
        self.entries
            .iter()
            .any(|binding| binding.key.binding_identity() == key.binding_identity())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelativeCursor {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) visible: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ViewMetadata {
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    /// `None` means use the View's binding declaration. `Some(empty)` is an
    /// explicit declaration that this rendered state has no local bindings.
    pub(crate) bindings: Option<BindingSet>,
}

/// Read-only metadata consumed by the host chrome. View-private content such
/// as editors, query lines, selections, and terminal surfaces is not exposed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ViewChrome {
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) bindings: Option<BindingSet>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RenderResult {
    pub(crate) cursor: Option<RelativeCursor>,
    pub(crate) metadata: ViewMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderContext {
    pub(crate) terminal: TerminalSize,
    pub(crate) image_picker: Option<crate::terminal::ImagePicker>,
}

impl RenderContext {
    pub(crate) fn new(
        terminal: TerminalSize,
        image_picker: Option<crate::terminal::ImagePicker>,
    ) -> Self {
        Self {
            terminal,
            image_picker,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_terminal(terminal: TerminalSize) -> Self {
        Self {
            terminal,
            image_picker: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectRequest {
    CopyToClipboard(String),
    RunPrepared {
        prepared: crate::execution::PreparedProcess,
        success_message: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ViewResult {
    pub(crate) value: Value,
}

impl PartialEq for ViewResult {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavigationRequest {
    pub(crate) target: String,
    pub(crate) query: ParsedQuery,
    pub(crate) input: Option<ViewInputSeed>,
    pub(crate) presentation: ViewPresentation,
}

impl NavigationRequest {
    pub(crate) fn new(target: impl Into<String>, query: ParsedQuery) -> Self {
        Self {
            target: target.into(),
            query,
            input: None,
            presentation: ViewPresentation::default(),
        }
    }

    pub(crate) fn with_input(mut self, text: impl Into<String>, cursor: usize) -> Result<Self> {
        let text = text.into();
        if cursor > text.len() || !text.is_char_boundary(cursor) {
            bail!("View input cursor is not a UTF-8 boundary");
        }
        self.input = Some(ViewInputSeed { text, cursor });
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewInputSeed {
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedQuery {
    pub(crate) target: String,
    pub(crate) schema: String,
    pub(crate) values: Value,
}

impl ParsedQuery {
    pub(crate) fn new(target: impl Into<String>, schema: impl Into<String>, values: Value) -> Self {
        Self {
            target: target.into(),
            schema: schema.into(),
            values,
        }
    }

    pub(crate) fn validate_shape(&self) -> Result<()> {
        if self.target.is_empty() || self.schema.is_empty() {
            bail!("parsed query target and schema must be non-empty");
        }
        Ok(())
    }
}

pub(crate) trait CallReturnHandler {
    /// Return processors run only after the child is closed and the caller
    /// has been activated.
    fn post_commit(&self) -> bool {
        false
    }

    fn resume(
        &self,
        source: &ViewLocation,
        caller: &ViewContext,
        snapshot: &ViewCommandSnapshot,
        result: &ViewResult,
    ) -> Result<ViewDecision>;
}

#[derive(Clone)]
pub(crate) struct CallBoundary {
    pub(crate) caller: ViewInstanceId,
    pub(crate) handler: Arc<dyn CallReturnHandler>,
}

impl std::fmt::Debug for CallBoundary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CallBoundary")
            .field("caller", &self.caller)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CallBoundary {
    fn eq(&self, other: &Self) -> bool {
        self.caller == other.caller && Arc::ptr_eq(&self.handler, &other.handler)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Continuation {
    None,
    ReturnTo(ViewInstanceId),
    Call(CallBoundary),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TransitionRequest {
    Push(NavigationRequest),
    Replace(NavigationRequest),
    Call {
        request: NavigationRequest,
        continuation: Continuation,
    },
}

#[derive(Debug)]
pub(crate) struct ViewOperationFailure {
    message: String,
}

impl std::fmt::Display for ViewOperationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ViewOperationFailure {}

pub(crate) fn operation_failure(error: impl std::fmt::Display) -> anyhow::Error {
    ViewOperationFailure {
        message: error.to_string(),
    }
    .into()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewDecision {
    Stay,
    Invalidate,
    Transition(TransitionRequest),
    Return(ViewResult),
    Effect(EffectRequest),
    RequestCommand(CommandRequest),
    Command(CommandResult),
    Batch(Vec<ViewDecision>),
    Close,
    CloseToRoot,
    CloseWithError(String),
    Exit,
}

impl ViewDecision {
    fn is_structural(&self) -> bool {
        matches!(
            self,
            Self::Transition(_)
                | Self::Return(_)
                | Self::RequestCommand(_)
                | Self::Command(_)
                | Self::Close
                | Self::CloseToRoot
                | Self::CloseWithError(_)
                | Self::Exit
        )
    }
}

fn flatten_decisions(decisions: Vec<ViewDecision>) -> Vec<ViewDecision> {
    fn append(decision: ViewDecision, flattened: &mut Vec<ViewDecision>) {
        match decision {
            ViewDecision::Batch(decisions) => {
                for decision in decisions {
                    append(decision, flattened);
                }
            }
            decision => flattened.push(decision),
        }
    }

    let mut flattened = Vec::new();
    for decision in decisions {
        append(decision, &mut flattened);
    }
    flattened
}

fn validate_batch(decisions: &[ViewDecision]) -> Result<()> {
    let effective = decisions
        .iter()
        .enumerate()
        .filter(|(_, decision)| !matches!(decision, ViewDecision::Stay | ViewDecision::Invalidate))
        .collect::<Vec<_>>();
    let effects = effective
        .iter()
        .filter(|(_, decision)| matches!(decision, ViewDecision::Effect(_)))
        .count();
    let structural = effective
        .iter()
        .filter(|(_, decision)| decision.is_structural())
        .collect::<Vec<_>>();

    anyhow::ensure!(effects <= 1, "a View batch may contain at most one effect");
    anyhow::ensure!(
        structural.len() <= 1,
        "a View batch may contain at most one structural decision"
    );
    if effects == 1 && !structural.is_empty() {
        let effect_then_exit = effective.len() == 2
            && matches!(effective[0].1, ViewDecision::Effect(_))
            && matches!(effective[1].1, ViewDecision::Exit);
        anyhow::ensure!(
            effect_then_exit,
            "an effect cannot be combined with a structural decision except Effect followed by Exit"
        );
    }
    if let Some((index, _)) = structural.first() {
        let last_effective = effective.last().map(|(index, _)| *index);
        anyhow::ensure!(
            Some(*index) == last_effective,
            "a structural decision must be the final effective decision in a View batch"
        );
    }
    Ok(())
}

pub(crate) trait View {
    fn preferred_top_inset(&self) -> u16 {
        0
    }

    fn bindings(&self, context: &ViewContext) -> BindingSet;

    /// Configured business commands are projected separately from Engine input
    /// actions so the host can own command presentation and palette folding.
    fn command_bindings(&self) -> Option<&crate::protocol::ViewCommandBindings> {
        None
    }

    fn business_bindings(&self, _context: &ViewContext) -> BindingSet {
        let Some(commands) = self.command_bindings() else {
            return BindingSet::default();
        };
        BindingSet::new(commands.business.iter().filter_map(|(key, label)| {
            key.map(|key| Binding {
                key,
                label: Some(label.clone()),
            })
        }))
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: "test".to_string(),
            parameters: Value::Null,
            raw_input: String::new(),
            runtime: Value::Null,
            publication: None,
            revision: 0,
        }
    }

    #[allow(dead_code)]
    fn publication(&self) -> Option<&ViewPublication> {
        None
    }

    fn chrome(&self, context: &ViewContext) -> Result<ViewChrome> {
        Ok(ViewChrome {
            bindings: Some(self.bindings(context)),
            ..ViewChrome::default()
        })
    }

    fn event(&mut self, event: ViewEvent, context: &ViewContext) -> Result<ViewDecision>;

    fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        context: &RenderContext,
    ) -> Result<RenderResult>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteTarget {
    pub(crate) reference: String,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteCandidate {
    pub(crate) target: RouteTarget,
    pub(crate) label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuerySchema {
    pub(crate) id: String,
}

pub(crate) trait RouteCatalog {
    fn resolve(&self, selector: &str) -> Option<RouteTarget>;
    fn complete(&self, prefix: &str) -> Vec<RouteCandidate>;
    fn query_schema(&self, target: &str) -> Option<QuerySchema>;

    fn validate_query(&self, query: &ParsedQuery) -> Result<()> {
        query.validate_shape()?;
        let schema = self
            .query_schema(&query.target)
            .with_context(|| format!("unknown query schema for {:?}", query.target))?;
        anyhow::ensure!(
            schema.id == query.schema,
            "query schema does not match target"
        );
        Ok(())
    }
}

pub(crate) trait HostServices {
    fn task_runtime(&self) -> Option<crate::task::TaskRuntime> {
        None
    }
}

pub(crate) struct ViewServices<'a> {
    pub(crate) host: &'a dyn HostServices,
    pub(crate) routes: &'a dyn RouteCatalog,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectError {
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouterError {
    pub(crate) source: Option<ViewInstanceId>,
    pub(crate) message: String,
}

pub(crate) enum EffectResult {
    Complete,
    Error(EffectError),
}

pub(crate) trait EffectExecutor {
    fn execute(&mut self, effect: EffectRequest, context: &ViewContext) -> Result<EffectResult>;
}

pub(crate) trait ViewFactory {
    fn create(
        &self,
        request: &NavigationRequest,
        instance: ViewInstanceId,
        services: &ViewServices<'_>,
    ) -> Result<Box<dyn View>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StackState {
    Active,
    Covered,
}

fn deliver_lifecycle(
    view: &mut dyn View,
    context: &ViewContext,
    instance: ViewInstanceId,
    lifecycle: LifecycleEvent,
) -> Result<()> {
    let decision = view.event(ViewEvent::Lifecycle(lifecycle.clone()), context)?;
    anyhow::ensure!(
        matches!(decision, ViewDecision::Stay | ViewDecision::Invalidate),
        "View {:?} returned an invalid decision for {:?} lifecycle",
        instance,
        lifecycle
    );
    Ok(())
}

pub(crate) struct ViewInstance {
    pub(crate) id: ViewInstanceId,
    pub(crate) context: ViewContext,
    pub(crate) view: Box<dyn View>,
    state: StackState,
    pub(crate) continuation: Continuation,
}

#[cfg(test)]
impl ViewInstance {
    pub(crate) fn is_active(&self) -> bool {
        self.state == StackState::Active
    }
}

pub(crate) struct Router {
    routes: Box<dyn RouteCatalog>,
    factory: Box<dyn ViewFactory>,
    host: Box<dyn HostServices>,
    global_bindings: BindingSet,
    global_actions: std::collections::HashMap<crate::input::BindingKey, ViewDecision>,
    stack: Vec<ViewInstance>,
    next_instance: u64,
    pending_result: Option<ViewResult>,
    result_committed: bool,
    last_error: Option<RouterError>,
    pending_info: Option<(ViewInstanceId, String, String)>,
    popup_closed: bool,
}

impl Router {
    #[cfg(test)]
    pub(crate) fn new(routes: Box<dyn RouteCatalog>, factory: Box<dyn ViewFactory>) -> Self {
        struct TestHost;
        impl HostServices for TestHost {}
        Self::with_host_services(routes, factory, Box::new(TestHost))
    }

    pub(crate) fn with_host_services(
        routes: Box<dyn RouteCatalog>,
        factory: Box<dyn ViewFactory>,
        host: Box<dyn HostServices>,
    ) -> Self {
        Self {
            routes,
            factory,
            host,
            global_bindings: BindingSet::default(),
            global_actions: std::collections::HashMap::new(),
            stack: Vec::new(),
            next_instance: 1,
            pending_result: None,
            result_committed: false,
            last_error: None,
            pending_info: None,
            popup_closed: false,
        }
    }

    pub(crate) fn stack(&self) -> &[ViewInstance] {
        &self.stack
    }

    pub(crate) fn active(&self) -> Option<&ViewInstance> {
        self.stack.last()
    }

    pub(crate) fn take_popup_closed(&mut self) -> bool {
        std::mem::take(&mut self.popup_closed)
    }

    fn pop_view(&mut self) -> Option<ViewInstance> {
        let view = self.stack.pop()?;
        if view.context.presentation.mode == ViewPresentationMode::Popup {
            self.popup_closed = true;
        }
        Some(view)
    }

    fn remove_view(&mut self, index: usize) -> ViewInstance {
        let view = self.stack.remove(index);
        if view.context.presentation.mode == ViewPresentationMode::Popup {
            self.popup_closed = true;
        }
        view
    }

    #[cfg(test)]
    pub(crate) fn set_global_bindings(&mut self, bindings: BindingSet) {
        self.global_bindings = bindings;
        self.global_actions.clear();
    }

    /// Replaces global bindings and actions atomically. Duplicate physical
    /// keys are rejected before either map is changed.
    #[cfg(test)]
    pub(crate) fn set_global_actions(
        &mut self,
        actions: impl IntoIterator<Item = (Binding, ViewDecision)>,
    ) -> Result<()> {
        let mut bindings = Vec::new();
        let mut resolved = std::collections::HashMap::new();
        for (binding, decision) in actions {
            let identity = binding.key.binding_identity();
            anyhow::ensure!(
                !resolved.contains_key(&identity),
                "global binding key {:?} is registered more than once",
                binding.key.binding_name()
            );
            resolved.insert(identity, decision);
            bindings.push(binding);
        }
        self.global_bindings = BindingSet::new(bindings);
        self.global_actions = resolved;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_global_action(&mut self, key: Key, decision: ViewDecision) -> Result<()> {
        let mut actions = self
            .global_bindings
            .entries
            .iter()
            .cloned()
            .map(|binding| {
                let decision = self
                    .global_actions
                    .get(&binding.key.binding_identity())
                    .cloned()
                    .unwrap_or(ViewDecision::Stay);
                (binding, decision)
            })
            .collect::<Vec<_>>();
        actions.push((Binding { key, label: None }, decision));
        self.set_global_actions(actions)
    }

    pub(crate) fn global_bindings(&self) -> &BindingSet {
        &self.global_bindings
    }

    pub(crate) fn take_result(&mut self) -> Option<ViewResult> {
        if !self.result_committed {
            return None;
        }
        self.result_committed = false;
        self.pending_result.take()
    }

    pub(crate) fn take_recorded_error(&mut self) -> Option<RouterError> {
        self.last_error.take()
    }

    #[cfg(test)]
    pub(crate) fn take_error(&mut self) -> Option<RouterError> {
        self.take_recorded_error()
    }

    pub(crate) fn push(&mut self, request: NavigationRequest) -> Result<ViewInstanceId> {
        self.transition_new(request, false, Continuation::None, None)
    }

    #[cfg(test)]
    pub(crate) fn replace(&mut self, request: NavigationRequest) -> Result<ViewInstanceId> {
        self.transition_new(request, true, Continuation::None, None)
    }

    #[cfg(test)]
    pub(crate) fn call(
        &mut self,
        request: NavigationRequest,
        continuation: Continuation,
    ) -> Result<ViewInstanceId> {
        self.transition_new(request, false, continuation, None)
    }

    fn transition_new(
        &mut self,
        request: NavigationRequest,
        replace: bool,
        continuation: Continuation,
        source: Option<ViewInstanceId>,
    ) -> Result<ViewInstanceId> {
        let source_id = source.or_else(|| self.stack.last().map(|entry| entry.id));
        let target = match self.routes.resolve(&request.target) {
            Some(target) => target,
            None => {
                let error = anyhow::anyhow!("unknown navigation target {:?}", request.target);
                self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
                return Err(error);
            }
        };
        if request.query.target != target.reference {
            let error = anyhow::anyhow!(
                "navigation query target {:?} does not match target {:?}",
                request.query.target,
                target.reference
            );
            self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
            return Err(error);
        }
        if let Err(error) = self.routes.validate_query(&request.query) {
            self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
            return Err(error);
        }
        if let Some(id) = match &continuation {
            Continuation::ReturnTo(id) => Some(*id),
            Continuation::Call(boundary) => Some(boundary.caller),
            Continuation::None => None,
        } && !self.stack.iter().any(|entry| entry.id == id)
        {
            let error = anyhow::anyhow!("continuation target {:?} is not mounted", id);
            self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
            return Err(error);
        }
        let instance = ViewInstanceId(self.next_instance);
        self.next_instance = self.next_instance.wrapping_add(1).max(1);
        let mut canonical_request = request.clone();
        canonical_request.target = target.reference.clone();
        canonical_request.query.target = target.reference.clone();
        let services = ViewServices {
            host: &*self.host,
            routes: &*self.routes,
        };
        let mut view = match self.factory.create(&canonical_request, instance, &services) {
            Ok(view) => view,
            Err(error) => {
                self.notify_transition_rejected(source_id, instance, &error);
                return Err(error);
            }
        };
        let context = ViewContext {
            instance,
            location: ViewLocation {
                target: target.reference,
                alias: target.label,
            },
            has_parent: self.stack.len() > usize::from(replace),
            presentation: request.presentation.clone(),
            query: canonical_request.query.clone(),
        };
        if let Err(error) =
            deliver_lifecycle(&mut *view, &context, instance, LifecycleEvent::Mounted)
        {
            let _ = deliver_lifecycle(&mut *view, &context, instance, LifecycleEvent::Closing);
            let _ = deliver_lifecycle(&mut *view, &context, instance, LifecycleEvent::Closed);
            self.notify_transition_rejected(source_id, instance, &error);
            return Err(error);
        }

        // Mount is staged before the old stack is touched. If covering fails,
        // close the staged View and preserve the old stack.
        if let Some(previous) = self.stack.last_mut() {
            if let Err(error) = deliver_lifecycle(
                &mut *previous.view,
                &previous.context,
                previous.id,
                LifecycleEvent::Covered,
            ) {
                // Covered may have released resources before reporting a
                // failure. Restore the source lifecycle before rejecting.
                let restore_error = deliver_lifecycle(
                    &mut *previous.view,
                    &previous.context,
                    previous.id,
                    LifecycleEvent::Activated,
                )
                .err();
                let _ = deliver_lifecycle(&mut *view, &context, instance, LifecycleEvent::Closing);
                let _ = deliver_lifecycle(&mut *view, &context, instance, LifecycleEvent::Closed);
                self.notify_transition_rejected(source_id, instance, &error);
                if let Some(restore_error) = restore_error {
                    self.record_error(
                        source_id,
                        &anyhow::anyhow!("could not restore covered View: {restore_error}"),
                    );
                }
                return Err(error);
            }
            previous.state = StackState::Covered;
        }
        self.stack.push(ViewInstance {
            id: instance,
            context,
            view,
            state: StackState::Active,
            continuation,
        });
        let activated = {
            let active = self.stack.last_mut().expect("new View was committed");
            deliver_lifecycle(
                &mut *active.view,
                &active.context,
                active.id,
                LifecycleEvent::Activated,
            )
        };
        if let Err(error) = activated {
            self.rollback_activation(source_id, instance, &error)?;
            return Err(error);
        }
        if replace && self.stack.len() > 1 {
            let previous = self.stack.len() - 2;
            if let Err(error) = self.close_instance_at(previous) {
                // A replace is atomic across source cleanup. The staged View
                // must not become visible when the old View cannot close.
                let mut rejected = self.pop_view().expect("new View was committed");
                let cleanup_error = deliver_lifecycle(
                    &mut *rejected.view,
                    &rejected.context,
                    rejected.id,
                    LifecycleEvent::Closing,
                )
                .and_then(|_| {
                    deliver_lifecycle(
                        &mut *rejected.view,
                        &rejected.context,
                        rejected.id,
                        LifecycleEvent::Closed,
                    )
                })
                .err();
                let restore_error = if let Some(previous) = self.stack.last_mut() {
                    previous.state = StackState::Active;
                    deliver_lifecycle(
                        &mut *previous.view,
                        &previous.context,
                        previous.id,
                        LifecycleEvent::Activated,
                    )
                    .err()
                } else {
                    None
                };
                self.notify_transition_rejected(source_id, instance, &error);
                if let Some(cleanup_error) = cleanup_error {
                    self.record_error(
                        source_id,
                        &anyhow::anyhow!("could not roll back replacement View: {cleanup_error}"),
                    );
                }
                if let Some(restore_error) = restore_error {
                    self.record_error(
                        source_id,
                        &anyhow::anyhow!("could not restore replaced View: {restore_error}"),
                    );
                }
                return Err(error);
            }
            self.remove_view(previous);
        }
        // The stack is committed at this point. A source callback is
        // observational, so its failure must not report the committed
        // transition as rejected or roll it back.
        self.notify_transition_committed(source_id, instance);
        Ok(instance)
    }

    fn rollback_activation(
        &mut self,
        source: Option<ViewInstanceId>,
        target: ViewInstanceId,
        activation_error: &anyhow::Error,
    ) -> Result<()> {
        let rejected = self.stack.len() - 1;
        if let Err(cleanup_error) = self.close_instance_at(rejected) {
            let error = anyhow::anyhow!(
                "View activation failed ({activation_error}); cleanup failed ({cleanup_error})"
            );
            self.record_error(Some(target), &error);
            return Err(error);
        }
        self.pop_view();
        if let Some(previous) = self.stack.last_mut() {
            let previous_id = previous.id;
            previous.state = StackState::Active;
            if let Err(error) = deliver_lifecycle(
                &mut *previous.view,
                &previous.context,
                previous.id,
                LifecycleEvent::Activated,
            ) {
                self.record_error(Some(previous_id), &error);
                return Err(error);
            }
        }
        self.notify_transition_rejected(source, target, activation_error);
        Ok(())
    }

    fn close_instance_at(&mut self, index: usize) -> Result<()> {
        self.close_start_at(index)?;
        self.close_finish_at(index)
    }

    fn close_start_at(&mut self, index: usize) -> Result<()> {
        let instance = self
            .stack
            .get(index)
            .map(|entry| entry.id)
            .ok_or_else(|| anyhow::anyhow!("View stack index {index} is out of bounds"))?;
        let result = {
            let entry = self
                .stack
                .get_mut(index)
                .expect("View stack index was checked above");
            deliver_lifecycle(
                &mut *entry.view,
                &entry.context,
                instance,
                LifecycleEvent::Closing,
            )
        };
        if let Err(error) = result {
            self.record_error(Some(instance), &error);
            return Err(error);
        }
        Ok(())
    }

    fn close_finish_at(&mut self, index: usize) -> Result<()> {
        let instance = self
            .stack
            .get(index)
            .map(|entry| entry.id)
            .ok_or_else(|| anyhow::anyhow!("View stack index {index} is out of bounds"))?;
        let result = {
            let entry = self
                .stack
                .get_mut(index)
                .expect("View stack index was checked above");
            deliver_lifecycle(
                &mut *entry.view,
                &entry.context,
                instance,
                LifecycleEvent::Closed,
            )
        };
        if let Err(error) = result {
            self.record_error(Some(instance), &error);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn take_info(&mut self) -> Option<(ViewInstanceId, String, String)> {
        self.pending_info.take()
    }

    pub(crate) fn record_error(&mut self, source: Option<ViewInstanceId>, error: &anyhow::Error) {
        self.last_error = Some(RouterError {
            source,
            message: error.to_string(),
        });
    }

    fn notify_transition_committed(
        &mut self,
        source: Option<ViewInstanceId>,
        target: ViewInstanceId,
    ) {
        let Some(source) = source else {
            return;
        };
        let Some(entry) = self.stack.iter_mut().find(|entry| entry.id == source) else {
            return;
        };
        if let Err(error) = deliver_lifecycle(
            &mut *entry.view,
            &entry.context,
            entry.id,
            LifecycleEvent::TransitionCommitted { target },
        ) {
            self.record_error(Some(source), &error);
        }
    }

    fn notify_transition_rejected(
        &mut self,
        source: Option<ViewInstanceId>,
        target: ViewInstanceId,
        error: &anyhow::Error,
    ) {
        let message = error.to_string();
        self.record_error(source, error);
        let callback_error = source.and_then(|source| {
            self.stack
                .iter_mut()
                .find(|entry| entry.id == source)
                .and_then(|entry| {
                    deliver_lifecycle(
                        &mut *entry.view,
                        &entry.context,
                        entry.id,
                        LifecycleEvent::TransitionRejected {
                            target,
                            error: message,
                        },
                    )
                    .err()
                })
        });
        if let (Some(source), Some(callback_error)) = (source, callback_error) {
            let combined = anyhow::anyhow!(
                "transition rejection callback failed for {:?}: {callback_error}",
                source
            );
            self.record_error(Some(source), &combined);
        }
    }

    #[cfg(test)]
    pub(crate) fn dispatch(&mut self, event: ViewEvent) -> Result<ViewDecision> {
        self.dispatch_inner(event, &mut None)
    }

    pub(crate) fn dispatch_with_effects(
        &mut self,
        event: ViewEvent,
        executor: &mut dyn EffectExecutor,
    ) -> Result<ViewDecision> {
        self.dispatch_inner(event, &mut Some(executor))
    }

    pub(crate) fn process_with_effects(
        &mut self,
        decision: ViewDecision,
        source: ViewInstanceId,
        executor: &mut dyn EffectExecutor,
    ) -> Result<()> {
        self.process_decision_inner(decision, &mut Some(executor), Some(source))
    }

    fn dispatch_inner(
        &mut self,
        event: ViewEvent,
        executor: &mut Option<&mut dyn EffectExecutor>,
    ) -> Result<ViewDecision> {
        if let ViewEvent::Input(InputEvent::Key { key, .. }) = &event
            && self.global_bindings.contains(*key)
        {
            let decision = self
                .global_actions
                .get(&key.binding_identity())
                .cloned()
                .unwrap_or(ViewDecision::Stay);
            self.process_decision_inner(decision.clone(), executor, None)?;
            return Ok(decision);
        }
        if matches!(event, ViewEvent::Tick) {
            let ids = self.stack.iter().map(|entry| entry.id).collect::<Vec<_>>();
            let mut decision = ViewDecision::Stay;
            for id in ids {
                let Some(index) = self.stack.iter().position(|entry| entry.id == id) else {
                    continue;
                };
                let entry = &mut self.stack[index];
                decision = match entry.view.event(event.clone(), &entry.context) {
                    Ok(decision) => decision,
                    Err(error) => {
                        self.record_error(Some(id), &error);
                        return Err(error);
                    }
                };
                self.process_decision_inner(decision.clone(), executor, Some(id))?;
            }
            return Ok(decision);
        }
        let target = match &event {
            ViewEvent::Task(task) => self
                .stack
                .iter()
                .position(|entry| entry.id == task.instance),
            _ => self.stack.len().checked_sub(1),
        };
        let Some(target) = target else {
            // A task for a closed or replaced instance is intentionally ignored.
            return Ok(ViewDecision::Stay);
        };
        let source = self.stack[target].id;
        let entry = &mut self.stack[target];
        let decision = match entry.view.event(event, &entry.context) {
            Ok(decision) => decision,
            Err(error) => {
                self.record_error(Some(source), &error);
                return Err(error);
            }
        };
        self.process_decision_inner(decision.clone(), executor, Some(source))?;
        Ok(decision)
    }

    fn process_decision_inner(
        &mut self,
        decision: ViewDecision,
        executor: &mut Option<&mut dyn EffectExecutor>,
        source: Option<ViewInstanceId>,
    ) -> Result<()> {
        if decision.is_structural()
            && let Some(source) = source
            && self.active().map(|entry| entry.id) != Some(source)
        {
            let error = anyhow::anyhow!(
                "covered or closed View {:?} cannot apply a structural decision",
                source
            );
            self.record_error(Some(source), &error);
            return Err(error);
        }
        match decision {
            ViewDecision::Transition(TransitionRequest::Push(request)) => {
                self.transition_new(request, false, Continuation::None, source)?;
            }
            ViewDecision::Transition(TransitionRequest::Replace(request)) => {
                self.transition_new(request, true, Continuation::None, source)?;
            }
            ViewDecision::Transition(TransitionRequest::Call {
                request,
                continuation,
            }) => {
                self.transition_new(request, false, continuation, source)?;
            }
            ViewDecision::Return(result) => {
                self.pending_result = Some(result);
                self.result_committed = false;
                self.finish_return(executor)?;
            }
            ViewDecision::Batch(decisions) => {
                let mut decisions = flatten_decisions(decisions);
                if let Err(error) = validate_batch(&decisions) {
                    self.record_error(source, &error);
                    return Err(error);
                }
                for decision in decisions.drain(..) {
                    self.process_decision_inner(decision, executor, source)?;
                }
            }
            ViewDecision::Effect(effect) => {
                let context = match source {
                    Some(source) => self
                        .stack
                        .iter()
                        .find(|entry| entry.id == source)
                        .map(|entry| entry.context.clone())
                        .ok_or_else(|| {
                            anyhow::anyhow!("effect source {:?} is no longer mounted", source)
                        })?,
                    None => self
                        .active()
                        .map(|entry| entry.context.clone())
                        .ok_or_else(|| {
                            anyhow::anyhow!("cannot execute an effect without a View")
                        })?,
                };
                let success_message = match &effect {
                    EffectRequest::CopyToClipboard(_) => Some("Copied to clipboard".to_string()),
                    EffectRequest::RunPrepared {
                        success_message, ..
                    } => success_message.clone(),
                };
                let result = match executor.as_mut() {
                    Some(executor) => match (**executor).execute(effect, &context) {
                        Ok(result) => result,
                        Err(error) => {
                            self.record_error(Some(context.instance), &error);
                            return Err(error);
                        }
                    },
                    None => {
                        let error = anyhow::anyhow!("effect executor is unavailable");
                        self.record_error(Some(context.instance), &error);
                        return Err(error);
                    }
                };
                match result {
                    EffectResult::Complete => {
                        if let Some(message) = success_message {
                            self.pending_info =
                                Some((context.instance, context.location.target.clone(), message));
                        }
                    }
                    EffectResult::Error(error) => {
                        let EffectError::Failed(message) = error;
                        let error = anyhow::anyhow!("external effect failed: {message}");
                        self.record_error(Some(context.instance), &error);
                        return Err(error);
                    }
                }
            }
            ViewDecision::Command(command) => {
                let target = match source {
                    Some(source) => self
                        .stack
                        .iter()
                        .position(|entry| entry.id == source)
                        .ok_or_else(|| {
                            anyhow::anyhow!("command source {:?} is no longer mounted", source)
                        })?,
                    None => self.stack.len().checked_sub(1).ok_or_else(|| {
                        anyhow::anyhow!("cannot deliver a command without a View")
                    })?,
                };
                let source = self.stack[target].id;
                let next = {
                    let entry = &mut self.stack[target];
                    match entry
                        .view
                        .event(ViewEvent::Command(command), &entry.context)
                    {
                        Ok(decision) => decision,
                        Err(error) => {
                            self.record_error(Some(source), &error);
                            return Err(error);
                        }
                    }
                };
                self.process_decision_inner(next, executor, Some(source))?;
            }
            ViewDecision::Close => {
                self.pending_result = None;
                self.result_committed = false;
                self.finish_return(executor)?;
            }
            ViewDecision::CloseToRoot => {
                self.pending_result = None;
                self.result_committed = false;
                if self.stack.len() > 1 {
                    self.finish_return_to(executor, Some(0))?;
                }
            }
            ViewDecision::CloseWithError(message) => {
                let error = anyhow::anyhow!(message);
                self.record_error(source, &error);
                self.pending_result = None;
                self.result_committed = false;
                self.finish_return(executor)?;
            }
            ViewDecision::Exit => {
                self.close_all()?;
            }
            ViewDecision::RequestCommand(_) | ViewDecision::Stay | ViewDecision::Invalidate => {}
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn return_active(&mut self) -> Result<Option<ViewResult>> {
        self.finish_return(&mut None)?;
        Ok(self.pending_result.take())
    }

    fn finish_return(&mut self, executor: &mut Option<&mut dyn EffectExecutor>) -> Result<()> {
        self.finish_return_to(executor, None)
    }

    fn finish_return_to(
        &mut self,
        executor: &mut Option<&mut dyn EffectExecutor>,
        target_override: Option<usize>,
    ) -> Result<()> {
        let Some(active_index) = self.stack.len().checked_sub(1) else {
            return Ok(());
        };
        let active_id = self.stack[active_index].id;
        let source_location = self.stack[active_index].context.location.clone();
        let continuation = if target_override.is_some() {
            Continuation::None
        } else {
            self.stack[active_index].continuation.clone()
        };
        let call_boundary = match &continuation {
            Continuation::Call(boundary) => Some(boundary.clone()),
            Continuation::None | Continuation::ReturnTo(_) => None,
        };
        let target_id = call_boundary.as_ref().map(|boundary| boundary.caller);
        let target = match target_id
            .map(Continuation::ReturnTo)
            .unwrap_or(continuation)
        {
            Continuation::ReturnTo(id) => {
                match self.stack.iter().position(|entry| entry.id == id) {
                    Some(target) => target,
                    None => {
                        let error = anyhow::anyhow!("continuation target {:?} was closed", id);
                        self.record_error(Some(active_id), &error);
                        return Err(error);
                    }
                }
            }
            Continuation::Call(_) => unreachable!("call boundary target was resolved above"),
            Continuation::None => target_override.unwrap_or(active_index.saturating_sub(1)),
        };
        let post_commit_processor = call_boundary
            .as_ref()
            .is_some_and(|boundary| boundary.handler.post_commit());
        let post_commit_result = post_commit_processor
            .then(|| self.pending_result.as_ref())
            .flatten()
            .cloned();
        let continuation_decision = if post_commit_processor {
            None
        } else {
            match (&call_boundary, self.pending_result.as_ref()) {
                (Some(boundary), Some(result)) => {
                    let caller = self
                        .stack
                        .iter()
                        .find(|entry| entry.id == boundary.caller)
                        .expect("validated call continuation target must remain mounted");
                    let snapshot = caller.view.command_snapshot();
                    Some(boundary.handler.resume(
                        &source_location,
                        &caller.context,
                        &snapshot,
                        result,
                    )?)
                }
                _ => None,
            }
        };

        // Covered frames below the continuation target are fully closed first.
        // The active frame is only closed after its parent has successfully
        // activated, so an activation failure can retry the same result.
        while self.stack.len() > target + 2 {
            let index = self.stack.len() - 2;
            self.close_instance_at(index)?;
            self.remove_view(index);
        }
        if self.stack.len() == 1 {
            self.close_instance_at(0)?;
            self.pop_view();
            self.result_committed = self.pending_result.is_some();
            return Ok(());
        }

        let active_index = self.stack.len() - 1;
        self.close_start_at(active_index)?;
        let parent = &mut self.stack[active_index - 1];
        let parent_id = parent.id;
        parent.state = StackState::Active;
        if let Err(error) = deliver_lifecycle(
            &mut *parent.view,
            &parent.context,
            parent_id,
            LifecycleEvent::Activated,
        ) {
            parent.state = StackState::Covered;
            let active = &mut self.stack[active_index];
            let restore_error = deliver_lifecycle(
                &mut *active.view,
                &active.context,
                active_id,
                LifecycleEvent::Activated,
            )
            .err();
            self.record_error(Some(parent_id), &error);
            if let Some(restore_error) = restore_error {
                self.record_error(Some(active_id), &restore_error);
            }
            return Err(error);
        }
        if let Err(error) = self.close_finish_at(active_index) {
            self.stack[active_index - 1].state = StackState::Covered;
            let active = &mut self.stack[active_index];
            let restore_error = deliver_lifecycle(
                &mut *active.view,
                &active.context,
                active_id,
                LifecycleEvent::Activated,
            )
            .err();
            self.record_error(Some(active_id), &error);
            if let Some(restore_error) = restore_error {
                self.record_error(Some(active_id), &restore_error);
            }
            return Err(error);
        }
        self.pop_view();
        let Some(boundary) = call_boundary else {
            self.result_committed = self.pending_result.is_some();
            return Ok(());
        };
        self.pending_result.take();
        self.result_committed = false;
        let decision = if post_commit_processor {
            let Some(result) = post_commit_result.as_ref() else {
                return Ok(());
            };
            let caller = self
                .stack
                .iter()
                .find(|entry| entry.id == boundary.caller)
                .expect("validated call continuation target must remain mounted");
            let snapshot = caller.view.command_snapshot();
            boundary
                .handler
                .resume(&source_location, &caller.context, &snapshot, result)?
        } else {
            let Some(decision) = continuation_decision else {
                return Ok(());
            };
            decision
        };
        self.process_decision_inner(decision, executor, Some(boundary.caller))
    }

    fn close_all(&mut self) -> Result<()> {
        while let Some(index) = self.stack.len().checked_sub(1) {
            self.close_instance_at(index)?;
            self.pop_view();
            if let Some(previous) = self.stack.last_mut() {
                previous.state = StackState::Active;
            }
        }
        Ok(())
    }

    pub(crate) fn render_at(
        &self,
        index: usize,
        frame: &mut Frame,
        area: Rect,
        context: &RenderContext,
    ) -> Result<RenderResult> {
        let view = self
            .stack
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("View stack index {index} is out of bounds"))?;
        view.view.render(frame, area, context)
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MapRouteCatalog {
    routes: BTreeMap<String, String>,
}

#[cfg(test)]
impl MapRouteCatalog {
    pub(crate) fn insert(&mut self, selector: impl Into<String>, target: impl Into<String>) {
        self.routes.insert(selector.into(), target.into());
    }
}

#[cfg(test)]
impl RouteCatalog for MapRouteCatalog {
    fn resolve(&self, selector: &str) -> Option<RouteTarget> {
        self.routes
            .get(selector)
            .cloned()
            .map(|reference| RouteTarget {
                label: (selector != reference).then(|| selector.to_string()),
                reference,
            })
    }

    fn complete(&self, prefix: &str) -> Vec<RouteCandidate> {
        self.routes
            .keys()
            .filter(|route| route.starts_with(prefix))
            .map(|route| RouteCandidate {
                target: RouteTarget {
                    reference: self.routes[route].clone(),
                    label: Some(route.clone()),
                },
                label: route.clone(),
            })
            .collect()
    }

    fn query_schema(&self, target: &str) -> Option<QuerySchema> {
        self.routes
            .values()
            .any(|reference| reference == target)
            .then(|| QuerySchema {
                id: "query".to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::contracts::TaskOutcome;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TestView {
        runtime: Value,
    }
    impl View for TestView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }
        fn command_snapshot(&self) -> ViewCommandSnapshot {
            ViewCommandSnapshot {
                engine_type: "test".to_string(),
                parameters: Value::Null,
                raw_input: String::new(),
                runtime: self.runtime.clone(),
                publication: None,
                revision: 0,
            }
        }
        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(match event {
                ViewEvent::Lifecycle(_) => ViewDecision::Stay,
                ViewEvent::Command(CommandResult::EditInput { value, cursor }) => {
                    self.runtime = serde_json::json!({
                        "edited": value,
                        "cursor": cursor,
                    });
                    ViewDecision::Invalidate
                }
                ViewEvent::Input(InputEvent::Eof) => ViewDecision::Exit,
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Enter, ..
                }) => ViewDecision::Return(ViewResult {
                    value: Value::String("done".into()),
                }),
                _ => ViewDecision::Invalidate,
            })
        }
        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    struct TestFactory;
    impl ViewFactory for TestFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(TestView {
                runtime: Value::Null,
            }))
        }
    }

    struct PushThenReturnView {
        target: String,
    }

    impl View for PushThenReturnView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(match event {
                ViewEvent::Lifecycle(_) => ViewDecision::Stay,
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Enter, ..
                }) if self.target == "child" => {
                    ViewDecision::Transition(TransitionRequest::Push(request("grandchild")))
                }
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Enter, ..
                }) => ViewDecision::Return(ViewResult {
                    value: Value::String("grandchild-value".into()),
                }),
                _ => ViewDecision::Stay,
            })
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    struct PushThenReturnFactory;

    impl ViewFactory for PushThenReturnFactory {
        fn create(
            &self,
            request: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(PushThenReturnView {
                target: request.target.clone(),
            }))
        }
    }

    #[derive(Clone)]
    struct LifecycleFactory {
        events: Rc<RefCell<Vec<String>>>,
        reject_activation: bool,
    }

    struct LifecycleView {
        events: Rc<RefCell<Vec<String>>>,
        reject_activation: bool,
    }

    impl View for LifecycleView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            if let ViewEvent::Lifecycle(ref lifecycle) = event {
                self.events.borrow_mut().push(format!("{lifecycle:?}"));
                if *lifecycle == LifecycleEvent::Activated && self.reject_activation {
                    return Ok(ViewDecision::Exit);
                }
            }
            if matches!(event, ViewEvent::Task(_)) {
                self.events.borrow_mut().push("Task".to_string());
            }
            if matches!(event, ViewEvent::Input(InputEvent::Eof)) {
                return Ok(ViewDecision::Exit);
            }
            Ok(ViewDecision::Stay)
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    impl ViewFactory for LifecycleFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(LifecycleView {
                events: Rc::clone(&self.events),
                reject_activation: self.reject_activation,
            }))
        }
    }

    fn request(target: &str) -> NavigationRequest {
        NavigationRequest::new(target, ParsedQuery::new(target, "query", Value::Null))
    }

    fn task_event(instance: ViewInstanceId, task: TaskId, generation: u64) -> TaskEvent {
        TaskEvent {
            instance,
            task,
            generation,
            outcome: TaskOutcome::Completed(Value::Null),
        }
    }

    #[test]
    fn view_task_registry_accepts_only_the_registered_tuple() {
        let owner = ViewInstanceId(7);
        let mut registry = ViewTaskRegistry::new(owner);
        registry.register(TaskId(3), 11);

        assert!(registry.accepts(&task_event(owner, TaskId(3), 11)));
        assert!(!registry.accepts(&task_event(owner, TaskId(4), 11)));
        assert!(!registry.accepts(&task_event(ViewInstanceId(8), TaskId(3), 11)));
    }

    #[test]
    fn view_task_registry_replaces_stale_generations() {
        let owner = ViewInstanceId(7);
        let mut registry = ViewTaskRegistry::new(owner);
        registry.register(TaskId(3), 11);
        registry.register(TaskId(3), 12);

        assert!(!registry.accepts(&task_event(owner, TaskId(3), 11)));
        assert!(registry.accepts(&task_event(owner, TaskId(3), 12)));
    }

    #[test]
    fn view_task_registry_invalidates_one_or_all_tasks() {
        let owner = ViewInstanceId(7);
        let mut registry = ViewTaskRegistry::new(owner);
        registry.register(TaskId(3), 11);
        registry.register(TaskId(4), 12);
        registry.invalidate(TaskId(3));

        assert!(!registry.accepts(&task_event(owner, TaskId(3), 11)));
        assert!(registry.accepts(&task_event(owner, TaskId(4), 12)));
        registry.invalidate_all();
        assert!(!registry.accepts(&task_event(owner, TaskId(4), 12)));
    }

    #[derive(Clone, Copy)]
    enum Fault {
        Activation,
        Closing,
        TransitionCommitted,
    }

    struct FaultFactory {
        events: Rc<RefCell<Vec<String>>>,
        target: String,
        fault: Fault,
    }

    struct FaultView {
        events: Rc<RefCell<Vec<String>>>,
        fault: Option<Fault>,
    }

    impl View for FaultView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            if let ViewEvent::Lifecycle(ref lifecycle) = event {
                self.events.borrow_mut().push(format!("{lifecycle:?}"));
                if matches!(self.fault, Some(Fault::Activation))
                    && *lifecycle == LifecycleEvent::Activated
                {
                    anyhow::bail!("activation failed")
                }
                if matches!(self.fault, Some(Fault::Closing))
                    && *lifecycle == LifecycleEvent::Closing
                {
                    anyhow::bail!("closing failed")
                }
                if matches!(self.fault, Some(Fault::TransitionCommitted))
                    && matches!(lifecycle, LifecycleEvent::TransitionCommitted { .. })
                {
                    anyhow::bail!("transition callback failed")
                }
            }
            if matches!(event, ViewEvent::Input(InputEvent::Eof)) {
                return Ok(ViewDecision::Exit);
            }
            Ok(ViewDecision::Stay)
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    impl ViewFactory for FaultFactory {
        fn create(
            &self,
            request: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(FaultView {
                events: Rc::clone(&self.events),
                fault: (request.target == self.target).then_some(self.fault),
            }))
        }
    }

    #[test]
    fn navigation_input_rejects_non_boundary_cursor() {
        let query = ParsedQuery::new("core:default", "query", Value::Null);
        assert!(
            NavigationRequest::new("core:default", query)
                .with_input("é", 1)
                .is_err()
        );
    }

    #[test]
    fn router_stages_mount_then_commits_before_activation_and_closes_root() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(LifecycleFactory {
                events: Rc::clone(&events),
                reject_activation: false,
            }),
        );
        router.push(request("root")).unwrap();
        router.push(request("child")).unwrap();
        let root_id = router.stack()[0].id;
        router
            .dispatch(ViewEvent::Task(TaskEvent {
                instance: root_id,
                task: TaskId(1),
                generation: 1,
                outcome: TaskOutcome::Completed(Value::Null),
            }))
            .unwrap();
        assert_eq!(
            &*events.borrow(),
            &[
                "Mounted",
                "Activated",
                "Mounted",
                "Covered",
                "Activated",
                "TransitionCommitted { target: ViewInstanceId(2) }",
                "Task"
            ]
        );
        router.return_active().unwrap();
        assert_eq!(
            &*events.borrow(),
            &[
                "Mounted",
                "Activated",
                "Mounted",
                "Covered",
                "Activated",
                "TransitionCommitted { target: ViewInstanceId(2) }",
                "Task",
                "Closing",
                "Activated",
                "Closed"
            ]
        );
        router.dispatch(ViewEvent::Input(InputEvent::Eof)).unwrap();
        assert!(router.stack().is_empty());
        assert_eq!(
            &*events.borrow(),
            &[
                "Mounted",
                "Activated",
                "Mounted",
                "Covered",
                "Activated",
                "TransitionCommitted { target: ViewInstanceId(2) }",
                "Task",
                "Closing",
                "Activated",
                "Closed",
                "Closing",
                "Closed"
            ]
        );
    }

    struct ParentActivationFactory {
        activations: Rc<RefCell<u32>>,
    }

    struct ParentActivationView {
        is_root: bool,
        activations: Rc<RefCell<u32>>,
    }

    impl View for ParentActivationView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            if self.is_root && matches!(event, ViewEvent::Lifecycle(LifecycleEvent::Activated)) {
                let mut activations = self.activations.borrow_mut();
                *activations += 1;
                if *activations == 2 {
                    anyhow::bail!("parent activation failed once")
                }
            }
            Ok(ViewDecision::Stay)
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    impl ViewFactory for ParentActivationFactory {
        fn create(
            &self,
            request: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(ParentActivationView {
                is_root: request.target == "root",
                activations: Rc::clone(&self.activations),
            }))
        }
    }

    #[test]
    fn return_retries_pending_view_close_after_parent_activation_failure() {
        let activations = Rc::new(RefCell::new(0));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(ParentActivationFactory {
                activations: Rc::clone(&activations),
            }),
        );
        let root = router.push(request("root")).unwrap();
        let child = router.push(request("child")).unwrap();
        assert!(router.return_active().is_err());
        assert_eq!(router.active().map(|entry| entry.id), Some(child));
        assert!(router.active().unwrap().is_active());
        assert_eq!(router.take_error().unwrap().source, Some(root));
        router.return_active().unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
    }

    #[test]
    fn call_continuation_returns_to_its_declared_instance() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(LifecycleFactory {
                events,
                reject_activation: false,
            }),
        );
        let root = router.push(request("root")).unwrap();
        router
            .call(request("child"), Continuation::ReturnTo(root))
            .unwrap();
        router.return_active().unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
    }

    struct RecordingCallHandler {
        calls: Rc<RefCell<Vec<(String, ViewInstanceId, Value)>>>,
        decision: ViewDecision,
    }

    impl CallReturnHandler for RecordingCallHandler {
        fn resume(
            &self,
            source: &ViewLocation,
            caller: &ViewContext,
            _: &ViewCommandSnapshot,
            result: &ViewResult,
        ) -> Result<ViewDecision> {
            self.calls.borrow_mut().push((
                source.target.clone(),
                caller.instance,
                result.value.clone(),
            ));
            Ok(self.decision.clone())
        }
    }

    fn call_boundary(
        caller: ViewInstanceId,
        calls: Rc<RefCell<Vec<(String, ViewInstanceId, Value)>>>,
        decision: ViewDecision,
    ) -> Continuation {
        Continuation::Call(CallBoundary {
            caller,
            handler: Arc::new(RecordingCallHandler { calls, decision }),
        })
    }

    struct FailingCallHandler;

    impl CallReturnHandler for FailingCallHandler {
        fn resume(
            &self,
            _: &ViewLocation,
            _: &ViewContext,
            _: &ViewCommandSnapshot,
            _: &ViewResult,
        ) -> Result<ViewDecision> {
            anyhow::bail!("continuation failed")
        }
    }

    #[test]
    fn failed_call_continuation_keeps_the_child_and_result_for_retry() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        let child = router
            .call(
                request("child"),
                Continuation::Call(CallBoundary {
                    caller: root,
                    handler: Arc::new(FailingCallHandler),
                }),
            )
            .unwrap();

        let error = router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: vec![b'\r'],
            }))
            .unwrap_err();
        assert!(error.to_string().contains("continuation failed"));
        assert_eq!(router.active().map(|entry| entry.id), Some(child));
        assert_eq!(router.stack().len(), 2);
        assert!(router.pending_result.is_some());
        assert!(router.take_result().is_none());
    }

    #[test]
    fn returning_from_a_pushed_view_does_not_skip_to_an_older_call_boundary() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        routes.insert("grandchild", "grandchild");
        let mut router = Router::new(Box::new(routes), Box::new(PushThenReturnFactory));
        let root = router.push(request("root")).unwrap();
        let child = router
            .call(
                request("child"),
                call_boundary(root, Rc::clone(&calls), ViewDecision::Stay),
            )
            .unwrap();

        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\\r".to_vec(),
            }))
            .unwrap();
        let grandchild = router.active().unwrap().id;
        assert_ne!(grandchild, child);

        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\\r".to_vec(),
            }))
            .unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(child));
        assert!(calls.borrow().is_empty());

        router.return_active().unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn call_continuation_command_result_is_delivered_to_the_caller() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        router
            .call(
                request("child"),
                call_boundary(
                    root,
                    calls,
                    ViewDecision::Command(CommandResult::EditInput {
                        value: "continued".to_string(),
                        cursor: 4,
                    }),
                ),
            )
            .unwrap();

        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: vec![b'\r'],
            }))
            .unwrap();
        let active = router.active().unwrap();
        assert_eq!(active.id, root);
        assert_eq!(
            active.view.command_snapshot().runtime,
            serde_json::json!({"edited": "continued", "cursor": 4})
        );
    }

    #[test]
    fn close_restores_a_parent_without_publishing_a_result_and_closes_a_root() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        let child = router
            .call(
                request("child"),
                call_boundary(root, Rc::clone(&calls), ViewDecision::Stay),
            )
            .unwrap();

        let mut executor = None;
        router
            .process_decision_inner(ViewDecision::Close, &mut executor, Some(child))
            .unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.take_result().is_none());
        assert!(calls.borrow().is_empty());

        router
            .process_decision_inner(ViewDecision::Close, &mut executor, Some(root))
            .unwrap();
        assert!(router.stack().is_empty());
        assert!(router.take_result().is_none());
    }

    #[test]
    fn close_to_root_discards_nested_calls_and_preserves_the_root_state() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        for target in ["root", "child", "grandchild"] {
            routes.insert(target, target);
        }
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        let mut executor = None;
        router
            .process_decision_inner(
                ViewDecision::Command(CommandResult::EditInput {
                    value: "root query".into(),
                    cursor: 4,
                }),
                &mut executor,
                Some(root),
            )
            .unwrap();
        let snapshot = router.active().unwrap().view.command_snapshot().runtime;
        let child = router
            .call(
                request("child"),
                call_boundary(root, Rc::clone(&calls), ViewDecision::Exit),
            )
            .unwrap();
        let grandchild = router
            .call(
                request("grandchild"),
                call_boundary(child, Rc::clone(&calls), ViewDecision::Exit),
            )
            .unwrap();
        router.pending_result = Some(ViewResult { value: Value::Null });
        router
            .process_decision_inner(ViewDecision::CloseToRoot, &mut executor, Some(grandchild))
            .unwrap();
        assert_eq!(router.stack().len(), 1);
        let active = router.active().unwrap();
        assert_eq!(active.id, root);
        assert_eq!(active.view.command_snapshot().runtime, snapshot);
        assert!(router.take_result().is_none());
        assert!(calls.borrow().is_empty());

        router
            .process_decision_inner(ViewDecision::CloseToRoot, &mut executor, Some(root))
            .unwrap();
        assert_eq!(router.active().unwrap().id, root);
    }

    #[test]
    fn close_to_root_closes_intermediate_views_without_activating_them() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        for target in ["root", "child", "grandchild"] {
            routes.insert(target, target);
        }
        let mut router = Router::new(
            Box::new(routes),
            Box::new(LifecycleFactory {
                events: Rc::clone(&events),
                reject_activation: false,
            }),
        );
        let root = router.push(request("root")).unwrap();
        router.push(request("child")).unwrap();
        let grandchild = router.push(request("grandchild")).unwrap();
        events.borrow_mut().clear();
        router
            .process_decision_inner(ViewDecision::CloseToRoot, &mut None, Some(grandchild))
            .unwrap();
        assert_eq!(router.active().unwrap().id, root);
        assert_eq!(
            *events.borrow(),
            ["Closing", "Closed", "Closing", "Activated", "Closed"]
        );
    }

    #[test]
    fn nested_call_null_result_is_delivered_and_parent_accepts_next_input() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        let child = router
            .call(
                request("child"),
                call_boundary(root, Rc::clone(&calls), ViewDecision::Stay),
            )
            .unwrap();

        let mut executor = None;
        router
            .process_decision_inner(
                ViewDecision::Return(ViewResult { value: Value::Null }),
                &mut executor,
                Some(child),
            )
            .unwrap();

        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
        assert!(router.take_result().is_none());
        assert_eq!(
            calls.borrow().as_slice(),
            &[("child".to_string(), root, Value::Null)]
        );

        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\r".to_vec(),
            }))
            .unwrap();
        assert!(router.stack().is_empty());
        assert_eq!(
            router.take_result().map(|result| result.value),
            Some(Value::String("done".to_string()))
        );
    }

    #[test]
    fn nested_call_return_runs_handler_in_restored_parent() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        let child = router
            .call(
                request("child"),
                call_boundary(
                    root,
                    Rc::clone(&calls),
                    ViewDecision::Return(ViewResult {
                        value: Value::String("continued".to_string()),
                    }),
                ),
            )
            .unwrap();

        let mut executor = None;
        router
            .process_decision_inner(
                ViewDecision::Return(ViewResult {
                    value: Value::String("child-value".to_string()),
                }),
                &mut executor,
                Some(child),
            )
            .unwrap();

        assert!(router.stack().is_empty());
        assert_eq!(
            calls.borrow().as_slice(),
            &[(
                "child".to_string(),
                root,
                Value::String("child-value".to_string())
            )]
        );
        assert_eq!(
            router.take_result().map(|result| result.value),
            Some(Value::String("continued".to_string()))
        );
    }

    #[test]
    fn route_catalog_validates_structured_query_schema_and_shape() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("alias", "core:default");
        let catalog = routes;
        let query = ParsedQuery::new("core:default", "wrong", Value::Null);
        assert!(catalog.validate_query(&query).is_err());
        let query = ParsedQuery::new("core:default", "query", Value::Null);
        catalog.validate_query(&query).unwrap();
        assert!(
            catalog
                .validate_query(&ParsedQuery::new("missing", "query", Value::Null))
                .is_err()
        );

        struct StrictCatalog;
        impl RouteCatalog for StrictCatalog {
            fn resolve(&self, selector: &str) -> Option<RouteTarget> {
                (selector == "strict").then(|| RouteTarget {
                    reference: "strict".to_string(),
                    label: None,
                })
            }
            fn complete(&self, _: &str) -> Vec<RouteCandidate> {
                Vec::new()
            }
            fn query_schema(&self, target: &str) -> Option<QuerySchema> {
                (target == "strict").then(|| QuerySchema {
                    id: "required".into(),
                })
            }
            fn validate_query(&self, query: &ParsedQuery) -> Result<()> {
                query.validate_shape()?;
                anyhow::ensure!(query.target == "strict", "unexpected target");
                anyhow::ensure!(query.schema == "required", "unexpected schema");
                anyhow::ensure!(
                    query.values.get("name").and_then(Value::as_str).is_some(),
                    "required name is missing"
                );
                Ok(())
            }
        }
        let strict = StrictCatalog;
        assert!(
            strict
                .validate_query(&ParsedQuery::new("strict", "required", Value::Null))
                .is_err()
        );
        assert!(
            strict
                .validate_query(&ParsedQuery::new(
                    "strict",
                    "required",
                    serde_json::json!({"name": "ok"})
                ))
                .is_ok()
        );
    }

    struct CoverFailView {
        events: Rc<RefCell<Vec<String>>>,
    }

    impl View for CoverFailView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            if let ViewEvent::Lifecycle(lifecycle) = event {
                self.events.borrow_mut().push(format!("{lifecycle:?}"));
                if lifecycle == LifecycleEvent::Covered {
                    anyhow::bail!("cover failed")
                }
            }
            Ok(ViewDecision::Stay)
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    struct CoverFailFactory {
        events: Rc<RefCell<Vec<String>>>,
    }

    impl ViewFactory for CoverFailFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(CoverFailView {
                events: Rc::clone(&self.events),
            }))
        }
    }

    #[test]
    fn covered_failure_closes_staged_view_and_preserves_active_stack() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(CoverFailFactory {
                events: Rc::clone(&events),
            }),
        );
        let root = router.push(request("root")).unwrap();
        assert!(router.push(request("child")).is_err());
        assert_eq!(router.stack().len(), 1);
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
        assert_eq!(router.take_error().unwrap().source, Some(root));
        let events = events.borrow();
        assert!(events.iter().any(|event| event == "Closing"));
        assert!(events.iter().any(|event| event == "Closed"));
    }

    #[test]
    fn router_rejects_activation_without_leaving_a_new_stack_entry() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(LifecycleFactory {
                events: Rc::clone(&events),
                reject_activation: false,
            }),
        );
        router.push(request("root")).unwrap();
        let mut rejecting = Router::new(
            Box::new({
                let mut routes = MapRouteCatalog::default();
                routes.insert("root", "root");
                routes.insert("child", "child");
                routes
            }),
            Box::new(LifecycleFactory {
                events: Rc::clone(&events),
                reject_activation: true,
            }),
        );
        rejecting.push(request("root")).unwrap_err();
        assert!(rejecting.stack().is_empty());
        assert_eq!(router.stack().len(), 1);
    }

    #[test]
    fn activation_error_rolls_back_without_losing_the_previous_view() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(FaultFactory {
                events: Rc::clone(&events),
                target: "child".to_string(),
                fault: Fault::Activation,
            }),
        );
        let root = router.push(request("root")).unwrap();
        assert!(router.push(request("child")).is_err());
        assert_eq!(router.stack().len(), 1);
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
        assert_eq!(router.take_error().unwrap().source, Some(root));
        assert!(events.borrow().iter().any(|event| event == "Closing"));
        assert!(events.borrow().iter().any(|event| event == "Closed"));
    }

    #[test]
    fn return_close_error_keeps_the_active_instance_for_retry_or_reporting() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(FaultFactory {
                events,
                target: "child".to_string(),
                fault: Fault::Closing,
            }),
        );
        router.push(request("root")).unwrap();
        let child = router.push(request("child")).unwrap();
        assert!(router.return_active().is_err());
        assert_eq!(router.active().map(|entry| entry.id), Some(child));
        assert!(router.active().unwrap().is_active());
        assert_eq!(router.take_error().unwrap().source, Some(child));
    }

    #[test]
    fn commit_callback_failure_does_not_reject_an_already_committed_transition() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(FaultFactory {
                events,
                target: "root".to_string(),
                fault: Fault::TransitionCommitted,
            }),
        );
        let root = router.push(request("root")).unwrap();
        let child = router.push(request("child")).unwrap();
        assert_eq!(router.active().map(|entry| entry.id), Some(child));
        assert_eq!(router.stack().len(), 2);
        let error = router.take_error().expect("callback failure is recorded");
        assert_eq!(error.source, Some(root));
        assert!(error.message.contains("transition callback failed"));
    }

    #[test]
    fn replace_and_exit_cleanup_errors_preserve_stack_and_record_the_source() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(FaultFactory {
                events: Rc::new(RefCell::new(Vec::new())),
                target: "root".to_string(),
                fault: Fault::Closing,
            }),
        );
        let root = router.push(request("root")).unwrap();
        let child = router.replace(request("child")).unwrap_err();
        assert!(child.to_string().contains("closing failed"));
        assert_eq!(router.stack().len(), 1);
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert!(router.active().unwrap().is_active());
        assert_eq!(router.take_error().unwrap().source, Some(root));

        let mut router = Router::new(
            Box::new({
                let mut routes = MapRouteCatalog::default();
                routes.insert("root", "root");
                routes
            }),
            Box::new(FaultFactory {
                events: Rc::new(RefCell::new(Vec::new())),
                target: "root".to_string(),
                fault: Fault::Closing,
            }),
        );
        let root = router.push(request("root")).unwrap();
        assert!(router.dispatch(ViewEvent::Input(InputEvent::Eof)).is_err());
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert_eq!(router.take_error().unwrap().source, Some(root));
    }

    struct EffectExecutorError;

    impl EffectExecutor for EffectExecutorError {
        fn execute(&mut self, _: EffectRequest, _: &ViewContext) -> Result<EffectResult> {
            anyhow::bail!("executor unavailable")
        }
    }

    #[test]
    fn executor_errors_are_recorded_with_the_originating_instance() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(EffectFactory));
        let root = router.push(request("root")).unwrap();
        let error = router
            .dispatch_with_effects(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: vec![b'x'],
                }),
                &mut EffectExecutorError,
            )
            .unwrap_err();
        assert!(error.to_string().contains("executor unavailable"));
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert_eq!(router.take_error().unwrap().source, Some(root));
    }

    #[test]
    fn stale_task_for_a_closed_instance_is_ignored() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(
            Box::new(routes),
            Box::new(LifecycleFactory {
                events: Rc::clone(&events),
                reject_activation: false,
            }),
        );
        let root = router.push(request("root")).unwrap();
        let child = router.push(request("child")).unwrap();
        router.return_active().unwrap();
        events.borrow_mut().clear();
        assert_eq!(
            router
                .dispatch(ViewEvent::Task(TaskEvent {
                    instance: child,
                    task: TaskId(1),
                    generation: 99,
                    outcome: TaskOutcome::Completed(Value::Null),
                }))
                .unwrap(),
            ViewDecision::Stay
        );
        assert!(events.borrow().is_empty());
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
    }

    struct EffectView;

    impl View for EffectView {
        fn bindings(&self, _: &ViewContext) -> BindingSet {
            BindingSet::default()
        }

        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            if matches!(
                event,
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    ..
                })
            ) {
                return Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                    "value".to_string(),
                )));
            }
            Ok(ViewDecision::Stay)
        }

        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    struct EffectFactory;
    impl ViewFactory for EffectFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(EffectView))
        }
    }

    struct EffectRecorder {
        calls: Vec<EffectRequest>,
    }

    impl EffectExecutor for EffectRecorder {
        fn execute(&mut self, effect: EffectRequest, _: &ViewContext) -> Result<EffectResult> {
            self.calls.push(effect);
            Ok(EffectResult::Complete)
        }
    }

    struct FailingEffectExecutor;

    impl EffectExecutor for FailingEffectExecutor {
        fn execute(&mut self, _: EffectRequest, _: &ViewContext) -> Result<EffectResult> {
            Ok(EffectResult::Error(EffectError::Failed("denied".into())))
        }
    }

    #[test]
    fn effect_failure_is_observable_without_clearing_the_active_view() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(EffectFactory));
        let root = router.push(request("root")).unwrap();
        let error = router
            .dispatch_with_effects(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: vec![b'x'],
                }),
                &mut FailingEffectExecutor,
            )
            .unwrap_err();
        assert!(error.to_string().contains("denied"));
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
        assert_eq!(router.take_error().unwrap().source, Some(root));
        assert!(router.take_info().is_none());
    }

    #[test]
    fn router_executes_effects_without_changing_the_stack() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(EffectFactory));
        router.push(request("root")).unwrap();
        let mut executor = EffectRecorder { calls: Vec::new() };
        router
            .dispatch_with_effects(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: b"x".to_vec(),
                }),
                &mut executor,
            )
            .unwrap();
        assert_eq!(executor.calls.len(), 1);
        assert_eq!(router.stack().len(), 1);
        assert!(router.take_result().is_none());
        assert_eq!(
            router.take_info(),
            Some((
                router.active().unwrap().id,
                "root".into(),
                "Copied to clipboard".into()
            ))
        );
        assert!(router.take_info().is_none());
    }

    #[test]
    fn batch_is_prevalidated_before_effects_or_transitions_run() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        routes.insert("child", "child");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let root = router.push(request("root")).unwrap();
        router
            .set_global_action(
                Key::Char('x'),
                ViewDecision::Batch(vec![
                    ViewDecision::Effect(EffectRequest::CopyToClipboard("value".into())),
                    ViewDecision::Transition(TransitionRequest::Push(request("child"))),
                ]),
            )
            .unwrap();
        let mut executor = EffectRecorder { calls: Vec::new() };
        let error = router
            .dispatch_with_effects(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: vec![b'x'],
                }),
                &mut executor,
            )
            .unwrap_err();
        assert!(error.to_string().contains("cannot be combined"));
        assert!(executor.calls.is_empty());
        assert_eq!(router.stack().len(), 1);
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
    }

    #[test]
    fn nested_batch_cannot_hide_multiple_effects() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        router.push(request("root")).unwrap();
        router
            .set_global_action(
                Key::Char('x'),
                ViewDecision::Batch(vec![ViewDecision::Batch(vec![
                    ViewDecision::Effect(EffectRequest::CopyToClipboard("first".into())),
                    ViewDecision::Effect(EffectRequest::CopyToClipboard("second".into())),
                ])]),
            )
            .unwrap();
        let mut executor = EffectRecorder { calls: Vec::new() };
        assert!(
            router
                .dispatch_with_effects(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Char('x'),
                        raw: vec![b'x'],
                    }),
                    &mut executor,
                )
                .is_err()
        );
        assert!(executor.calls.is_empty());
        assert_eq!(router.stack().len(), 1);
    }

    #[test]
    fn effect_then_exit_runs_exit_only_after_effect_success() {
        let action = ViewDecision::Batch(vec![
            ViewDecision::Effect(EffectRequest::CopyToClipboard("value".into())),
            ViewDecision::Exit,
        ]);

        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        router.push(request("root")).unwrap();
        router
            .set_global_action(Key::Char('x'), action.clone())
            .unwrap();
        let mut executor = EffectRecorder { calls: Vec::new() };
        router
            .dispatch_with_effects(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: vec![b'x'],
                }),
                &mut executor,
            )
            .unwrap();
        assert_eq!(executor.calls.len(), 1);
        assert!(router.stack().is_empty());

        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        router.push(request("root")).unwrap();
        router.set_global_action(Key::Char('x'), action).unwrap();
        assert!(
            router
                .dispatch_with_effects(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Char('x'),
                        raw: vec![b'x'],
                    }),
                    &mut FailingEffectExecutor,
                )
                .is_err()
        );
        assert_eq!(router.stack().len(), 1);
    }

    #[test]
    fn effect_without_an_executor_is_an_error() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(EffectFactory));
        let root = router.push(request("root")).unwrap();
        let error = router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Char('x'),
                raw: vec![b'x'],
            }))
            .unwrap_err();
        assert!(error.to_string().contains("executor is unavailable"));
        assert_eq!(router.active().map(|entry| entry.id), Some(root));
    }

    #[test]
    fn global_action_registration_is_atomic_for_duplicate_physical_keys() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "root");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        assert!(
            router
                .set_global_actions([
                    (
                        Binding {
                            key: Key::Char('g'),
                            label: None
                        },
                        ViewDecision::Stay,
                    ),
                    (
                        Binding {
                            key: Key::Char('G'),
                            label: None
                        },
                        ViewDecision::Exit,
                    ),
                ])
                .is_err()
        );
        assert!(router.global_bindings().entries().is_empty());
    }

    #[test]
    fn router_commits_a_mounted_view_and_owns_stack() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("default", "core:default");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
        let request = NavigationRequest::new(
            "default",
            ParsedQuery::new("core:default", "query", Value::Null),
        );
        let id = router.push(request).expect("mount succeeds");
        assert_eq!(router.active().map(|view| view.id), Some(id));
        assert_eq!(
            router.active().unwrap().context.location.target,
            "core:default"
        );

        router.set_global_bindings(BindingSet::new([Binding {
            key: Key::Char('x'),
            label: None,
        }]));
        assert_eq!(
            router
                .dispatch(ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: b"x".to_vec(),
                }))
                .unwrap(),
            ViewDecision::Stay
        );
        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\r".to_vec(),
            }))
            .unwrap();
        assert!(router.stack().is_empty());
        assert_eq!(
            router.take_result().map(|result| result.value),
            Some(Value::String("done".into()))
        );
    }

    #[test]
    fn popup_closing_records_popup_closed_flag_in_router() {
        let mut routes = MapRouteCatalog::default();
        routes.insert("root", "core:root");
        routes.insert("child_popup", "core:child_popup");
        let mut router = Router::new(Box::new(routes), Box::new(TestFactory));

        let root_request =
            NavigationRequest::new("root", ParsedQuery::new("core:root", "query", Value::Null));
        router.push(root_request).expect("mount succeeds");
        assert!(!router.take_popup_closed());

        let mut popup_request = NavigationRequest::new(
            "child_popup",
            ParsedQuery::new("core:child_popup", "query", Value::Null),
        );
        popup_request.presentation.mode = ViewPresentationMode::Popup;
        router.push(popup_request).expect("mount popup succeeds");
        assert!(!router.take_popup_closed());

        router
            .dispatch(ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\r".to_vec(),
            }))
            .unwrap();

        assert!(router.take_popup_closed());
        assert!(!router.take_popup_closed());
    }
}
