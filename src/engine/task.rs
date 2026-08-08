use super::runtime::RuntimeHandle;
use crate::cancellation::CancellationToken;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
#[cfg(test)]
use std::time::Duration;

pub(crate) type TaskId = u64;

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

#[derive(Clone)]
pub(crate) struct TaskScheduler {
    inner: Arc<SchedulerInner>,
}

pub(crate) struct TaskHandle<R> {
    scheduler: Arc<SchedulerInner>,
    control: Arc<TaskControl>,
    completion: Receiver<TaskCompletion<R>>,
    key: Option<String>,
    id: TaskId,
}

struct SchedulerInner {
    runtime: RuntimeHandle,
    active: Mutex<HashMap<String, ActiveTask>>,
    next_id: AtomicU64,
}

struct ActiveTask {
    id: TaskId,
    control: Arc<TaskControl>,
}

const PENDING: u8 = 0;
const RUNNING: u8 = 1;
const CANCELLED: u8 = 2;

struct TaskControl {
    state: AtomicU8,
    cancellation: CancellationToken,
}

impl TaskControl {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(PENDING),
            cancellation: CancellationToken::new(),
        }
    }

    fn try_start(&self) -> bool {
        self.state
            .compare_exchange(PENDING, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn cancel(&self) {
        let _ =
            self.state
                .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
        self.cancellation.cancel();
    }
}

impl TaskScheduler {
    pub(crate) fn new(runtime: RuntimeHandle) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                runtime,
                active: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(0),
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn submit<T, R, F>(&self, value: T, worker: F) -> TaskHandle<R>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, Value, CancellationToken) -> R + Send + 'static,
    {
        self.start_task(None, value, worker)
    }

    pub(crate) fn submit_keyed<T, R, F>(&self, value: T, key: String, worker: F) -> TaskHandle<R>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, Value, CancellationToken) -> R + Send + 'static,
    {
        let mut active = self
            .inner
            .active
            .lock()
            .expect("task scheduler state was poisoned");
        if let Some(existing) = active.get(&key) {
            existing.control.cancel();
        }
        let handle = self.start_task(Some(key.clone()), value, worker);
        active.insert(
            key,
            ActiveTask {
                id: handle.id,
                control: Arc::clone(&handle.control),
            },
        );
        handle
    }

    fn start_task<T, R, F>(&self, key: Option<String>, value: T, worker: F) -> TaskHandle<R>
    where
        T: Send + 'static,
        R: Send + 'static,
        F: FnOnce(T, Value, CancellationToken) -> R + Send + 'static,
    {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let control = Arc::new(TaskControl::new());
        let (completion_sender, completion) = sync_channel(1);
        let runtime = self.inner.runtime.clone();
        let worker_control = Arc::clone(&control);
        thread::spawn(move || {
            let completion =
                if !worker_control.try_start() || worker_control.cancellation.is_cancelled() {
                    TaskCompletion::Cancelled
                } else {
                    let runtime = runtime.read();
                    if worker_control.cancellation.is_cancelled() {
                        TaskCompletion::Cancelled
                    } else {
                        let value = worker(value, runtime, worker_control.cancellation.clone());
                        if worker_control.cancellation.is_cancelled() {
                            TaskCompletion::Cancelled
                        } else {
                            TaskCompletion::Completed(value)
                        }
                    }
                };
            let _ = completion_sender.send(completion);
        });
        TaskHandle::new(Arc::clone(&self.inner), control, completion, key, id)
    }
}

impl SchedulerInner {
    fn is_current(&self, key: Option<&str>, id: TaskId) -> bool {
        let Some(key) = key else {
            return true;
        };
        self.active
            .lock()
            .expect("task scheduler state was poisoned")
            .get(key)
            .is_some_and(|task| task.id == id)
    }

    fn remove_if_current(&self, key: Option<&str>, id: TaskId) {
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

impl<R> TaskHandle<R> {
    fn new(
        scheduler: Arc<SchedulerInner>,
        control: Arc<TaskControl>,
        completion: Receiver<TaskCompletion<R>>,
        key: Option<String>,
        id: TaskId,
    ) -> Self {
        Self {
            scheduler,
            control,
            completion,
            key,
            id,
        }
    }

    pub(crate) fn try_recv(&mut self) -> std::result::Result<TaskResponse<R>, TryRecvError> {
        self.completion.try_recv().map(|completion| TaskResponse {
            id: self.id,
            completion,
            current: self.scheduler.is_current(self.key.as_deref(), self.id),
        })
    }

    #[cfg(test)]
    pub(crate) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<TaskResponse<R>, RecvTimeoutError> {
        self.completion
            .recv_timeout(timeout)
            .map(|completion| TaskResponse {
                id: self.id,
                completion,
                current: self.scheduler.is_current(self.key.as_deref(), self.id),
            })
    }
}

impl<R> Drop for TaskHandle<R> {
    fn drop(&mut self) {
        self.control.cancel();
        self.scheduler
            .remove_if_current(self.key.as_deref(), self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::runtime::RuntimeStore;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, mpsc};

    #[test]
    fn independent_tasks_with_different_keys_run_in_parallel() {
        let store = RuntimeStore::new();
        let (started_tx, started_rx) = mpsc::channel();
        let barrier = Arc::new(Barrier::new(3));
        let scheduler = TaskScheduler::new(store.handle());
        let first_barrier = Arc::clone(&barrier);
        let first_started = started_tx.clone();
        let mut first = scheduler.submit_keyed(
            "first".to_string(),
            "first-key".to_string(),
            move |value, _, _| {
                first_started.send(value.clone()).unwrap();
                first_barrier.wait();
                value
            },
        );
        let second_barrier = Arc::clone(&barrier);
        let mut second = scheduler.submit_keyed(
            "second".to_string(),
            "second-key".to_string(),
            move |value, _, _| {
                started_tx.send(value.clone()).unwrap();
                second_barrier.wait();
                value
            },
        );
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
    fn replacing_a_key_cancels_the_old_task_and_marks_its_result_stale() {
        let store = RuntimeStore::new();
        let scheduler = TaskScheduler::new(store.handle());
        let (started_tx, started_rx) = mpsc::channel();
        let mut first = scheduler.submit_keyed(
            "first".to_string(),
            "items".to_string(),
            move |value, _, cancellation| {
                started_tx.send(()).unwrap();
                while !cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                value
            },
        );
        started_rx.recv().unwrap();
        let mut second =
            scheduler.submit_keyed("second".to_string(), "items".to_string(), |value, _, _| {
                value
            });

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
    fn task_reads_runtime_when_worker_starts() {
        let mut store = RuntimeStore::new();
        store.replace(json!({"version": "initial"}));
        let scheduler = TaskScheduler::new(store.handle());
        let mut runtime = store.lock_shared_for_test();
        let (submitted_tx, submitted_rx) = mpsc::channel();
        let submitter = thread::spawn(move || {
            submitted_tx
                .send(scheduler.submit("task".to_string(), |_, runtime, _| runtime))
                .unwrap();
        });

        let submitted = submitted_rx.recv_timeout(Duration::from_secs(1));
        *runtime = json!({"version": "latest"});
        drop(runtime);
        submitter.join().unwrap();
        let mut task =
            submitted.expect("submitting a task must not wait for the runtime read lock");

        let response = task.recv_timeout(Duration::from_secs(1)).unwrap();
        let TaskCompletion::Completed(runtime) = response.into_completion() else {
            panic!("task should complete");
        };
        assert_eq!(runtime["version"], "latest");
    }

    #[test]
    fn task_cancelled_before_start_does_not_call_worker() {
        let store = RuntimeStore::new();
        let scheduler = TaskScheduler::new(store.handle());
        let runtime = store.lock_shared_for_test();
        let called = Arc::new(AtomicBool::new(false));
        let worker_called = Arc::clone(&called);
        let mut task = scheduler.submit_keyed("task", "items".to_string(), move |_, _, _| {
            worker_called.store(true, Ordering::SeqCst);
        });
        let mut replacement =
            scheduler.submit_keyed("replacement", "items".to_string(), |_, _, _| ());

        drop(runtime);
        let completion = task.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            completion.into_completion(),
            TaskCompletion::Cancelled
        ));
        assert!(!called.load(Ordering::SeqCst));
        assert!(matches!(
            replacement
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .into_completion(),
            TaskCompletion::Completed(())
        ));
    }
}
