use super::items::{ItemsRequest, ItemsResponse, PickerItemsLoader};
use crate::input::ViewMountId;
use crate::task::{MountTaskLease, MountTaskStarter, TaskHandle, TaskTags};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PickerItemsScheduler {
    mount_id: ViewMountId,
    lane: String,
}

impl PickerItemsScheduler {
    pub(crate) fn new(lease: MountTaskLease, view_ref: &str) -> Self {
        Self {
            mount_id: lease.mount_id(),
            lane: format!("picker-items:{view_ref}"),
        }
    }

    pub(crate) fn submit_items(
        &self,
        starter: &MountTaskStarter,
        loader: &Arc<dyn PickerItemsLoader>,
        request: ItemsRequest,
    ) -> TaskHandle<ItemsResponse> {
        debug_assert_eq!(self.mount_id, starter.mount_id());
        debug_assert_eq!(self.mount_id, request.identity.mount_id);
        debug_assert!(request.validate().is_ok());
        let ItemsRequest {
            view,
            identity,
            engine_state,
        } = request;
        let lane = self.replacement_lane(identity.source);
        let loader = Arc::clone(loader);
        starter.spawn_latest_tagged(lane, TaskTags::new("picker", "items"), move |context| {
            let request = ItemsRequest {
                view: view.clone(),
                identity: identity.clone(),
                engine_state: engine_state.clone(),
            };
            let outcome = loader.load(&request, &context.cancellation);
            if outcome.managed_child_reaped {
                context.mark_process_reaped();
            }
            Ok(ItemsResponse {
                view,
                identity,
                result: outcome.result.map_err(|error| error.to_string()),
            })
        })
    }

    fn replacement_lane(&self, target: crate::input::InputSourceIdentity) -> String {
        // Buffer generations share a mount lane so a newer query replaces older
        // work on the same mount without cancelling another mount of this view.
        format!("{}:frame-{}", self.lane, target.frame.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputSourceIdentity;
    use crate::task::{TaskCompletion, TaskRuntime};
    use crate::workflow::parameter::ParameterSnapshot;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    fn receive<T>(handle: &mut TaskHandle<T>) -> TaskCompletion<T> {
        for _ in 0..200 {
            match handle.try_recv() {
                Ok(completion) => return completion,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("task completion channel disconnected")
                }
            }
        }
        panic!("task did not complete before the test deadline")
    }

    struct EmptyLoader;

    impl PickerItemsLoader for EmptyLoader {
        fn load(
            &self,
            _request: &ItemsRequest,
            _cancellation: &crate::lifecycle::CancellationToken,
        ) -> super::super::items::ItemsLoadOutcome {
            super::super::items::ItemsLoadOutcome::without_managed_child(Ok(
                super::super::items::ItemsResult::default(),
            ))
        }
    }

    struct ReapedFailedLoader;

    impl PickerItemsLoader for ReapedFailedLoader {
        fn load(
            &self,
            _request: &ItemsRequest,
            _cancellation: &crate::lifecycle::CancellationToken,
        ) -> super::super::items::ItemsLoadOutcome {
            super::super::items::ItemsLoadOutcome {
                result: Err(anyhow::anyhow!("script exited with status 7")),
                managed_child_reaped: true,
            }
        }
    }

    struct BlockingFirstLoader {
        started: Arc<AtomicBool>,
    }

    impl PickerItemsLoader for BlockingFirstLoader {
        fn load(
            &self,
            _request: &ItemsRequest,
            cancellation: &crate::lifecycle::CancellationToken,
        ) -> super::super::items::ItemsLoadOutcome {
            if !self.started.swap(true, Ordering::Release) {
                while !cancellation.is_cancelled() {
                    thread::yield_now();
                }
            }
            super::super::items::ItemsLoadOutcome::without_managed_child(Ok(
                super::super::items::ItemsResult::default(),
            ))
        }
    }

    fn request_for(mount_id: ViewMountId, generation: u64) -> ItemsRequest {
        let source = InputSourceIdentity {
            frame: mount_id,
            generation: 3,
        };
        let page_parameters = ParameterSnapshot::from_parts(
            json!({"query": "needle"}),
            "binding".to_string(),
            source,
            7,
        );
        let identity = super::super::items::ItemsRequestIdentity::new(
            mount_id,
            source,
            generation,
            "needle".to_string(),
            7,
            "binding".to_string(),
            page_parameters,
        )
        .unwrap();
        ItemsRequest::new("apps:main".to_string(), identity).unwrap()
    }

    #[test]
    fn scheduler_starts_only_when_given_the_post_commit_starter() {
        let tasks = TaskRuntime::new();
        let mount_id = ViewMountId(51);
        let scheduler = PickerItemsScheduler::new(MountTaskLease::new(mount_id), "apps:main");
        let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(mount_id));
        let loader: Arc<dyn PickerItemsLoader> = Arc::new(EmptyLoader);
        let request = request_for(mount_id, 1);
        let identity = request.identity.clone();
        let mut handle = scheduler.submit_items(&starter, &loader, request);

        match receive(&mut handle) {
            TaskCompletion::Completed(response) => {
                assert_eq!(response.identity, identity);
                assert!(response.result.unwrap().items.is_empty());
            }
            TaskCompletion::Failed(error) => panic!("scheduler task failed: {error}"),
            TaskCompletion::Cancelled => panic!("scheduler task was cancelled"),
        }
        tasks.shutdown_and_wait();
    }

    #[test]
    fn picker_marks_a_reaped_failed_script_load() {
        let tasks = TaskRuntime::new();
        let mount_id = ViewMountId(53);
        let scheduler = PickerItemsScheduler::new(MountTaskLease::new(mount_id), "apps:main");
        let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(mount_id));
        let loader: Arc<dyn PickerItemsLoader> = Arc::new(ReapedFailedLoader);
        let mut handle = scheduler.submit_items(&starter, &loader, request_for(mount_id, 1));

        match receive(&mut handle) {
            TaskCompletion::Completed(response) => {
                assert!(response.result.unwrap_err().contains("status 7"));
            }
            TaskCompletion::Failed(error) => panic!("picker task failed: {error}"),
            TaskCompletion::Cancelled => panic!("picker task was cancelled"),
        }
        assert!(
            tasks
                .metrics_snapshot()
                .recent_terminal
                .last()
                .unwrap()
                .process_reaped_at
                .is_some()
        );
        tasks.shutdown_and_wait();
    }

    #[test]
    fn replacement_lane_replaces_previous_request_on_the_same_mount() {
        let tasks = TaskRuntime::new();
        let mount_id = ViewMountId(52);
        let scheduler = PickerItemsScheduler::new(MountTaskLease::new(mount_id), "apps:main");
        let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(mount_id));
        let started = Arc::new(AtomicBool::new(false));
        let loader: Arc<dyn PickerItemsLoader> = Arc::new(BlockingFirstLoader {
            started: Arc::clone(&started),
        });
        let first = scheduler.submit_items(&starter, &loader, request_for(mount_id, 1));
        for _ in 0..200 {
            if started.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started.load(Ordering::Acquire));
        let mut replacement = scheduler.submit_items(&starter, &loader, request_for(mount_id, 2));
        let mut first = first;
        assert!(matches!(receive(&mut first), TaskCompletion::Cancelled));
        match receive(&mut replacement) {
            TaskCompletion::Completed(response) => assert_eq!(response.identity.generation, 2),
            TaskCompletion::Failed(error) => panic!("replacement task failed: {error}"),
            TaskCompletion::Cancelled => panic!("replacement task was cancelled"),
        }
        tasks.shutdown_and_wait();
    }

    #[test]
    fn replacement_lane_uses_mount_frame_but_not_buffer_generation() {
        let tasks = TaskRuntime::new();
        let scheduler =
            PickerItemsScheduler::new(MountTaskLease::new(ViewMountId(51)), "apps:main");
        let first = InputSourceIdentity {
            frame: ViewMountId(51),
            generation: 0,
        };
        let next_buffer = InputSourceIdentity {
            frame: ViewMountId(51),
            generation: 1,
        };
        let other_mount = InputSourceIdentity {
            frame: ViewMountId(52),
            generation: 0,
        };

        assert_eq!(
            scheduler.replacement_lane(first),
            scheduler.replacement_lane(next_buffer)
        );
        assert_ne!(
            scheduler.replacement_lane(first),
            scheduler.replacement_lane(other_mount)
        );
        tasks.shutdown_and_wait();
    }
}
