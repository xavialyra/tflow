use super::*;
use super::{MountTaskLease, MountTaskStarter, TaskCompletion, TaskRuntime};
use crate::input::ViewMountId;
use crate::protocol::contracts::{TaskId, TaskOutcome, ViewInstanceId};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

struct TestClock {
    nanos: AtomicU64,
}

impl TestClock {
    fn advance(&self, duration: Duration) {
        self.nanos
            .fetch_add(duration.as_nanos().try_into().unwrap(), Ordering::AcqRel);
    }
}

impl super::TaskClock for TestClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.nanos.load(Ordering::Acquire))
    }
}

fn test_runtime(clock: Arc<TestClock>) -> TaskRuntime {
    TaskRuntime::with_clock(clock)
}

fn receive<T>(handle: &mut super::TaskHandle<T>) -> TaskCompletion<T> {
    for _ in 0..200 {
        match handle.try_recv() {
            Ok(result) => return result,
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

#[test]
fn correlated_mount_tasks_emit_router_task_events() {
    let tasks = TaskRuntime::new();
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(73)))
        .for_task(TaskId(4), 9);
    let mut handle =
        starter.spawn_latest_with_test_snapshot("items", json!({}), |_context| Ok("done"));
    assert!(matches!(
        receive(&mut handle),
        TaskCompletion::Completed("done")
    ));

    let mut event = None;
    for _ in 0..200 {
        event = tasks.drain_events().into_iter().next();
        if event.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let event = event.expect("correlated task did not emit its Router event");
    assert_eq!(event.instance, ViewInstanceId(73));
    assert_eq!(event.task, TaskId(4));
    assert_eq!(event.generation, 9);
    assert_eq!(
        event.outcome,
        TaskOutcome::Completed(serde_json::Value::Null)
    );
}

#[test]
fn uses_the_runtime_snapshot_supplied_at_submission() {
    let snapshot = json!({"marker": "before"});
    let tasks = TaskRuntime::new();
    let mut handle = tasks.spawn_latest_with_test_snapshot("snapshot", snapshot, |context| {
        Ok(context.runtime["marker"].clone())
    });

    match receive(&mut handle) {
        TaskCompletion::Completed(value) => assert_eq!(value, json!("before")),
        TaskCompletion::Failed(error) => panic!("task failed: {error}"),
        TaskCompletion::Cancelled => panic!("task was cancelled"),
    }
    tasks.shutdown_and_wait();
}

#[test]
fn latest_submission_can_use_an_explicit_runtime_snapshot() {
    let tasks = TaskRuntime::new();
    let explicit = json!({"marker": "prepared"});

    let mut handle = tasks.spawn_latest_with_test_snapshot("latest", explicit, |context| {
        Ok(context.runtime["marker"].clone())
    });

    match receive(&mut handle) {
        TaskCompletion::Completed(value) => assert_eq!(value, json!("prepared")),
        TaskCompletion::Failed(error) => panic!("task failed: {error}"),
        TaskCompletion::Cancelled => panic!("task was cancelled"),
    }
    tasks.shutdown_and_wait();
}

#[test]
fn cancellation_is_reported_when_a_running_task_is_stopped() {
    let tasks = TaskRuntime::new();
    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut handle = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(())
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    tasks.cancel_all();
    assert!(matches!(receive(&mut handle), TaskCompletion::Cancelled));
    tasks.shutdown_and_wait();
}

#[test]
fn latest_submission_cancels_the_active_task_before_running_the_new_one() {
    let tasks = TaskRuntime::new();
    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut first = tasks.spawn_latest("latest", move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(1)
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    let mut second = tasks.spawn_latest("latest", |_context| Ok(2));
    assert!(matches!(receive(&mut first), TaskCompletion::Cancelled));
    assert!(matches!(receive(&mut second), TaskCompletion::Completed(2)));
    tasks.shutdown_and_wait();
}

#[test]
fn latest_submission_only_replaces_work_in_the_same_lane() {
    let tasks = TaskRuntime::new();
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let release_task = Arc::clone(&release);
    let mut first = tasks.spawn_latest("first", move |context| {
        started_task.store(true, Ordering::Release);
        while !release_task.load(Ordering::Acquire) && !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(1)
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    let mut second = tasks.spawn_latest("second", |_context| Ok(2));
    thread::sleep(Duration::from_millis(5));
    assert!(matches!(
        first.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    release.store(true, Ordering::Release);
    assert!(matches!(receive(&mut first), TaskCompletion::Completed(1)));
    assert!(matches!(receive(&mut second), TaskCompletion::Completed(2)));
    tasks.shutdown_and_wait();
}

#[test]
fn task_starter_replaces_the_same_lane_within_a_mount() {
    let tasks = TaskRuntime::new();
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(81)));
    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut first = starter.spawn_latest_with_test_snapshot("refresh", json!({}), move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(1)
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    let mut replacement =
        starter.spawn_latest_with_test_snapshot("refresh", json!({}), |_context| Ok(2));

    assert!(matches!(receive(&mut first), TaskCompletion::Cancelled));
    assert!(matches!(
        receive(&mut replacement),
        TaskCompletion::Completed(2)
    ));
    tasks.shutdown_and_wait();
}

#[test]
fn task_starters_isolate_the_same_local_lane_between_mounts() {
    let tasks = TaskRuntime::new();
    let first_starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(91)));
    let second_starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(92)));
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let release_task = Arc::clone(&release);
    let mut first =
        first_starter.spawn_latest_with_test_snapshot("refresh", json!({}), move |context| {
            started_task.store(true, Ordering::Release);
            while !release_task.load(Ordering::Acquire) && !context.cancellation.is_cancelled() {
                thread::yield_now();
            }
            Ok(1)
        });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    let mut second =
        second_starter.spawn_latest_with_test_snapshot("refresh", json!({}), |_context| Ok(2));
    thread::sleep(Duration::from_millis(5));
    assert!(matches!(
        first.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    release.store(true, Ordering::Release);
    assert!(matches!(receive(&mut first), TaskCompletion::Completed(1)));
    assert!(matches!(receive(&mut second), TaskCompletion::Completed(2)));
    tasks.shutdown_and_wait();
}

#[test]
fn task_starter_rejects_a_different_mount_identity() {
    let tasks = TaskRuntime::new();
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(101)));
    assert_eq!(starter.mount_id(), ViewMountId(101));
    assert!(starter.ensure_mount(ViewMountId(102)).is_err());
    tasks.shutdown_and_wait();
}

#[test]
fn dropping_the_last_runtime_cancels_and_joins_active_work() {
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let mut handle;
    {
        let tasks = TaskRuntime::new();
        let started_task = Arc::clone(&started);
        let finished_task = Arc::clone(&finished);
        handle = tasks.spawn(move |context| {
            started_task.store(true, Ordering::Release);
            while !context.cancellation.is_cancelled() {
                thread::yield_now();
            }
            finished_task.store(true, Ordering::Release);
            Ok(())
        });
        for _ in 0..200 {
            if started.load(Ordering::Acquire) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(started.load(Ordering::Acquire));
    }
    assert!(finished.load(Ordering::Acquire));
    assert!(matches!(receive(&mut handle), TaskCompletion::Cancelled));
}

#[test]
fn pending_same_lane_replacement_reports_cancellation() {
    let tasks = TaskRuntime::new();
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let release_task = Arc::clone(&release);
    let mut active = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !release_task.load(Ordering::Acquire) && !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(1)
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    let mut pending = tasks.spawn_latest("latest", |_context| Ok(2));
    let mut replacement = tasks.spawn_latest("latest", |_context| Ok(3));

    assert!(matches!(receive(&mut pending), TaskCompletion::Cancelled));
    release.store(true, Ordering::Release);
    assert!(matches!(receive(&mut active), TaskCompletion::Completed(1)));
    assert!(matches!(
        receive(&mut replacement),
        TaskCompletion::Completed(3)
    ));
    tasks.shutdown_and_wait();
}

#[test]
fn concurrent_last_owner_drop_cancels_and_joins_active_work() {
    let tasks = TaskRuntime::new();
    let first_owner = tasks.clone();
    let second_owner = tasks.clone();
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let finished_task = Arc::clone(&finished);
    let mut handle = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        finished_task.store(true, Ordering::Release);
        Ok(())
    });

    for _ in 0..200 {
        if started.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(started.load(Ordering::Acquire));
    drop(tasks);

    let barrier = Arc::new(Barrier::new(3));
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        first_barrier.wait();
        drop(first_owner);
    });
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        drop(second_owner);
    });
    barrier.wait();
    first.join().unwrap();
    second.join().unwrap();

    assert!(finished.load(Ordering::Acquire));
    assert!(matches!(receive(&mut handle), TaskCompletion::Cancelled));
}

#[test]
fn submission_after_shutdown_is_cancelled_without_restarting_worker() {
    let tasks = TaskRuntime::new();
    let mut initial = tasks.spawn(|_context| Ok(()));
    assert!(matches!(
        receive(&mut initial),
        TaskCompletion::Completed(())
    ));
    tasks.shutdown_and_wait();
    assert!(
        tasks
            .owner
            .registry
            .worker
            .lock()
            .expect("task registry worker was poisoned")
            .handle
            .is_none()
    );

    let ran = Arc::new(AtomicBool::new(false));
    let ran_task = Arc::clone(&ran);
    let mut rejected = tasks.spawn(move |_context| {
        ran_task.store(true, Ordering::Release);
        Ok(())
    });

    assert!(matches!(receive(&mut rejected), TaskCompletion::Cancelled));
    assert!(!ran.load(Ordering::Acquire));
    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.submission_after_shutdown_total, 1);
    assert!(metrics.recent_terminal.iter().any(|sample| {
        sample.cancellation.as_ref().is_some_and(|cancellation| {
            cancellation.reason == super::TaskCancellationReason::SubmissionAfterShutdown
        })
    }));
    assert!(
        tasks
            .owner
            .registry
            .worker
            .lock()
            .expect("task registry worker was poisoned")
            .handle
            .is_none()
    );
}

#[test]
fn panic_fails_one_task_and_worker_runs_the_next_task() {
    let tasks = TaskRuntime::new();
    let mut panicked =
        tasks.spawn(|_context| -> Result<(), String> { panic!("expected task panic") });
    let mut following = tasks.spawn(|_context| Ok(7));

    match receive(&mut panicked) {
        TaskCompletion::Failed(error) => assert_eq!(error, "task panicked"),
        TaskCompletion::Completed(_) => panic!("panicking task unexpectedly completed"),
        TaskCompletion::Cancelled => panic!("panicking task was cancelled"),
    }
    assert!(matches!(
        receive(&mut following),
        TaskCompletion::Completed(7)
    ));
    tasks.shutdown_and_wait();
}

#[test]
fn mount_cancellation_does_not_cancel_another_mount() {
    let tasks = TaskRuntime::new();
    let first = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(1)));
    let second = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(2)));
    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut first_handle =
        first.spawn_latest_with_test_snapshot("work", serde_json::Value::Null, move |context| {
            started_task.store(true, Ordering::Release);
            while !context.cancellation.is_cancelled() {
                thread::yield_now();
            }
            Ok(1)
        });
    let mut second_handle =
        second.spawn_latest_with_test_snapshot("work", serde_json::Value::Null, |_context| Ok(2));
    while !started.load(Ordering::Acquire) {
        thread::yield_now();
    }

    first.cancel_all();

    assert!(matches!(
        receive(&mut first_handle),
        TaskCompletion::Cancelled
    ));
    assert!(matches!(
        receive(&mut second_handle),
        TaskCompletion::Completed(2)
    ));
    tasks.shutdown_and_wait();
}

#[test]
fn task_errors_are_returned_without_panicking_the_worker() {
    let tasks = TaskRuntime::new();
    let mut handle = tasks.spawn(|_context| Err::<(), _>("expected failure".to_string()));

    match receive(&mut handle) {
        TaskCompletion::Failed(error) => assert_eq!(error, "expected failure"),
        TaskCompletion::Completed(_) => panic!("task unexpectedly succeeded"),
        TaskCompletion::Cancelled => panic!("task was cancelled"),
    }
    tasks.shutdown_and_wait();
}

#[test]
fn metrics_use_monotonic_time_and_record_low_cardinality_tags() {
    let clock = Arc::new(TestClock {
        nanos: AtomicU64::new(0),
    });
    let tasks = test_runtime(Arc::clone(&clock));
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(17)));
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let release_task = Arc::clone(&release);
    let mut handle = starter.spawn_latest_with_test_snapshot_tagged(
        "items",
        serde_json::Value::Null,
        super::TaskTags::new("picker", "items"),
        move |context| {
            started_task.store(true, Ordering::Release);
            while !release_task.load(Ordering::Acquire) {
                thread::yield_now();
            }
            context.mark_process_reaped();
            Ok(())
        },
    );
    while !started.load(Ordering::Acquire) {
        thread::yield_now();
    }
    clock.advance(Duration::from_millis(3));
    release.store(true, Ordering::Release);
    assert!(matches!(
        receive(&mut handle),
        TaskCompletion::Completed(())
    ));

    let metrics = tasks.metrics_snapshot();
    let sample = metrics.recent_terminal.last().unwrap();
    assert_eq!(sample.tags, super::TaskTags::new("picker", "items"));
    assert!(sample.submitted_at <= sample.started_at.unwrap());
    assert!(sample.started_at.unwrap() <= sample.terminal_at);
    assert_eq!(sample.process_reaped_at, Some(sample.terminal_at));
    assert_eq!(metrics.completed_total, 1);
    tasks.shutdown_and_wait();
}

#[test]
fn metrics_track_queue_high_water_replacement_and_active_cancellation() {
    let clock = Arc::new(TestClock {
        nanos: AtomicU64::new(0),
    });
    let tasks = test_runtime(clock);
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let release_task = Arc::clone(&release);
    let mut active = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !release_task.load(Ordering::Acquire) && !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(())
    });
    while !started.load(Ordering::Acquire) {
        thread::yield_now();
    }
    let mut first_pending = tasks.spawn_latest("latest", |_context| Ok(1));
    let mut replacement = tasks.spawn_latest("latest", |_context| Ok(2));
    let mut unrelated = tasks.spawn(|_context| Ok(3));

    assert!(matches!(
        receive(&mut first_pending),
        TaskCompletion::Cancelled
    ));
    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.queue_depth, 2);
    assert_eq!(metrics.queue_high_water, 2);
    assert_eq!(metrics.active_tasks, 1);
    assert_eq!(
        metrics
            .recent_terminal
            .iter()
            .find(|sample| sample.outcome == super::TaskTerminalOutcome::Cancelled)
            .unwrap()
            .cancellation
            .as_ref()
            .unwrap()
            .reason,
        super::TaskCancellationReason::LatestWinsPendingReplacement
    );

    tasks.cancel_all();
    assert!(matches!(receive(&mut active), TaskCompletion::Cancelled));
    assert!(matches!(
        receive(&mut replacement),
        TaskCompletion::Cancelled
    ));
    assert!(matches!(receive(&mut unrelated), TaskCompletion::Cancelled));
    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.cancelled_total, 4);
    assert!(metrics.recent_terminal.iter().any(|sample| {
        sample.cancellation.as_ref().is_some_and(|cancellation| {
            cancellation.reason == super::TaskCancellationReason::RuntimeCancellation
        })
    }));
    tasks.shutdown_and_wait();
}

#[test]
fn explicit_handle_cancellation_is_observable() {
    let tasks = TaskRuntime::new();
    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut handle = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(())
    });
    while !started.load(Ordering::Acquire) {
        thread::yield_now();
    }
    handle.cancel();
    assert!(matches!(receive(&mut handle), TaskCompletion::Cancelled));
    assert!(
        tasks
            .metrics_snapshot()
            .recent_terminal
            .iter()
            .any(|sample| {
                sample.cancellation.as_ref().is_some_and(|cancellation| {
                    cancellation.reason == super::TaskCancellationReason::ExplicitHandle
                })
            })
    );
    tasks.shutdown_and_wait();
}

#[test]
fn metrics_distinguish_failure_panic_and_shutdown_queue_drain() {
    let tasks = TaskRuntime::new();
    let mut failed = tasks.spawn(|_context| Err::<(), _>("failure".to_string()));
    let mut panicked = tasks.spawn(|_context| -> Result<(), String> { panic!("panic") });
    assert!(matches!(receive(&mut failed), TaskCompletion::Failed(_)));
    assert!(matches!(receive(&mut panicked), TaskCompletion::Failed(_)));

    let started = Arc::new(AtomicBool::new(false));
    let started_task = Arc::clone(&started);
    let mut active = tasks.spawn(move |context| {
        started_task.store(true, Ordering::Release);
        while !context.cancellation.is_cancelled() {
            thread::yield_now();
        }
        Ok(())
    });
    while !started.load(Ordering::Acquire) {
        thread::yield_now();
    }
    let mut drained = tasks.spawn(|_context| Ok(()));
    tasks.shutdown_and_wait();
    assert!(matches!(receive(&mut active), TaskCompletion::Cancelled));
    assert!(matches!(receive(&mut drained), TaskCompletion::Cancelled));

    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.failed_total, 1);
    assert_eq!(metrics.panicked_total, 1);
    assert!(metrics.recent_terminal.iter().any(|sample| {
        sample.cancellation.as_ref().is_some_and(|cancellation| {
            cancellation.reason == super::TaskCancellationReason::ShutdownQueueDrain
        })
    }));
    assert_eq!(metrics.worker_joined_total, 1);
}

#[test]
fn cancellation_after_task_body_before_publication_wins_terminal_delivery() {
    let tasks = TaskRuntime::new();
    let gate = Arc::new(super::TaskPublicationGate::new());
    tasks.install_publication_gate(Arc::clone(&gate));
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(19)))
        .for_task(TaskId(9), 5);
    let mut handle = starter.spawn_latest_with_test_snapshot_tagged(
        "items",
        serde_json::Value::Null,
        super::TaskTags::new("picker", "items"),
        |_context| Ok(()),
    );

    // The task body has returned and the worker cannot publish until this
    // test has requested cancellation.
    gate.entered.wait();
    handle.cancel();
    gate.release.wait();

    assert!(matches!(receive(&mut handle), TaskCompletion::Cancelled));
    let event = (0..200)
        .find_map(|_| {
            let event = tasks.drain_events().into_iter().next();
            if event.is_none() {
                thread::sleep(Duration::from_millis(1));
            }
            event
        })
        .expect("cancelled correlated task did not emit an event");
    assert_eq!(event.outcome, TaskOutcome::Cancelled);
    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.cancelled_total, 1);
    assert_eq!(metrics.recent_terminal.len(), 1);
    assert_eq!(
        metrics.recent_terminal[0].outcome,
        super::TaskTerminalOutcome::Cancelled
    );
    tasks.shutdown_and_wait();
}

#[test]
fn metrics_bound_terminal_retention_and_preserve_correlated_delivery() {
    let tasks = TaskRuntime::new();
    for _ in 0..super::RECENT_TERMINAL_LIMIT + 1 {
        let mut handle = tasks.spawn(|_context| Ok(()));
        assert!(matches!(
            receive(&mut handle),
            TaskCompletion::Completed(())
        ));
    }
    let starter = MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(18)))
        .for_task(TaskId(8), 4);
    let mut handle = starter.spawn_latest_with_test_snapshot_tagged(
        "items",
        serde_json::Value::Null,
        super::TaskTags::new("picker", "items"),
        |_context| Ok(()),
    );
    assert!(matches!(
        receive(&mut handle),
        TaskCompletion::Completed(())
    ));
    let metrics = tasks.metrics_snapshot();
    assert_eq!(metrics.recent_terminal.len(), super::RECENT_TERMINAL_LIMIT);

    let event = (0..200)
        .find_map(|_| {
            let event = tasks.drain_events().into_iter().next();
            if event.is_none() {
                thread::sleep(Duration::from_millis(1));
            }
            event
        })
        .expect("correlated task did not emit after completion");
    assert_eq!(event.instance, ViewInstanceId(18));
    assert_eq!(event.task, TaskId(8));
    assert_eq!(event.generation, 4);
    assert_eq!(
        event.outcome,
        TaskOutcome::Completed(serde_json::Value::Null)
    );
    tasks.shutdown_and_wait();
}

mod preview_lane_tests {
    use super::*;
    #[test]
    fn preview_overflow_preserves_other_mount_and_combined_queue_metrics() {
        let tasks = TaskRuntime::new();
        let mount = |id| {
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(crate::input::ViewMountId(id)))
        };
        let first = mount(911);
        let second = mount(912);
        let third = mount(913);
        let (started_tx, started_rx) = sync_channel(2);
        let blocker = |tx: std::sync::mpsc::SyncSender<()>| {
            move |context: TaskContext| {
                tx.send(()).unwrap();
                while !context.cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(())
            }
        };
        let active = first.for_preview().spawn_latest_tagged(
            "preview",
            TaskTags::new("picker", "preview"),
            blocker(started_tx.clone()),
        );
        let serial = first.spawn_latest_tagged(
            "items",
            TaskTags::new("picker", "items"),
            blocker(started_tx),
        );
        for _ in 0..2 {
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        let items_pending =
            second.spawn_latest_tagged("items", TaskTags::new("picker", "items"), |_| Ok(3));
        let pending = second.for_preview().spawn_latest_tagged(
            "preview",
            TaskTags::new("picker", "preview"),
            |_| Ok(1),
        );
        let rejected: TaskHandle<()> = third
            .for_task(TaskId(2), 9)
            .for_preview()
            .spawn_latest_tagged("preview", TaskTags::new("picker", "preview"), |_| {
                panic!("overflow ran")
            });
        assert!(
            matches!(rejected.completion.recv_timeout(Duration::from_secs(1)).unwrap(), TaskCompletion::Failed(message) if message == "preview task queue is full")
        );
        assert!(matches!(
            pending.completion.try_recv(),
            Err(TryRecvError::Empty)
        ));
        assert!(!pending.cancellation.is_cancelled());
        let metrics = tasks.metrics_snapshot();
        assert_eq!(metrics.active_tasks, 2);
        assert_eq!(
            metrics.current_active.as_ref().unwrap().tags,
            TaskTags::new("picker", "items")
        );
        assert_eq!(
            metrics.preview_active.as_ref().unwrap().tags,
            TaskTags::new("picker", "preview")
        );
        assert_eq!(metrics.queue_depth, 2);
        assert_eq!(metrics.queue_high_water, 2);
        assert_eq!(metrics.failed_total, 1);
        assert!(tasks.drain_events().iter().any(|event| event.task == TaskId(2) && event.generation == 9 && matches!(&event.outcome, TaskOutcome::Failed(message) if message == "preview task queue is full")));
        // Replacement remains allowed for the mount that owns the pending slot.
        let replacement = second.for_preview().spawn_latest_tagged(
            "preview",
            TaskTags::new("picker", "preview"),
            |_| Ok(2),
        );
        assert!(matches!(
            pending
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            TaskCompletion::Cancelled
        ));
        assert_eq!(tasks.metrics_snapshot().queue_depth, 2);
        first.cancel_all();
        for handle in [active, serial] {
            assert!(matches!(
                handle
                    .completion
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap(),
                TaskCompletion::Cancelled
            ));
        }
        assert!(matches!(
            replacement
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            TaskCompletion::Completed(2)
        ));
        assert!(matches!(
            items_pending
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            TaskCompletion::Completed(3)
        ));
        tasks.shutdown_and_wait();
        assert_eq!(tasks.metrics_snapshot().queue_depth, 0);
    }

    #[test]
    fn shutdown_cancels_preview_before_waiting_for_serial_teardown() {
        let tasks = TaskRuntime::new();
        let starter = MountTaskStarter::from_lease(
            &tasks,
            MountTaskLease::new(crate::input::ViewMountId(914)),
        );
        let (started_tx, started_rx) = sync_channel(2);
        let (preview_cancelled_tx, preview_cancelled_rx) = sync_channel(1);
        let (observed_tx, observed_rx) = sync_channel(1);
        let preview_started = started_tx.clone();
        let preview = starter.for_preview().spawn_latest_tagged(
            "preview",
            TaskTags::new("picker", "preview"),
            move |context| {
                preview_started.send(()).unwrap();
                while !context.cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                preview_cancelled_tx.send(()).unwrap();
                Ok(())
            },
        );
        let serial = starter.spawn_latest_tagged(
            "items",
            TaskTags::new("picker", "items"),
            move |context| {
                started_tx.send(()).unwrap();
                while !context.cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                let cancelled_before_join = preview_cancelled_rx
                    .recv_timeout(Duration::from_secs(1))
                    .is_ok();
                observed_tx.send(cancelled_before_join).unwrap();
                Ok(())
            },
        );
        for _ in 0..2 {
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        tasks.shutdown_and_wait();
        assert!(
            observed_rx.try_recv().unwrap(),
            "preview was not cancelled during serial teardown"
        );
        for handle in [serial, preview] {
            assert!(matches!(
                handle.completion.try_recv(),
                Ok(TaskCompletion::Cancelled)
            ));
        }
        assert_eq!(tasks.metrics_snapshot().worker_joined_total, 2);
    }

    #[test]
    fn slow_preview_does_not_block_items_and_mount_shutdown_reaps_both_workers() {
        let tasks = TaskRuntime::new();
        let starter = MountTaskStarter::from_lease(
            &tasks,
            MountTaskLease::new(crate::input::ViewMountId(910)),
        );
        let (started_tx, started_rx) = sync_channel(1);
        let mut preview = starter
            .for_task(TaskId(2), 7)
            .for_preview()
            .spawn_latest_tagged(
                "preview",
                TaskTags::new("picker", "preview"),
                move |context| {
                    started_tx.send(()).unwrap();
                    while !context.cancellation.is_cancelled() {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                },
            );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let items = starter.for_task(TaskId(1), 3).spawn_latest_tagged(
            "items",
            TaskTags::new("picker", "items"),
            |_| Ok(42),
        );
        assert!(matches!(
            items
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            TaskCompletion::Completed(42)
        ));
        assert!(matches!(preview.try_recv(), Err(TryRecvError::Empty)));
        starter.cancel_all();
        assert!(matches!(
            preview
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap(),
            TaskCompletion::Cancelled
        ));
        tasks.shutdown_and_wait();
        let events = tasks.drain_events();
        assert!(
            events
                .iter()
                .any(|e| e.task == TaskId(1) && e.generation == 3)
        );
        assert!(
            events
                .iter()
                .any(|e| e.task == TaskId(2) && e.generation == 7)
        );
        let metrics = tasks.metrics_snapshot();
        assert_eq!(metrics.worker_started_total, 2);
        assert_eq!(metrics.worker_joined_total, 2);
        assert!(!tasks.has_active_tasks());
        let mut rejected = starter.for_preview().spawn_latest_tagged(
            "preview",
            TaskTags::new("picker", "preview"),
            |_| Ok(()),
        );
        assert!(matches!(rejected.try_recv(), Ok(TaskCompletion::Cancelled)));
    }
}
