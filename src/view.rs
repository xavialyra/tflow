//! Engine-neutral input, View, and navigation contracts.
//!

use crate::input::{InputEvent, Key};
use crate::workflow::config::ViewPresentation;
use anyhow::{Context, Result, bail};
use ratatui::{Frame, layout::Rect};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::protocol::contracts::{TaskEvent, TaskId, ViewInstanceId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ViewLocation {
    pub(crate) target: String,
    pub(crate) alias: Option<String>,
}

impl ViewLocation {
    #[cfg_attr(not(test), allow(dead_code))]
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
    #[cfg_attr(not(test), allow(dead_code))]
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
    Task(TaskEvent),
    Tick,
    Resize(TerminalSize),
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

#[cfg(test)]
impl BindingSet {
    pub(crate) fn new(entries: impl IntoIterator<Item = Binding>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    pub(crate) fn entries(&self) -> &[Binding] {
        &self.entries
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
    pub(crate) overflow_command: Option<(String, String)>,
    pub(crate) has_unbound: bool,
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
    ShowFeedback {
        message: String,
        level: crate::protocol::FeedbackLevel,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewResult {
    pub(crate) value: Value,
}

impl ViewResult {
    pub(crate) fn new(value: Value) -> Self {
        Self { value }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NavigationRequest {
    pub(crate) target: String,
    pub(crate) query: ParsedQuery,
    pub(crate) input: Option<ViewInputSeed>,
    pub(crate) presentation: ViewPresentation,
    pub(crate) focus: Option<String>,
}

impl NavigationRequest {
    pub(crate) fn new(target: impl Into<String>, query: ParsedQuery) -> Self {
        Self {
            target: target.into(),
            query,
            input: None,
            presentation: ViewPresentation::default(),
            focus: None,
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

    pub(crate) fn with_focus(mut self, focus: impl Into<String>) -> Self {
        self.focus = Some(focus.into());
        self
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
        message: format!("{error:#}"),
    }
    .into()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViewDecision {
    Stay,
    Invalidate,
    ClearInput,
    Transition(TransitionRequest),
    Return(ViewResult),
    Effect(EffectRequest),
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

pub(crate) trait FallbackInputReceiver {
    fn on_unbound_key(
        &mut self,
        key: Key,
        raw: &[u8],
        context: &ViewContext,
    ) -> Result<ViewDecision>;
}

pub(crate) trait View {
    fn preferred_top_inset(&self) -> u16 {
        0
    }

    fn engine_commands(&self, _context: &ViewContext) -> Vec<crate::command::CommandEntry> {
        Vec::new()
    }

    fn on_command(&mut self, _id: &str, _context: &ViewContext) -> Result<ViewDecision> {
        Ok(ViewDecision::Stay)
    }

    /// Consume the View's editable input. Called when a command's `navigate`
    /// operation requests `clear_input`, so a completed jump does not resurrect
    /// the query when the View is revealed again. Views without editable input
    /// ignore it.
    fn clear_input(&mut self, _context: &ViewContext) -> Result<()> {
        Ok(())
    }

    fn dispatch_key_event(
        &mut self,
        key: Key,
        raw: &[u8],
        context: &ViewContext,
    ) -> Result<ViewDecision> {
        for cmd in self.engine_commands(context) {
            if cmd.matches_binding(key) {
                return match cmd.handler {
                    crate::command::CommandHandler::Action(action) => action.execute(),
                    crate::command::CommandHandler::Event => self.on_command(&cmd.id, context),
                };
            }
        }
        if let Some(receiver) = self.fallback_receiver() {
            return receiver.on_unbound_key(key, raw, context);
        }
        Ok(ViewDecision::Stay)
    }

    fn fallback_receiver(&mut self) -> Option<&mut dyn FallbackInputReceiver> {
        None
    }

    /// Region of this View that may briefly display retained pixels from the
    /// previous View while a navigation or replace target is still loading.
    ///
    /// The default retains the whole content area. Views that own live input,
    /// cursors, or completion surfaces should narrow this to the region that
    /// only displays loaded content, or return `None` to opt out entirely.
    fn retained_content_area(&self, area: Rect) -> Option<Rect> {
        Some(area)
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

    #[cfg_attr(not(test), allow(dead_code))]
    fn publication(&self) -> Option<&ViewPublication> {
        None
    }

    fn chrome(&self, _context: &ViewContext) -> Result<ViewChrome> {
        Ok(ViewChrome::default())
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
pub(crate) struct QuerySchema {
    pub(crate) id: String,
}

pub(crate) trait RouteCatalog {
    /// Resolve a navigation target selector to the View it names, with the
    /// suite alias (when any) that the footer and route prefixes display.
    fn resolve(&self, selector: &str) -> Option<ViewLocation>;
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
    stack: Vec<ViewInstance>,
    next_instance: u64,
    pending_result: Option<ViewResult>,
    result_committed: bool,
    last_error: Option<RouterError>,
    pending_info: Option<(ViewInstanceId, String, String)>,
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
            stack: Vec::new(),
            next_instance: 1,
            pending_result: None,
            result_committed: false,
            last_error: None,
            pending_info: None,
        }
    }

    pub(crate) fn stack(&self) -> &[ViewInstance] {
        &self.stack
    }

    pub(crate) fn active(&self) -> Option<&ViewInstance> {
        self.stack.last()
    }

    pub(crate) fn active_mut(&mut self) -> Option<&mut ViewInstance> {
        self.stack.last_mut()
    }

    fn pop_view(&mut self) -> Option<ViewInstance> {
        self.stack.pop()
    }

    fn remove_view(&mut self, index: usize) -> ViewInstance {
        self.stack.remove(index)
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
        let location = match self.routes.resolve(&request.target) {
            Some(location) => location,
            None => {
                let error = anyhow::anyhow!("unknown navigation target {:?}", request.target);
                self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
                return Err(error);
            }
        };
        if request.query.target != location.target {
            let error = anyhow::anyhow!(
                "navigation query target {:?} does not match target {:?}",
                request.query.target,
                location.target
            );
            self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
            return Err(error);
        }
        if let Err(error) = self.routes.validate_query(&request.query) {
            self.notify_transition_rejected(source_id, ViewInstanceId(0), &error);
            return Err(error);
        }
        let continuation = match continuation {
            Continuation::Call(mut boundary) => {
                if boundary.caller == ViewInstanceId(0)
                    && let Some(parent_id) = source_id.or_else(|| self.stack.last().map(|e| e.id))
                {
                    boundary.caller = parent_id;
                }
                Continuation::Call(boundary)
            }
            other => other,
        };
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
        canonical_request.target = location.target.clone();
        canonical_request.query.target = location.target.clone();
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
            location,
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
            ViewDecision::ClearInput => {
                if let Some(source) = source
                    && let Some(index) = self.stack.iter().position(|entry| entry.id == source)
                {
                    let context = self.stack[index].context.clone();
                    self.stack[index].view.clear_input(&context)?;
                }
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
                    EffectRequest::ShowFeedback { .. } => None,
                };
                let result = match executor.as_mut() {
                    Some(executor) => match (**executor).execute(effect.clone(), &context) {
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
                        if let EffectRequest::ShowFeedback { message, level } = &effect {
                            match level {
                                crate::protocol::FeedbackLevel::Error => {
                                    self.record_error(
                                        Some(context.instance),
                                        &anyhow::anyhow!("{message}"),
                                    );
                                }
                                crate::protocol::FeedbackLevel::Warning
                                | crate::protocol::FeedbackLevel::Info => {
                                    self.pending_info = Some((
                                        context.instance,
                                        context.location.target.clone(),
                                        message.clone(),
                                    ));
                                }
                            }
                        } else if let Some(message) = success_message {
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
            ViewDecision::Stay | ViewDecision::Invalidate => {}
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
            .then_some(self.pending_result.as_ref())
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
    fn resolve(&self, selector: &str) -> Option<ViewLocation> {
        self.routes.get(selector).map(|reference| ViewLocation {
            alias: (selector != reference).then(|| selector.to_string()),
            target: reference.clone(),
        })
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
mod tests;
