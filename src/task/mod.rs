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
    ExplicitHandle,
    DroppedHandle,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskActiveSnapshot {
    pub(crate) tags: TaskTags,
    pub(crate) lane: Option<String>,
    pub(crate) submitted_at: TaskElapsed,
    pub(crate) started_at: Option<TaskElapsed>,
    pub(crate) cancellation: Option<TaskCancellationSample>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskRuntimeMetricsSnapshot {
    pub(crate) now: TaskElapsed,
    pub(crate) queue_depth: usize,
    pub(crate) active_tasks: usize,
    pub(crate) current_active: Option<TaskActiveSnapshot>,
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

#[cfg(test)]
mod stage5_evidence;

#[cfg(test)]
pub(crate) fn run_stage_5_scheduler_evidence() {
    stage5_evidence::run();
}

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

    fn queued(&self, depth: usize) {
        let mut state = self.state.lock().expect("task metrics state was poisoned");
        state.queue_high_water = state.queue_high_water.max(depth);
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
#[derive(Clone)]
pub(crate) struct MountTaskStarter {
    runtime: TaskRuntime,
    mount_id: crate::input::ViewMountId,
    lane_prefix: String,
    correlation: Option<(TaskId, u64)>,
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

    pub(crate) fn for_task(&self, task: TaskId, generation: u64) -> Self {
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
        self.spawn_latest_with_snapshot_tagged(lane, runtime_snapshot, TaskTags::DEFAULT, task)
    }

    pub(crate) fn spawn_latest_with_snapshot_tagged<T, F>(
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
        self.runtime.spawn_latest_with_snapshot_and_correlation(
            format!("{}:{}", self.lane_prefix, lane.as_ref()),
            runtime_snapshot,
            tags,
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

type TaskJob = Box<dyn FnOnce(Value, CancellationToken) + Send + 'static>;

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
                    metrics,
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
        self.spawn_latest_with_snapshot_and_correlation(
            lane,
            runtime_snapshot,
            TaskTags::DEFAULT,
            None,
            task,
        )
    }

    fn spawn_latest_with_snapshot_and_correlation<T, F>(
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
        self.spawn_with_snapshot(task, Some(lane.into()), tags, runtime_snapshot, correlation)
    }

    #[cfg(test)]
    fn spawn_with<T, F>(&self, task: F, replace_lane: Option<String>) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with_snapshot(task, replace_lane, TaskTags::DEFAULT, Value::Null, None)
    }

    fn spawn_with_snapshot<T, F>(
        &self,
        task: F,
        replace_lane: Option<String>,
        tags: TaskTags,
        runtime_snapshot: Value,
        correlation: Option<(ViewInstanceId, TaskId, u64)>,
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
        let execute = Box::new(move |runtime, cancellation: CancellationToken| {
            let (result, terminal_outcome) = if cancellation.is_cancelled() {
                (TaskCompletion::Cancelled, TaskTerminalOutcome::Cancelled)
            } else {
                match catch_unwind(AssertUnwindSafe(|| {
                    task(TaskContext {
                        runtime,
                        cancellation: cancellation.clone(),
                        metrics: Arc::clone(&execute_metrics),
                        record: Arc::clone(&execute_record),
                    })
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
                    Ok(Err(error)) => (TaskCompletion::Failed(error), TaskTerminalOutcome::Failed),
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
            let terminal_outcome = execute_metrics.terminalize(&execute_record, terminal_outcome);
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
        });
        self.owner.registry.submit(
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
    pub(crate) fn metrics_snapshot(&self) -> TaskRuntimeMetricsSnapshot {
        let (queue_depth, active) = {
            let state = self
                .owner
                .registry
                .state
                .lock()
                .expect("task registry state was poisoned");
            (
                state.pending.len(),
                state
                    .active
                    .as_ref()
                    .map(|active| Arc::clone(&active.record)),
            )
        };
        let current_active = active.map(|record| {
            let record = record.lock().expect("task record was poisoned");
            TaskActiveSnapshot {
                tags: record.tags,
                lane: record.lane.clone(),
                submitted_at: record.submitted_at,
                started_at: record.started_at,
                cancellation: record.cancellation.clone(),
            }
        });
        let state = self
            .owner
            .registry
            .metrics
            .state
            .lock()
            .expect("task metrics state was poisoned");
        TaskRuntimeMetricsSnapshot {
            now: self.owner.registry.metrics.now(),
            queue_depth,
            active_tasks: usize::from(current_active.is_some()),
            current_active,
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
        self.metrics.queued(state.pending.len());
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
                self.metrics
                    .cancel(&active.record, TaskCancellationReason::ShutdownActive);
                active.cancellation.cancel();
            }
            let pending = state.pending.drain(..).collect::<Vec<_>>();
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
mod tests {
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
        let mut handle = tasks.spawn_latest_with_snapshot("snapshot", snapshot, |context| {
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
        let mut handle = starter.spawn_latest_with_snapshot_tagged(
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
        let mut handle = starter.spawn_latest_with_snapshot_tagged(
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
        let mut handle = starter.spawn_latest_with_snapshot_tagged(
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
}
