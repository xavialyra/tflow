use crate::lifecycle::CancellationToken;
use serde_json::Value;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub(crate) enum TaskCompletion<T> {
    Completed(T),
    Failed(String),
    Cancelled,
}

/// A serialized background task runtime.
///
/// Task closures must observe `TaskContext::cancellation` at bounded I/O and
/// computation points. `shutdown_and_wait` cancels queued and active tasks,
/// then joins the worker, so a closure that ignores cancellation can delay
/// shutdown indefinitely.
#[derive(Clone)]
pub(crate) struct TaskRuntime {
    owner: Arc<TaskRuntimeOwner>,
}

struct TaskRuntimeOwner {
    registry: Arc<TaskRegistry>,
    events: Arc<Mutex<VecDeque<crate::view::TaskEvent>>>,
}

/// An inert mount identity handed to registration-owned setup code. It has no
/// scheduler or TaskRuntime and therefore cannot start work during preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MountTaskLease {
    mount_id: crate::input::ViewMountId,
}

impl MountTaskLease {
    pub(crate) fn new(mount_id: crate::input::ViewMountId) -> Self {
        Self { mount_id }
    }

    pub(crate) fn mount_id(self) -> crate::input::ViewMountId {
        self.mount_id
    }
}

/// Host-owned authority created only after the Host state has committed. A
/// prepared capability job must receive this value before it can affect the
/// scheduler.
#[derive(Clone)]
pub(crate) struct MountTaskStarter {
    runtime: TaskRuntime,
    mount_id: crate::input::ViewMountId,
    lane_prefix: String,
    correlation: Option<(crate::view::TaskId, u64)>,
}

impl MountTaskStarter {
    pub(crate) fn from_lease(runtime: &TaskRuntime, lease: MountTaskLease) -> Self {
        let mount_id = lease.mount_id();
        Self {
            runtime: runtime.clone(),
            mount_id,
            lane_prefix: format!("mount-{}", mount_id.0),
            correlation: None,
        }
    }

    pub(crate) fn mount_id(&self) -> crate::input::ViewMountId {
        self.mount_id
    }

    pub(crate) fn for_task(&self, task: crate::view::TaskId, generation: u64) -> Self {
        let mut starter = self.clone();
        starter.correlation = Some((task, generation));
        starter
    }

    pub(crate) fn cancel_all(&self) {
        self.runtime
            .cancel_lane_prefix(&format!("{}:", self.lane_prefix));
    }

    #[cfg(test)]
    pub(crate) fn ensure_mount(&self, mount_id: crate::input::ViewMountId) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mount_id() == mount_id,
            "task starter belongs to mount {:?}, requested mount {:?}",
            self.mount_id(),
            mount_id
        );
        Ok(())
    }

    pub(crate) fn spawn_latest_with_snapshot<T, F>(
        &self,
        lane: impl AsRef<str>,
        runtime_snapshot: Value,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.runtime.spawn_latest_with_snapshot_and_correlation(
            format!("{}:{}", self.lane_prefix, lane.as_ref()),
            runtime_snapshot,
            self.correlation.map(|(task, generation)| {
                (
                    crate::view::ViewInstanceId(self.mount_id.0),
                    task,
                    generation,
                )
            }),
            task,
        )
    }
}

pub(crate) struct TaskHandle<T> {
    cancellation: CancellationToken,
    completion: Receiver<TaskCompletion<T>>,
}

pub(crate) struct TaskContext {
    pub(crate) runtime: Value,
    /// Cooperative cancellation requested by the task handle or runtime.
    pub(crate) cancellation: CancellationToken,
}

type TaskJob = Box<dyn FnOnce(Value, CancellationToken) + Send + 'static>;

struct Job {
    lane: Option<String>,
    runtime: Value,
    cancellation: CancellationToken,
    execute: TaskJob,
}

struct ActiveJob {
    lane: Option<String>,
    cancellation: CancellationToken,
}

struct RegistryState {
    active: Option<ActiveJob>,
    pending: VecDeque<Job>,
    closed: bool,
}

struct WorkerSlot {
    handle: Option<thread::JoinHandle<()>>,
    thread_id: Option<thread::ThreadId>,
}

struct TaskRegistry {
    state: Mutex<RegistryState>,
    ready: Condvar,
    worker: Mutex<WorkerSlot>,
}

impl TaskRuntime {
    pub(crate) fn new() -> Self {
        Self {
            owner: Arc::new(TaskRuntimeOwner {
                registry: Arc::new(TaskRegistry {
                    state: Mutex::new(RegistryState {
                        active: None,
                        pending: VecDeque::new(),
                        closed: false,
                    }),
                    ready: Condvar::new(),
                    worker: Mutex::new(WorkerSlot {
                        handle: None,
                        thread_id: None,
                    }),
                }),
                events: Arc::new(Mutex::new(VecDeque::new())),
            }),
        }
    }

    /// Submit an ordinary serialized task.
    #[cfg(test)]
    pub(crate) fn spawn<T, F>(&self, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with(task, None)
    }

    /// Submit a task that replaces active and queued work in `lane`.
    #[cfg(test)]
    pub(crate) fn spawn_latest<T, F>(&self, lane: impl Into<String>, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with(task, Some(lane.into()))
    }

    /// Submit replacing work with an already committed runtime snapshot.
    #[cfg(test)]
    pub(crate) fn spawn_latest_with_snapshot<T, F>(
        &self,
        lane: impl Into<String>,
        runtime_snapshot: Value,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_latest_with_snapshot_and_correlation(lane, runtime_snapshot, None, task)
    }

    fn spawn_latest_with_snapshot_and_correlation<T, F>(
        &self,
        lane: impl Into<String>,
        runtime_snapshot: Value,
        correlation: Option<(crate::view::ViewInstanceId, crate::view::TaskId, u64)>,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(task, Some(lane.into()), runtime_snapshot, correlation)
    }

    #[cfg(test)]
    fn spawn_with<T, F>(&self, task: F, replace_lane: Option<String>) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(task, replace_lane, Value::Null, None)
    }

    fn spawn_with_snapshot<T, F>(
        &self,
        task: F,
        replace_lane: Option<String>,
        runtime_snapshot: Value,
        correlation: Option<(crate::view::ViewInstanceId, crate::view::TaskId, u64)>,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        let cancellation = CancellationToken::new();
        let (completion, receiver) = sync_channel(1);
        let events = Arc::clone(&self.owner.events);
        let execute = Box::new(move |runtime, cancellation: CancellationToken| {
            let result = if cancellation.is_cancelled() {
                TaskCompletion::Cancelled
            } else {
                match catch_unwind(AssertUnwindSafe(|| {
                    task(TaskContext {
                        runtime,
                        cancellation: cancellation.clone(),
                    })
                })) {
                    Ok(Ok(_value)) if cancellation.is_cancelled() => TaskCompletion::Cancelled,
                    Ok(Ok(value)) => TaskCompletion::Completed(value),
                    Ok(Err(_error)) if cancellation.is_cancelled() => TaskCompletion::Cancelled,
                    Ok(Err(error)) => TaskCompletion::Failed(error),
                    Err(_) if cancellation.is_cancelled() => TaskCompletion::Cancelled,
                    Err(_) => TaskCompletion::Failed("task panicked".to_string()),
                }
            };
            let outcome = match &result {
                TaskCompletion::Completed(_) => {
                    crate::view::TaskOutcome::Completed(serde_json::Value::Null)
                }
                TaskCompletion::Failed(message) => {
                    crate::view::TaskOutcome::Failed(message.clone())
                }
                TaskCompletion::Cancelled => crate::view::TaskOutcome::Cancelled,
            };
            let _ = completion.send(result);
            if let Some((instance, task, generation)) = correlation {
                events
                    .lock()
                    .expect("task event queue was poisoned")
                    .push_back(crate::view::TaskEvent {
                        instance,
                        task,
                        generation,
                        outcome,
                    });
            }
        });
        self.owner.registry.submit(
            Job {
                lane: replace_lane.clone(),
                runtime: runtime_snapshot,
                cancellation: cancellation.clone(),
                execute,
            },
            replace_lane.as_deref(),
        );
        TaskHandle {
            cancellation,
            completion: receiver,
        }
    }

    pub(crate) fn drain_events(&self) -> Vec<crate::view::TaskEvent> {
        self.owner
            .events
            .lock()
            .expect("task event queue was poisoned")
            .drain(..)
            .collect()
    }

    pub(crate) fn has_active_tasks(&self) -> bool {
        let state = self
            .owner
            .registry
            .state
            .lock()
            .expect("task registry state was poisoned");
        state.active.is_some() || !state.pending.is_empty()
    }

    pub(crate) fn has_pending_events(&self) -> bool {
        !self
            .owner
            .events
            .lock()
            .expect("task event queue was poisoned")
            .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn cancel_all(&self) {
        self.owner.registry.cancel_all();
    }

    fn cancel_lane_prefix(&self, prefix: &str) {
        self.owner.registry.cancel_lane_prefix(prefix);
    }

    /// Cancel all work and wait for the worker to exit.
    #[cfg(test)]
    pub(crate) fn shutdown_and_wait(&self) {
        self.owner.registry.shutdown_and_wait();
    }
}

impl Drop for TaskRuntimeOwner {
    fn drop(&mut self) {
        self.registry.shutdown_and_wait();
    }
}

impl TaskRegistry {
    fn submit(self: &Arc<Self>, job: Job, replace_lane: Option<&str>) {
        let mut state = self.state.lock().expect("task registry state was poisoned");
        if state.closed {
            drop(state);
            let Job {
                runtime,
                cancellation,
                execute,
                ..
            } = job;
            cancellation.cancel();
            execute(runtime, cancellation);
            return;
        }
        let mut replaced = Vec::new();
        if let Some(replace_lane) = replace_lane {
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.lane.as_deref() == Some(replace_lane))
                && let Some(active) = &state.active
            {
                active.cancellation.cancel();
            }
            let mut retained = VecDeque::new();
            for pending in state.pending.drain(..) {
                if pending.lane.as_deref() == Some(replace_lane) {
                    pending.cancellation.cancel();
                    replaced.push(pending);
                } else {
                    retained.push_back(pending);
                }
            }
            state.pending = retained;
        }
        state.pending.push_back(job);
        self.ensure_worker();
        drop(state);
        for Job {
            runtime,
            cancellation,
            execute,
            ..
        } in replaced
        {
            execute(runtime, cancellation);
        }
        self.ready.notify_one();
    }

    #[cfg(test)]
    fn cancel_all(&self) {
        let state = self.state.lock().expect("task registry state was poisoned");
        if let Some(active) = &state.active {
            active.cancellation.cancel();
        }
        for pending in &state.pending {
            pending.cancellation.cancel();
        }
    }

    fn cancel_lane_prefix(&self, prefix: &str) {
        let cancelled = {
            let mut state = self.state.lock().expect("task registry state was poisoned");
            if state.active.as_ref().is_some_and(|active| {
                active
                    .lane
                    .as_deref()
                    .is_some_and(|lane| lane.starts_with(prefix))
            }) && let Some(active) = &state.active
            {
                active.cancellation.cancel();
            }
            let mut retained = VecDeque::new();
            let mut cancelled = Vec::new();
            for pending in state.pending.drain(..) {
                if pending
                    .lane
                    .as_deref()
                    .is_some_and(|lane| lane.starts_with(prefix))
                {
                    pending.cancellation.cancel();
                    cancelled.push(pending);
                } else {
                    retained.push_back(pending);
                }
            }
            state.pending = retained;
            cancelled
        };
        for Job {
            runtime,
            cancellation,
            execute,
            ..
        } in cancelled
        {
            execute(runtime, cancellation);
        }
    }

    fn shutdown_and_wait(&self) {
        let pending = {
            let mut state = self.state.lock().expect("task registry state was poisoned");
            state.closed = true;
            if let Some(active) = &state.active {
                active.cancellation.cancel();
            }
            let pending = state.pending.drain(..).collect::<Vec<_>>();
            for pending_job in &pending {
                pending_job.cancellation.cancel();
            }
            pending
        };
        for Job {
            runtime,
            cancellation,
            execute,
            ..
        } in pending
        {
            execute(runtime, cancellation);
        }
        self.ready.notify_all();

        let worker = {
            let mut slot = self
                .worker
                .lock()
                .expect("task registry worker was poisoned");
            if slot.thread_id == Some(thread::current().id()) {
                return;
            }
            let Some(handle) = slot.handle.take() else {
                return;
            };
            handle
        };

        let _ = worker.join();
        self.worker
            .lock()
            .expect("task registry worker was poisoned")
            .thread_id = None;
    }

    fn ensure_worker(self: &Arc<Self>) {
        let mut slot = self
            .worker
            .lock()
            .expect("task registry worker was poisoned");
        if slot.handle.is_some() {
            return;
        }
        let registry = Arc::clone(self);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let worker = thread::spawn(move || {
            let id = thread::current().id();
            started_tx
                .send(id)
                .expect("task worker startup receiver disappeared");
            ready_rx
                .recv()
                .expect("task worker startup was not acknowledged");
            worker_loop(registry);
        });
        let thread_id = started_rx.recv().expect("task worker failed to start");
        slot.thread_id = Some(thread_id);
        slot.handle = Some(worker);
        ready_tx
            .send(())
            .expect("task worker startup sender disappeared");
    }
}

fn worker_loop(registry: Arc<TaskRegistry>) {
    loop {
        let job = {
            let mut state = registry
                .state
                .lock()
                .expect("task registry state was poisoned");
            while state.pending.is_empty() && !state.closed {
                state = registry
                    .ready
                    .wait(state)
                    .expect("task registry state was poisoned");
            }
            if state.pending.is_empty() && state.closed {
                return;
            }
            let job = state.pending.pop_front().expect("pending task disappeared");
            state.active = Some(ActiveJob {
                lane: job.lane.clone(),
                cancellation: job.cancellation.clone(),
            });
            job
        };

        let Job {
            runtime,
            cancellation,
            execute,
            ..
        } = job;
        execute(runtime, cancellation);
        registry
            .state
            .lock()
            .expect("task registry state was poisoned")
            .active = None;
    }
}

impl<T> TaskHandle<T> {
    pub(crate) fn try_recv(&mut self) -> std::result::Result<TaskCompletion<T>, TryRecvError> {
        self.completion.try_recv()
    }
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::{MountTaskLease, MountTaskStarter, TaskCompletion, TaskRuntime};
    use crate::input::ViewMountId;
    use crate::view::{TaskId, TaskOutcome, ViewInstanceId};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

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
            starter.spawn_latest_with_snapshot("items", json!({}), |_context| Ok("done"));
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
        let mut handle =
            tasks.spawn_latest_with_snapshot("snapshot", snapshot, |context| {
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

        let mut handle = tasks.spawn_latest_with_snapshot("latest", explicit, |context| {
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
        let mut first = starter.spawn_latest_with_snapshot("refresh", json!({}), move |context| {
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
            starter.spawn_latest_with_snapshot("refresh", json!({}), |_context| Ok(2));

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
        let first_starter =
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(91)));
        let second_starter =
            MountTaskStarter::from_lease(&tasks, MountTaskLease::new(ViewMountId(92)));
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let started_task = Arc::clone(&started);
        let release_task = Arc::clone(&release);
        let mut first =
            first_starter.spawn_latest_with_snapshot("refresh", json!({}), move |context| {
                started_task.store(true, Ordering::Release);
                while !release_task.load(Ordering::Acquire) && !context.cancellation.is_cancelled()
                {
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
            second_starter.spawn_latest_with_snapshot("refresh", json!({}), |_context| Ok(2));
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
            first.spawn_latest_with_snapshot("work", serde_json::Value::Null, move |context| {
                started_task.store(true, Ordering::Release);
                while !context.cancellation.is_cancelled() {
                    thread::yield_now();
                }
                Ok(1)
            });
        let mut second_handle =
            second.spawn_latest_with_snapshot("work", serde_json::Value::Null, |_context| Ok(2));
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
}
