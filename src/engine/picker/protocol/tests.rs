use super::*;
use crate::engine::ProjectedBindingConfig;
use crate::ui::chrome::input::format_visible_omnibar as visible_editor_query;
use crate::view::ViewContext;
use anyhow::bail;
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

fn apply_edit(view: &mut dyn View, decision: ViewDecision, context: &ViewContext) -> ViewDecision {
    if let ViewDecision::EditInput(edit) = decision {
        let mut input = crate::ui::chrome::HostInputState::for_view(view);
        if input.edit(edit) {
            view.on_host_input_changed(&input.editor.snapshot(), context)
                .unwrap()
        } else {
            ViewDecision::Stay
        }
    } else {
        decision
    }
}

fn key(view: &mut dyn View, key: Key, context: &ViewContext) -> ViewDecision {
    let decision = crate::view::dispatch_test_key(view, key, &[], context).unwrap();
    apply_edit(view, decision, context)
}

fn render_hosted(
    view: &dyn View,
    frame: &mut Frame,
    area: Rect,
    context: &RenderContext,
) -> RenderResult {
    let input = crate::ui::chrome::HostInputState::for_view(view);
    let layout = crate::ui::chrome::PaneLayout::new(
        area,
        input.mode.is_visible(),
        view.input_divider(),
        None,
    );
    if let Some(omnibar) = layout.omnibar {
        crate::ui::chrome::render_omnibar_widget(
            frame,
            omnibar,
            &input,
            &ResolvedTheme::terminal(),
        );
    }
    if let Some(divider) = layout.divider {
        frame.render_widget(
            ratatui::widgets::Paragraph::new("─".repeat(divider.width as usize)),
            divider,
        );
    }
    view.render(frame, layout.primary, context).unwrap()
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
    assert_eq!(hinted.text, "Search");
    assert_eq!(hinted.highlight, None);
    assert_eq!(hinted.placeholder, Some(0.."Search".len()));
    assert_eq!(hinted.cursor, 0);

    let typed = visible_editor_query(None, Some("Search"), "ab", 2, 20);
    assert_eq!(typed.text, "ab");
    assert_eq!(typed.placeholder, None);
}

#[test]
fn input_placeholder_follows_a_left_prefix() {
    let query = visible_editor_query(Some("sys"), Some("Search"), "", 0, 20);
    assert_eq!(query.text, "sys Search");
    assert_eq!(query.highlight, Some(0.."sys".len()));
    assert_eq!(query.placeholder, Some("sys ".len().."sys Search".len()));
    assert_eq!(query.cursor, 4);
}

#[test]
fn input_placeholder_is_clipped_at_narrow_widths() {
    let clipped = visible_editor_query(None, Some("Search the catalog"), "", 0, 8);
    assert_eq!(clipped.text, "Searc...");
    assert_eq!(clipped.placeholder, Some(0..clipped.text.len()));

    // A prefix that leaves no room must render the prefix alone, never a hint.
    let no_room = visible_editor_query(Some("sys"), Some("Search"), "", 0, 3);
    assert_eq!(no_room.text, "sys");
    assert_eq!(no_room.placeholder, None);

    // When only enough room for the prefix remains, no hint text is shown.
    let prefix_only = visible_editor_query(Some("sys"), Some("Search"), "", 0, 4);
    assert_eq!(prefix_only.text, "sys ");
    assert_eq!(prefix_only.placeholder, None);
    assert_eq!(prefix_only.cursor, 3);

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
    config.engine = ProjectedEngineConfig::for_picker(crate::engine::PickerConfig {
        input_placeholder: Some("Search".into()),
        ..Default::default()
    });
    let mut view =
        create_protocol_view(config, &request("core:default"), ViewInstanceId(1)).unwrap();

    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    let size = crate::view::TerminalSize {
        width: 40,
        height: 4,
    };
    let render_context = RenderContext::for_terminal(size);
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();
    let render = |view: &dyn View, terminal: &mut Terminal<TestBackend>| {
        terminal
            .draw(|frame| {
                render_hosted(view, frame, frame.area(), &render_context);
            })
            .unwrap();
    };

    render(view.as_ref(), &mut terminal);
    let buffer = terminal.backend().buffer();
    let row: String = (0..size.width)
        .map(|x| buffer.cell((x, 0)).unwrap().symbol())
        .collect();
    assert_eq!(row.trim_end(), "Search");
    let placeholder = ResolvedTheme::terminal().picker.placeholder;
    assert_eq!(buffer.cell((0, 0)).unwrap().symbol(), "S");
    assert_eq!(buffer.cell((0, 0)).unwrap().style().fg, placeholder.fg);
    assert_eq!(buffer.cell((0, 0)).unwrap().style().bg, placeholder.bg);

    // Typing proves the hint was presentation only: the fresh query starts at
    // the cursor with no leftover placeholder text behind it.
    key(view.as_mut(), Key::Char('a'), &context);
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
fn picker_retained_content_area_keeps_only_the_item_body() {
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
        Some(area),
        "Picker renders content only; Host owns the input rows"
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

struct CloseOnBackRuntime;

impl EngineRuntime for CloseOnBackRuntime {
    fn action(&mut self, input: EngineActionInput) -> Result<EngineEmission> {
        if input.invocation.id.as_str() == "picker.back" {
            Ok(EngineEmission::decision(EngineDecision::Close))
        } else {
            Ok(EngineEmission::decision(EngineDecision::Continue))
        }
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
        bindings: PickerBindings::from_defaults(None).unwrap(),
        left_prefix: None,
        theme: ResolvedTheme::terminal(),
        parameter_binding,
        editor: editor.snapshot(),
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
    view.editor = EditorBuffer::from_raw("", 0).snapshot();
    let mut context = ViewContext::new(ViewInstanceId(1), "core:default");
    context.has_parent = true;
    let decision = key(&mut view, Key::Backspace, &context);
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
fn root_picker_clears_input_on_back_before_closing() {
    let tasks = TaskRuntime::new();
    let mut view = view_with_stale_aware_runtime(
        Box::new(CloseOnBackRuntime),
        invalid_integer_binding(),
        &tasks,
    );
    view.editor = EditorBuffer::from_raw("query", 5).snapshot();

    let mut root_context = ViewContext::new(ViewInstanceId(1), "core:default");
    root_context.has_parent = false;

    // 1. First back on root view with non-empty input: should clear input and not close
    let decision = view.on_command(CMD_BACK, &root_context).unwrap();
    assert!(matches!(
        decision,
        ViewDecision::EditInput(InputEdit::Clear)
    ));
    apply_edit(&mut view, decision, &root_context);
    assert!(view.editor.raw.is_empty());

    // 2. Second back on root view with empty input: should close
    let decision = view.on_command(CMD_BACK, &root_context).unwrap();
    assert!(matches!(decision, ViewDecision::Close));

    // 3. Child view with non-empty input: should immediately close without clearing
    let mut child_context = ViewContext::new(ViewInstanceId(1), "core:default");
    child_context.has_parent = true;
    view.editor = EditorBuffer::from_raw("child query", 11).snapshot();
    let decision = view.on_command(CMD_BACK, &child_context).unwrap();
    assert!(matches!(decision, ViewDecision::Close));
    assert_eq!(view.editor.raw, "child query");

    tasks.shutdown_and_wait();
}

#[test]
fn tab_no_longer_opens_builtin_completion() {
    let tasks = TaskRuntime::new();
    let binding = invalid_integer_binding();
    let mut view = view_with_stale_aware_runtime(Box::new(ExitOnActionRuntime), binding, &tasks);
    view.editor = EditorBuffer::from_raw("oth", 3).snapshot();

    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let decision = key(&mut view, Key::Tab, &context);
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
    key(&mut view, Key::Char('x'), &context);
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
                    "[workflow]\napi = 1\nname = \"Display options test\"\nentrypoint = \"default\"\n[views.default]\nengine = \"picker\"\n[views.default.picker]\nitems = []\n{fields}\n"
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
        let decision = key(view.as_mut(), Key::Char('x'), &context);
        if !show_input {
            assert!(matches!(decision, ViewDecision::Stay));
        }
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| {
                let rendered = render_hosted(
                    view.as_ref(),
                    frame,
                    frame.area(),
                    &RenderContext::for_terminal(crate::view::TerminalSize {
                        width: 20,
                        height: 5,
                    }),
                );
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
                key(view.as_mut(), Key::Backspace, &context),
                ViewDecision::Stay
            ));
            let mut context = context.clone();
            context.has_parent = true;
            assert!(matches!(
                key(view.as_mut(), Key::Backspace, &context),
                ViewDecision::Close
            ));
        }
        drop(view);
        tasks.shutdown_and_wait();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn picker_leaves_cursor_rendering_to_host_across_lifecycle() {
    let tasks = TaskRuntime::new();
    let fixture = Arc::new(crate::workflow::config::load_test_fixture().unwrap());
    let services = crate::engine::picker::PickerRuntimeServices::new(
        fixture,
        MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1))),
        "core:default",
    )
    .view_services();
    let config = config_with_tasks(services, tasks.clone());
    let mut view = create_protocol_view(
        config,
        &request("core:default").with_input("test", 4).unwrap(),
        ViewInstanceId(1),
    )
    .unwrap();
    let context = ViewContext::new(ViewInstanceId(1), "core:default");
    let size = crate::view::TerminalSize {
        width: 40,
        height: 4,
    };
    let render_context = RenderContext::for_terminal(size);
    let mut terminal = Terminal::new(TestBackend::new(size.width, size.height)).unwrap();

    // 1. Activated: picker is active and focused
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    let mut rendered = None;
    terminal
        .draw(|frame| {
            rendered = Some(view.render(frame, frame.area(), &render_context).unwrap());
        })
        .unwrap();
    let active_render = rendered.take().unwrap();
    assert!(active_render.cursor.is_none());

    // 2. Covered: a popup opens, covering the picker
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context)
        .unwrap();
    terminal
        .draw(|frame| {
            rendered = Some(view.render(frame, frame.area(), &render_context).unwrap());
        })
        .unwrap();
    let covered_render = rendered.take().unwrap();
    assert!(covered_render.cursor.is_none());

    // 3. Activated: popup closed, picker restored to active
    view.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context)
        .unwrap();
    terminal
        .draw(|frame| {
            rendered = Some(view.render(frame, frame.area(), &render_context).unwrap());
        })
        .unwrap();
    let restored_render = rendered.take().unwrap();
    assert!(restored_render.cursor.is_none());

    tasks.shutdown_and_wait();
}
