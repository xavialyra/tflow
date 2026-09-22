use super::*;
use crate::engine::ProjectedBindingConfig;
use crate::view::ViewContext;
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
        parameter_binding,
        theme: ResolvedTheme::terminal(),
        left_prefix: None,
        prefix_backspace: None,
        runtime_snapshot: serde_json::json!({"view": {}}),
        tasks,
    }
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
fn left_prefix_is_rendered_only_when_present() {
    let hidden = visible_editor_query(None, None, "", 0, 20);
    assert_eq!(hidden.text, "");
    assert_eq!(hidden.cursor, 0);
    assert_eq!(hidden.highlight, None);
    assert_eq!(hidden.placeholder, None);

    let shown = visible_editor_query(Some("\u{3008}"), None, "", 0, 20);
    assert_eq!(shown.text, "\u{3008} ");
    // The full-width glyph is two columns plus its separating space.
    assert_eq!(shown.cursor, 3);
    assert_eq!(shown.highlight, Some(0.."\u{3008}".len()));
}

#[test]
fn input_placeholder_fills_an_empty_query_only() {
    let hinted = visible_editor_query(None, Some("Search"), "", 0, 20);
    // The leading cell is the cursor; the hint starts after it.
    assert_eq!(hinted.text, " Search");
    assert_eq!(hinted.highlight, None);
    assert_eq!(hinted.placeholder, Some(1.." Search".len()));
    assert_eq!(hinted.cursor, 0);

    let typed = visible_editor_query(None, Some("Search"), "ab", 2, 20);
    assert_eq!(typed.text, "ab");
    assert_eq!(typed.placeholder, None);
}

#[test]
fn input_placeholder_follows_a_left_prefix() {
    let query = visible_editor_query(Some("sys"), Some("Search"), "", 0, 20);
    assert_eq!(query.text, "sys  Search");
    assert_eq!(query.highlight, Some(0.."sys".len()));
    assert_eq!(query.placeholder, Some("sys  ".len().."sys  Search".len()));
    assert_eq!(query.cursor, 4);
}

#[test]
fn input_placeholder_is_clipped_at_narrow_widths() {
    // One column is reserved for the cursor before the hint is clipped.
    let clipped = visible_editor_query(None, Some("Search the catalog"), "", 0, 8);
    assert_eq!(clipped.text, " Sear...");
    assert_eq!(clipped.placeholder, Some(1..clipped.text.len()));

    // A prefix that leaves no room must render the prefix alone, never a hint.
    let no_room = visible_editor_query(Some("sys"), Some("Search"), "", 0, 3);
    assert_eq!(no_room.text, "sys");
    assert_eq!(no_room.placeholder, None);

    // A single free column shows only the cursor, with no hint text.
    let cursor_only = visible_editor_query(Some("sys"), Some("Search"), "", 0, 5);
    assert_eq!(cursor_only.text, "sys  ");
    assert_eq!(cursor_only.placeholder, None);
    assert_eq!(cursor_only.cursor, 4);

    // An empty hint behaves as if it were absent.
    let empty = visible_editor_query(None, Some(""), "", 0, 20);
    assert_eq!(empty.text, "");
    assert_eq!(empty.placeholder, None);
}

#[test]
fn left_prefix_precedes_typed_input() {
    let query = visible_editor_query(Some("sys"), None, "ab", 2, 20);
    assert_eq!(query.text, "sys ab");
    // The marker shares the accent style; typed text does not.
    assert_eq!(query.highlight, Some(0.."sys".len()));
    assert_eq!(query.cursor, 3 + 1 + 2);
}

#[test]
fn left_prefix_is_clipped_but_preserved_at_tiny_widths() {
    let query = visible_editor_query(Some("\u{3008}"), None, "abcdef", 6, 3);
    assert_eq!(query.text, "\u{3008} ");
    assert_eq!(query.highlight, Some(0.."\u{3008}".len()));

    let single = visible_editor_query(Some("\u{3008}"), None, "abcdef", 6, 1);
    assert_eq!(single.text, "\u{3008}");
    assert_eq!(single.highlight, Some(0.."\u{3008}".len()));
}

#[test]
fn input_placeholder_renders_in_the_query_row_without_touching_the_buffer() {
    let tasks = TaskRuntime::new();
    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let services = crate::engine::picker::PickerRuntimeServices::new(
        fixture,
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
        "core:default",
    )
    .view_services();
    let mut config = config_with_tasks(services, tasks.clone());
    config
        .engine
        .fields
        .insert("input_placeholder".into(), Value::String("Search".into()));
    let mut view =
        create_protocol_view(config, &request("core:default"), ViewInstanceId(1)).unwrap();

    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let size = crate::view::TerminalSize {
        width: 40,
        height: 4,
    };
    let render_context = RenderContext::for_terminal(size);
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
    let render = |view: &dyn View, terminal: &mut Terminal<TestBackend>| {
        terminal
            .draw(|frame| {
                view.render(frame, frame.area(), &render_context).unwrap();
            })
            .unwrap();
    };

    render(view.as_ref(), &mut terminal);
    let buffer = terminal.backend().buffer();
    let row: String = (0..size.width)
        .map(|x| buffer.cell((x, 0)).unwrap().symbol())
        .collect();
    assert_eq!(row.trim_end(), "\u{2588}Search");
    // The cursor keeps its own cell; the hint starts right after it and keeps
    // the dedicated placeholder style.
    let cursor = ResolvedTheme::terminal().picker.cursor;
    let cursor_cell = buffer.cell((0, 0)).unwrap();
    assert_eq!(cursor_cell.symbol(), PSEUDO_CURSOR_SYMBOL);
    assert_eq!(cursor_cell.style().fg, cursor.fg);
    let placeholder = ResolvedTheme::terminal().picker.placeholder;
    assert_eq!(buffer.cell((1, 0)).unwrap().symbol(), "S");
    assert_eq!(buffer.cell((1, 0)).unwrap().style().fg, placeholder.fg);
    assert_eq!(buffer.cell((1, 0)).unwrap().style().bg, placeholder.bg);

    // Typing proves the hint was presentation only: the fresh query starts at
    // the cursor with no leftover placeholder text behind it.
    view.event(
        ViewEvent::Input(InputEvent::Key {
            key: Key::Char('a'),
            raw: vec![b'a'],
        }),
        &context,
    )
    .unwrap();
    render(view.as_ref(), &mut terminal);
    let buffer = terminal.backend().buffer();
    let row: String = (0..size.width)
        .map(|x| buffer.cell((x, 0)).unwrap().symbol())
        .collect();
    assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "a");
    assert!(
        !row.contains("Search"),
        "placeholder survived typing: {row:?}"
    );

    tasks.shutdown_and_wait();
}

#[test]
fn picker_retained_content_area_keeps_only_the_item_and_preview_body() {
    let tasks = TaskRuntime::new();
    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let services = crate::engine::picker::PickerRuntimeServices::new(
        fixture,
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
        "core:default",
    )
    .view_services();
    let view = create_protocol_view(
        config_with_tasks(services, tasks.clone()),
        &request("core:default"),
        ViewInstanceId(1),
    )
    .unwrap();

    // Default options: one input row and one divider row.
    let area = Rect::new(0, 0, 40, 12);
    assert_eq!(
        view.retained_content_area(area),
        Some(Rect::new(0, 2, 40, 10)),
        "the input row and divider must stay on the new instance"
    );
    tasks.shutdown_and_wait();
}

#[test]
fn picker_task_registry_rejects_stale_events_and_consumes_the_current_completion() {
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

#[test]
fn publication_ready_changes_advance_dynamic_command_revision() {
    let tasks = TaskRuntime::new();
    let mut view = view_with_stale_aware_runtime(
        Box::new(ExitOnActionRuntime),
        invalid_integer_binding(),
        &tasks,
    );
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let current = serde_json::json!({
        "item": {"value": "row", "bindings": {"enter": "core:open"}}
    });

    view.map_emission(
        &context,
        EngineEmission::decision(EngineDecision::Continue).with_publication(
            crate::engine::ViewContextPublication::new(current.clone()).with_ready(false),
        ),
    )
    .unwrap();
    let loading_revision = view.state_revision;

    view.map_emission(
        &context,
        EngineEmission::decision(EngineDecision::Continue).with_publication(
            crate::engine::ViewContextPublication::new(current.clone()).with_ready(true),
        ),
    )
    .unwrap();
    assert_eq!(view.state_revision, loading_revision + 1);

    view.map_emission(
        &context,
        EngineEmission::decision(EngineDecision::Continue).with_publication(
            crate::engine::ViewContextPublication::new(current).with_ready(false),
        ),
    )
    .unwrap();
    assert_eq!(view.state_revision, loading_revision + 2);
    tasks.shutdown_and_wait();
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
        disabled_keys: HashSet::new(),
        left_prefix: None,
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
        defer_work_poll: false,
        task_completion_pending: false,
        publication_ready: false,
        diagnostic: None,
        content_size: (1, 1),
        has_parent: false,
    }
}

fn backspace_decision(
    left_prefix: Option<&str>,
    show_left_prefix: bool,
    behavior: Option<PrefixBackspace>,
) -> ViewDecision {
    let tasks = TaskRuntime::new();
    let mut view = view_with_stale_aware_runtime(
        Box::new(ExitOnActionRuntime),
        invalid_integer_binding(),
        &tasks,
    );
    view.left_prefix = left_prefix.map(str::to_string);
    view.options.show_left_prefix = show_left_prefix;
    view.options.prefix_backspace = behavior;
    view.editor.clear();
    let mut context = ViewContext::new(ViewInstanceId(1), "core:default");
    context.has_parent = true;
    let decision = view
        .event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Backspace,
                raw: Vec::new(),
            }),
            &context,
        )
        .unwrap();
    drop(view);
    tasks.shutdown_and_wait();
    decision
}

#[test]
fn backspace_on_a_prefixed_empty_input_obeys_the_configured_behavior() {
    assert!(matches!(
        backspace_decision(Some("sys"), true, None),
        ViewDecision::Invalidate
    ));
    assert!(matches!(
        backspace_decision(Some("sys"), true, Some(PrefixBackspace::Parent)),
        ViewDecision::Close
    ));
    assert!(matches!(
        backspace_decision(Some("sys"), true, Some(PrefixBackspace::Root)),
        ViewDecision::CloseToRoot
    ));
}

#[test]
fn backspace_ignores_the_setting_without_a_rendered_prefix() {
    // No configured marker.
    assert!(matches!(
        backspace_decision(None, true, Some(PrefixBackspace::Root)),
        ViewDecision::Invalidate
    ));
    // The View opted out of the marker, such as a completion popup.
    assert!(matches!(
        backspace_decision(Some("sys"), false, Some(PrefixBackspace::Root)),
        ViewDecision::Invalidate
    ));
}

#[test]
#[allow(clippy::type_complexity)]
fn preview_body_size_tracks_resize_and_committed_starts() {
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
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let resize = |height| ViewEvent::Resize(crate::view::TerminalSize { width: 40, height });
    view.event(resize(8), &context).unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 6))));
    view.start_prepared_work();
    assert_eq!(seen.lock().unwrap().last(), Some(&("auxiliary", (40, 6))));
    view.event(resize(12), &context).unwrap();
    assert_eq!(seen.lock().unwrap().last(), Some(&("size", (40, 10))));
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
        let body = view.body_layout(Rect::new(0, 0, 40, height))[2];
        assert_eq!(body.height, height.saturating_sub(2));
    }
}

#[test]
fn tab_no_longer_opens_builtin_completion() {
    let tasks = TaskRuntime::new();
    let binding = invalid_integer_binding();
    let mut view = view_with_stale_aware_runtime(Box::new(ExitOnActionRuntime), binding, &tasks);
    view.editor = EditorBuffer::from_raw("oth", 3);

    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let decision = view
        .event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Tab,
                raw: b"\t".to_vec(),
            }),
            &context,
        )
        .unwrap();
    assert!(matches!(decision, ViewDecision::Stay));
    assert_eq!(view.editor.raw, "oth");
    assert_eq!(
        view.task_generation, 0,
        "must not spawn tasks on current view"
    );
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
        "tflow-picker-display-options-{}",
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
fn explicitly_disabled_keys_patch_is_respected() {
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
    let mut view =
        create_protocol_view(picker_config, &request("core:default"), ViewInstanceId(1)).unwrap();
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
        let temp = std::env::temp_dir().join(format!("tflow-proto-preview-{}", std::process::id()));
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
            parameter_binding: config.parameter_binding(page).unwrap(),
            theme: ResolvedTheme::terminal(),
            left_prefix: None,
            prefix_backspace: None,
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
        let mut view = create_protocol_view(protocol_config, &request, instance).unwrap();
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
            content.contains("Mixed preview") && content.contains("Details"),
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

    #[test]
    fn a_remount_reuses_the_cached_preview_before_the_script_reruns() {
        let temp = std::env::temp_dir().join(format!(
            "tflow-proto-preview-remount-{}",
            std::process::id()
        ));
        let config = crate::engine::picker::create_preview_test_suite(&temp);
        let engines = crate::engine::EngineRegistry::new();
        let tasks = TaskRuntime::new();
        let page = "browser:main";
        let cache = crate::engine::PreviewDocumentCache::default();
        let request = NavigationRequest::new(
            page,
            ParsedQuery::new(
                page,
                "query",
                serde_json::json!({"search":"","owner":"browser"}),
            ),
        );
        // The remount is a self-navigation that updates a parameter, so the
        // request identity differs while the preview provider stays the same.
        let changed_request = NavigationRequest::new(
            page,
            ParsedQuery::new(
                page,
                "query",
                serde_json::json!({"search":"changed","owner":"browser"}),
            ),
        );

        let build = |instance: ViewInstanceId| {
            let mut services = crate::engine::picker::mount_data(
                &config,
                &Value::Null,
                page,
                MountTaskLease::new(ViewMountId(instance.0)),
            )
            .unwrap();
            services.set_preview_cache(cache.clone());
            let definition = engines.definition(&config, page).unwrap();
            PickerProtocolConfig {
                identity: ViewIdentity::new(page, "picker"),
                engine: crate::engine::project_engine_config(
                    &config,
                    page,
                    &definition,
                    Value::Null,
                )
                .unwrap(),
                bindings: crate::engine::project_binding_config(&config, page, &definition)
                    .unwrap(),
                services,
                parameter_binding: config.parameter_binding(page).unwrap(),
                theme: ResolvedTheme::terminal(),
                left_prefix: None,
                prefix_backspace: None,
                runtime_snapshot: serde_json::json!({"view":{}}),
                tasks: tasks.clone(),
            }
        };

        // First mount: run items and the preview script to completion so the
        // shared cache holds the rendered document.
        let first = ViewInstanceId(911);
        let mut first_view = create_protocol_view(build(first), &request, first).unwrap();
        let first_context = ViewContext::new(first, page);
        open_preview_and_drive(&mut first_view, &tasks, &first_context, true, "Details");
        first_view
            .event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &first_context)
            .unwrap();
        drop(first_view);

        // Make the second preview script hang, so only the cache can paint.
        std::fs::write(
            temp.join("workflows/browser/scripts/preview.py"),
            "#!/usr/bin/env python3\nimport time\ntime.sleep(10)\n",
        )
        .unwrap();

        let second = ViewInstanceId(912);
        let mut second_view =
            create_protocol_view(build(second), &changed_request, second).unwrap();
        let second_context = ViewContext::new(second, page);
        let content =
            open_preview_and_drive(&mut second_view, &tasks, &second_context, false, "Details");
        assert!(
            content.contains("Details"),
            "a parameter-updating remount must render the cached preview: {content}"
        );
        second_view
            .event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &second_context)
            .unwrap();
        drop(second_view);
        tasks.shutdown_and_wait();
        std::fs::remove_dir_all(temp).unwrap();
    }

    fn open_preview_and_drive(
        view: &mut Box<dyn View>,
        tasks: &TaskRuntime,
        context: &ViewContext,
        deliver_preview: bool,
        want: &str,
    ) -> String {
        view.event(
            ViewEvent::Resize(crate::view::TerminalSize {
                width: 80,
                height: 24,
            }),
            context,
        )
        .unwrap();
        view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), context)
            .unwrap();
        view.event(
            ViewEvent::Input(InputEvent::Key {
                key: Key::Ctrl('p'),
                raw: vec![0x10],
            }),
            context,
        )
        .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut content = String::new();
        while std::time::Instant::now() < deadline {
            view.event(ViewEvent::Tick, context).unwrap();
            for event in tasks.drain_events() {
                if event.task == TaskId(2) && !deliver_preview {
                    continue;
                }
                view.event(ViewEvent::Task(event), context).unwrap();
            }
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
            content = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if content.contains(want) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        content
    }
}
