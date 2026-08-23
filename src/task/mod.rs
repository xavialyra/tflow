use crate::lifecycle::CancellationToken;
use crate::runtime::RuntimeHandle;
use serde_json::Value;
use std::collections::VecDeque;
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
    runtime: RuntimeHandle,
    registry: Arc<TaskRegistry>,
    owner: Arc<()>,
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

struct TaskRegistry {
    state: Mutex<RegistryState>,
    ready: Condvar,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl TaskRuntime {
    pub(crate) fn new(runtime: RuntimeHandle) -> Self {
        Self {
            runtime,
            registry: Arc::new(TaskRegistry {
                state: Mutex::new(RegistryState {
                    active: None,
                    pending: VecDeque::new(),
                    closed: false,
                }),
                ready: Condvar::new(),
                worker: Mutex::new(None),
            }),
            owner: Arc::new(()),
        }
    }

    /// Submit an ordinary serialized task.
    #[allow(dead_code)]
    pub(crate) fn spawn<T, F>(&self, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with(task, None)
    }

    /// Submit a task that replaces active and queued work in `lane`.
    pub(crate) fn spawn_latest<T, F>(&self, lane: impl Into<String>, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        self.spawn_with(task, Some(lane.into()))
    }

    fn spawn_with<T, F>(&self, task: F, replace_lane: Option<String>) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(TaskContext) -> Result<T, String> + Send + 'static,
    {
        let cancellation = CancellationToken::new();
        let (completion, receiver) = sync_channel(1);
        let execute = Box::new(move |runtime, cancellation: CancellationToken| {
            let result = if cancellation.is_cancelled() {
                TaskCompletion::Cancelled
            } else {
                match task(TaskContext {
                    runtime,
                    cancellation: cancellation.clone(),
                }) {
                    Ok(_value) if cancellation.is_cancelled() => TaskCompletion::Cancelled,
                    Ok(value) => TaskCompletion::Completed(value),
                    Err(_error) if cancellation.is_cancelled() => TaskCompletion::Cancelled,
                    Err(error) => TaskCompletion::Failed(error),
                }
            };
            let _ = completion.send(result);
        });
        self.registry.submit(
            Job {
                lane: replace_lane.clone(),
                runtime: self.runtime.read(),
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

    pub(crate) fn cancel_all(&self) {
        self.registry.cancel_all();
    }

    /// Cancel all work and wait for the worker to exit.
    pub(crate) fn shutdown_and_wait(&self) {
        self.registry.shutdown_and_wait();
    }
}

impl Drop for TaskRuntime {
    fn drop(&mut self) {
        if Arc::strong_count(&self.owner) == 1 {
            self.registry.shutdown_and_wait();
        }
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
                } else {
                    retained.push_back(pending);
                }
            }
            state.pending = retained;
        }
        state.pending.push_back(job);
        self.ensure_worker();
        drop(state);
        self.ready.notify_one();
    }

    fn cancel_all(&self) {
        let state = self.state.lock().expect("task registry state was poisoned");
        if let Some(active) = &state.active {
            active.cancellation.cancel();
        }
        for pending in &state.pending {
            pending.cancellation.cancel();
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
        let worker = self
            .worker
            .lock()
            .expect("task registry state was poisoned")
            .take();
        if let Some(worker) = worker
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }

    fn ensure_worker(self: &Arc<Self>) {
        let mut worker = self
            .worker
            .lock()
            .expect("task registry state was poisoned");
        if worker.is_some() {
            return;
        }
        let registry = Arc::clone(self);
        *worker = Some(thread::spawn(move || worker_loop(registry)));
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
    use super::{TaskCompletion, TaskRuntime};
    use crate::runtime::RuntimeStore;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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
    fn captures_runtime_snapshot_at_submission() {
        let mut runtime = RuntimeStore::new();
        runtime.set("/marker", json!("before")).unwrap();
        let tasks = TaskRuntime::new(runtime.handle());
        let mut handle = tasks.spawn(|context| Ok(context.runtime["marker"].clone()));

        match receive(&mut handle) {
            TaskCompletion::Completed(value) => assert_eq!(value, json!("before")),
            TaskCompletion::Failed(error) => panic!("task failed: {error}"),
            TaskCompletion::Cancelled => panic!("task was cancelled"),
        }
        tasks.shutdown_and_wait();
    }

    #[test]
    fn cancellation_is_reported_when_a_running_task_is_stopped() {
        let tasks = TaskRuntime::new(RuntimeStore::new().handle());
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
        let tasks = TaskRuntime::new(RuntimeStore::new().handle());
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
        let tasks = TaskRuntime::new(RuntimeStore::new().handle());
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
    fn dropping_the_last_runtime_cancels_and_joins_active_work() {
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let mut handle;
        {
            let tasks = TaskRuntime::new(RuntimeStore::new().handle());
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
    fn task_errors_are_returned_without_panicking_the_worker() {
        let tasks = TaskRuntime::new(RuntimeStore::new().handle());
        let mut handle = tasks.spawn(|_context| Err::<(), _>("expected failure".to_string()));

        match receive(&mut handle) {
            TaskCompletion::Failed(error) => assert_eq!(error, "expected failure"),
            TaskCompletion::Completed(_) => panic!("task unexpectedly succeeded"),
            TaskCompletion::Cancelled => panic!("task was cancelled"),
        }
        tasks.shutdown_and_wait();
    }
}
