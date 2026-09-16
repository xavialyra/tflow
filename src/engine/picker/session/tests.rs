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
    fn selected_feed_item_updates_publication_and_owner() {
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
        assert_eq!(picker.selected_item_owner(), Some("apps:main"));
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
            crate::engine::picker::mount_data(&config, &Value::Null, page, MountTaskLease::new(mount))
                .unwrap();
        let preview_config = crate::engine::picker::preview::parse(
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
            result: Ok(crate::engine::picker::items::ItemsResult {
                items: vec![Item {
                    text: "Needle".into(),
                    display: crate::engine::picker::ItemDisplayInput::Plain("Needle".into()).into(),
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
                crate::engine::picker::mount_data(&config, &input, page, MountTaskLease::new(mount))
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
            let (preview_ratio, preview_min_width, _) = crate::engine::picker::preview_options(
                preview_ratio.as_ref(),
                preview_min_width.as_ref(),
                None,
            )
            .unwrap();
            let preview_config = crate::engine::picker::preview::parse(
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
            picker.preview = crate::engine::picker::preview::PickerPreview::new(
                crate::engine::picker::preview::parse(0.35, 24, Some(json!({"producer":"declared", "document":(0..10).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")}))).unwrap()
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
