use super::runtime::RuntimeHandle;
use crate::cancellation::CancellationToken;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
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

pub(crate) struct TaskCoordinator<T, R, K = String> {
    runtime: RuntimeHandle,
    task: Arc<TaskFunction<T, R>>,
    responses: Receiver<RawTaskResponse<R>>,
    response_sender: Sender<RawTaskResponse<R>>,
    active: HashMap<K, TaskId>,
    tasks: HashMap<TaskId, TaskRecord<K>>,
    next_id: TaskId,
}

type TaskFunction<T, R> = dyn Fn(T, Value, CancellationToken) -> R + Send + Sync + 'static;

struct TaskRecord<K> {
    key: Option<K>,
    cancellation: CancellationToken,
}

struct RawTaskResponse<R> {
    id: TaskId,
    completion: TaskCompletion<R>,
}

impl<T, R, K> Drop for TaskCoordinator<T, R, K> {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.cancellation.cancel();
        }
    }
}

impl<T, R, K> TaskCoordinator<T, R, K>
where
    T: Send + 'static,
    R: Send + 'static,
    K: Eq + std::hash::Hash + Clone,
{
    pub(crate) fn spawn<F>(runtime: RuntimeHandle, task: F) -> Self
    where
        F: Fn(T, Value, CancellationToken) -> R + Send + Sync + 'static,
    {
        let (response_sender, responses) = mpsc::channel();
        Self {
            runtime,
            task: Arc::new(task),
            responses,
            response_sender,
            active: HashMap::new(),
            tasks: HashMap::new(),
            next_id: 0,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn submit(&mut self, value: T) -> Result<TaskId> {
        self.start(None, value)
    }

    pub(crate) fn submit_keyed(&mut self, value: T, key: K, mode: TaskMode) -> Result<TaskId> {
        if let Some(id) = self.active.get(&key).copied() {
            match mode {
                TaskMode::Join => return Ok(id),
                TaskMode::Replace => self.cancel(id),
            }
        }
        self.start(Some(key), value)
    }

    pub(crate) fn try_recv(&mut self) -> std::result::Result<TaskResponse<R>, TryRecvError> {
        self.responses
            .try_recv()
            .map(|response| self.finish_response(response))
    }

    #[cfg(test)]
    pub(crate) fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::result::Result<TaskResponse<R>, RecvTimeoutError> {
        self.responses
            .recv_timeout(timeout)
            .map(|response| self.finish_response(response))
    }

    fn start(&mut self, key: Option<K>, value: T) -> Result<TaskId> {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let cancellation = CancellationToken::new();
        if let Some(key) = key.as_ref() {
            self.active.insert(key.clone(), id);
        }
        self.tasks.insert(
            id,
            TaskRecord {
                key,
                cancellation: cancellation.clone(),
            },
        );

        let task = Arc::clone(&self.task);
        let runtime = self.runtime.clone();
        let responses = self.response_sender.clone();
        thread::spawn(move || {
            let runtime = runtime.read();
            let value = task(value, runtime, cancellation.clone());
            let completion = if cancellation.is_cancelled() {
                TaskCompletion::Cancelled
            } else {
                TaskCompletion::Completed(value)
            };
            let _ = responses.send(RawTaskResponse { id, completion });
        });
        Ok(id)
    }

    fn cancel(&self, id: TaskId) {
        if let Some(task) = self.tasks.get(&id) {
            task.cancellation.cancel();
        }
    }

    fn finish_response(&mut self, response: RawTaskResponse<R>) -> TaskResponse<R> {
        let current = self
            .tasks
            .get(&response.id)
            .is_some_and(|task| match task.key.as_ref() {
                Some(key) => self.active.get(key) == Some(&response.id),
                None => true,
            });
        if let Some(task) = self.tasks.remove(&response.id)
            && let Some(key) = task.key
            && self.active.get(&key) == Some(&response.id)
        {
            self.active.remove(&key);
        }
        TaskResponse {
            id: response.id,
            completion: response.completion,
            current,
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
        let mut task =
            TaskCoordinator::<String, String, String>::spawn(store.handle(), move |value, _, _| {
                if value == "first" {
                    started_tx.send(value.clone()).unwrap();
                    first_barrier.wait();
                } else if value == "second" {
                    started_tx.send(value.clone()).unwrap();
                    second_barrier.wait();
                }
                value
            });

        task.submit_keyed(
            "first".to_string(),
            "first-key".to_string(),
            TaskMode::Replace,
        )
        .unwrap();
        task.submit_keyed(
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

        let first = task.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = task.recv_timeout(Duration::from_secs(1)).unwrap();
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
        let mut task = TaskCoordinator::<String, String, String>::spawn(
            store.handle(),
            move |value: String, _, _| {
                task_calls.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                value
            },
        );

        let first = task
            .submit_keyed("first".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();
        started_rx.recv().unwrap();
        let joined = task
            .submit_keyed(
                "different-input".to_string(),
                "items".to_string(),
                TaskMode::Join,
            )
            .unwrap();
        assert_eq!(joined, first);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release_tx.send(()).unwrap();

        let response = task.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(response.id, first);
        assert!(response.is_current());
        assert!(matches!(
            response.into_completion(),
            TaskCompletion::Completed(value) if value == "first"
        ));
    }

    #[test]
    fn replacing_a_key_cancels_the_old_task_and_marks_its_result_stale() {
        let store = RuntimeStore::new();
        let (started_tx, started_rx) = mpsc::channel();
        let mut task = TaskCoordinator::<String, String, String>::spawn(
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

        task.submit_keyed("first".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();
        started_rx.recv().unwrap();
        task.submit_keyed("second".to_string(), "items".to_string(), TaskMode::Replace)
            .unwrap();

        let mut responses = [
            task.recv_timeout(Duration::from_secs(1)).unwrap(),
            task.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        responses.sort_by_key(|response| response.id);
        let [first, second] = responses;
        assert_eq!(first.id, 1);
        assert!(!first.is_current());
        assert!(matches!(first.into_completion(), TaskCompletion::Cancelled));
        assert_eq!(second.id, 2);
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
        let mut task = TaskCoordinator::<String, Value, String>::spawn(
            store.handle(),
            move |value: String, runtime, _| {
                if value == "first" {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }
                json!({"value": value, "runtime": runtime})
            },
        );

        task.submit("first".to_string()).unwrap();
        started_rx.recv().unwrap();
        store.replace(json!({"version": "latest"}));
        task.submit("second".to_string()).unwrap();
        release_tx.send(()).unwrap();

        let first = task.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = task.recv_timeout(Duration::from_secs(1)).unwrap();
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
