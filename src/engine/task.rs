use super::runtime::RuntimeHandle;
use crate::cancellation::CancellationToken;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;

pub(crate) type TaskId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum TaskMode {
    Replace,
    Join,
}

#[derive(Clone)]
pub(crate) enum TaskCompletion<R> {
    Completed(R),
    Cancelled,
}

pub(crate) struct TaskResponse<R> {
    #[allow(dead_code)]
    pub(crate) id: TaskId,
    completion: TaskCompletion<R>,
    current: bool,
}

impl<R> TaskResponse<R> {
    pub(crate) fn is_current(&self) -> bool {
        self.current
    }

    pub(crate) fn into_completion(self) -> TaskCompletion<R> {
        self.completion
    }
}

pub(crate) struct TaskScheduler<T, R, K = String> {
    inner: Arc<SchedulerInner<T, R, K>>,
}

pub(crate) struct TaskHandle<T, R, K = String>
where
    K: Eq + Hash + Clone,
{
    scheduler: Arc<SchedulerInner<T, R, K>>,
    task: Arc<SharedTask<R>>,
    key: Option<K>,
    id: TaskId,
    seen: bool,
}

struct SchedulerInner<T, R, K> {
    runtime: RuntimeHandle,
    task: Arc<TaskFunction<T, R>>,
    active: Mutex<HashMap<K, ActiveTask<R>>>,
    next_id: AtomicU64,
}

type TaskFunction<T, R> = dyn Fn(T, Value, CancellationToken) -> R + Send + Sync + 'static;

struct ActiveTask<R> {
    id: TaskId,
    task: Arc<SharedTask<R>>,
}

struct SharedTask<R> {
    result: Mutex<Option<TaskCompletion<R>>>,
    ready: Condvar,
    subscribers: AtomicUsize,
    cancellation: CancellationToken,
}

impl<T, R, K> Clone for TaskScheduler<T, R, K> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T, R, K> TaskScheduler<T, R, K>
where
    T: Send + 'static,
    R: Clone + Send + 'static,
    K: Eq + Hash + Clone,
{
    pub(crate) fn spawn<F>(runtime: RuntimeHandle, task: F) -> Self
    where
        F: Fn(T, Value, CancellationToken) -> R + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(SchedulerInner {
                runtime,
                task: Arc::new(task),
                active: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(0),
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn submit(&self, value: T) -> Result<TaskHandle<T, R, K>> {
        Ok(self.start(None, value))
    }

    pub(crate) fn submit_keyed(
        &self,
        value: T,
        key: K,
        mode: TaskMode,
    ) -> Result<TaskHandle<T, R, K>> {
        let mut active = self
            .inner
            .active
            .lock()
            .expect("task scheduler state was poisoned");
        if let Some(existing) = active.get(&key) {
            match mode {
                TaskMode::Join => {
                    existing.task.subscribers.fetch_add(1, Ordering::Relaxed);
                    return Ok(TaskHandle::new(
                        Arc::clone(&self.inner),
                        Arc::clone(&existing.task),
                        Some(key),
                        existing.id,
                    ));
                }
                TaskMode::Replace => existing.task.cancellation.cancel(),
            }
        }
        Ok(self.start_keyed(&mut active, key, value))
    }

    fn start(&self, key: Option<K>, value: T) -> TaskHandle<T, R, K> {
        if let Some(key) = key {
            let mut active = self
                .inner
                .active
                .lock()
                .expect("task scheduler state was poisoned");
            self.start_keyed(&mut active, key, value)
        } else {
            self.start_task(None, value)
        }
    }

    fn start_keyed(
        &self,
        active: &mut HashMap<K, ActiveTask<R>>,
        key: K,
        value: T,
    ) -> TaskHandle<T, R, K> {
        let handle = self.start_task(Some(key.clone()), value);
        active.insert(
            key,
            ActiveTask {
                id: handle.id,
                task: Arc::clone(&handle.task),
            },
        );
        handle
    }

    fn start_task(&self, key: Option<K>, value: T) -> TaskHandle<T, R, K> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let cancellation = CancellationToken::new();
        let task = Arc::new(SharedTask {
            result: Mutex::new(None),
            ready: Condvar::new(),
            subscribers: AtomicUsize::new(1),
            cancellation: cancellation.clone(),
        });
        let worker = Arc::clone(&self.inner.task);
        let runtime = self.inner.runtime.clone();
        let result = Arc::clone(&task);
        thread::spawn(move || {
            let runtime = runtime.read();
            let value = worker(value, runtime, cancellation.clone());
            let completion = if cancellation.is_cancelled() {
                TaskCompletion::Cancelled
            } else {
                TaskCompletion::Completed(value)
            };
            *result
                .result
                .lock()
                .expect("task result state was poisoned") = Some(completion);
            result.ready.notify_all();
        });
        TaskHandle::new(Arc::clone(&self.inner), task, key, id)
    }
}

impl<T, R, K> SchedulerInner<T, R, K>
where
    K: Eq + Hash,
{
    fn is_current(&self, key: Option<&K>, id: TaskId) -> bool {
        let Some(key) = key else {
            return true;
        };
        self.active
            .lock()
            .expect("task scheduler state was poisoned")
            .get(key)
            .is_some_and(|task| task.id == id)
    }

    fn remove_if_current(&self, key: Option<&K>, id: TaskId) {
        let Some(key) = key else {
            return;
        };
        let mut active = self
            .active
            .lock()
            .expect("task scheduler state was poisoned");
        if active.get(key).is_some_and(|task| task.id == id) {
            active.remove(key);
        }
    }
}

impl<T, R, K> TaskHandle<T, R, K>
where
    R: Clone,
    K: Eq + Hash + Clone,
{
    fn new(
        scheduler: Arc<SchedulerInner<T, R, K>>,
        task: Arc<SharedTask<R>>,
        key: Option<K>,
        id: TaskId,
    ) -> Self {
        Self {
            scheduler,
            task,
            key,
            id,
            seen: false,
        }
    }

    pub(crate) fn try_recv(&mut self) -> std::result::Result<TaskResponse<R>, TryRecvError> {
        if self.seen {
            return Err(TryRecvError::Empty);
        }
        let completion = self
            .task
            .result
            .lock()
            .expect("task result state was poisoned")
            .clone();
        let Some(completion) = completion else {
            return Err(TryRecvError::Empty);
        };
        self.seen = true;
        let current = self.scheduler_is_current();
        Ok(TaskResponse {
            id: self.id,
            completion,
            current,
        })
    }

    #[cfg(test)]
    pub(crate) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<TaskResponse<R>, RecvTimeoutError> {
        if self.seen {
            return Err(RecvTimeoutError::Timeout);
        }
        let result = self
            .task
            .result
            .lock()
            .expect("task result state was poisoned");
        let (result, wait) = self
            .task
            .ready
            .wait_timeout_while(result, timeout, |result| result.is_none())
            .expect("task result state was poisoned");
        let Some(completion) = result.clone() else {
            if wait.timed_out() {
                return Err(RecvTimeoutError::Timeout);
            }
            return Err(RecvTimeoutError::Disconnected);
        };
        self.seen = true;
        Ok(TaskResponse {
            id: self.id,
            completion,
            current: self.scheduler_is_current(),
        })
    }

    fn scheduler_is_current(&self) -> bool {
        self.key
            .as_ref()
            .map_or(true, |key| self.scheduler_is_current_key(key))
    }

    fn scheduler_is_current_key(&self, key: &K) -> bool {
        self.scheduler.is_current(Some(key), self.id)
    }
}

impl<T, R, K> Clone for TaskHandle<T, R, K>
where
    K: Eq + Hash + Clone,
{
    fn clone(&self) -> Self {
        self.task.subscribers.fetch_add(1, Ordering::Relaxed);
        Self {
            scheduler: Arc::clone(&self.scheduler),
            task: Arc::clone(&self.task),
            key: self.key.clone(),
            id: self.id,
            seen: false,
        }
    }
}

impl<T, R, K> Drop for TaskHandle<T, R, K>
where
    K: Eq + Hash + Clone,
{
    fn drop(&mut self) {
        if self.task.subscribers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.task.cancellation.cancel();
            self.scheduler.remove_if_current(self.key.as_ref(), self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::runtime::RuntimeStore;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, mpsc};

    #[test]
    fn independent_tasks_with_different_keys_run_in_parallel() {
        let store = RuntimeStore::new();
        let (started_tx, started_rx) = mpsc::channel();
        let barrier = Arc::new(Barrier::new(3));
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);
        let scheduler =
            TaskScheduler::<String, String, String>::spawn(store.handle(), move |value, _, _| {
                if value == "first" {
                    started_tx.send(value.clone()).unwrap();
                    first_barrier.wait();
                } else if value == "second" {
                    started_tx.send(value.clone()).unwrap();
                    second_barrier.wait();
                }
                value
            });

        let mut first = scheduler
            .submit_keyed(
                "first".to_string(),
                "first-key".to_string(),
                TaskMode::Replace,
            )
            .unwrap();
        let mut second = scheduler
            .submit_keyed(
                "second".to_string(),
                "second-key".to_string(),
                TaskMode::Replace,
            )
            .unwrap();
        let mut started = [
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        started.sort();
        assert_eq!(started, ["first", "second"]);
        barrier.wait();

        let first = first.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = second.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(first.is_current());
        assert!(second.is_current());
        assert!(matches!(
            first.into_completion(),
            TaskCompletion::Completed(_)
        ));
        assert!(matches!(
            second.into_completion(),
            TaskCompletion::Completed(_)
        ));
    }

    #[test]
    fn joins_an_active_task_with_the_same_key() {
        let store = RuntimeStore::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let calls = Arc::new(AtomicUsize::new(0));
        let task_calls = Arc::clone(&calls);
        let scheduler = TaskScheduler::<String, String, String>::spawn(
            store.handle(),
            move |value: String, _, _| {
                task_calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                value
            },
        );

        let mut first = scheduler
            .submit_keyed("first".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();
        started_rx.recv().unwrap();
        let joined_scheduler = scheduler.clone();
        let mut joined = joined_scheduler
            .submit_keyed(
                "different-input".to_string(),
                "items".to_string(),
                TaskMode::Join,
            )
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release_tx.send(()).unwrap();

        let response = joined.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(response.id, first.id);
        assert!(response.is_current());
        assert!(matches!(
            response.into_completion(),
            TaskCompletion::Completed(value) if value == "first"
        ));
        assert!(matches!(
            first.recv_timeout(Duration::from_secs(1)),
            Ok(response) if response.is_current()
        ));
    }

    #[test]
    fn replacing_a_key_cancels_the_old_task_and_marks_its_result_stale() {
        let store = RuntimeStore::new();
        let (started_tx, started_rx) = mpsc::channel();
        let scheduler = TaskScheduler::<String, String, String>::spawn(
            store.handle(),
            move |value: String, _, cancellation| {
                if value == "first" {
                    started_tx.send(()).unwrap();
                    while !cancellation.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                value
            },
        );

        let mut first = scheduler
            .submit_keyed("first".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();
        started_rx.recv().unwrap();
        let replacement_scheduler = scheduler.clone();
        let mut second = replacement_scheduler
            .submit_keyed("second".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();

        let first = first.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = second.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!first.is_current());
        assert!(matches!(first.into_completion(), TaskCompletion::Cancelled));
        assert!(second.is_current());
        assert!(matches!(
            second.into_completion(),
            TaskCompletion::Completed(value) if value == "second"
        ));
    }

    #[test]
    fn each_task_reads_runtime_when_it_starts() {
        let mut store = RuntimeStore::new();
        store.replace(json!({"version": "initial"}));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let scheduler = TaskScheduler::<String, Value, String>::spawn(
            store.handle(),
            move |value: String, runtime, _| {
                if value == "first" {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }
                json!({"value": value, "runtime": runtime})
            },
        );

        let mut first = scheduler.submit("first".to_string()).unwrap();
        started_rx.recv().unwrap();
        store.replace(json!({"version": "latest"}));
        let mut second = scheduler.submit("second".to_string()).unwrap();
        release_tx.send(()).unwrap();

        let first = first.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = second.recv_timeout(Duration::from_secs(1)).unwrap();
        let TaskCompletion::Completed(first) = first.into_completion() else {
            panic!("first task should complete");
        };
        let TaskCompletion::Completed(second) = second.into_completion() else {
            panic!("second task should complete");
        };
        assert_eq!(first["runtime"]["version"], "initial");
        assert_eq!(second["runtime"]["version"], "latest");
    }
}
