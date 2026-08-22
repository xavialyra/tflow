use super::picker::{ItemsRequest, ItemsResponse, load_items_for_page};
use super::runtime::RuntimeHandle;
use crate::cancellation::CancellationToken;
use crate::config::Config;
use serde_json::Value;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

pub(crate) enum TaskCompletion {
    Completed(ItemsResponse),
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct TaskScheduler {
    runtime: RuntimeHandle,
    registry: Arc<TaskRegistry>,
}

pub(crate) struct TaskHandle {
    cancellation: CancellationToken,
    completion: Receiver<TaskCompletion>,
}

struct Job {
    config: Arc<Config>,
    request: ItemsRequest,
    runtime: Value,
    cancellation: CancellationToken,
    completion: SyncSender<TaskCompletion>,
}

struct RegistryState {
    active: Option<CancellationToken>,
    pending: Option<Job>,
    closed: bool,
}

struct TaskRegistry {
    state: Mutex<RegistryState>,
    ready: Condvar,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl TaskScheduler {
    pub(crate) fn new(runtime: RuntimeHandle) -> Self {
        Self {
            runtime,
            registry: Arc::new(TaskRegistry {
                state: Mutex::new(RegistryState {
                    active: None,
                    pending: None,
                    closed: false,
                }),
                ready: Condvar::new(),
                worker: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn submit_items(&self, config: &Arc<Config>, request: ItemsRequest) -> TaskHandle {
        let cancellation = CancellationToken::new();
        let (completion, receiver) = sync_channel(1);
        let job = Job {
            config: Arc::clone(config),
            request,
            runtime: self.runtime.read(),
            cancellation: cancellation.clone(),
            completion,
        };
        let mut state = self
            .registry
            .state
            .lock()
            .expect("task registry state was poisoned");
        if state.closed {
            cancellation.cancel();
            let _ = job.completion.send(TaskCompletion::Cancelled);
        } else {
            if let Some(active) = &state.active {
                active.cancel();
            }
            if let Some(pending) = state.pending.replace(job) {
                pending.cancellation.cancel();
            }
            self.registry.ensure_worker();
            drop(state);
            self.registry.ready.notify_one();
        }
        TaskHandle {
            cancellation,
            completion: receiver,
        }
    }

    pub(crate) fn cancel_all(&self) {
        let state = self
            .registry
            .state
            .lock()
            .expect("task registry state was poisoned");
        if let Some(active) = &state.active {
            active.cancel();
        }
        if let Some(pending) = &state.pending {
            pending.cancellation.cancel();
        }
    }

    pub(crate) fn shutdown_and_wait(&self) {
        {
            let mut state = self
                .registry
                .state
                .lock()
                .expect("task registry state was poisoned");
            state.closed = true;
            if let Some(active) = &state.active {
                active.cancel();
            }
            if let Some(pending) = state.pending.take() {
                pending.cancellation.cancel();
            }
        }
        self.registry.ready.notify_all();
        let worker = self
            .registry
            .worker
            .lock()
            .expect("task registry state was poisoned")
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

impl TaskRegistry {
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
            while state.pending.is_none() && !state.closed {
                state = registry
                    .ready
                    .wait(state)
                    .expect("task registry state was poisoned");
            }
            if state.pending.is_none() && state.closed {
                return;
            }
            let job = state.pending.take().expect("pending items job disappeared");
            state.active = Some(job.cancellation.clone());
            job
        };

        let completion = if job.cancellation.is_cancelled() {
            TaskCompletion::Cancelled
        } else {
            let result = load_items_for_page(
                &job.config,
                &job.request.view,
                &job.request.page_state,
                &job.request.binding_raw,
                &job.runtime,
                &job.cancellation,
            )
            .map_err(|error| error.to_string());
            let response = ItemsResponse {
                view: job.request.view,
                generation: job.request.generation,
                input: job.request.input,
                query: job.request.binding_raw,
                result,
            };
            if job.cancellation.is_cancelled() {
                TaskCompletion::Cancelled
            } else {
                TaskCompletion::Completed(response)
            }
        };
        let _ = job.completion.send(completion);
        registry
            .state
            .lock()
            .expect("task registry state was poisoned")
            .active = None;
    }
}

impl TaskHandle {
    pub(crate) fn try_recv(&mut self) -> std::result::Result<TaskCompletion, TryRecvError> {
        self.completion.try_recv()
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}
