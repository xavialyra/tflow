use crate::lifecycle::CancellationToken;
use crate::protocol::contracts::{TaskEvent, TaskId, TaskOutcome, ViewInstanceId};
use serde_json::Value;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) enum TaskCompletion<T> {
    Completed(T),
    Failed(String),
    Cancelled,
}

/// Low-cardinality labels supplied by task clients. These identify task kinds,
/// not individual Views, queries, or workflow values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskTags {
    pub(crate) engine: &'static str,
    pub(crate) task_class: &'static str,
}

impl TaskTags {
    pub(crate) const fn new(engine: &'static str, task_class: &'static str) -> Self {
        Self { engine, task_class }
    }

    #[cfg(test)]
    const DEFAULT: Self = Self::new("runtime", "task");
}

/// An elapsed timestamp relative to this `TaskRuntime`'s construction.
/// It is monotonic runtime-local time, never a wall-clock value or `Instant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TaskElapsed(pub(crate) Duration);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskTerminalOutcome {
    Completed,
    Failed,
    Panicked,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaskCancellationReason {
    LatestWinsPendingReplacement,
    LatestWinsActiveReplacement,
    MountClosed,
    #[cfg(test)]
    ExplicitHandle,
    DroppedHandle,
    #[cfg(test)]
    RuntimeCancellation,
    ShutdownActive,
    ShutdownQueueDrain,
    SubmissionAfterShutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskCancellationSample {
    pub(crate) reason: TaskCancellationReason,
    pub(crate) requested_at: TaskElapsed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskTerminalSample {
    pub(crate) tags: TaskTags,
    pub(crate) lane: Option<String>,
    pub(crate) submitted_at: TaskElapsed,
    pub(crate) started_at: Option<TaskElapsed>,
    pub(crate) cancellation: Option<TaskCancellationSample>,
    pub(crate) terminal_at: TaskElapsed,
    pub(crate) outcome: TaskTerminalOutcome,
    /// Set only when task code positively knows its managed child process has
    /// been waited/reaped. `None` means no child process was involved or its
    /// reap point was not observable; no timestamp is inferred from completion.
    pub(crate) process_reaped_at: Option<TaskElapsed>,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskActiveSnapshot {
    pub(crate) tags: TaskTags,
    pub(crate) lane: Option<String>,
    pub(crate) submitted_at: TaskElapsed,
    pub(crate) started_at: Option<TaskElapsed>,
    pub(crate) cancellation: Option<TaskCancellationSample>,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskRuntimeMetricsSnapshot {
    pub(crate) now: TaskElapsed,
    pub(crate) queue_depth: usize,
    pub(crate) active_tasks: usize,
    /// Active task on the default serialized worker.
    pub(crate) current_active: Option<TaskActiveSnapshot>,
    /// Active task on the independently scheduled preview worker.
    pub(crate) preview_active: Option<TaskActiveSnapshot>,
    pub(crate) queue_high_water: usize,
    pub(crate) submitted_total: u64,
    pub(crate) started_total: u64,
    pub(crate) completed_total: u64,
    pub(crate) failed_total: u64,
    pub(crate) panicked_total: u64,
    pub(crate) cancelled_total: u64,
    pub(crate) submission_after_shutdown_total: u64,
    pub(crate) worker_started_total: u64,
    pub(crate) worker_joined_total: u64,
    pub(crate) recent_terminal: Vec<TaskTerminalSample>,
}

const RECENT_TERMINAL_LIMIT: usize = 256;

trait TaskClock: Send + Sync {
    fn now(&self) -> Duration;
}

struct MonotonicTaskClock {
    origin: Instant,
}

impl TaskClock for MonotonicTaskClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

struct TaskRecord {
    tags: TaskTags,
    lane: Option<String>,
    submitted_at: TaskElapsed,
    started_at: Option<TaskElapsed>,
    cancellation: Option<TaskCancellationSample>,
    process_reaped_at: Option<TaskElapsed>,
    terminal: bool,
}

struct TaskMetricsState {
    queue_depths: [usize; 2],
    queue_high_water: usize,
    submitted_total: u64,
    started_total: u64,
    completed_total: u64,
    failed_total: u64,
    panicked_total: u64,
    cancelled_total: u64,
    submission_after_shutdown_total: u64,
    worker_started_total: u64,
    worker_joined_total: u64,
    recent_terminal: VecDeque<TaskTerminalSample>,
}

struct TaskMetrics {
    clock: Arc<dyn TaskClock>,
    state: Mutex<TaskMetricsState>,
}

/// A background runtime with a serialized default worker and an isolated,
/// bounded preview worker using the same lifecycle and event machinery.
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
    preview_registry: Arc<TaskRegistry>,
    events: Arc<Mutex<VecDeque<TaskEvent>>>,
    #[cfg(test)]
    publication_gate: Arc<Mutex<Option<Arc<TaskPublicationGate>>>>,
}

#[cfg(test)]
struct TaskPublicationGate {
    entered: std::sync::Barrier,
    release: std::sync::Barrier,
}

#[cfg(test)]
impl TaskPublicationGate {
    fn new() -> Self {
        Self {
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        }
    }
}

impl TaskMetrics {
    fn now(&self) -> TaskElapsed {
        TaskElapsed(self.clock.now())
    }

    fn submit(&self, tags: TaskTags, lane: Option<String>) -> Arc<Mutex<TaskRecord>> {
        let record = Arc::new(Mutex::new(TaskRecord {
            tags,
            lane,
            submitted_at: self.now(),
            started_at: None,
            cancellation: None,
            process_reaped_at: None,
            terminal: false,
        }));
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.submitted_total = state.submitted_total.saturating_add(1);
        record
    }

    fn queued(&self, class: TaskExecutionClass, depth: usize) {
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.queue_depths[usize::from(matches!(class, TaskExecutionClass::Preview))] = depth;
        state.queue_high_water = state.queue_high_water.max(state.queue_depths.iter().sum());
    }

    fn started(&self, record: &Arc<Mutex<TaskRecord>>) {
        let now = self.now();
        let mut record = record.lock().expect("task record was poisoned");
        if record.started_at.is_none() {
            record.started_at = Some(now);
            let mut state = self.state.lock().expect("task metrics state was poisoned");
            state.started_total = state.started_total.saturating_add(1);
        }
    }

    fn cancel(&self, record: &Arc<Mutex<TaskRecord>>, reason: TaskCancellationReason) {
        let now = self.now();
        let mut record = record.lock().expect("task record was poisoned");
        if !record.terminal && record.cancellation.is_none() {
            record.cancellation = Some(TaskCancellationSample {
                reason,
                requested_at: now,
            });
        }
    }

    fn process_reaped(&self, record: &Arc<Mutex<TaskRecord>>) {
        let now = self.now();
        let mut record = record.lock().expect("task record was poisoned");
        if !record.terminal && record.process_reaped_at.is_none() {
            record.process_reaped_at = Some(now);
        }
    }

    /// Seal terminal metrics and resolve a cancellation that arrived before
    /// publication. The record remains locked until its terminal sample is in
    /// the metrics store, so cancellation cannot race a published result.
    fn terminalize(
        &self,
        record: &Arc<Mutex<TaskRecord>>,
        candidate: TaskTerminalOutcome,
    ) -> TaskTerminalOutcome {
        let terminal_at = self.now();
        let mut record = record.lock().expect("task record was poisoned");
        if record.terminal {
            return candidate;
        }
        let outcome = if record.cancellation.is_some() {
            TaskTerminalOutcome::Cancelled
        } else {
            candidate
        };
        record.terminal = true;
        let sample = TaskTerminalSample {
            tags: record.tags,
            lane: record.lane.clone(),
            submitted_at: record.submitted_at,
            started_at: record.started_at,
            cancellation: record.cancellation.clone(),
            terminal_at,
            outcome,
            process_reaped_at: record.process_reaped_at,
        };
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        match outcome {
            TaskTerminalOutcome::Completed => {
                state.completed_total = state.completed_total.saturating_add(1)
            }
            TaskTerminalOutcome::Failed => {
                state.failed_total = state.failed_total.saturating_add(1)
            }
            TaskTerminalOutcome::Panicked => {
                state.panicked_total = state.panicked_total.saturating_add(1)
            }
            TaskTerminalOutcome::Cancelled => {
                state.cancelled_total = state.cancelled_total.saturating_add(1)
            }
        }
        if state.recent_terminal.len() == RECENT_TERMINAL_LIMIT {
            state.recent_terminal.pop_front();
        }
        state.recent_terminal.push_back(sample);
        outcome
    }

    fn submission_after_shutdown(&self) {
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.submission_after_shutdown_total =
            state.submission_after_shutdown_total.saturating_add(1);
    }

    fn worker_started(&self) {
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.worker_started_total = state.worker_started_total.saturating_add(1);
    }

    fn worker_joined(&self) {
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.worker_joined_total = state.worker_joined_total.saturating_add(1);
    }
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
#[derive(Clone, Copy, Default)]
pub(crate) enum TaskExecutionClass {
    #[default]
    Serial,
    Preview,
}

#[derive(Clone)]
pub(crate) struct MountTaskStarter {
    runtime: TaskRuntime,
    mount_id: crate::input::ViewMountId,
    lane_prefix: String,
    correlation: Option<(TaskId, u64)>,
    execution_class: TaskExecutionClass,
}

impl MountTaskStarter {
    pub(crate) fn from_lease(runtime: &TaskRuntime, lease: MountTaskLease) -> Self {
        let mount_id = lease.mount_id();
        Self {
            runtime: runtime.clone(),
            mount_id,
            lane_prefix: format!("mount-{}", mount_id.0),
            correlation: None,
            execution_class: TaskExecutionClass::Serial,
        }
    }

    pub(crate) fn mount_id(&self) -> crate::input::ViewMountId {
        self.mount_id
    }

    pub(crate) fn for_task(&self, task: TaskId, generation: u64) -> Self {
        let mut starter = self.clone();
        starter.correlation = Some((task, generation));
        starter
    }

    pub(crate) fn for_preview(&self) -> Self {
        let mut starter = self.clone();
        starter.execution_class = TaskExecutionClass::Preview;
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

    #[cfg(test)]
    pub(crate) fn spawn_latest_with_test_snapshot<T, F>(
        &self,
        lane: impl AsRef<str>,
        runtime_snapshot: Value,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_latest_with_test_snapshot_tagged(lane, runtime_snapshot, TaskTags::DEFAULT, task)
    }

    #[cfg(test)]
    pub(crate) fn spawn_latest_with_test_snapshot_tagged<T, F>(
        &self,
        lane: impl AsRef<str>,
        runtime_snapshot: Value,
        tags: TaskTags,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.runtime
            .spawn_latest_with_test_snapshot_and_correlation(
                format!("{}:{}", self.lane_prefix, lane.as_ref()),
                runtime_snapshot,
                tags,
                self.correlation
                    .map(|(task, generation)| (ViewInstanceId(self.mount_id.0), task, generation)),
                task,
            )
    }

    pub(crate) fn spawn_latest_tagged<T, F>(
        &self,
        lane: impl AsRef<str>,
        tags: TaskTags,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.runtime.spawn_latest_with_correlation(
            format!("{}:{}", self.lane_prefix, lane.as_ref()),
            tags,
            self.execution_class,
            self.correlation
                .map(|(task, generation)| (ViewInstanceId(self.mount_id.0), task, generation)),
            task,
        )
    }
}

pub(crate) struct TaskHandle<T> {
    cancellation: CancellationToken,
    completion: Receiver<TaskCompletion<T>>,
    metrics: Arc<TaskMetrics>,
    record: Arc<Mutex<TaskRecord>>,
}

pub(crate) struct TaskContext {
    #[cfg(test)]
    pub(crate) runtime: Value,
    /// Cooperative cancellation requested by the task handle or runtime.
    pub(crate) cancellation: CancellationToken,
    metrics: Arc<TaskMetrics>,
    record: Arc<Mutex<TaskRecord>>,
}

impl TaskContext {
    /// Record a managed-child reap only when the task's process owner has
    /// positively waited for that child. Task functions without a child leave it unset.
    pub(crate) fn mark_process_reaped(&self) {
        self.metrics.process_reaped(&self.record);
    }
}

type TaskJob = Box<dyn FnOnce(Value, CancellationToken, Option<String>) + Send + 'static>;

struct Job {
    lane: Option<String>,
    runtime: Value,
    cancellation: CancellationToken,
    record: Arc<Mutex<TaskRecord>>,
    execute: TaskJob,
}

struct ActiveJob {
    lane: Option<String>,
    cancellation: CancellationToken,
    record: Arc<Mutex<TaskRecord>>,
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
    metrics: Arc<TaskMetrics>,
    max_pending: Option<usize>,
    execution_class: TaskExecutionClass,
}

impl TaskRuntime {
    pub(crate) fn new() -> Self {
        Self::with_clock(Arc::new(MonotonicTaskClock {
            origin: Instant::now(),
        }))
    }

    fn with_clock(clock: Arc<dyn TaskClock>) -> Self {
        let metrics = Arc::new(TaskMetrics {
            clock,
            state: Mutex::new(TaskMetricsState {
                queue_depths: [0; 2],
                queue_high_water: 0,
                submitted_total: 0,
                started_total: 0,
                completed_total: 0,
                failed_total: 0,
                panicked_total: 0,
                cancelled_total: 0,
                submission_after_shutdown_total: 0,
                worker_started_total: 0,
                worker_joined_total: 0,
                recent_terminal: VecDeque::new(),
            }),
        });
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
                    metrics: Arc::clone(&metrics),
                    max_pending: None,
                    execution_class: TaskExecutionClass::Serial,
                }),
                preview_registry: Arc::new(TaskRegistry {
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
                    metrics,
                    max_pending: Some(1),
                    execution_class: TaskExecutionClass::Preview,
                }),
                events: Arc::new(Mutex::new(VecDeque::new())),
                #[cfg(test)]
                publication_gate: Arc::new(Mutex::new(None)),
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

    /// Test-only hook for exercising explicit task-context snapshots.
    #[cfg(test)]
    pub(crate) fn spawn_latest_with_test_snapshot<T, F>(
        &self,
        lane: impl Into<String>,
        runtime_snapshot: Value,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_latest_with_test_snapshot_and_correlation(
            lane,
            runtime_snapshot,
            TaskTags::DEFAULT,
            None,
            task,
        )
    }

    fn spawn_latest_with_correlation<T, F>(
        &self,
        lane: impl Into<String>,
        tags: TaskTags,
        execution_class: TaskExecutionClass,
        correlation: Option<(ViewInstanceId, TaskId, u64)>,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(
            task,
            Some(lane.into()),
            tags,
            Value::Null,
            correlation,
            execution_class,
        )
    }

    #[cfg(test)]
    fn spawn_latest_with_test_snapshot_and_correlation<T, F>(
        &self,
        lane: impl Into<String>,
        runtime_snapshot: Value,
        tags: TaskTags,
        correlation: Option<(ViewInstanceId, TaskId, u64)>,
        task: F,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(
            task,
            Some(lane.into()),
            tags,
            runtime_snapshot,
            correlation,
            TaskExecutionClass::Serial,
        )
    }

    #[cfg(test)]
    fn spawn_with<T, F>(&self, task: F, replace_lane: Option<String>) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(
            task,
            replace_lane,
            TaskTags::DEFAULT,
            Value::Null,
            None,
            TaskExecutionClass::Serial,
        )
    }

    fn spawn_with_snapshot<T, F>(
        &self,
        task: F,
        replace_lane: Option<String>,
        tags: TaskTags,
        runtime_snapshot: Value,
        correlation: Option<(ViewInstanceId, TaskId, u64)>,
        execution_class: TaskExecutionClass,
    ) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        let cancellation = CancellationToken::new();
        let (completion, receiver) = sync_channel(1);
        let events = Arc::clone(&self.owner.events);
        #[cfg(test)]
        let publication_gate = Arc::clone(&self.owner.publication_gate);
        let metrics = Arc::clone(&self.owner.registry.metrics);
        let record = metrics.submit(tags, replace_lane.clone());
        let execute_metrics = Arc::clone(&metrics);
        let execute_record = Arc::clone(&record);
        let execute = Box::new(
            move |runtime, cancellation: CancellationToken, rejection: Option<String>| {
                let (result, terminal_outcome) = if let Some(error) = rejection {
                    (TaskCompletion::Failed(error), TaskTerminalOutcome::Failed)
                } else if cancellation.is_cancelled() {
                    (TaskCompletion::Cancelled, TaskTerminalOutcome::Cancelled)
                } else {
                    match catch_unwind(AssertUnwindSafe(|| {
                        #[cfg(test)]
                        let task_context = TaskContext {
                            runtime,
                            cancellation: cancellation.clone(),
                            metrics: Arc::clone(&execute_metrics),
                            record: Arc::clone(&execute_record),
                        };
                        #[cfg(not(test))]
                        let task_context = {
                            let _ = runtime;
                            TaskContext {
                                cancellation: cancellation.clone(),
                                metrics: Arc::clone(&execute_metrics),
                                record: Arc::clone(&execute_record),
                            }
                        };
                        task(task_context)
                    })) {
                        Ok(Ok(_value)) if cancellation.is_cancelled() => {
                            (TaskCompletion::Cancelled, TaskTerminalOutcome::Cancelled)
                        }
                        Ok(Ok(value)) => (
                            TaskCompletion::Completed(value),
                            TaskTerminalOutcome::Completed,
                        ),
                        Ok(Err(_error)) if cancellation.is_cancelled() => {
                            (TaskCompletion::Cancelled, TaskTerminalOutcome::Cancelled)
                        }
                        Ok(Err(error)) => {
                            (TaskCompletion::Failed(error), TaskTerminalOutcome::Failed)
                        }
                        Err(_) if cancellation.is_cancelled() => {
                            (TaskCompletion::Cancelled, TaskTerminalOutcome::Cancelled)
                        }
                        Err(_) => (
                            TaskCompletion::Failed("task panicked".to_string()),
                            TaskTerminalOutcome::Panicked,
                        ),
                    }
                };
                #[cfg(test)]
                if let Some(gate) = publication_gate
                    .lock()
                    .expect("task publication gate was poisoned")
                    .as_ref()
                    .cloned()
                {
                    gate.entered.wait();
                    gate.release.wait();
                }
                let terminal_outcome =
                    execute_metrics.terminalize(&execute_record, terminal_outcome);
                let result = if terminal_outcome == TaskTerminalOutcome::Cancelled {
                    TaskCompletion::Cancelled
                } else {
                    result
                };
                let outcome = match &result {
                    TaskCompletion::Completed(_) => TaskOutcome::Completed(serde_json::Value::Null),
                    TaskCompletion::Failed(message) => TaskOutcome::Failed(message.clone()),
                    TaskCompletion::Cancelled => TaskOutcome::Cancelled,
                };
                let _ = completion.send(result);
                if let Some((instance, task, generation)) = correlation {
                    events
                        .lock()
                        .expect("task event queue was poisoned")
                        .push_back(TaskEvent {
                            instance,
                            task,
                            generation,
                            outcome,
                        });
                }
            },
        );
        let registry = match execution_class {
            TaskExecutionClass::Serial => &self.owner.registry,
            TaskExecutionClass::Preview => &self.owner.preview_registry,
        };
        registry.submit(
            Job {
                lane: replace_lane.clone(),
                runtime: runtime_snapshot,
                cancellation: cancellation.clone(),
                record: Arc::clone(&record),
                execute,
            },
            replace_lane.as_deref(),
        );
        TaskHandle {
            cancellation,
            completion: receiver,
            metrics,
            record,
        }
    }

    pub(crate) fn drain_events(&self) -> Vec<TaskEvent> {
        self.owner
            .events
            .lock()
            .expect("task event queue was poisoned")
            .drain(..)
            .collect()
    }

    /// Internal diagnostics observation point. This does not affect task or
    /// event delivery and deliberately has no user-facing presentation API.
    #[cfg(test)]
    pub(crate) fn metrics_snapshot(&self) -> TaskRuntimeMetricsSnapshot {
        let active = {
            let state = self
                .owner
                .registry
                .state
                .lock()
                .expect("task registry state was poisoned");
            state
                .active
                .as_ref()
                .map(|active| Arc::clone(&active.record))
        };
        let preview_active = {
            let state = self
                .owner
                .preview_registry
                .state
                .lock()
                .expect("preview registry poisoned");
            state
                .active
                .as_ref()
                .map(|active| Arc::clone(&active.record))
        };
        let active_tasks = usize::from(active.is_some()) + usize::from(preview_active.is_some());
        let snapshot_active = |record: Arc<Mutex<TaskRecord>>| {
            let record = record.lock().expect("task record was poisoned");
            TaskActiveSnapshot {
                tags: record.tags,
                lane: record.lane.clone(),
                submitted_at: record.submitted_at,
                started_at: record.started_at,
                cancellation: record.cancellation.clone(),
            }
        };
        let current_active = active.map(snapshot_active);
        let preview_active = preview_active.map(snapshot_active);
        let state = self
            .owner
            .registry
            .metrics
            .state
            .lock()
            .expect("task metrics state was poisoned");
        TaskRuntimeMetricsSnapshot {
            now: self.owner.registry.metrics.now(),
            queue_depth: state.queue_depths.iter().sum(),
            active_tasks,
            current_active,
            preview_active,
            queue_high_water: state.queue_high_water,
            submitted_total: state.submitted_total,
            started_total: state.started_total,
            completed_total: state.completed_total,
            failed_total: state.failed_total,
            panicked_total: state.panicked_total,
            cancelled_total: state.cancelled_total,
            submission_after_shutdown_total: state.submission_after_shutdown_total,
            worker_started_total: state.worker_started_total,
            worker_joined_total: state.worker_joined_total,
            recent_terminal: state.recent_terminal.iter().cloned().collect(),
        }
    }

    pub(crate) fn has_active_tasks(&self) -> bool {
        let state = self
            .owner
            .registry
            .state
            .lock()
            .expect("task registry state was poisoned");
        let preview = self
            .owner
            .preview_registry
            .state
            .lock()
            .expect("preview registry poisoned");
        state.active.is_some()
            || !state.pending.is_empty()
            || preview.active.is_some()
            || !preview.pending.is_empty()
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
        self.owner.preview_registry.cancel_all();
    }

    #[cfg(test)]
    fn install_publication_gate(&self, gate: Arc<TaskPublicationGate>) {
        *self
            .owner
            .publication_gate
            .lock()
            .expect("task publication gate was poisoned") = Some(gate);
    }

    fn cancel_lane_prefix(&self, prefix: &str) {
        self.owner.registry.cancel_lane_prefix(prefix);
        self.owner.preview_registry.cancel_lane_prefix(prefix);
    }

    /// Cancel all work and wait for the worker to exit.
    #[cfg(test)]
    pub(crate) fn shutdown_and_wait(&self) {
        self.owner.registry.begin_shutdown();
        self.owner.preview_registry.begin_shutdown();
        self.owner.registry.shutdown_and_wait();
        self.owner.preview_registry.shutdown_and_wait();
    }
}

impl Drop for TaskRuntimeOwner {
    fn drop(&mut self) {
        self.registry.begin_shutdown();
        self.preview_registry.begin_shutdown();
        self.registry.shutdown_and_wait();
        self.preview_registry.shutdown_and_wait();
    }
}

impl TaskRegistry {
    fn submit(self: &Arc<Self>, job: Job, replace_lane: Option<&str>) {
        let mut state = self.state.lock().expect("task registry state was poisoned");
        if state.closed {
            drop(state);
            self.metrics.submission_after_shutdown();
            let Job {
                runtime,
                cancellation,
                record,
                execute,
                ..
            } = job;
            self.metrics
                .cancel(&record, TaskCancellationReason::SubmissionAfterShutdown);
            cancellation.cancel();
            execute(runtime, cancellation, None);
            return;
        }
        // Capacity is shared across mounts, but replacement authority is lane-local.
        // Reject before cancelling active work if another lane owns the pending slot.
        if self
            .max_pending
            .is_some_and(|limit| state.pending.len() >= limit)
            && !state
                .pending
                .iter()
                .any(|pending| replace_lane.is_some() && pending.lane.as_deref() == replace_lane)
        {
            drop(state);
            (job.execute)(
                job.runtime,
                job.cancellation,
                Some("preview task queue is full".into()),
            );
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
                self.metrics.cancel(
                    &active.record,
                    TaskCancellationReason::LatestWinsActiveReplacement,
                );
                active.cancellation.cancel();
            }
            let mut retained = VecDeque::new();
            for pending in state.pending.drain(..) {
                if pending.lane.as_deref() == Some(replace_lane) {
                    self.metrics.cancel(
                        &pending.record,
                        TaskCancellationReason::LatestWinsPendingReplacement,
                    );
                    pending.cancellation.cancel();
                    replaced.push(pending);
                } else {
                    retained.push_back(pending);
                }
            }
            state.pending = retained;
        }
        state.pending.push_back(job);
        self.metrics
            .queued(self.execution_class, state.pending.len());
        self.ensure_worker();
        drop(state);
        for Job {
            runtime,
            cancellation,
            execute,
            ..
        } in replaced
        {
            execute(runtime, cancellation, None);
        }
        self.ready.notify_one();
    }

    #[cfg(test)]
    fn cancel_all(&self) {
        let state = self.state.lock().expect("task registry state was poisoned");
        if let Some(active) = &state.active {
            self.metrics
                .cancel(&active.record, TaskCancellationReason::RuntimeCancellation);
            active.cancellation.cancel();
        }
        for pending in &state.pending {
            self.metrics
                .cancel(&pending.record, TaskCancellationReason::RuntimeCancellation);
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
                self.metrics
                    .cancel(&active.record, TaskCancellationReason::MountClosed);
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
                    self.metrics
                        .cancel(&pending.record, TaskCancellationReason::MountClosed);
                    pending.cancellation.cancel();
                    cancelled.push(pending);
                } else {
                    retained.push_back(pending);
                }
            }
            state.pending = retained;
            self.metrics
                .queued(self.execution_class, state.pending.len());
            cancelled
        };
        for Job {
            runtime,
            cancellation,
            execute,
            ..
        } in cancelled
        {
            execute(runtime, cancellation, None);
        }
    }

    fn begin_shutdown(&self) {
        let pending = {
            let mut state = self.state.lock().expect("task registry state was poisoned");
            state.closed = true;
            if let Some(active) = &state.active {
                self.metrics
                    .cancel(&active.record, TaskCancellationReason::ShutdownActive);
                active.cancellation.cancel();
            }
            let pending = state.pending.drain(..).collect::<Vec<_>>();
            self.metrics.queued(self.execution_class, 0);
            for pending_job in &pending {
                self.metrics.cancel(
                    &pending_job.record,
                    TaskCancellationReason::ShutdownQueueDrain,
                );
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
            execute(runtime, cancellation, None);
        }
        self.ready.notify_all();
    }

    fn shutdown_and_wait(&self) {
        self.begin_shutdown();
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
        self.metrics.worker_joined();
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
        self.metrics.worker_started();
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
            registry
                .metrics
                .queued(registry.execution_class, state.pending.len());
            state.active = Some(ActiveJob {
                lane: job.lane.clone(),
                cancellation: job.cancellation.clone(),
                record: Arc::clone(&job.record),
            });
            job
        };

        let Job {
            runtime,
            cancellation,
            record,
            execute,
            ..
        } = job;
        registry.metrics.started(&record);
        execute(runtime, cancellation, None);
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

    #[cfg(test)]
    pub(crate) fn cancel(&self) {
        self.metrics
            .cancel(&self.record, TaskCancellationReason::ExplicitHandle);
        self.cancellation.cancel();
    }
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) {
        self.metrics
            .cancel(&self.record, TaskCancellationReason::DroppedHandle);
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod stage5_evidence;
#[cfg(test)]
mod tests;
