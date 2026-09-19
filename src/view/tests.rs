#![allow(clippy::arc_with_non_send_sync)]

use super::*;
use crate::protocol::contracts::TaskOutcome;
use std::cell::RefCell;
use std::rc::Rc;

struct TestView {
    runtime: Value,
}
impl View for TestView {
    fn command_snapshot(&self) -> ViewCommandSnapshot {
        ViewCommandSnapshot {
            engine_type: "test".to_string(),
            parameters: Value::Null,
            raw_input: String::new(),
            runtime: self.runtime.clone(),
            publication: None,
            revision: 0,
            owner_view: None,
        }
    }
    fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
        Ok(match event {
            ViewEvent::Lifecycle(_) => ViewDecision::Stay,
            ViewEvent::Input(InputEvent::Key {
                key: Key::Char('m'),
                ..
            }) => {
                self.runtime = serde_json::json!("modified");
                ViewDecision::Invalidate
            }
            ViewEvent::Input(InputEvent::Eof) => ViewDecision::Exit,
            ViewEvent::Input(InputEvent::Key {
                key: Key::Enter, ..
            }) => ViewDecision::Return(ViewResult::new(Value::String("done".into()))),
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
            }) => ViewDecision::Return(ViewResult::new(Value::String("grandchild-value".into()))),
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
    fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
        if let ViewEvent::Lifecycle(ref lifecycle) = event {
            self.events.borrow_mut().push(format!("{lifecycle:?}"));
            if matches!(self.fault, Some(Fault::Activation))
                && *lifecycle == LifecycleEvent::Activated
            {
                anyhow::bail!("activation failed")
            }
            if matches!(self.fault, Some(Fault::Closing)) && *lifecycle == LifecycleEvent::Closing {
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
fn call_continuation_delivers_structured_command_selection_result_to_caller_handler() {
    struct TypedRecordingHandler {
        received: Rc<RefCell<Option<Value>>>,
    }
    impl CallReturnHandler for TypedRecordingHandler {
        fn resume(
            &self,
            _: &ViewLocation,
            _: &ViewContext,
            _: &ViewCommandSnapshot,
            result: &ViewResult,
        ) -> Result<ViewDecision> {
            *self.received.borrow_mut() = Some(result.value.clone());
            Ok(ViewDecision::Stay)
        }
    }

    struct CommandSelectionChildView;
    impl View for CommandSelectionChildView {
        fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            match event {
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Enter, ..
                }) => Ok(ViewDecision::Return(ViewResult::new(serde_json::json!(
                    "app:run"
                )))),
                _ => Ok(ViewDecision::Stay),
            }
        }
        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult::default())
        }
    }

    struct CustomFactory;
    impl ViewFactory for CustomFactory {
        fn create(
            &self,
            req: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            if req.target == "child" {
                Ok(Box::new(CommandSelectionChildView))
            } else {
                Ok(Box::new(TestView {
                    runtime: Value::Null,
                }))
            }
        }
    }

    let received = Rc::new(RefCell::new(None));
    let mut routes = MapRouteCatalog::default();
    routes.insert("root", "root");
    routes.insert("child", "child");
    let mut router = Router::new(Box::new(routes), Box::new(CustomFactory));
    let root = router.push(request("root")).unwrap();
    router
        .call(
            request("child"),
            Continuation::Call(CallBoundary {
                caller: root,
                handler: Arc::new(TypedRecordingHandler {
                    received: Rc::clone(&received),
                }),
            }),
        )
        .unwrap();

    router
        .dispatch(ViewEvent::Input(InputEvent::Key {
            key: Key::Enter,
            raw: vec![b'\r'],
        }))
        .unwrap();

    assert_eq!(router.active().unwrap().id, root);
    assert_eq!(*received.borrow(), Some(serde_json::json!("app:run")));
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
        .dispatch(ViewEvent::Input(InputEvent::Key {
            key: Key::Char('m'),
            raw: vec![b'm'],
        }))
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
    router.pending_result = Some(ViewResult::new(Value::Null));
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
            ViewDecision::Return(ViewResult::new(Value::Null)),
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
                ViewDecision::Return(ViewResult::new(Value::String("continued".to_string()))),
            ),
        )
        .unwrap();

    let mut executor = None;
    router
        .process_decision_inner(
            ViewDecision::Return(ViewResult::new(Value::String("child-value".to_string()))),
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
    let decision = ViewDecision::Batch(vec![
        ViewDecision::Effect(EffectRequest::CopyToClipboard("value".into())),
        ViewDecision::Transition(TransitionRequest::Push(request("child"))),
    ]);
    let mut executor = EffectRecorder { calls: Vec::new() };
    let error = router
        .process_with_effects(decision, root, &mut executor)
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
    let root = router.push(request("root")).unwrap();
    let decision = ViewDecision::Batch(vec![ViewDecision::Batch(vec![
        ViewDecision::Effect(EffectRequest::CopyToClipboard("first".into())),
        ViewDecision::Effect(EffectRequest::CopyToClipboard("second".into())),
    ])]);
    let mut executor = EffectRecorder { calls: Vec::new() };
    assert!(
        router
            .process_with_effects(decision, root, &mut executor)
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
    let root = router.push(request("root")).unwrap();
    let mut executor = EffectRecorder { calls: Vec::new() };
    router
        .process_with_effects(action.clone(), root, &mut executor)
        .unwrap();
    assert_eq!(executor.calls.len(), 1);
    assert!(router.stack().is_empty());

    let mut routes = MapRouteCatalog::default();
    routes.insert("root", "root");
    let mut router = Router::new(Box::new(routes), Box::new(TestFactory));
    let root = router.push(request("root")).unwrap();
    assert!(
        router
            .process_with_effects(action, root, &mut FailingEffectExecutor)
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
