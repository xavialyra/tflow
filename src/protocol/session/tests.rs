use super::*;
use crate::command::{CommandEntry, CommandRegistry, CommandScope};
use crate::protocol::ProtocolCommandService;
use crate::view::{
    EffectRequest, EffectResult, MapRouteCatalog, ParsedQuery, View, ViewContext,
    ViewFactory, ViewMetadata, ViewServices,
};
use ratatui::{Terminal, backend::TestBackend, layout::Position};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

fn test_invocation(
    config: &crate::workflow::config::CompiledConfig,
    root_view: &str,
) -> Arc<crate::workflow::InvocationContext> {
    Arc::new(
        crate::workflow::InvocationContext::new(
            root_view.to_string(),
            serde_json::json!({
                "stdin": {"path": null, "length": 0, "is_tty": true}
            }),
            config.instantiate_parameters(root_view).unwrap(),
        )
        .unwrap(),
    )
}

fn request(target: &str) -> NavigationRequest {
    NavigationRequest::new(target, ParsedQuery::new(target, "query", Value::Null))
}

#[derive(Clone)]
struct Factory {
    events: Rc<RefCell<Vec<InputEvent>>>,
}

struct SyntheticView {
    target: String,
    events: Rc<RefCell<Vec<InputEvent>>>,
    runtime: Value,
    publication: Option<crate::view::ViewPublication>,
    revision: u64,
}

impl View for SyntheticView {
    fn preferred_top_inset(&self) -> u16 {
        if self.target == "zero_inset" { 0 } else { 1 }
    }

    fn engine_commands(&self, _: &ViewContext) -> Vec<CommandEntry> {
        vec![CommandEntry::new(
            "local",
            Some("ok".to_string()),
            Some(crate::input::Key::Enter),
            CommandScope::Engine,
            Arc::new(|| Ok(ViewDecision::Stay)),
        )]
    }

    fn retained_content_area(&self, area: Rect) -> Option<Rect> {
        if self.target.starts_with("picker_") {
            // Stand-in for a Picker body: the first row belongs to this
            // instance's live input and must never be covered.
            let skip = area.height.min(1);
            Some(Rect::new(
                area.x,
                area.y.saturating_add(skip),
                area.width,
                area.height.saturating_sub(skip),
            ))
        } else {
            Some(area)
        }
    }

    fn command_snapshot(&self) -> ViewCommandSnapshot {
        let engine_type = if self.target.starts_with("picker") {
            crate::workflow::config::ENGINE_PICKER.to_string()
        } else {
            "test".to_string()
        };
        ViewCommandSnapshot {
            engine_type,
            parameters: Value::Null,
            raw_input: String::new(),
            runtime: self.runtime.clone(),
            publication: self.publication.clone(),
            revision: self.revision,
        }
    }

    fn event(&mut self, event: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
        match event {
            ViewEvent::Lifecycle(_) => Ok(ViewDecision::Stay),
            ViewEvent::Input(input) => {
                self.events.borrow_mut().push(input.clone());
                match input {
                    InputEvent::Key {
                        key: crate::input::Key::Char('p'),
                        ..
                    } => Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                        "payload".to_string(),
                    ))),
                    InputEvent::Key {
                        key: crate::input::Key::Char('n'),
                        ..
                    } => {
                        let query = ParsedQuery::new("child", "query", Value::Null);
                        let mut request = NavigationRequest::new("child", query);
                        request.presentation.mode =
                            crate::workflow::config::ViewPresentationMode::Popup;
                        request.presentation.width = Some(10);
                        request.presentation.height = Some(4);
                        Ok(ViewDecision::Transition(
                            crate::view::TransitionRequest::Push(request),
                        ))
                    }
                    InputEvent::Key {
                        key: crate::input::Key::Char('q'),
                        ..
                    } => {
                        let target = if self.target == "root" {
                            "child"
                        } else {
                            "grandchild"
                        };
                        let query = ParsedQuery::new(target, "query", Value::Null);
                        let mut request = NavigationRequest::new(target, query);
                        request.presentation.mode =
                            crate::workflow::config::ViewPresentationMode::Popup;
                        if self.target == "root" {
                            request.presentation.width = Some(20);
                            request.presentation.height = Some(8);
                        } else {
                            request.presentation.width = Some(10);
                            request.presentation.height = Some(4);
                        }
                        Ok(ViewDecision::Transition(
                            crate::view::TransitionRequest::Push(request),
                        ))
                    }
                    InputEvent::Key {
                        key: crate::input::Key::Char('y'),
                        ..
                    } => Ok(ViewDecision::Close),
                    InputEvent::Key {
                        key: crate::input::Key::Char('e'),
                        ..
                    } => anyhow::bail!("protocol View failure"),
                    InputEvent::Key {
                        key: crate::input::Key::Char('r'),
                        ..
                    } => Ok(ViewDecision::Return(ViewResult::new(Value::String(
                        self.target.clone(),
                    )))),
                    InputEvent::Eof => Ok(ViewDecision::Exit),
                    _ => Ok(ViewDecision::Invalidate),
                }
            }
            ViewEvent::Task(task) => {
                self.publication = Some(crate::view::ViewPublication::new(
                    serde_json::json!({
                        "instance": task.instance.0,
                        "generation": task.generation,
                    }),
                    true,
                ));
                self.revision = self.revision.wrapping_add(1);
                Ok(ViewDecision::Invalidate)
            }
            ViewEvent::Resize(size) => {
                self.runtime = serde_json::json!({
                    "width": size.width,
                    "height": size.height
                });
                self.revision = self.revision.wrapping_add(1);
                Ok(ViewDecision::Invalidate)
            }
            _ => Ok(ViewDecision::Stay),
        }
    }

    fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        _context: &RenderContext,
    ) -> Result<RenderResult> {
        let mut lines = vec![ratatui::text::Line::from(self.target.clone())];
        if self.target.starts_with("body_") {
            lines.push(ratatui::text::Line::from("old body"));
        }
        frame.render_widget(ratatui::widgets::Paragraph::new(lines), area);
        Ok(RenderResult {
            cursor: Some(crate::view::RelativeCursor {
                x: 1,
                y: 1,
                visible: true,
            }),
            metadata: ViewMetadata {
                status: Some(self.target.clone()),
                error: None,
                bindings: None,
            },
        })
    }
}

impl ViewFactory for Factory {
    fn create(
        &self,
        request: &NavigationRequest,
        _: ViewInstanceId,
        services: &ViewServices<'_>,
    ) -> Result<Box<dyn View>> {
        let _ = services.routes.query_schema(&request.query.target);
        let publication = if request.target.starts_with("async_")
            || request.target.starts_with("picker_loading")
        {
            Some(crate::view::ViewPublication::new(Value::Null, false))
        } else {
            None
        };
        Ok(Box::new(SyntheticView {
            target: request.target.clone(),
            events: Rc::clone(&self.events),
            runtime: Value::Null,
            publication,
            revision: 0,
        }))
    }
}

struct Effects {
    calls: Rc<RefCell<Vec<EffectRequest>>>,
}

impl EffectExecutor for Effects {
    fn execute(
        &mut self,
        effect: EffectRequest,
        _: &crate::view::ViewContext,
    ) -> Result<EffectResult> {
        self.calls.borrow_mut().push(effect);
        Ok(EffectResult::Complete)
    }
}

#[allow(clippy::type_complexity)]
fn session() -> (
    ProtocolSession,
    Rc<RefCell<Vec<InputEvent>>>,
    Rc<RefCell<Vec<EffectRequest>>>,
) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let effects = Rc::new(RefCell::new(Vec::new()));
    let mut routes = MapRouteCatalog::default();
    routes.insert("root", "root");
    routes.insert("child", "child");
    routes.insert("grandchild", "grandchild");
    routes.insert("zero_inset", "zero_inset");
    routes.insert("async_target", "async_target");
    routes.insert("async_child", "async_child");
    routes.insert("picker_loading", "picker_loading");
    routes.insert("body_root", "body_root");
    routes.insert("root_overlay", "root_overlay");
    let router = Router::new(
        Box::new(routes),
        Box::new(Factory {
            events: Rc::clone(&events),
        }),
    );
    let session = ProtocolSession::new(
        router,
        Box::new(Effects {
            calls: Rc::clone(&effects),
        }),
    );
    (session, events, effects)
}

#[test]
fn command_registry_resolution_follows_view_over_engine_over_host() {
    let mut registry = CommandRegistry::new();
    registry
        .replace_scope(
            CommandScope::Host,
            vec![
                CommandEntry::new(
                    "host_x",
                    Some("host-x".into()),
                    Some(crate::input::Key::Char('x')),
                    CommandScope::Host,
                    Arc::new(|| Ok(ViewDecision::Stay)),
                ),
                CommandEntry::new(
                    "host_g",
                    Some("host-only".into()),
                    Some(crate::input::Key::Char('g')),
                    CommandScope::Host,
                    Arc::new(|| Ok(ViewDecision::Stay)),
                ),
            ],
        )
        .unwrap();
    registry
        .replace_scope(
            CommandScope::View,
            vec![
                CommandEntry::new(
                    "view_x",
                    Some("view-x".into()),
                    Some(crate::input::Key::Char('x')),
                    CommandScope::View,
                    Arc::new(|| Ok(ViewDecision::Stay)),
                ),
                CommandEntry::new(
                    "view_l",
                    Some("view-only".into()),
                    Some(crate::input::Key::Char('l')),
                    CommandScope::View,
                    Arc::new(|| Ok(ViewDecision::Stay)),
                ),
            ],
        )
        .unwrap();

    let snapshot = ChromeSnapshot::from_registry(&registry);
    let bindings = snapshot.to_binding_set();
    assert_eq!(bindings.entries().len(), 3);
    assert_eq!(
        registry.resolve(crate::input::Key::Char('x')).unwrap().id,
        "view_x"
    );
}

#[test]
fn copy_feedback_renders_until_input_and_errors_take_priority() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('p'),
            raw: vec![b'p'],
        })
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    let render_message = |session: &mut ProtocolSession, terminal: &mut Terminal<TestBackend>| {
        terminal
            .draw(|frame| {
                session.render(frame, frame.area(), None).unwrap();
            })
            .unwrap();
        (0..80)
            .map(|x| terminal.backend().buffer()[(x, 9)].symbol())
            .collect::<String>()
    };
    assert!(
        render_message(&mut session, &mut terminal).contains("INFO [root]: Copied to clipboard")
    );
    session
        .dispatch_with_owned_effects(ViewEvent::Tick)
        .unwrap();
    assert!(render_message(&mut session, &mut terminal).contains("Copied to clipboard"));
    session.report_error("copy failed");
    session.report_info("another notification");
    let row = render_message(&mut session, &mut terminal);
    assert!(row.contains("ERROR [root]: copy failed"));
    assert!(!row.contains("another notification"));
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('a'),
            raw: vec![b'a'],
        })
        .unwrap();
    let row = render_message(&mut session, &mut terminal);
    assert!(!row.contains("INFO"));
    assert!(!row.contains("ERROR"));
    assert!(row.contains("ok"));
}

#[test]
fn info_expiration_honors_deadline_and_replacement() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    let before = Instant::now();
    session.report_info("first");
    let deadline = session.active_info.as_ref().unwrap().expires_at;
    assert!(deadline >= before + Duration::from_secs(3));
    assert!(deadline <= Instant::now() + Duration::from_secs(3));
    session.expire_info(deadline - Duration::from_nanos(1));
    assert!(session.active_info.is_some());
    session.expire_info(deadline);
    assert!(session.active_info.is_none());

    session.report_info("old");
    let old_deadline = Instant::now() - Duration::from_secs(1);
    session.active_info.as_mut().unwrap().expires_at = old_deadline;
    session.report_info("replacement");
    session.expire_info(old_deadline);
    assert_eq!(
        session.active_info.as_ref().unwrap().label,
        "INFO [root]: replacement"
    );
    assert!(session.active_info.as_ref().unwrap().expires_at > old_deadline);
}

#[test]
fn idle_tick_expires_info_in_footer_and_popup_without_clearing_errors() {
    for popup in [false, true] {
        let (mut session, _, _) = session();
        let mut root = request("root");
        if popup {
            root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
            root.presentation.width = Some(60);
            root.presentation.height = Some(6);
        }
        session.start_root(root).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let render_text = |session: &mut ProtocolSession, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| {
                    session.render(frame, frame.area(), None).unwrap();
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..12)
                .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        session.report_info("temporary feedback");
        assert!(render_text(&mut session, &mut terminal).contains("temporary feedback"));
        session.active_info.as_mut().unwrap().expires_at = Instant::now();
        session.tick().unwrap();
        let text = render_text(&mut session, &mut terminal);
        assert!(!text.contains("temporary feedback"));
        assert!(text.contains("ok"));

        session.report_error("persistent error");
        session.report_info("hidden feedback");
        session.active_info.as_mut().unwrap().expires_at = Instant::now();
        session.tick().unwrap();
        assert!(session.active_info.is_none());
        assert!(render_text(&mut session, &mut terminal).contains("persistent error"));
    }
}

#[test]
fn popup_copy_feedback_uses_bottom_border_and_clears_on_navigation() {
    let (mut session, _, _) = session();
    let mut root = request("root");
    root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
    root.presentation.width = Some(60);
    root.presentation.height = Some(6);
    session.start_root(root).unwrap();
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('p'),
            raw: vec![b'p'],
        })
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let contents = (0..12)
        .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>();
    assert!(
        contents
            .iter()
            .any(|row| row.contains("INFO [root]: Copied to clipboard"))
    );
    assert!(contents[11].trim().is_empty());
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('q'),
            raw: vec![b'q'],
        })
        .unwrap();
    assert_eq!(
        session.router.active().unwrap().context.location.target,
        "child"
    );
    assert!(session.active_info.is_none());
}

#[test]
fn delivers_lossless_input_and_honors_global_precedence() {
    let (mut session, events, effects) = session();
    session.start_root(request("root")).unwrap();
    let global_cmd = CommandEntry::new(
        "global.copy",
        None,
        Some(crate::input::Key::Char('g')),
        CommandScope::Host,
        Arc::new(|| {
            Ok(ViewDecision::Effect(EffectRequest::CopyToClipboard(
                "global".to_string(),
            )))
        }),
    );
    session
        .registry
        .write()
        .unwrap()
        .replace_scope(CommandScope::Host, vec![global_cmd])
        .unwrap();
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('g'),
            raw: vec![0x1b, b'g'],
        })
        .unwrap();
    assert!(events.borrow().is_empty());
    assert_eq!(
        effects.borrow().as_slice(),
        &[EffectRequest::CopyToClipboard("global".to_string())]
    );
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('x'),
            raw: vec![b'x'],
        })
        .unwrap();
    session
        .input(InputEvent::Paste {
            text: Some("paste".to_string()),
            raw: b"\x1b[200~paste\x1b[201~".to_vec(),
        })
        .unwrap();
    session.input(InputEvent::Bytes(vec![0xff])).unwrap();
    session.input(InputEvent::Eof).unwrap();
    assert_eq!(events.borrow().len(), 4);
    assert_eq!(
        events.borrow()[0],
        InputEvent::Key {
            key: crate::input::Key::Char('x'),
            raw: vec![b'x']
        }
    );
    assert_eq!(events.borrow()[2], InputEvent::Bytes(vec![0xff]));
    assert_eq!(events.borrow()[3], InputEvent::Eof);
}

#[test]
fn view_errors_are_visible_through_protocol_session_and_router() {
    let (mut session, _, _) = session();
    let root = session.start_root(request("root")).unwrap();
    let error = session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('e'),
            raw: vec![b'e'],
        })
        .unwrap_err();
    assert!(error.to_string().contains("protocol View failure"));
    let router_error = session.take_error().unwrap();
    assert_eq!(router_error.source, Some(root));
    assert!(router_error.message.contains("protocol View failure"));
    assert_eq!(session.router().active().unwrap().id, root);
}

#[test]
fn passthrough_binding_still_prepares_its_command_decision() {
    let config = crate::workflow::config::load_test_fixture().unwrap();
    let cancellation = crate::lifecycle::CancellationToken::new();
    let context = ViewContext::new(ViewInstanceId(40), "dmenu:main");
    let parameters = config.instantiate_parameters("dmenu:main").unwrap();
    let snapshot = ViewCommandSnapshot {
        engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
        parameters: config.parameter_values(&parameters).unwrap(),
        raw_input: "typed".to_string(),
        runtime: Value::Null,
        publication: Some(crate::view::ViewPublication::new(
            serde_json::json!({
                "item": null,
                "input": "typed",
            }),
            true,
        )),
        revision: 0,
    };

    let invocation = test_invocation(&config, "dmenu:main");
    let service = ProtocolCommandService::new(Arc::new(config), invocation, cancellation);
    let view_cmds = service.build_view_commands(&context, &snapshot).unwrap();
    let accept_cmd = view_cmds
        .into_iter()
        .find(|entry| entry.id == "accept")
        .expect("view commands must contain accept");
    assert!(matches!(
        accept_cmd.execute_action().unwrap(),
        ViewDecision::Return(_)
    ));
}

#[test]
fn command_call_records_caller_and_runs_non_null_return_continuation() {
    let config = crate::workflow::config::load_test_fixture().unwrap();
    let cancellation = crate::lifecycle::CancellationToken::new();
    let caller = ViewContext::new(ViewInstanceId(41), "dmenu:main");
    let parameters = config.instantiate_parameters("dmenu:main").unwrap();
    let snapshot = ViewCommandSnapshot {
        engine_type: crate::workflow::config::ENGINE_PICKER.to_string(),
        parameters: config.parameter_values(&parameters).unwrap(),
        raw_input: String::new(),
        runtime: serde_json::json!({"revision": 2}),
        publication: Some(crate::view::ViewPublication::new(
            serde_json::json!({
                "item": {
                    "text": "first",
                    "value": "0",
                    "metadata": {},
                },
                "input": ""
            }),
            true,
        )),
        revision: 2,
    };

    let stdin_path =
        std::env::temp_dir().join(format!("tflow-protocol-session-{}", std::process::id()));
    std::fs::write(&stdin_path, b"first\n").unwrap();
    let invocation = Arc::new(
        crate::workflow::InvocationContext::new(
            "dmenu:main".to_string(),
            serde_json::json!({
                "stdin": {
                    "path": stdin_path.to_string_lossy(),
                    "length": 6,
                    "is_tty": false
                }
            }),
            config.instantiate_parameters("dmenu:main").unwrap(),
        )
        .unwrap(),
    );
    let service = ProtocolCommandService::new(Arc::new(config), invocation, cancellation);
    let shared_snapshot = Arc::new(RwLock::new(ChromeSnapshot::default()));
    let registry = Arc::new(RwLock::new(CommandRegistry::new()));
    let host_cmds = service
        .build_host_commands(shared_snapshot.clone(), Arc::clone(&registry))
        .unwrap();

    let engine_cmds = service.build_engine_commands(&caller, &snapshot).unwrap();
    registry
        .write()
        .unwrap()
        .replace_scope(CommandScope::Host, host_cmds.clone())
        .unwrap();
    registry
        .write()
        .unwrap()
        .replace_scope(CommandScope::Engine, engine_cmds)
        .unwrap();
    *shared_snapshot.write().unwrap() = ChromeSnapshot::from_registry(&registry.read().unwrap())
        .with_active_instance(Some(caller.instance));

    let cmd_entry = host_cmds.into_iter().find(|e| e.id == "commands").unwrap();
    let call_decision = cmd_entry.execute_action().unwrap();
    let ViewDecision::Transition(crate::view::TransitionRequest::Call {
        request: _req,
        continuation: crate::view::Continuation::Call(boundary),
    }) = call_decision
    else {
        panic!("commands must call");
    };

    let selected_command = serde_json::json!({"ref": {"id": "accept"}});
    let continued = boundary
        .handler
        .resume(
            &crate::view::ViewLocation::new("__commands:main"),
            &caller,
            &snapshot,
            &ViewResult::new(selected_command),
        )
        .unwrap();
    let ViewDecision::Return(result) = continued else {
        panic!("selected command must continue into its configured return");
    };
    assert_eq!(result.value, serde_json::json!("first"));

    // Expired or unknown command ID does not execute old callback and returns Stay
    let unknown_command = serde_json::json!({"ref": {"id": "nonexistent"}});
    let continued_unknown = boundary
        .handler
        .resume(
            &crate::view::ViewLocation::new("__commands:main"),
            &caller,
            &snapshot,
            &ViewResult::new(unknown_command),
        )
        .unwrap();
    assert!(matches!(continued_unknown, ViewDecision::Stay));

    // Selected Host parameters command executes and transitions to form call
    let parameters_command = serde_json::json!({"ref": {"id": "parameters"}});
    let continued_params = boundary
        .handler
        .resume(
            &crate::view::ViewLocation::new("__commands:main"),
            &caller,
            &snapshot,
            &ViewResult::new(parameters_command),
        )
        .unwrap();
    let ViewDecision::Transition(crate::view::TransitionRequest::Call { request, .. }) =
        continued_params
    else {
        panic!("parameters command must transition to form call");
    };
    assert_eq!(request.target, "__form:main");

    std::fs::remove_file(stdin_path).unwrap();
}

#[test]
fn root_popup_uses_the_same_content_host_geometry_as_nested_popups() {
    let (mut session, _, _) = session();
    let mut root = request("root");
    root.presentation.mode = crate::workflow::config::ViewPresentationMode::Popup;
    root.presentation.width = Some(12);
    root.presentation.height = Some(6);
    session.start_root(root).unwrap();
    session
        .resize(TerminalSize {
            width: 40,
            height: 10,
        })
        .unwrap();
    assert_eq!(
        session
            .router()
            .active()
            .unwrap()
            .view
            .command_snapshot()
            .runtime,
        serde_json::json!({"width": 10, "height": 4})
    );

    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        Position { x: 16, y: 4 }
    );
    assert_eq!(
        terminal.backend().buffer().cell((14, 2)).unwrap().symbol(),
        "┌"
    );
    assert_eq!(
        terminal.backend().buffer().cell((15, 3)).unwrap().symbol(),
        "r"
    );
}

#[test]
fn resize_and_render_follow_nested_content_host_geometry() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    session
        .resize(TerminalSize {
            width: 40,
            height: 10,
        })
        .unwrap();
    assert_eq!(
        session
            .router()
            .active()
            .unwrap()
            .view
            .command_snapshot()
            .runtime,
        serde_json::json!({"width": 38, "height": 8})
    );

    for expected in [
        ("child", serde_json::json!({"width": 18, "height": 6})),
        ("grandchild", serde_json::json!({"width": 8, "height": 2})),
    ] {
        session
            .input(InputEvent::Key {
                key: crate::input::Key::Char('q'),
                raw: vec![b'q'],
            })
            .unwrap();
        let active = session.router().active().unwrap();
        assert_eq!(active.context.location.target, expected.0);
        assert_eq!(active.view.command_snapshot().runtime, expected.1);
    }

    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let mut rendered = None;
    terminal
        .draw(|frame| rendered = Some(session.render(frame, frame.area(), None).unwrap()))
        .unwrap();
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        Position { x: 17, y: 5 }
    );
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), " ");
    assert_eq!(buffer.cell((1, 1)).unwrap().symbol(), "r");
    assert_eq!(buffer.cell((10, 1)).unwrap().symbol(), "┌");
    assert_eq!(buffer.cell((11, 2)).unwrap().symbol(), "c");
    assert_eq!(buffer.cell((15, 3)).unwrap().symbol(), "┌");
    assert_eq!(buffer.cell((16, 4)).unwrap().symbol(), "g");
    assert_eq!(rendered.unwrap().footer.location.label(), "grandchild");
}

#[test]
fn transitions_effects_tasks_and_popup_render_footer_are_hosted() {
    let (mut session, events, effects) = session();
    session.start_root(request("root")).unwrap();
    assert!(session.start_root(request("root")).is_err());
    assert_eq!(
        session.router().active().unwrap().context.presentation.mode,
        crate::workflow::config::ViewPresentationMode::Inline
    );
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('p'),
            raw: vec![b'p'],
        })
        .unwrap();
    assert_eq!(effects.borrow().len(), 1);
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('n'),
            raw: vec![b'n'],
        })
        .unwrap();
    assert_eq!(session.router().stack().len(), 2);
    let child = session.router().active().unwrap().id;
    session
        .task(TaskEvent {
            instance: child,
            task: TaskId(1),
            generation: 1,
            outcome: TaskOutcome::Completed(Value::Null),
        })
        .unwrap();
    assert_eq!(events.borrow().len(), 2);
    assert_eq!(
        session
            .router()
            .active()
            .unwrap()
            .view
            .command_snapshot()
            .publication
            .map(|publication| publication.current),
        Some(serde_json::json!({"instance": child.0, "generation": 1}))
    );
    assert_eq!(
        session.router().active().unwrap().context.presentation.mode,
        crate::workflow::config::ViewPresentationMode::Popup
    );
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let mut rendered = None;
    terminal
        .draw(|frame| {
            rendered = Some(session.render(frame, frame.area(), None).unwrap());
        })
        .unwrap();
    let rendered = rendered.unwrap();
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        Position { x: 17, y: 5 }
    );
    assert_eq!(
        terminal.backend().buffer().cell((0, 0)).unwrap().symbol(),
        " "
    );
    assert_eq!(
        terminal.backend().buffer().cell((1, 0)).unwrap().symbol(),
        " "
    );
    assert_eq!(
        terminal.backend().buffer().cell((1, 1)).unwrap().symbol(),
        "r"
    );
    assert_eq!(
        terminal.backend().buffer().cell((15, 3)).unwrap().symbol(),
        "┌"
    );
    assert_eq!(
        terminal.backend().buffer().cell((16, 4)).unwrap().symbol(),
        "c"
    );
    // Popup bottom border contains hints
    assert_eq!(
        terminal.backend().buffer().cell((15, 6)).unwrap().symbol(),
        "└"
    );
    assert_eq!(
        terminal.backend().buffer().cell((24, 6)).unwrap().symbol(),
        "┘"
    );
    let bottom_border: String = (15..=24)
        .map(|x| terminal.backend().buffer().cell((x, 6)).unwrap().symbol())
        .collect();
    assert!(bottom_border.contains("ok"));

    // Global footer row (y = 9) is blank while popup is active
    for x in 0..40 {
        assert_eq!(
            terminal.backend().buffer().cell((x, 9)).unwrap().symbol(),
            " "
        );
    }

    assert_eq!(rendered.footer.location.label(), "child");
    assert_eq!(rendered.footer.status.as_deref(), Some("child"));
    session.eof().unwrap();
    assert!(session.router().stack().is_empty());
}

#[test]
fn view_diagnostic_clears_when_resolved_and_session_error_clears_on_input() {
    let view_error = Rc::new(RefCell::new(None));
    struct DiagView(Rc<RefCell<Option<String>>>);
    impl View for DiagView {
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
        fn event(&mut self, _: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(ViewDecision::Stay)
        }
        fn render(&self, _: &mut Frame, _: Rect, _: &RenderContext) -> Result<RenderResult> {
            Ok(RenderResult {
                cursor: None,
                metadata: ViewMetadata {
                    status: None,
                    error: None,
                    bindings: None,
                },
            })
        }
        fn chrome(&self, _context: &ViewContext) -> Result<crate::view::ViewChrome> {
            Ok(crate::view::ViewChrome {
                status: None,
                error: self.0.borrow().clone(),
                ..Default::default()
            })
        }
    }
    struct DiagFactory(Rc<RefCell<Option<String>>>);
    impl ViewFactory for DiagFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(DiagView(Rc::clone(&self.0))))
        }
    }
    let mut routes = MapRouteCatalog::default();
    routes.insert("root", "root");
    let router = Router::new(
        Box::new(routes),
        Box::new(DiagFactory(Rc::clone(&view_error))),
    );
    let mut session = ProtocolSession::new(
        router,
        Box::new(Effects {
            calls: Rc::new(RefCell::new(Vec::new())),
        }),
    );
    session.start_root(request("root")).unwrap();

    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let render_footer_error = |session: &mut ProtocolSession,
                               terminal: &mut Terminal<TestBackend>| {
        let mut res = None;
        terminal
            .draw(|frame| {
                res = Some(
                    session
                        .render(frame, frame.area(), None)
                        .unwrap()
                        .footer
                        .error,
                );
            })
            .unwrap();
        res.unwrap()
    };

    // 1. Initially no error
    assert_eq!(render_footer_error(&mut session, &mut terminal), None);

    // 2. View produces an error
    *view_error.borrow_mut() = Some("invalid input syntax".to_string());
    assert_eq!(
        render_footer_error(&mut session, &mut terminal),
        Some("ERROR [root]: invalid input syntax".to_string())
    );

    // 3. View clears its error (e.g. user corrected the input)
    *view_error.borrow_mut() = None;
    assert_eq!(render_footer_error(&mut session, &mut terminal), None);
    assert_eq!(session.active_error, None);

    // 4. Session reports an error
    session.report_error("session navigation failure");
    // Render does NOT clear session error even though view_error is None
    assert_eq!(
        render_footer_error(&mut session, &mut terminal),
        Some("ERROR [root]: session navigation failure".to_string())
    );
    assert_eq!(
        render_footer_error(&mut session, &mut terminal),
        Some("ERROR [root]: session navigation failure".to_string())
    );

    // 5. Next user input clears the session error
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('a'),
            raw: vec![b'a'],
        })
        .unwrap();
    assert_eq!(session.active_error, None);
    assert_eq!(render_footer_error(&mut session, &mut terminal), None);
}

#[test]
fn focus_exclusive_status_lifecycle() {
    let (mut session, _events, _effects) = session();
    session.start_root(request("root")).unwrap();

    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    // 1. Root is focused: global footer row (y=9) has root's label and local bindings
    terminal
        .draw(|frame| {
            let _ = session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    let footer_row: String = (0..40)
        .map(|x| terminal.backend().buffer().cell((x, 9)).unwrap().symbol())
        .collect();
    assert!(footer_row.contains("root"));
    assert!(footer_row.contains("ok"));

    // 2. Open child popup ('n' key)
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('n'),
            raw: vec![b'n'],
        })
        .unwrap();

    terminal
        .draw(|frame| {
            let _ = session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();

    // Global footer is blank
    for x in 0..40 {
        assert_eq!(
            terminal.backend().buffer().cell((x, 9)).unwrap().symbol(),
            " "
        );
    }
    // Child popup (15..=24, y=6) bottom border has local key hints
    let child_bottom: String = (15..=24)
        .map(|x| terminal.backend().buffer().cell((x, 6)).unwrap().symbol())
        .collect();
    assert!(child_bottom.contains("ok"));

    // 3. Child returns to root ('r' key)
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('r'),
            raw: vec![b'r'],
        })
        .unwrap();

    terminal
        .draw(|frame| {
            let _ = session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();

    // Global footer is restored with root's label and local bindings
    let restored_footer: String = (0..40)
        .map(|x| terminal.backend().buffer().cell((x, 9)).unwrap().symbol())
        .collect();
    assert!(restored_footer.contains("root"));
    assert!(restored_footer.contains("ok"));
}

#[test]
fn view_preferred_top_inset_controls_content_area_top_offset_and_resize() {
    let (mut session, _, _) = session();
    session.start_root(request("zero_inset")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    terminal
        .draw(|frame| {
            let _ = session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();

    // zero_inset has preferred_top_inset() == 0, so content starts at y = 0
    assert_eq!(
        terminal.backend().buffer().cell((1, 0)).unwrap().symbol(),
        "z"
    );

    // resize dispatches height = 10 - 0 (top) - 1 (footer) = 9
    session
        .resize(TerminalSize {
            width: 40,
            height: 10,
        })
        .unwrap();
    assert_eq!(
        session.router().stack()[0].view.command_snapshot().runtime,
        serde_json::json!({
            "width": 38,
            "height": 9
        })
    );
}

#[test]
fn custom_view_commands_switch_to_exclusive_commands_in_session() {
    let (mut session, _events, _effects) = session();
    session.start_root(request("root")).unwrap();

    // Initially, root view commands are active (SyntheticView returns "local")
    assert!(
        session
            .registry
            .read()
            .unwrap()
            .resolve_id("local")
            .is_some()
    );

    // When custom commands are returned by active view (e.g. completion)
    let accept = CommandEntry::new(
        "completion.accept",
        Some("Accept".to_string()),
        Some(crate::input::Key::Enter),
        CommandScope::View,
        Arc::new(|| Ok(ViewDecision::Stay)),
    );
    let cancel = CommandEntry::new(
        "completion.cancel",
        Some("Cancel".to_string()),
        Some(crate::input::Key::Escape),
        CommandScope::View,
        Arc::new(|| Ok(ViewDecision::Stay)),
    );

    session
        .registry
        .write()
        .unwrap()
        .replace_scope(CommandScope::View, vec![accept, cancel])
        .unwrap();
    session
        .registry
        .write()
        .unwrap()
        .replace_scope(CommandScope::Engine, Vec::new())
        .unwrap();

    assert_eq!(
        session
            .registry
            .read()
            .unwrap()
            .resolve(crate::input::Key::Enter)
            .unwrap()
            .id,
        "completion.accept"
    );
    assert_eq!(
        session
            .registry
            .read()
            .unwrap()
            .resolve(crate::input::Key::Escape)
            .unwrap()
            .id,
        "completion.cancel"
    );
    assert!(
        session
            .registry
            .read()
            .unwrap()
            .resolve_id("local")
            .is_none()
    );
}

#[test]
fn session_reconciles_engine_commands_from_active_view() {
    struct EngineCmdView;
    impl View for EngineCmdView {
        fn engine_commands(&self, _: &ViewContext) -> Vec<CommandEntry> {
            vec![
                CommandEntry::new(
                    "engine.copy",
                    Some("Copy".to_string()),
                    Some(crate::input::Key::Enter),
                    CommandScope::Engine,
                    Arc::new(|| Ok(ViewDecision::Stay)),
                ),
                CommandEntry::new(
                    "engine.back",
                    Some("Back".to_string()),
                    Some(crate::input::Key::Escape),
                    CommandScope::Engine,
                    Arc::new(|| Ok(ViewDecision::Close)),
                ),
            ]
        }

        fn event(&mut self, _: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(ViewDecision::Stay)
        }

        fn render(
            &self,
            _frame: &mut Frame,
            _area: Rect,
            _context: &RenderContext,
        ) -> Result<RenderResult> {
            Ok(RenderResult {
                cursor: None,
                metadata: ViewMetadata::default(),
            })
        }
    }

    #[derive(Clone)]
    struct EngineFactory;
    impl ViewFactory for EngineFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(EngineCmdView))
        }
    }

    let mut routes = MapRouteCatalog::default();
    routes.insert("engine_view", "engine_view");
    let router = Router::new(Box::new(routes), Box::new(EngineFactory));
    let effects = Rc::new(RefCell::new(Vec::new()));
    let mut session = ProtocolSession::new(
        router,
        Box::new(Effects {
            calls: Rc::clone(&effects),
        }),
    );
    session.start_root(request("engine_view")).unwrap();

    let registry = session.registry.read().unwrap();
    assert_eq!(
        registry.resolve(crate::input::Key::Enter).unwrap().id,
        "engine.copy"
    );
    assert_eq!(
        registry.resolve(crate::input::Key::Escape).unwrap().id,
        "engine.back"
    );
    assert_eq!(
        registry.resolve(crate::input::Key::Enter).unwrap().scope,
        CommandScope::Engine
    );
}

#[test]
#[allow(clippy::type_complexity)]
fn session_dispatches_to_fallback_receiver_when_key_unbound() {
    use crate::input::Key;
    use crate::view::FallbackInputReceiver;

    struct Receiver(Rc<RefCell<Vec<(Key, Vec<u8>)>>>);
    impl FallbackInputReceiver for Receiver {
        fn on_unbound_key(
            &mut self,
            key: Key,
            raw: &[u8],
            _context: &ViewContext,
        ) -> Result<ViewDecision> {
            self.0.borrow_mut().push((key, raw.to_vec()));
            Ok(ViewDecision::Stay)
        }
    }

    struct FallbackView {
        receiver: Receiver,
    }
    impl View for FallbackView {
        fn fallback_receiver(&mut self) -> Option<&mut dyn FallbackInputReceiver> {
            Some(&mut self.receiver)
        }

        fn event(&mut self, _: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(ViewDecision::Stay)
        }

        fn render(
            &self,
            _frame: &mut Frame,
            _area: Rect,
            _context: &RenderContext,
        ) -> Result<RenderResult> {
            Ok(RenderResult {
                cursor: None,
                metadata: ViewMetadata::default(),
            })
        }
    }

    #[derive(Clone)]
    struct FallbackFactory(Rc<RefCell<Vec<(Key, Vec<u8>)>>>);
    impl ViewFactory for FallbackFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(FallbackView {
                receiver: Receiver(Rc::clone(&self.0)),
            }))
        }
    }

    let unbound_log = Rc::new(RefCell::new(Vec::new()));
    let mut routes = MapRouteCatalog::default();
    routes.insert("fallback_view", "fallback_view");
    let router = Router::new(
        Box::new(routes),
        Box::new(FallbackFactory(Rc::clone(&unbound_log))),
    );
    let effects = Rc::new(RefCell::new(Vec::new()));
    let mut session = ProtocolSession::new(
        router,
        Box::new(Effects {
            calls: Rc::clone(&effects),
        }),
    );
    session.start_root(request("fallback_view")).unwrap();

    let decision = session
        .input(InputEvent::Key {
            key: Key::Char('z'),
            raw: b"z".to_vec(),
        })
        .unwrap();
    assert_eq!(decision, ViewDecision::Stay);

    let captured = unbound_log.borrow();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, Key::Char('z'));
    assert_eq!(captured[0].1, b"z");
}

#[test]
fn session_dispatches_view_handler_command_directly_to_view_on_command() {
    use crate::command::CommandEntry;
    use crate::input::Key;

    struct CommandReceiverView {
        received: Rc<RefCell<Vec<(String, String)>>>,
    }

    impl View for CommandReceiverView {
        fn engine_commands(&self, _context: &ViewContext) -> Vec<CommandEntry> {
            vec![CommandEntry::for_event(
                "custom.action",
                Some("Custom Action".to_string()),
                Some(Key::Down),
                CommandScope::Engine,
            )]
        }

        fn on_command(&mut self, id: &str, context: &ViewContext) -> Result<ViewDecision> {
            self.received
                .borrow_mut()
                .push((id.to_string(), context.location.target.clone()));
            Ok(ViewDecision::Invalidate)
        }

        fn event(&mut self, _: ViewEvent, _: &ViewContext) -> Result<ViewDecision> {
            Ok(ViewDecision::Stay)
        }

        fn render(
            &self,
            _frame: &mut Frame,
            _area: Rect,
            _context: &RenderContext,
        ) -> Result<RenderResult> {
            Ok(RenderResult {
                cursor: None,
                metadata: ViewMetadata::default(),
            })
        }
    }

    #[derive(Clone)]
    struct CommandReceiverFactory(Rc<RefCell<Vec<(String, String)>>>);
    impl ViewFactory for CommandReceiverFactory {
        fn create(
            &self,
            _: &NavigationRequest,
            _: ViewInstanceId,
            _: &ViewServices<'_>,
        ) -> Result<Box<dyn View>> {
            Ok(Box::new(CommandReceiverView {
                received: Rc::clone(&self.0),
            }))
        }
    }

    let received_log = Rc::new(RefCell::new(Vec::new()));
    let mut routes = MapRouteCatalog::default();
    routes.insert("cmd_view", "cmd_view");
    let router = Router::new(
        Box::new(routes),
        Box::new(CommandReceiverFactory(Rc::clone(&received_log))),
    );
    let effects = Rc::new(RefCell::new(Vec::new()));
    let mut session = ProtocolSession::new(
        router,
        Box::new(Effects {
            calls: Rc::clone(&effects),
        }),
    );
    session.start_root(request("cmd_view")).unwrap();

    let decision = session
        .input(InputEvent::Key {
            key: Key::Down,
            raw: Vec::new(),
        })
        .unwrap();
    assert_eq!(decision, ViewDecision::Invalidate);

    let received = received_log.borrow();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, "custom.action");
    assert_eq!(received[0].1, "cmd_view");
}

#[test]
fn navigation_grace_retains_previous_content_during_loading() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let now = Instant::now();

    // 1. Initial render shows "root"
    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = (0..4)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(content, "root");

    // 2. Navigate to an async loading view (publication.ready == false)
    session.router.push(request("async_target")).unwrap();
    session.sync_active_commands().unwrap();

    // Retention keeps the previous frame on screen.
    terminal
        .draw(|frame| {
            let result = session.render_at(frame, frame.area(), None, now).unwrap();
            assert_eq!(result.footer.location.label(), "async_target");
        })
        .unwrap();
    assert!(session.navigation.is_retaining());
    let buffer = terminal.backend().buffer();
    let content: String = (0..4)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(content, "root", "retention must keep the previous content");
}

#[test]
fn navigation_grace_clears_immediately_when_target_becomes_ready() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let now = Instant::now();

    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();

    let target_id = session.router.push(request("async_target")).unwrap();
    session.sync_active_commands().unwrap();

    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();
    assert!(session.navigation.is_retaining());

    // Target completes async loading and becomes ready
    session
        .task(TaskEvent {
            task: TaskId(1),
            instance: target_id,
            generation: 1,
            outcome: TaskOutcome::Completed(Value::Null),
        })
        .unwrap();

    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();
    assert!(!session.navigation.is_retaining());
    let buffer = terminal.backend().buffer();
    let content: String = (0..12)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(content, "async_target");
}

#[test]
fn navigation_grace_expires_after_timeout_and_never_rearms() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
    let now = Instant::now();

    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();

    session.router.push(request("async_target")).unwrap();
    session.sync_active_commands().unwrap();

    terminal
        .draw(|frame| {
            session.render_at(frame, frame.area(), None, now).unwrap();
        })
        .unwrap();
    assert!(session.navigation.is_retaining());

    // Past the deadline the still-loading target must show its own frame.
    let expired = now + Duration::from_millis(200);
    terminal
        .draw(|frame| {
            session
                .render_at(frame, frame.area(), None, expired)
                .unwrap();
        })
        .unwrap();
    assert!(!session.navigation.is_retaining());
    let buffer = terminal.backend().buffer();
    let content: String = (0..12)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(content, "async_target");

    // A target that never finishes loading must not re-arm retention.
    terminal
        .draw(|frame| {
            session
                .render_at(
                    frame,
                    frame.area(),
                    None,
                    expired + Duration::from_millis(1),
                )
                .unwrap();
        })
        .unwrap();
    assert!(
        !session.navigation.is_retaining(),
        "expired retention must not re-arm while the target keeps loading"
    );
}

#[test]
fn navigation_grace_covers_a_same_target_replace() {
    let (mut session, _, _) = session();
    session.start_root(request("async_target")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    // 1. Initial async target finishes loading and renders.
    let root_id = session.router.active().unwrap().id;
    session
        .task(TaskEvent {
            task: TaskId(1),
            instance: root_id,
            generation: 1,
            outcome: TaskOutcome::Completed(Value::Null),
        })
        .unwrap();
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(session.navigation.settled().is_some());

    // 2. Replace with a new instance of the same target, the way a Picker
    //    multi-select toggle navigates with `replace = true`.
    session.router.replace(request("async_target")).unwrap();
    session.sync_active_commands().unwrap();

    // 3. Replace mounts a loading instance, so it participates in grace.
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(
        session.navigation.is_retaining(),
        "same-target replace must participate in navigation grace"
    );
    let buffer = terminal.backend().buffer();
    let content: String = (0..12)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(content, "async_target");
}

#[test]
fn popup_does_not_settle_a_base_frame() {
    let (mut session, _, _) = session();
    session.start_root(request("root")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    // 1. Render base view
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    let cached_before = session.navigation.settled().unwrap().clone();

    // 2. Push a popup on top
    let mut popup_req = request("child");
    popup_req.presentation = crate::workflow::config::ViewPresentation {
        mode: crate::workflow::config::ViewPresentationMode::Popup,
        width: Some(20),
        height: Some(5),
    };
    session.router.push(popup_req).unwrap();
    session.sync_active_commands().unwrap();

    // 3. Render while popup is active
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();

    // 4. The settled base frame must not be replaced by the popup frame
    let cached_after = session.navigation.settled().unwrap().clone();
    assert_eq!(
        cached_after.instance, cached_before.instance,
        "Popup render must not replace base snapshot instance"
    );
    let row: String = (1..5)
        .map(|x| cached_after.buffer.cell((x, 1)).unwrap().symbol())
        .collect();
    assert_eq!(
        row, "root",
        "Snapshot must remain the clean base frame, not the popup"
    );
}

#[test]
fn navigation_grace_for_a_loading_picker_retains_only_the_body() {
    let (mut session, _, _) = session();
    session.start_root(request("body_root")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    // 1. Render the ready previous View so a frame snapshot exists.
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(session.navigation.settled().is_some());

    // 2. Navigate to a loading Picker target.
    session.router.push(request("picker_loading")).unwrap();
    session.sync_active_commands().unwrap();

    // 3. The Picker participates in grace, but only its body may be covered;
    //    its own input line must stay on screen.
    terminal
        .draw(|frame| {
            let result = session.render(frame, frame.area(), None).unwrap();
            assert_eq!(result.footer.location.label(), "picker_loading");
        })
        .unwrap();
    assert!(
        session.navigation.is_retaining(),
        "a loading Picker target must participate in navigation grace"
    );

    let buffer = terminal.backend().buffer();
    let input_row: String = (0..14)
        .map(|x| buffer.cell((x + 1, 1)).unwrap().symbol())
        .collect();
    assert_eq!(
        input_row, "picker_loading",
        "the new input row must not be covered by retained pixels"
    );
    let body_row: String = (0..8)
        .map(|x| buffer.cell((x + 1, 2)).unwrap().symbol())
        .collect();
    assert_eq!(
        body_row, "old body",
        "the body must retain the previous frame while loading"
    );
}

#[test]
fn navigation_grace_does_not_flash_a_closed_view_when_returning() {
    let (mut session, _, _) = session();
    session.start_root(request("async_target")).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

    // 1. The root is still loading, so it never becomes a cached frame.
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(session.navigation.settled().is_none());

    // 2. Push a child that finishes loading and is snapshotted.
    let root_id = session.router.active().unwrap().id;
    let child = session.router.push(request("async_child")).unwrap();
    session.sync_active_commands().unwrap();
    session
        .task(TaskEvent {
            task: TaskId(1),
            instance: child,
            generation: 1,
            outcome: TaskOutcome::Completed(Value::Null),
        })
        .unwrap();
    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(session.navigation.settled().is_some());

    // 3. Close the child, returning to the older still-loading root.
    session
        .input(InputEvent::Key {
            key: crate::input::Key::Char('y'),
            raw: vec![b'y'],
        })
        .unwrap();
    assert_eq!(session.router.active().unwrap().id, root_id);

    terminal
        .draw(|frame| {
            session.render(frame, frame.area(), None).unwrap();
        })
        .unwrap();
    assert!(
        !session.navigation.is_retaining(),
        "returning to an older instance must not retain the closed View's frame"
    );
    let content: String = (0..12)
        .map(|x| {
            terminal
                .backend()
                .buffer()
                .cell((x + 1, 1))
                .unwrap()
                .symbol()
        })
        .collect();
    assert_eq!(content, "async_target");
}
