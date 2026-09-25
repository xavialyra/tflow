use super::*;
use crate::view::{ParsedQuery, RenderContext, TerminalSize};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn config(content: Value, tasks: TaskRuntime) -> FormProtocolConfig {
    FormProtocolConfig {
        engine: ProjectedEngineConfig {
            fields: [("content".into(), content)].into_iter().collect(),
            ..Default::default()
        },
        bindings: ProjectedBindingConfig::default(),
        runtime_snapshot: Value::Null,
        raw_input: "immutable query".into(),
        theme: ResolvedTheme::terminal(),
        tasks,
    }
}

fn request() -> NavigationRequest {
    NavigationRequest::new(
        "form",
        ParsedQuery::new("form", "query", json!({"schema": {"anything": true}})),
    )
}

fn declared(fields: Value) -> FormView {
    let mut form = FormView::new(
        config(
            json!({"producer":"declared", "handler":{"fields":fields}}),
            TaskRuntime::new(),
        ),
        &request(),
        ViewInstanceId(1),
    )
    .unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    form
}

fn context() -> ViewContext {
    ViewContext::new(ViewInstanceId(1), "form")
}
fn key(form: &mut FormView, key: Key) -> ViewDecision {
    form.event(
        ViewEvent::Input(InputEvent::Key {
            key,
            raw: Vec::new(),
        }),
        &context(),
    )
    .unwrap()
}
fn paste(form: &mut FormView, text: &str) {
    form.event(
        ViewEvent::Input(InputEvent::Paste {
            text: Some(text.into()),
            raw: text.as_bytes().to_vec(),
        }),
        &context(),
    )
    .unwrap();
}

#[test]
fn drafts_validate_typed_values_without_mutating_query_or_submitting() {
    let mut form = declared(json!([
        {"name":"name", "required":true},
        {"name":"age", "type":"integer", "value":3},
        {"name":"enabled", "type":"boolean"},
        {"name":"options", "type":"json"}
    ]));
    assert_eq!(form.publication.current["valid"], false);
    assert_eq!(form.publication.current["errors"]["name"], "Required");
    assert_eq!(form.publication.current["dirty"], false);
    paste(&mut form, "é界");
    key(&mut form, Key::Left);
    key(&mut form, Key::Backspace);
    assert_eq!(form.publication.current["values"]["name"], "界");
    key(&mut form, Key::Tab);
    paste(&mut form, "x");
    assert_eq!(form.publication.current["drafts"]["age"], "3x");
    assert_eq!(form.publication.current["values"]["age"], Value::Null);
    assert_eq!(
        form.publication.current["errors"]["age"],
        "Enter an integer"
    );
    key(&mut form, Key::Backspace);
    key(&mut form, Key::Tab);
    key(&mut form, Key::Char(' '));
    key(&mut form, Key::Tab);
    paste(&mut form, "{\n\"tags\":[1,true]}");
    let state = &form.publication.current;
    assert_eq!(
        state["values"],
        json!({"name":"界", "age":3,"enabled":true,"options":{"tags":[1,true]}})
    );
    assert_eq!(state["valid"], true);
    assert_eq!(state["dirty"], true);
    assert_eq!(key(&mut form, Key::Enter), ViewDecision::Stay);
    assert_eq!(form.command_snapshot().parameters, request().query.values);
    assert_eq!(form.command_snapshot().raw_input, "immutable query");
    assert_eq!(
        form.command_snapshot().publication.unwrap().current,
        form.publication.current
    );
}

#[test]
fn content_rejects_structural_errors_but_allows_incomplete_required_fields() {
    for fields in [
        json!([{"name":"x"},{"name":"x"}]),
        json!([{"name":" "}]),
        json!([{"name":"x", "type":"secret"}]),
        json!([{"name":"x", "type":"integer", "value":"2"}]),
        json!([{"name":"x", "help":"Removed field help"}]),
        json!([{"name":"x", "unknown":true}]),
    ] {
        assert!(parse_content(json!({"fields":fields})).is_err());
    }
    assert!(parse_content(json!({"fields":[{"name":"x","required":true}]})).is_ok());
    assert!(parse_content(json!({"fields":[],"query":{}})).is_err());
}

#[test]
fn focus_wraps_and_covered_views_preserve_drafts_and_ignore_input() {
    let mut form = declared(json!([{"name":"a","value":"initial"},{"name":"b"}]));
    paste(&mut form, " edit");
    key(&mut form, Key::BackTab);
    assert_eq!(form.publication.current["focused"], "b");
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context())
        .unwrap();
    paste(&mut form, "ignored");
    key(&mut form, Key::Char('x'));
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    key(&mut form, Key::Tab);
    assert_eq!(
        form.publication.current["values"],
        json!({"a":"initial edit","b":""})
    );
    key(&mut form, Key::Ctrl('u'));
    paste(&mut form, "initial");
    assert_eq!(form.publication.current["dirty"], false);
}

#[test]
fn required_and_optional_scalar_validation_is_explicit() {
    let mut form = declared(json!([
        {"name":"number","type":"number","required":true},
        {"name":"json","type":"json","required":true},
        {"name":"bool","type":"boolean","value":false,"required":true},
        {"name":"optional","type":"integer"}
    ]));
    paste(&mut form, "1.25");
    key(&mut form, Key::Tab);
    paste(&mut form, "null");
    assert_eq!(form.publication.current["errors"]["json"], "Required");
    key(&mut form, Key::Ctrl('u'));
    paste(&mut form, "[]");
    assert_eq!(form.publication.current["valid"], true);
    assert_eq!(form.publication.current["values"]["number"], 1.25);
    assert_eq!(form.publication.current["values"]["optional"], Value::Null);
}

#[test]
fn rendering_keeps_focused_unicode_editor_visible_in_small_areas() {
    let mut form = declared(json!(
        (0..12)
            .map(|n| json!({"name":format!("field{n}"),"value":"界界界é"}))
            .collect::<Vec<_>>()
    ));
    for _ in 0..11 {
        key(&mut form, Key::Tab);
    }
    for (width, height) in [(12, 6), (3, 3), (1, 1), (20, 2)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let result = form
                    .render(
                        frame,
                        frame.area(),
                        &RenderContext::for_terminal(TerminalSize { width, height }),
                    )
                    .unwrap();
                let cursor = result.cursor.unwrap();
                assert!(cursor.visible && cursor.x < width && cursor.y < height);
            })
            .unwrap();
    }
}

#[test]
fn required_fields_use_the_inline_marker_without_an_additional_row() {
    let form = declared(json!([{"name":"name", "label":"Name", "required":true}]));
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
    terminal
        .draw(|frame| {
            form.render(
                frame,
                frame.area(),
                &RenderContext::for_terminal(TerminalSize {
                    width: 80,
                    height: 8,
                }),
            )
            .unwrap();
        })
        .unwrap();
    let screen = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(screen.contains("Name *"));
    assert!(!screen.contains("Required"));
}

#[test]
fn field_errors_render_on_the_bottom_border_without_an_extra_row() {
    let mut form = declared(json!([
        {"name":"age", "label":"Age", "type":"integer", "value":3}
    ]));
    paste(&mut form, "x");
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal
        .draw(|frame| {
            form.render_form(frame, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows = (0..5)
        .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let matches = rows
        .iter()
        .filter(|row| row.contains("Enter an integer"))
        .count();
    assert_eq!(matches, 1, "the error must occupy a single row: {rows:?}");
    let error_row = rows
        .iter()
        .position(|row| row.contains("Enter an integer"))
        .unwrap();
    assert!(
        rows[error_row].contains('╰'),
        "the error must share the bottom border: {rows:?}"
    );
}

#[test]
fn errors_are_omitted_when_the_border_cannot_fit_them() {
    let mut form = declared(json!([
        {"name":"age", "label":"Age", "type":"integer", "value":3}
    ]));
    paste(&mut form, "x");
    for (width, height) in [(3u16, 3u16), (1, 1)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                form.render_form(frame, frame.area());
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("Enter"), "{width}x{height}: {text:?}");
    }
}

#[test]
fn form_column_uses_the_compact_default_maximum_width() {
    let form = declared(json!([{"name":"name", "value":"value"}]));
    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    terminal
        .draw(|frame| {
            form.render(
                frame,
                frame.area(),
                &RenderContext::for_terminal(TerminalSize {
                    width: 80,
                    height: 10,
                }),
            )
            .unwrap();
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((16, 3)).unwrap().symbol(), "╭");
    assert_eq!(buffer.cell((63, 3)).unwrap().symbol(), "╮");
    assert_eq!(buffer.cell((64, 3)).unwrap().symbol(), " ");
}

#[test]
fn async_content_starts_on_activation_and_accepts_only_its_task() {
    let tasks = TaskRuntime::new();
    let mut form = FormView::new(config(json!({"producer":"script","handler":{"script":"#!/bin/sh\nprintf '%s' '{\"version\":1,\"content\":{\"fields\":[{\"name\":\"loaded\",\"value\":\"ok\"}]}}'"}}), tasks.clone()), &request(), ViewInstanceId(1)).unwrap();
    assert!(form.task.is_none());
    assert!(!form.publication.ready);
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    let stale = crate::protocol::contracts::TaskEvent {
        instance: ViewInstanceId(1),
        task: TaskId(1),
        generation: 2,
        outcome: crate::protocol::contracts::TaskOutcome::Completed(Value::Null),
    };
    assert_eq!(
        form.event(ViewEvent::Task(stale), &context()).unwrap(),
        ViewDecision::Stay
    );
    assert!(!form.publication.ready);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !form.publication.ready && form.error.is_none() && std::time::Instant::now() < deadline {
        for event in tasks.drain_events() {
            assert_eq!(event.instance, ViewInstanceId(1));
            assert_eq!(event.task, TaskId(1));
            assert_eq!(event.generation, 1);
            assert_eq!(
                form.event(ViewEvent::Task(event), &context()).unwrap(),
                ViewDecision::Invalidate
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(form.publication.ready, "{:?}", form.error);
    assert_eq!(form.publication.current["values"]["loaded"], "ok");
    assert!(
        tasks
            .metrics_snapshot()
            .recent_terminal
            .last()
            .unwrap()
            .process_reaped_at
            .is_some()
    );
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context())
        .unwrap();
    assert!(form.task.is_none());
    tasks.shutdown_and_wait();
}

#[test]
fn failed_content_never_publishes_a_ready_or_valid_form() {
    for response in [
        "not json",
        r#"{"version":2,"content":{"fields":[]}}"#,
        r#"{"version":1,"content":{"fields":[]},"query":{}}"#,
        r#"{"version":1,"content":{"fields":[{"name":"x"},{"name":"x"}]}}"#,
    ] {
        let tasks = TaskRuntime::new();
        let script = format!("#!/bin/sh\nprintf '%s' '{response}'");
        let mut form = FormView::new(
            config(
                json!({"producer":"script","handler":{"script":script}}),
                tasks.clone(),
            ),
            &request(),
            ViewInstanceId(1),
        )
        .unwrap();
        form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while form.error.is_none() && std::time::Instant::now() < deadline {
            for event in tasks.drain_events() {
                form.event(ViewEvent::Task(event), &context()).unwrap();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(form.error.is_some());
        assert!(!form.publication.ready);
        assert_eq!(form.publication.current["valid"], false);
        assert!(form.fields.is_empty());
        tasks.shutdown_and_wait();
    }
}

#[test]
fn closing_cancels_content_work_and_rejects_late_completions() {
    let tasks = TaskRuntime::new();
    let mut form = FormView::new(config(json!({"producer":"script","handler":{"script":"#!/bin/sh\nsleep 30\nprintf '%s' '{\"version\":1,\"content\":{\"fields\":[]}}'"}}), tasks.clone()), &request(), ViewInstanceId(1)).unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context())
        .unwrap();
    let state = form.publication.clone();
    tasks.shutdown_and_wait();
    let event = crate::protocol::contracts::TaskEvent {
        instance: ViewInstanceId(1),
        task: TaskId(1),
        generation: 1,
        outcome: crate::protocol::contracts::TaskOutcome::Completed(json!({"fields":[]})),
    };
    assert_eq!(
        form.event(ViewEvent::Task(event), &context()).unwrap(),
        ViewDecision::Stay
    );
    assert_eq!(form.publication, state);
    assert!(form.task.is_none());
}

#[test]
fn registry_accepts_form_and_rejects_picker_sources_or_custom_keymaps() {
    let registry = crate::engine::EngineRegistry::new();
    assert_eq!(
        registry
            .definition_for_engine("form")
            .unwrap()
            .factory_fields
            .runtime,
        &["content"]
    );
    let valid = "[engine]\ntype = 'form'\n[engine.config.content]\nproducer = 'declared'\nhandler = { fields = [{ name = 'title' }] }\n";
    registry
        .validate_config("form", &toml::from_str(valid).unwrap())
        .unwrap();
    let extra = "[engine.config]\nitems = []";
    assert!(
        registry
            .validate_config("form", &toml::from_str(&format!("{valid}{extra}")).unwrap())
            .is_err()
    );
}

#[test]
fn closing_rollback_restores_drafts_and_only_closed_is_final() {
    let mut form = declared(json!([{"name":"title","value":"initial"}]));
    paste(&mut form, " edited");
    let state = form.publication.clone();
    assert_eq!(
        form.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context())
            .unwrap(),
        ViewDecision::Stay
    );
    assert!(!form.closed);
    assert_eq!(
        form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
            .unwrap(),
        ViewDecision::Invalidate
    );
    assert_eq!(form.publication, state);
    paste(&mut form, " restored");
    assert_eq!(
        form.publication.current["values"]["title"],
        "initial edited restored"
    );
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Closed), &context())
        .unwrap();
    let final_state = form.publication.clone();
    assert_eq!(
        form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
            .unwrap(),
        ViewDecision::Stay
    );
    paste(&mut form, "ignored");
    assert_eq!(form.publication, final_state);
}

#[test]
fn closing_rollback_restarts_incomplete_content_with_new_task_generation() {
    let tasks = TaskRuntime::new();
    let mut form = FormView::new(config(json!({"producer":"script","handler":{"script":"#!/bin/sh\nprintf '%s' '{\"version\":1,\"content\":{\"fields\":[{\"name\":\"loaded\",\"value\":\"restored\"}]}}'"}}), tasks.clone()), &request(), ViewInstanceId(1)).unwrap();
    assert_eq!(
        form.event(ViewEvent::Lifecycle(LifecycleEvent::Mounted), &context())
            .unwrap(),
        ViewDecision::Stay
    );
    assert!(form.task.is_none());
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    assert_eq!(form.task_generation, 1);
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Closing), &context())
        .unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    assert_eq!(form.task_generation, 2);
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Covered), &context())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !form.publication.ready && std::time::Instant::now() < deadline {
        for event in tasks.drain_events() {
            let current = event.generation == 2;
            assert_eq!(
                form.event(ViewEvent::Task(event), &context()).unwrap(),
                if current {
                    ViewDecision::Invalidate
                } else {
                    ViewDecision::Stay
                }
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(form.publication.ready, "{:?}", form.error);
    assert!(!form.active, "covered completion must remain local");
    assert_eq!(form.publication.current["values"]["loaded"], "restored");
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();
    assert_eq!(form.task_generation, 2, "completed content must not reload");
    tasks.shutdown_and_wait();
}

#[test]
fn form_chrome_status_reports_the_focused_field_position() {
    let mut form = declared(json!([
        {"name": "first", "value": "1"},
        {"name": "second", "value": "2"},
        {"name": "third", "value": "3"}
    ]));
    let chrome = form.chrome(&context()).unwrap();
    assert!(chrome.bindings.is_none());
    assert_eq!(chrome.status.as_deref(), Some("Ready \u{b7} 1 of 3"));
    key(&mut form, Key::Tab);
    assert_eq!(
        form.chrome(&context()).unwrap().status.as_deref(),
        Some("Ready \u{b7} 2 of 3")
    );
    key(&mut form, Key::Tab);
    key(&mut form, Key::Tab);
    assert_eq!(
        form.chrome(&context()).unwrap().status.as_deref(),
        Some("Ready \u{b7} 1 of 3")
    );
}

#[test]
fn nullable_string_initializes_as_empty_and_evaluates_to_empty_string() {
    let form = declared(json!([{"name": "nullable_str", "value": null, "type": "string"}]));
    let snapshot = form.command_snapshot();
    let state = &snapshot.publication.as_ref().unwrap().current;
    assert_eq!(state["values"]["nullable_str"], json!(""));
    assert_eq!(state["valid"], json!(true));
}

#[test]
fn password_field_initializes_and_evaluates_correctly() {
    let mut form = declared(json!([
        {"name": "pass", "label": "Password", "type": "password", "required": true, "value": "secret"}
    ]));
    let snapshot = form.command_snapshot();
    let state = &snapshot.publication.as_ref().unwrap().current;
    assert_eq!(state["values"]["pass"], json!("secret"));
    assert_eq!(state["drafts"]["pass"], json!("secret"));
    assert_eq!(state["valid"], json!(true));

    // Clear the field and check required validation
    key(&mut form, Key::Ctrl('u'));
    let snapshot = form.command_snapshot();
    let state = &snapshot.publication.as_ref().unwrap().current;
    assert_eq!(state["valid"], json!(false));
    assert_eq!(state["errors"]["pass"], json!("Required"));

    // Type a new password
    paste(&mut form, "my_new_pass");
    let snapshot = form.command_snapshot();
    let state = &snapshot.publication.as_ref().unwrap().current;
    assert_eq!(state["values"]["pass"], json!("my_new_pass"));
    assert_eq!(state["valid"], json!(true));
}

#[test]
fn password_field_renders_masked_in_form() {
    let form = declared(json!([
        {"name": "pass", "label": "Password", "type": "password", "value": "secret123"}
    ]));
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal
        .draw(|f| {
            form.render_form(f, f.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered_text = (0..5)
        .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered_text.contains("secret123"));
    assert!(rendered_text.contains("*********"));
}

#[test]
fn form_discrete_navigation_and_exit_commands() {
    let mut form = declared(json!([
        {"name": "first", "value": "1"},
        {"name": "second", "value": "2"},
        {"name": "third", "value": "3"}
    ]));

    let cmds = form.engine_commands(&context());
    assert_eq!(cmds.len(), 7);
    assert!(
        cmds.iter()
            .all(|c| matches!(c.handler, crate::command::CommandHandler::Event))
    );

    assert_eq!(form.publication.current["focused"], "first");

    // Test Down arrow navigates to next
    assert_eq!(key(&mut form, Key::Down), ViewDecision::Invalidate);
    assert_eq!(form.publication.current["focused"], "second");

    // Test Tab navigates to next
    assert_eq!(key(&mut form, Key::Tab), ViewDecision::Invalidate);
    assert_eq!(form.publication.current["focused"], "third");

    // Test Up arrow navigates to previous
    assert_eq!(key(&mut form, Key::Up), ViewDecision::Invalidate);
    assert_eq!(form.publication.current["focused"], "second");

    // Test BackTab navigates to previous
    assert_eq!(key(&mut form, Key::BackTab), ViewDecision::Invalidate);
    assert_eq!(form.publication.current["focused"], "first");

    // Test Escape produces Close
    assert_eq!(key(&mut form, Key::Escape), ViewDecision::Close);

    // Test Ctrl-C produces Exit
    assert_eq!(key(&mut form, Key::Ctrl('c')), ViewDecision::Exit);

    // Test Ctrl-D produces Exit
    assert_eq!(key(&mut form, Key::Ctrl('d')), ViewDecision::Exit);
}

#[test]
fn form_bindings_can_be_customized_and_disabled_via_keymap() {
    let mut config = config(
        json!({"producer":"declared", "handler":{"fields":[{"name": "a"}]}}),
        TaskRuntime::new(),
    );
    config.bindings = ProjectedBindingConfig {
        defaults: Some(json!({
            "focus_next": ["ctrl+n"],
            "cancel": ["ctrl+q"]
        })),
        view_keymap: Some(json!({
            "ctrl+q": false,
            "alt+x": "cancel"
        })),
        engine_fields: Default::default(),
    };
    let mut form = FormView::new(config, &request(), ViewInstanceId(1)).unwrap();
    form.event(ViewEvent::Lifecycle(LifecycleEvent::Activated), &context())
        .unwrap();

    let cmds = form.engine_commands(&context());
    let keys: Vec<_> = cmds.iter().filter_map(|c| c.key).collect();
    assert!(keys.contains(&Key::Ctrl('n')));
    assert!(keys.contains(&Key::Alt('x')));
    assert!(!keys.contains(&Key::Ctrl('q')));
    assert!(!keys.contains(&Key::Escape));
}

#[test]
fn enum_field_validation_and_parsing_rules() {
    // Valid enum field
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": ["dev", "staging", "prod"],
                "value": "dev"
            }]
        }))
        .is_ok()
    );

    // Valid enum with null initial value
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": ["dev", "prod"]
            }]
        }))
        .is_ok()
    );

    // Rejects enum without options
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum"
            }]
        }))
        .is_err()
    );

    // Rejects enum with empty options
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": []
            }]
        }))
        .is_err()
    );

    // Rejects duplicate options
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": ["dev", "dev"]
            }]
        }))
        .is_err()
    );

    // Rejects blank option string
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": ["dev", "  "]
            }]
        }))
        .is_err()
    );

    // Rejects initial value not in options
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "enum",
                "options": ["dev", "prod"],
                "value": "staging"
            }]
        }))
        .is_err()
    );

    // Rejects options configured on non-enum field
    assert!(
        parse_content(json!({
            "fields": [{
                "name": "env",
                "type": "string",
                "options": ["dev", "prod"]
            }]
        }))
        .is_err()
    );
}

#[test]
fn enum_field_inline_switching_and_keyboard_interaction() {
    let mut form = declared(json!([
        {
            "name": "env",
            "type": "enum",
            "options": ["dev", "staging", "prod"],
            "value": "dev",
            "required": true
        },
        {
            "name": "optional_tier",
            "type": "enum",
            "options": ["free", "pro", "enterprise"]
        }
    ]));

    assert_eq!(form.publication.current["values"]["env"], "dev");
    assert_eq!(form.publication.current["valid"], true);
    assert_eq!(form.publication.current["dirty"], false);

    // Space cycles forward: dev -> staging -> prod -> dev
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["env"], "staging");
    assert_eq!(form.publication.current["drafts"]["env"], "staging");
    assert_eq!(form.publication.current["dirty"], true);

    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["env"], "prod");

    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["env"], "dev");

    // Right cycles forward, Left cycles backward
    key(&mut form, Key::Right);
    assert_eq!(form.publication.current["values"]["env"], "staging");
    key(&mut form, Key::Left);
    assert_eq!(form.publication.current["values"]["env"], "dev");
    key(&mut form, Key::Left);
    assert_eq!(form.publication.current["values"]["env"], "prod");

    // Home jumps to first, End jumps to last
    key(&mut form, Key::Home);
    assert_eq!(form.publication.current["values"]["env"], "dev");
    key(&mut form, Key::End);
    assert_eq!(form.publication.current["values"]["env"], "prod");

    // First-letter jumping: 's' jumps to staging
    key(&mut form, Key::Char('s'));
    assert_eq!(form.publication.current["values"]["env"], "staging");
    // 'd' jumps to dev
    key(&mut form, Key::Char('d'));
    assert_eq!(form.publication.current["values"]["env"], "dev");

    // Backspace clears required enum field -> error "Required"
    key(&mut form, Key::Backspace);
    assert_eq!(form.publication.current["drafts"]["env"], "");
    assert_eq!(form.publication.current["values"]["env"], Value::Null);
    assert_eq!(form.publication.current["errors"]["env"], "Required");
    assert_eq!(form.publication.current["valid"], false);

    // Space on cleared field selects options[0]
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["env"], "dev");
    assert_eq!(form.publication.current["valid"], true);

    // Navigate to optional enum field (initially null)
    key(&mut form, Key::Tab);
    assert_eq!(form.publication.current["focused"], "optional_tier");
    assert_eq!(
        form.publication.current["values"]["optional_tier"],
        Value::Null
    );
    assert_eq!(form.publication.current["valid"], true);

    // Space on null optional enum selects first option
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["optional_tier"], "free");

    // Clear it with Ctrl+U -> becomes Null again and form stays valid
    key(&mut form, Key::Ctrl('u'));
    assert_eq!(
        form.publication.current["values"]["optional_tier"],
        Value::Null
    );
    assert_eq!(form.publication.current["valid"], true);
}

#[test]
fn enum_field_paste_and_invalid_value_reporting() {
    let mut form = declared(json!([
        {
            "name": "tier",
            "type": "enum",
            "options": ["basic", "standard", "premium"],
            "value": "basic"
        }
    ]));

    // Paste valid option replaces current value
    paste(&mut form, "premium");
    assert_eq!(form.publication.current["values"]["tier"], "premium");
    assert_eq!(form.publication.current["valid"], true);

    // Paste invalid value reports "Choose from options"
    paste(&mut form, "unknown_tier");
    assert_eq!(form.publication.current["drafts"]["tier"], "unknown_tier");
    assert_eq!(form.publication.current["values"]["tier"], Value::Null);
    assert_eq!(
        form.publication.current["errors"]["tier"],
        "Choose from options"
    );
    assert_eq!(form.publication.current["valid"], false);
}

#[test]
fn enum_field_renders_inline_selector_decoration() {
    let form = declared(json!([
        {
            "name": "env",
            "type": "enum",
            "options": ["dev", "prod"],
            "value": "prod"
        }
    ]));
    let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
    terminal
        .draw(|frame| {
            form.render(
                frame,
                frame.area(),
                &RenderContext::for_terminal(TerminalSize {
                    width: 40,
                    height: 5,
                }),
            )
            .unwrap();
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..5)
        .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen.contains("< prod >"));
}

#[test]
fn boolean_field_renders_inline_selector_decoration() {
    let form = declared(json!([
        {
            "name": "pinned",
            "label": "Pinned",
            "type": "boolean",
            "value": true
        },
        {
            "name": "archived",
            "label": "Archived",
            "type": "boolean",
            "value": false
        }
    ]));
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal
        .draw(|frame| {
            form.render(
                frame,
                frame.area(),
                &RenderContext::for_terminal(TerminalSize {
                    width: 40,
                    height: 8,
                }),
            )
            .unwrap();
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let screen = (0..8)
        .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(screen.contains("< true >"));
    assert!(screen.contains("< false >"));
}

#[test]
fn boolean_field_keyboard_interaction_and_safety() {
    let mut form = declared(json!([
        {
            "name": "flag",
            "label": "Flag",
            "type": "boolean",
            "value": false,
            "required": true
        }
    ]));

    assert_eq!(form.publication.current["values"]["flag"], false);
    assert_eq!(form.publication.current["valid"], true);

    // Space toggles false -> true
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["flag"], true);
    assert_eq!(form.publication.current["dirty"], true);

    // Space toggles true -> false
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["flag"], false);

    // Enter also toggles
    key(&mut form, Key::Enter);
    assert_eq!(form.publication.current["values"]["flag"], true);

    // Right / Left cycles like enum
    key(&mut form, Key::Right);
    assert_eq!(form.publication.current["values"]["flag"], false);
    key(&mut form, Key::Left);
    assert_eq!(form.publication.current["values"]["flag"], true);

    // Home jumps to first (true), End jumps to last (false)
    key(&mut form, Key::End);
    assert_eq!(form.publication.current["values"]["flag"], false);
    key(&mut form, Key::Home);
    assert_eq!(form.publication.current["values"]["flag"], true);

    // 't' jumps to true, 'f' jumps to false
    key(&mut form, Key::Char('f'));
    assert_eq!(form.publication.current["values"]["flag"], false);
    key(&mut form, Key::Char('t'));
    assert_eq!(form.publication.current["values"]["flag"], true);

    // Arbitrary keys (e.g. 'a', 'z', '!') are ignored and do not corrupt boolean state
    key(&mut form, Key::Char('a'));
    key(&mut form, Key::Char('z'));
    key(&mut form, Key::Char('!'));
    assert_eq!(form.publication.current["values"]["flag"], true);
    assert_eq!(form.publication.current["valid"], true);

    // Backspace clears required boolean field -> reports "Required"
    key(&mut form, Key::Backspace);
    assert_eq!(form.publication.current["values"]["flag"], Value::Null);
    assert_eq!(form.publication.current["errors"]["flag"], "Required");
    assert_eq!(form.publication.current["valid"], false);

    // Space re-engages it as true
    key(&mut form, Key::Char(' '));
    assert_eq!(form.publication.current["values"]["flag"], true);
    assert_eq!(form.publication.current["valid"], true);
}

#[test]
fn boolean_field_paste_normalization() {
    let mut form = declared(json!([
        {
            "name": "flag",
            "type": "boolean",
            "value": false
        }
    ]));

    paste(&mut form, "yes");
    assert_eq!(form.publication.current["values"]["flag"], true);
    assert_eq!(form.publication.current["valid"], true);

    paste(&mut form, "no");
    assert_eq!(form.publication.current["values"]["flag"], false);
    assert_eq!(form.publication.current["valid"], true);

    paste(&mut form, "1");
    assert_eq!(form.publication.current["values"]["flag"], true);

    paste(&mut form, "0");
    assert_eq!(form.publication.current["values"]["flag"], false);

    // Invalid paste reports "Enter true or false"
    paste(&mut form, "not_a_bool");
    assert_eq!(form.publication.current["values"]["flag"], Value::Null);
    assert_eq!(
        form.publication.current["errors"]["flag"],
        "Enter true or false"
    );
    assert_eq!(form.publication.current["valid"], false);
}
