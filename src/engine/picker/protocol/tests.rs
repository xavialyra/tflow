use super::*;
use crate::engine::ProjectedBindingConfig;
use crate::view::{MapRouteCatalog, RouteCatalog, ViewContext};
use anyhow::bail;
use ratatui::style::{Color, Style};
use ratatui::{Terminal, backend::TestBackend};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn request(target: &str) -> NavigationRequest {
    NavigationRequest::new(
        target,
        ParsedQuery::new(target, "query", Value::String(String::new())),
    )
}

fn config_with_tasks(services: PickerViewServices, tasks: TaskRuntime) -> PickerProtocolConfig {
    let config = crate::workflow::config::load_test_fixture().unwrap();
    let parameter_binding = config.parameter_binding("core:default").unwrap();
    PickerProtocolConfig {
        identity: ViewIdentity::new("core:default", crate::workflow::config::ENGINE_PICKER),
        engine: ProjectedEngineConfig::default(),
        bindings: ProjectedBindingConfig::default(),
        services,
        parameter_bindings: BTreeMap::new(),
        parameter_binding,
        theme: ResolvedTheme::terminal(),
        route_entry: true,
        query_prefix: None,
        runtime_snapshot: serde_json::json!({"view": {}}),
        tasks,
    }
}

#[test]
fn completion_candidates_are_read_only_and_replace_only_matching_revision() {
    let mut routes = MapRouteCatalog::default();
    routes.insert("sy", "sys:main");
    assert_eq!(routes.complete("sy")[0].target.reference, "sys:main");
}

#[test]
fn navigation_input_seed_is_separate_from_the_structured_query() {
    let query = ParsedQuery::new("picker", "query", Value::String("ok".into()));
    let request = NavigationRequest::new("picker", query.clone())
        .with_input("draft", 3)
        .unwrap();
    assert_eq!(request.query, query);
    assert_eq!(request.input.as_ref().unwrap().cursor, 3);
    let _ = ViewContext::new(ViewInstanceId(1), "picker");
}

#[test]
fn completion_selection_cycles_through_picker_matches() {
    assert_eq!(cycle_completion_selection(0, 3, -1), 2);
    assert_eq!(cycle_completion_selection(2, 3, 1), 0);
    assert_eq!(cycle_completion_selection(0, 0, 1), 0);
}

#[test]
fn picker_task_registry_rejects_stale_events_and_consumes_the_current_completion() {
    let mut routes = MapRouteCatalog::default();
    routes.insert("core:default", "core:default");
    let tasks = TaskRuntime::new();
    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let services = crate::engine::picker::PickerRuntimeServices::new(
        fixture,
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
        "core:default",
    )
    .view_services();
    let mut view = create_protocol_view(
        config_with_tasks(services, tasks.clone()),
        &request("core:default"),
        ViewInstanceId(1),
        &routes,
    )
    .unwrap();
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context)
        .unwrap();
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    let correlation = (TaskId(1), 1);

    assert!(matches!(
        view.event(
            ViewEvent::Task(TaskEvent {
                instance: ViewInstanceId(1),
                task: correlation.0,
                generation: correlation.1 + 1,
                outcome: TaskOutcome::Failed("stale".to_string()),
            }),
            &context,
        )
        .unwrap(),
        ViewDecision::Stay
    ));
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
        .unwrap();
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    let event = wait_for_task_event(&tasks);
    assert_eq!((event.task, event.generation), correlation);
    view.event(ViewEvent::Task(event), &context).unwrap();
    assert!(matches!(
        view.event(
            ViewEvent::Task(TaskEvent {
                instance: ViewInstanceId(1),
                task: correlation.0,
                generation: correlation.1,
                outcome: TaskOutcome::Completed(Value::Null),
            }),
            &context,
        )
        .unwrap(),
        ViewDecision::Stay
    ));
}

fn wait_for_task_event(tasks: &TaskRuntime) -> TaskEvent {
    for _ in 0..200 {
        if let Some(event) = tasks.drain_events().into_iter().next() {
            return event;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("timed out waiting for task event");
}

struct ExitOnActionRuntime;

impl EngineRuntime for ExitOnActionRuntime {
    fn action(&mut self, _input: EngineActionInput) -> Result<EngineEmission> {
        Ok(EngineEmission::decision(EngineDecision::Exit))
    }

    fn render_model(&self) -> crate::engine::RenderModel {
        crate::engine::RenderModel::new("picker", ())
    }
}

struct StaleAwareTaskRuntime {
    task: Option<crate::task::TaskHandle<()>>,
    input_rejected: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
}

impl EngineRuntime for StaleAwareTaskRuntime {
    fn input_rejected(
        &mut self,
        _expected: crate::engine::ViewContextIdentity,
    ) -> Result<EngineEmission> {
        self.input_rejected.store(true, Ordering::Release);
        Ok(EngineEmission::decision(EngineDecision::Continue))
    }

    fn start_prepared_work(&mut self, starter: &MountTaskStarter) -> bool {
        if self.task.is_some() {
            return false;
        }
        self.task = Some(starter.spawn_latest_with_test_snapshot("items", Value::Null, |_| Ok(())));
        true
    }

    fn poll_work(&mut self) -> Result<Option<EngineEmission>> {
        let Some(mut task) = self.task.take() else {
            return Ok(None);
        };
        match task.try_recv() {
            Ok(crate::task::TaskCompletion::Completed(())) => {
                self.polls.fetch_add(1, Ordering::AcqRel);
                let emission = EngineEmission::decision(EngineDecision::Continue);
                if self.input_rejected.load(Ordering::Acquire) {
                    // The Engine identifies this completion as stale and
                    // consumes it without a publication.
                    Ok(Some(emission))
                } else {
                    Ok(Some(emission.with_publication(
                        crate::engine::ViewContextPublication::new(
                            serde_json::json!({"stale": true}),
                        ),
                    )))
                }
            }
            Ok(crate::task::TaskCompletion::Failed(error)) => bail!(error),
            Ok(crate::task::TaskCompletion::Cancelled) => {
                bail!("test task was unexpectedly cancelled")
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.task = Some(task);
                Ok(None)
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                bail!("test task completion channel disconnected")
            }
        }
    }

    fn render_model(&self) -> crate::engine::RenderModel {
        crate::engine::RenderModel::new("picker", ())
    }
}

fn invalid_integer_binding() -> ParameterBinding {
    let registry = Arc::new(
        crate::workflow::parameter::ParameterRegistry::compile(&serde_json::json!({
            "workflows": {"core": {"views": {"default": {"query": {
                "type": "object",
                "count": {"type": "integer", "default": 1}
            }}}}}
        }))
        .unwrap(),
    );
    registry.parameter_binding("core:default").unwrap()
}

fn view_with_stale_aware_runtime(
    runtime: Box<dyn EngineRuntime>,
    parameter_binding: ParameterBinding,
    tasks: &TaskRuntime,
) -> PickerProtocolView {
    let instance = ViewInstanceId(1);
    let identity = ViewIdentity::new("core:default", crate::workflow::config::ENGINE_PICKER);
    let editor = EditorBuffer::from_raw("1", 1);
    let parameters = ParameterSnapshot::from_parts(
        serde_json::json!({"count": 1}),
        editor.raw.clone(),
        InputSourceIdentity {
            frame: ViewMountId(instance.0),
            generation: editor.revision,
        },
        0,
    );
    PickerProtocolView {
        runtime,
        renderer: create_renderer(RendererFactoryContext).unwrap(),
        options: PickerOptions::default(),
        keymap: PickerKeymap::from_values(None, None).unwrap(),
        route_candidates: Vec::new(),
        recognized_route_selectors: HashSet::new(),
        route_schemas: BTreeMap::new(),
        route_resolutions: BTreeMap::new(),
        completion_prefixes: BTreeMap::new(),
        disabled_keys: HashSet::new(),
        parameter_bindings: BTreeMap::new(),
        route_entry: false,
        query_prefix: None,
        theme: ResolvedTheme::terminal(),
        parameter_binding,
        editor: editor.clone(),
        parameters: parameters.clone(),
        engine_context: engine_context(
            instance,
            &identity,
            &parameters,
            &Value::Null,
            0,
            None,
            editor.snapshot(),
        ),
        runtime_snapshot: Value::Null,
        publication: None,
        state_revision: 0,
        starter: MountTaskStarter::from_lease(tasks, MountTaskLease::new(ViewMountId(1))),
        instance,
        task_registry: ViewTaskRegistry::new(instance),
        task_generation: 0,
        active: false,
        activated_once: false,
        closed: false,
        completion: None,
        route_transition_pending: false,
        defer_work_poll: false,
        task_completion_pending: false,
        publication_ready: false,
        diagnostic: None,
        content_size: (1, 1),
    }
}

#[test]
#[allow(clippy::type_complexity)]
fn preview_body_size_tracks_resize_completion_and_committed_starts() {
    struct SizedRuntime {
        size: (u16, u16),
        seen: Arc<std::sync::Mutex<Vec<(&'static str, (u16, u16))>>>,
    }
    impl EngineRuntime for SizedRuntime {
        fn set_auxiliary_content_size(&mut self, size: (u16, u16)) {
            self.size = size;
            self.seen.lock().unwrap().push(("size", size));
        }
        fn start_prepared_work(&mut self, _: &MountTaskStarter) -> bool {
            self.seen.lock().unwrap().push(("main", self.size));
            false
        }
        fn start_prepared_auxiliary_work(&mut self, _: &MountTaskStarter) -> Vec<(TaskId, u64)> {
            self.seen.lock().unwrap().push(("auxiliary", self.size));
            Vec::new()
        }
        fn action(&mut self, _: EngineActionInput) -> Result<EngineEmission> {
            self.seen.lock().unwrap().push(("action", self.size));
            Ok(EngineEmission::decision(EngineDecision::Continue))
        }
        fn render_model(&self) -> crate::engine::RenderModel {
            crate::engine::RenderModel::new("picker", ())
        }
    }
    let tasks = TaskRuntime::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let runtime = SizedRuntime {
        size: (99, 99),
        seen: seen.clone(),
    };
    let mut view =
        view_with_stale_aware_runtime(Box::new(runtime), invalid_integer_binding(), &tasks);
    view.options.show_input = true;
    view.options.show_divider = true;
    view.route_entry = true;
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let resize = |height| ViewEvent::Resize(crate::view::TerminalSize { width: 40, height });
    view.event(resize(8), &context).unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 6))));
    view.start_prepared_work();
    assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 6))));
    view.event(
        ViewEvent::Input(InputEvent::Key {
            key: Key::Tab,
            raw: Vec::new(),
        }),
        &context,
    )
    .unwrap();
    assert!(view.completion.is_some());
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 0))));
    view.start_prepared_work();
    assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 0))));
    view.event(resize(12), &context).unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 0))));
    view.event(
        ViewEvent::Input(InputEvent::Key {
            key: Key::Escape,
            raw: Vec::new(),
        }),
        &context,
    )
    .unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 10))));
    view.action(&context, "picker.toggle_preview").unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("action", (40, 10))));
    view.event(resize(1), &context).unwrap();
    view.start_prepared_work();
    assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 0))));
    for height in 0..12 {
        let body = view.body_layout(Rect::new(0, 0, 40, height))[3];
        assert_eq!(body.height, height.saturating_sub(2));
    }
}

#[test]
fn explicit_enter_action_precedes_route_submission() {
    let tasks = TaskRuntime::new();
    let binding = invalid_integer_binding();
    let mut view =
        view_with_stale_aware_runtime(Box::new(ExitOnActionRuntime), binding.clone(), &tasks);
    view.route_entry = true;
    view.editor = EditorBuffer::from_raw("other 1", 7);
    view.route_resolutions.insert(
        "other".to_string(),
        crate::view::RouteTarget {
            reference: "other".to_string(),
            label: None,
        },
    );
    view.route_schemas.insert(
        "other".to_string(),
        crate::view::QuerySchema {
            id: "query".to_string(),
        },
    );
    view.parameter_bindings.insert("other".to_string(), binding);
    view.keymap = PickerKeymap::from_values(
        Some(serde_json::json!({
            "exit": ["enter"]
        })),
        None,
    )
    .unwrap();

    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let decision = view
        .event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Enter,
                raw: b"\r".to_vec(),
            }),
            &context,
        )
        .unwrap();
    assert!(matches!(decision, ViewDecision::Exit));
    tasks.shutdown_and_wait();
}

#[test]
fn invalid_input_keeps_the_active_task_registered_until_its_stale_completion_is_consumed() {
    let tasks = TaskRuntime::new();
    let input_rejected = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let runtime = StaleAwareTaskRuntime {
        task: None,
        input_rejected: Arc::clone(&input_rejected),
        polls: Arc::clone(&polls),
    };
    let mut view =
        view_with_stale_aware_runtime(Box::new(runtime), invalid_integer_binding(), &tasks);
    let context = ViewContext::new(ViewInstanceId(1), "core:default");

    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    view.event(
        ViewEvent::Input(InputEvent::Key {
            key: Key::Char('x'),
            raw: Vec::new(),
        }),
        &context,
    )
    .unwrap();
    assert!(input_rejected.load(Ordering::Acquire));

    let event = wait_for_task_event(&tasks);
    assert_eq!((event.task, event.generation), (TaskId(1), 1));
    view.event(ViewEvent::Task(event.clone()), &context)
        .unwrap();
    assert_eq!(polls.load(Ordering::Acquire), 1);
    assert!(view.publication().is_none());

    // Consumption invalidates the registry, so a duplicate completion
    // cannot be polled or published.
    view.event(ViewEvent::Task(event), &context).unwrap();
    assert_eq!(polls.load(Ordering::Acquire), 1);

    drop(view);
    tasks.shutdown_and_wait();
}

#[test]
fn static_display_options_reach_picker_rendering_and_input() {
    let root = std::env::temp_dir().join(format!(
        "tlaunch-picker-display-options-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(
        root.join("suite.toml"),
        "[suite]\napi = 1\nname = \"Display test\"\nentrypoint = \"core:default\"\n[workflows]\ncore = { file = \"workflows/core.toml\" }\n",
    )
    .unwrap();
    for (fields, show_input, show_divider) in [
        ("", true, true),
        ("show_input = true\nshow_divider = true", true, true),
        ("show_input = true\nshow_divider = false", true, false),
        ("show_input = false\nshow_divider = true", false, true),
        ("show_input = false\nshow_divider = false", false, false),
    ] {
        std::fs::write(
                root.join("workflows/core.toml"),
                format!(
                    "[workflow]\napi = 1\nname = \"Display options test\"\nentrypoint = \"default\"\n[views.default.engine]\ntype = \"picker\"\n[views.default.engine.config]\nitems = []\n{fields}\n"
                ),
            )
            .unwrap();
        let fixture = Arc::new(
            crate::workflow::config::CompiledConfig::load_suite_unvalidated(
                &root.join("suite.toml"),
                None,
            )
                .unwrap()
                .compile()
                .unwrap(),
        );
        let engines = crate::engine::EngineRegistry::new();
        fixture.validate_with_engines(&engines).unwrap();
        let tasks = TaskRuntime::new();
        let services = crate::engine::picker::PickerRuntimeServices::new(
            Arc::clone(&fixture),
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
            "core:default",
        )
        .view_services();
        let mut picker_config = config_with_tasks(services, tasks.clone());
        picker_config.parameter_binding = fixture.parameter_binding("core:default").unwrap();
        picker_config.engine = crate::engine::project_engine_config(
            &fixture,
            "core:default",
            &engines.definition(&fixture, "core:default").unwrap(),
            Value::Null,
        )
        .unwrap();
        let mut view = create_protocol_view(
            picker_config,
            &request("core:default").with_input("seed", 4).unwrap(),
            ViewInstanceId(1),
            &MapRouteCatalog::default(),
        )
        .unwrap();
        let context = ViewContext::new(ViewInstanceId(1), "core:default");
        let decision = view
            .event(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Char('x'),
                    raw: Vec::new(),
                }),
                &context,
            )
            .unwrap();
        if !show_input {
            assert!(matches!(decision, ViewDecision::Stay));
        }
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| {
                let rendered = view
                    .render(
                        frame,
                        frame.area(),
                        &RenderContext::for_terminal(crate::view::TerminalSize {
                            width: 20,
                            height: 5,
                        }),
                    )
                    .unwrap();
                assert!(rendered.cursor.is_none(), "{fields}");
            })
            .unwrap();
        let rows = terminal
            .backend()
            .buffer()
            .content
            .chunks(20)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>();
        assert_eq!(rows[0].starts_with("seedx"), show_input, "{fields}");
        assert_eq!(
            rows.iter().any(|row| row.chars().all(|ch| ch == '─')),
            show_input && show_divider,
            "{fields}"
        );
        if !show_input {
            assert!(matches!(
                view.event(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Backspace,
                        raw: Vec::new(),
                    }),
                    &context,
                )
                .unwrap(),
                ViewDecision::Stay
            ));
            let mut context = context.clone();
            context.has_parent = true;
            assert!(matches!(
                view.event(
                    ViewEvent::Input(InputEvent::Key {
                        key: Key::Backspace,
                        raw: Vec::new(),
                    }),
                    &context,
                )
                .unwrap(),
                ViewDecision::Close
            ));
        }
        drop(view);
        tasks.shutdown_and_wait();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_query_backspace_returns_to_root_only_from_a_child() {
    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let tasks = TaskRuntime::new();
    let services = crate::engine::picker::PickerRuntimeServices::new(
        fixture,
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
        "core:default",
    )
    .view_services();
    let mut picker_config = config_with_tasks(services, tasks.clone());
    picker_config.route_entry = false;
    picker_config.query_prefix = Some("app".to_string());
    let mut view = create_protocol_view(
        picker_config,
        &request("core:default"),
        ViewInstanceId(1),
        &MapRouteCatalog::default(),
    )
    .unwrap();
    let mut context = ViewContext::new(ViewInstanceId(1), "core:default");
    for (has_parent, expected) in [
        (true, ViewDecision::CloseToRoot),
        (false, ViewDecision::Invalidate),
    ] {
        context.has_parent = has_parent;
        assert_eq!(
            view.event(
                ViewEvent::Input(InputEvent::Key {
                    key: Key::Backspace,
                    raw: Vec::new(),
                }),
                &context,
            )
            .unwrap(),
            expected,
        );
    }
    drop(view);
    tasks.shutdown_and_wait();
}

#[test]
fn pseudo_cursor_styles_existing_and_trailing_cells() {
    let style = Style::default().fg(Color::Magenta).bg(Color::Green);
    let mut terminal = Terminal::new(TestBackend::new(8, 1)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new("ab界"), frame.area());
            render_pseudo_cursor(frame, frame.area(), 2, style);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), "界");
    assert_eq!(
        buffer.cell((2, 0)).unwrap().style().fg,
        Some(Color::Magenta)
    );
    assert_eq!(buffer.cell((2, 0)).unwrap().style().bg, Some(Color::Green));

    let mut terminal = Terminal::new(TestBackend::new(4, 1)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new("ab界"), frame.area());
            render_pseudo_cursor(frame, frame.area(), 3, style);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((2, 0)).unwrap().symbol(), "界");
    assert_eq!(buffer.cell((2, 0)).unwrap().style().bg, Some(Color::Green));
    assert_ne!(buffer.cell((3, 0)).unwrap().symbol(), PSEUDO_CURSOR_SYMBOL);

    let mut terminal = Terminal::new(TestBackend::new(8, 1)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new("abc"), frame.area());
            render_pseudo_cursor(frame, frame.area(), 3, style);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let cursor_cell = buffer.cell((3, 0)).unwrap();
    assert_eq!(cursor_cell.symbol(), PSEUDO_CURSOR_SYMBOL);
    assert_eq!(cursor_cell.style().fg, Some(Color::Magenta));
    assert_eq!(cursor_cell.style().bg, Some(Color::Green));
}

#[test]
fn editor_cursor_uses_route_prefix_and_unicode_display_width() {
    let query = visible_editor_query(Some("app"), None, "a界bc", "a界".len(), 8);
    assert_eq!(query.text, "app a界b");
    assert_eq!(query.cursor, 7);
    assert_eq!(query.highlight, Some(0..3));

    let query = visible_editor_query(Some("app"), None, "界", "界".len(), 6);
    assert_eq!(query.text, "app 界");
    assert_eq!(query.cursor.min(5), 5);
    assert_eq!(query.highlight, Some(0..3));

    let query = visible_editor_query(None, None, "abc", 3, 3);
    assert_eq!(query.text, "abc");
    assert_eq!(query.cursor.min(2), 2);
    assert_eq!(query.highlight, None);

    let combining = "a\u{301}bc";
    let query = visible_editor_query(None, None, combining, combining.len(), 2);
    assert_eq!(query.text, "c");
    assert_eq!(query.cursor, 1);
    let emoji = "x👩‍💻yz";
    let query = visible_editor_query(None, None, emoji, "x👩‍💻".len(), 4);
    assert_eq!(query.text, "x👩‍💻y");
    assert_eq!(query.cursor, 3);
    let query = visible_editor_query(None, None, emoji, emoji.len(), 4);
    assert_eq!(query.text, "...");
    assert_eq!(query.cursor, 3);

    let query = visible_editor_query(Some("abcdef"), None, "x", 0, 3);
    assert_eq!(query.text, "abc");
    assert_eq!(query.cursor, 2);
    assert_eq!(query.highlight, None);

    let query = visible_editor_query(None, Some(3), "app needle", 10, 20);
    assert_eq!(query.highlight, Some(0..3));
}

#[test]
fn completion_disabled_patch_is_respected() {
    let disabled = explicitly_disabled_keys(
        None,
        Some(&serde_json::json!({
            "tab": false,
            "escape": false,
        })),
    );
    assert!(disabled.contains(&Key::Tab.binding_identity()));
    assert!(disabled.contains(&Key::Escape.binding_identity()));
    let empty_override = explicitly_disabled_keys(Some(&serde_json::json!({"back": []})), None);
    assert!(empty_override.contains(&Key::Escape.binding_identity()));
}

#[test]
fn route_query_contains_only_target_binding_values() {
    let config = crate::workflow::config::load_test_fixture().unwrap();
    let binding = config.parameter_binding("core:default").unwrap();
    let query = parsed_route_query(&binding, "core:default", "query", "needle").unwrap();
    assert_eq!(query.values, Value::String("needle".to_string()));
}

#[test]
fn invalid_route_query_is_rejected_before_navigation() {
    let registry = crate::workflow::parameter::ParameterRegistry::compile(&serde_json::json!({
        "workflows": {"target": {"views": {"detail": {"query": {
            "type": "object",
            "count": {"type": "integer"}
        }}}}}
    }))
    .unwrap();
    let binding = std::sync::Arc::new(registry)
        .parameter_binding("target:detail")
        .unwrap();
    assert!(parsed_route_query(&binding, "target:detail", "query", "bad").is_err());
}

#[test]
fn picker_preview_declared_image_renders_after_decode_and_encoding() {
    let temp_dir = std::env::temp_dir().join(format!("test-picker-img-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let image_path = temp_dir.join("test.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([255, 0, 0])))
        .save(&image_path)
        .unwrap();

    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let tasks = TaskRuntime::new();
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1)));
    let services =
        crate::engine::picker::PickerRuntimeServices::new(fixture, starter, "core:default")
            .view_services();
    let mut picker_config = config_with_tasks(services, tasks.clone());
    picker_config.engine.fields.insert(
        "preview".to_string(),
        serde_json::json!({
            "producer": "declared",
            "document": {"type": "image", "path": image_path.to_string_lossy()}
        }),
    );
    let mut view = create_protocol_view(
        picker_config,
        &request("core:default"),
        ViewInstanceId(1),
        &MapRouteCatalog::default(),
    )
    .unwrap();
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let size = crate::view::TerminalSize {
        width: 80,
        height: 24,
    };
    view.event(ViewEvent::Resize(size), &context).unwrap();
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    view.event(
        ViewEvent::Input(InputEvent::Key {
            key: Key::Ctrl('p'),
            raw: vec![0x10],
        }),
        &context,
    )
    .unwrap();
    let render_context =
        RenderContext::new(size, Some(crate::terminal::ImagePicker::test_halfblocks()));
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        view.event(ViewEvent::Tick, &context).unwrap();
        for event in tasks.drain_events() {
            view.event(ViewEvent::Task(event), &context).unwrap();
        }
        terminal
            .draw(|frame| {
                view.render(frame, frame.area(), &render_context).unwrap();
            })
            .unwrap();
        // The known red pixels must reach the preview pane through both async pools.
        if (40..size.width).any(|x| {
            terminal.backend().buffer()[(x, 2)].bg == ratatui::style::Color::Rgb(255, 0, 0)
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "declared image was never rendered: {:?}",
            terminal.backend().buffer()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
        .unwrap();
    drop(view);
    tasks.shutdown_and_wait();
    std::fs::remove_dir_all(temp_dir).unwrap();
}

mod preview_correlation_tests {
    use super::*;
    #[test]
    fn preview_events_have_their_own_registry_entry_and_render_after_items_complete() {
        let temp =
            std::env::temp_dir().join(format!("tlaunch-proto-preview-{}", std::process::id()));
        let config = crate::engine::picker::create_preview_test_suite(&temp);
        let engines = crate::engine::EngineRegistry::new();
        let tasks = TaskRuntime::new();
        let instance = ViewInstanceId(901);
        let page = "browser:main";
        let services = crate::engine::picker::mount_data(
            &config,
            &Value::Null,
            page,
            MountTaskLease::new(ViewMountId(instance.0)),
        )
        .unwrap();
        let definition = engines.definition(&config, page).unwrap();
        let protocol_config = PickerProtocolConfig {
            identity: ViewIdentity::new(page, "picker"),
            engine: crate::engine::project_engine_config(&config, page, &definition, Value::Null)
                .unwrap(),
            bindings: crate::engine::project_binding_config(&config, page, &definition).unwrap(),
            services,
            parameter_bindings: BTreeMap::new(),
            parameter_binding: config.parameter_binding(page).unwrap(),
            theme: ResolvedTheme::terminal(),
            route_entry: false,
            query_prefix: None,
            runtime_snapshot: serde_json::json!({"view":{}}),
            tasks: tasks.clone(),
        };
        let request = NavigationRequest::new(
            page,
            ParsedQuery::new(
                page,
                "query",
                serde_json::json!({"search":"","owner":"browser"}),
            ),
        );
        let mut view = create_protocol_view(
            protocol_config,
            &request,
            instance,
            &crate::view::MapRouteCatalog::default(),
        )
        .unwrap();
        let context = ViewContext::new(instance, page);
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 80,
                height: 24,
            }),
            &context,
        )
        .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
            .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Ctrl('p'),
                raw: vec![0x10],
            }),
            &context,
        )
        .unwrap();
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            view.event(ViewEvent::Tick, &context).unwrap();
            for event in tasks.drain_events() {
                events.push(event.clone());
                view.event(ViewEvent::Task(event), &context).unwrap();
            }
            if events.iter().any(|event| event.task == TaskId(2)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(events.iter().any(|event| event.task == TaskId(1)));
        let preview_event = events
            .iter()
            .find(|event| event.task == TaskId(2))
            .expect("preview event missing");
        assert_eq!(preview_event.instance, instance);
        // Duplicate and stale preview events cannot consume an item completion.
        assert!(matches!(
            view.event(ViewEvent::Task(preview_event.clone()), &context)
                .unwrap(),
            ViewDecision::Stay
        ));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                view.render(
                    frame,
                    frame.area(),
                    &RenderContext::for_terminal(crate::view::TerminalSize {
                        width: 80,
                        height: 24,
                    }),
                )
                .unwrap();
            })
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            content.contains("Mixed preview")
                && content.contains("Details")
                && content.contains("library"),
            "{content}"
        );
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
            .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context)
            .unwrap();
        drop(view);
        tasks.shutdown_and_wait();
        std::fs::remove_dir_all(temp).unwrap();
    }
}
