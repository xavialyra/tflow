use crate::terminal::{ImagePicker, ImagePickerFingerprint};
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

const MAX_CONCURRENT_PROTOCOL_ENCODES: usize = 2;
static IMAGE_PROTOCOL_POOL: OnceLock<std::result::Result<ImageProtocolPool, String>> =
    OnceLock::new();
static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);

type EncodeResult = std::result::Result<StatefulProtocol, String>;
type Encoder = dyn Fn(Arc<DynamicImage>, ImagePicker, Size) -> EncodeResult + Send + Sync + 'static;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ImageProtocolKey {
    revision: u64,
    block: usize,
    image: usize,
    area: Size,
    picker: ImagePickerFingerprint,
}

impl ImageProtocolKey {
    pub(super) fn new(
        revision: u64,
        block: usize,
        image: &Arc<DynamicImage>,
        area: Size,
        picker: ImagePicker,
    ) -> Self {
        Self {
            revision,
            block,
            image: Arc::as_ptr(image) as usize,
            area,
            picker: picker.fingerprint(),
        }
    }
}

pub(super) struct DesiredImageProtocol {
    pub(super) key: ImageProtocolKey,
    pub(super) image: Arc<DynamicImage>,
    pub(super) picker: ImagePicker,
}

enum CachedProtocol {
    Pending(Arc<AtomicBool>),
    Ready(Box<StatefulProtocol>),
    Failed(String),
}

struct ProtocolCompletion {
    key: ImageProtocolKey,
    result: EncodeResult,
}

struct ProtocolJob {
    owner: u64,
    key: ImageProtocolKey,
    image: Arc<DynamicImage>,
    picker: ImagePicker,
    cancellation: Arc<AtomicBool>,
    completion: Sender<ProtocolCompletion>,
}

struct PoolState {
    jobs: VecDeque<ProtocolJob>,
    closed: bool,
}

struct SharedPool {
    state: Mutex<PoolState>,
    ready: Condvar,
    encoder: Arc<Encoder>,
}

struct ImageProtocolPool {
    shared: Arc<SharedPool>,
    workers: Vec<JoinHandle<()>>,
}

impl ImageProtocolPool {
    fn new(worker_count: usize, encoder: Arc<Encoder>) -> std::io::Result<Self> {
        assert!(worker_count > 0, "image protocol pool needs a worker");
        let shared = Arc::new(SharedPool {
            state: Mutex::new(PoolState {
                jobs: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
            encoder,
        });
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let worker = Arc::clone(&shared);
            match thread::Builder::new()
                .name(format!("tui-image-protocol-{index}"))
                .spawn(move || protocol_worker(worker))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    close_pool(&shared);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self { shared, workers })
    }

    fn submit(
        &self,
        owner: u64,
        desired: DesiredImageProtocol,
        completion: Sender<ProtocolCompletion>,
    ) -> Arc<AtomicBool> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let job = ProtocolJob {
            owner,
            key: desired.key,
            image: desired.image,
            picker: desired.picker,
            cancellation: Arc::clone(&cancellation),
            completion,
        };
        let mut state = self
            .shared
            .state
            .lock()
            .expect("image protocol queue was poisoned");
        if let Some(index) = state
            .jobs
            .iter()
            .position(|pending| pending.owner == job.owner && pending.key.block == job.key.block)
        {
            let previous = state
                .jobs
                .remove(index)
                .expect("pending image protocol job disappeared");
            previous.cancellation.store(true, Ordering::Release);
        }
        state.jobs.push_back(job);
        drop(state);
        self.shared.ready.notify_one();
        cancellation
    }
}

impl Drop for ImageProtocolPool {
    fn drop(&mut self) {
        close_pool(&self.shared);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn close_pool(shared: &SharedPool) {
    let mut state = shared
        .state
        .lock()
        .expect("image protocol queue was poisoned");
    state.closed = true;
    for job in state.jobs.drain(..) {
        job.cancellation.store(true, Ordering::Release);
    }
    drop(state);
    shared.ready.notify_all();
}

fn protocol_worker(shared: Arc<SharedPool>) {
    loop {
        let job = {
            let mut state = shared
                .state
                .lock()
                .expect("image protocol queue was poisoned");
            while state.jobs.is_empty() && !state.closed {
                state = shared
                    .ready
                    .wait(state)
                    .expect("image protocol queue was poisoned");
            }
            if state.closed {
                return;
            }
            state
                .jobs
                .pop_front()
                .expect("image protocol job disappeared")
        };
        if job.cancellation.load(Ordering::Acquire) {
            continue;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            (shared.encoder)(job.image, job.picker, job.key.area)
        }))
        .unwrap_or_else(|_| Err("image protocol encoder panicked".to_string()));
        if !job.cancellation.load(Ordering::Acquire) {
            let _ = job.completion.send(ProtocolCompletion {
                key: job.key,
                result,
            });
        }
    }
}

fn default_pool() -> std::result::Result<&'static ImageProtocolPool, String> {
    match IMAGE_PROTOCOL_POOL.get_or_init(|| {
        ImageProtocolPool::new(MAX_CONCURRENT_PROTOCOL_ENCODES, Arc::new(encode_protocol))
            .map_err(|error| format!("could not start image protocol workers: {error}"))
    }) {
        Ok(pool) => Ok(pool),
        Err(error) => Err(error.clone()),
    }
}

fn encode_protocol(image: Arc<DynamicImage>, picker: ImagePicker, area: Size) -> EncodeResult {
    let mut protocol = picker.new_resize_protocol(image.as_ref().clone());
    let resize = Resize::Fit(None);
    let encoded_size = protocol.size_for(resize.clone(), area);
    protocol.resize_encode(&resize, encoded_size);
    match protocol.last_encoding_result() {
        Some(Ok(())) => Ok(protocol),
        Some(Err(error)) => Err(error.to_string()),
        None => Err("image protocol did not produce an encoding result".to_string()),
    }
}

pub(crate) struct ImageProtocolCache {
    id: u64,
    entries: HashMap<usize, (ImageProtocolKey, CachedProtocol)>,
    completion_tx: Sender<ProtocolCompletion>,
    completion_rx: Receiver<ProtocolCompletion>,
    #[cfg(test)]
    submissions: usize,
}

impl ImageProtocolCache {
    pub(crate) fn new() -> Self {
        let (completion_tx, completion_rx) = channel();
        Self {
            id: NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed),
            entries: HashMap::new(),
            completion_tx,
            completion_rx,
            #[cfg(test)]
            submissions: 0,
        }
    }

    pub(super) fn update(&mut self, desired: Vec<DesiredImageProtocol>) {
        self.collect();
        let desired_blocks = desired
            .iter()
            .map(|desired| desired.key.block)
            .collect::<Vec<_>>();
        self.entries.retain(|block, state| {
            let keep = desired_blocks.contains(block);
            if !keep && let CachedProtocol::Pending(cancellation) = &state.1 {
                cancellation.store(true, Ordering::Release);
            }
            keep
        });

        for desired in desired {
            let unchanged = self
                .entries
                .get(&desired.key.block)
                .is_some_and(|(key, _)| *key == desired.key);
            if unchanged {
                continue;
            }
            if let Some((_, CachedProtocol::Pending(cancellation))) =
                self.entries.remove(&desired.key.block)
            {
                cancellation.store(true, Ordering::Release);
            }
            let key = desired.key;
            let cancellation = match default_pool() {
                Ok(pool) => pool.submit(self.id, desired, self.completion_tx.clone()),
                Err(error) => {
                    self.entries
                        .insert(key.block, (key, CachedProtocol::Failed(error)));
                    continue;
                }
            };
            #[cfg(test)]
            {
                self.submissions += 1;
            }
            self.entries
                .insert(key.block, (key, CachedProtocol::Pending(cancellation)));
        }
    }

    pub(crate) fn clear(&mut self) {
        for (_, state) in self.entries.drain() {
            if let CachedProtocol::Pending(cancellation) = state.1 {
                cancellation.store(true, Ordering::Release);
            }
        }
    }

    fn collect(&mut self) {
        while let Ok(completed) = self.completion_rx.try_recv() {
            let Some((key, state)) = self.entries.get_mut(&completed.key.block) else {
                continue;
            };
            if *key != completed.key {
                continue;
            }
            *state = match completed.result {
                Ok(protocol) => CachedProtocol::Ready(Box::new(protocol)),
                Err(error) => CachedProtocol::Failed(error),
            };
        }
    }

    pub(super) fn protocol(&mut self, key: ImageProtocolKey) -> Option<&mut StatefulProtocol> {
        self.collect();
        match self.entries.get_mut(&key.block) {
            Some((cached_key, CachedProtocol::Ready(protocol))) if *cached_key == key => {
                Some(protocol.as_mut())
            }
            _ => None,
        }
    }

    pub(super) fn error(&self, key: ImageProtocolKey) -> Option<&str> {
        match self.entries.get(&key.block) {
            Some((cached_key, CachedProtocol::Failed(error))) if *cached_key == key => {
                Some(error.as_str())
            }
            _ => None,
        }
    }
}

impl Drop for ImageProtocolCache {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::time::Duration;

    fn key(revision: u64, block: usize) -> ImageProtocolKey {
        ImageProtocolKey {
            revision,
            block,
            image: revision as usize,
            area: Size::new(20, 10),
            picker: ImagePicker::test_halfblocks().fingerprint(),
        }
    }

    #[test]
    fn pending_job_for_a_block_is_replaced_by_the_latest() {
        let barrier = Arc::new(Barrier::new(2));
        let started = Arc::new(AtomicBool::new(false));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let encoder = {
            let barrier = Arc::clone(&barrier);
            let started = Arc::clone(&started);
            let executed = Arc::clone(&executed);
            Arc::new(
                move |image: Arc<DynamicImage>, picker: ImagePicker, area: Size| {
                    let revision = image.width() as u64;
                    executed.lock().unwrap().push(revision);
                    if !started.swap(true, Ordering::AcqRel) {
                        barrier.wait();
                    }
                    encode_protocol(image, picker, area)
                },
            )
        };
        let pool = ImageProtocolPool::new(1, encoder).unwrap();
        let (completion_tx, completion) = channel();
        let submit = |revision| DesiredImageProtocol {
            key: key(revision, 0),
            image: Arc::new(DynamicImage::new_rgba8(revision as u32, 1)),
            picker: ImagePicker::test_halfblocks(),
        };
        let first = pool.submit(1, submit(1), completion_tx.clone());
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let _old = pool.submit(1, submit(2), completion_tx.clone());
        let _latest = pool.submit(1, submit(3), completion_tx);
        barrier.wait();
        for _ in 0..200 {
            if executed.lock().unwrap().as_slice() == [1, 3] {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let completions = completion.try_iter().count();
        first.store(true, Ordering::Release);
        assert_eq!(*executed.lock().unwrap(), [1, 3]);
        assert_eq!(completions, 2);
    }

    #[test]
    fn pending_jobs_from_different_caches_do_not_replace_each_other() {
        let barrier = Arc::new(Barrier::new(2));
        let started = Arc::new(AtomicBool::new(false));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let encoder = {
            let barrier = Arc::clone(&barrier);
            let started = Arc::clone(&started);
            let executed = Arc::clone(&executed);
            Arc::new(
                move |image: Arc<DynamicImage>, picker: ImagePicker, area: Size| {
                    executed.lock().unwrap().push(image.width());
                    if !started.swap(true, Ordering::AcqRel) {
                        barrier.wait();
                    }
                    encode_protocol(image, picker, area)
                },
            )
        };
        let pool = ImageProtocolPool::new(1, encoder).unwrap();
        let (completion_tx, _completion_rx) = channel();
        let desired = |revision| DesiredImageProtocol {
            key: key(revision, 0),
            image: Arc::new(DynamicImage::new_rgba8(revision as u32, 1)),
            picker: ImagePicker::test_halfblocks(),
        };

        pool.submit(10, desired(1), completion_tx.clone());
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        pool.submit(10, desired(2), completion_tx.clone());
        pool.submit(20, desired(4), completion_tx.clone());
        pool.submit(10, desired(3), completion_tx);
        barrier.wait();
        for _ in 0..200 {
            if executed.lock().unwrap().len() == 3 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }

        assert_eq!(*executed.lock().unwrap(), [1, 4, 3]);
    }

    #[test]
    fn stable_key_is_submitted_only_once() {
        let mut cache = ImageProtocolCache::new();
        let image = Arc::new(DynamicImage::new_rgba8(2, 2));
        let picker = ImagePicker::test_halfblocks();
        let key = ImageProtocolKey::new(1, 0, &image, Size::new(20, 10), picker);
        let desired = || DesiredImageProtocol {
            key,
            image: Arc::clone(&image),
            picker,
        };

        cache.update(vec![desired()]);
        cache.update(vec![desired()]);
        cache.update(vec![desired()]);

        assert_eq!(cache.submissions, 1);
    }

    #[test]
    fn stale_completion_does_not_replace_a_new_revision() {
        let mut cache = ImageProtocolCache::new();
        let picker = ImagePicker::test_halfblocks();
        let old_image = Arc::new(DynamicImage::new_rgba8(1, 1));
        let new_image = Arc::new(DynamicImage::new_rgba8(2, 2));
        let old_key = ImageProtocolKey::new(1, 0, &old_image, Size::new(20, 10), picker);
        let new_key = ImageProtocolKey::new(2, 0, &new_image, Size::new(20, 10), picker);
        cache.entries.insert(
            0,
            (
                new_key,
                CachedProtocol::Pending(Arc::new(AtomicBool::new(false))),
            ),
        );
        let old_protocol = encode_protocol(old_image, picker, old_key.area).unwrap();
        cache
            .completion_tx
            .send(ProtocolCompletion {
                key: old_key,
                result: Ok(old_protocol),
            })
            .unwrap();

        cache.collect();

        assert!(matches!(
            cache.entries.get(&0),
            Some((key, CachedProtocol::Pending(_))) if *key == new_key
        ));
    }

    #[test]
    fn clearing_cache_cancels_pending_work() {
        let mut cache = ImageProtocolCache::new();
        let cancellation = Arc::new(AtomicBool::new(false));
        cache.entries.insert(
            0,
            (
                key(1, 0),
                CachedProtocol::Pending(Arc::clone(&cancellation)),
            ),
        );

        cache.clear();

        assert!(cache.entries.is_empty());
        assert!(cancellation.load(Ordering::Acquire));
    }

    #[test]
    fn encoder_panic_becomes_a_failure_and_worker_continues() {
        let calls = Arc::new(AtomicU64::new(0));
        let encoder = {
            let calls = Arc::clone(&calls);
            Arc::new(
                move |image: Arc<DynamicImage>, picker: ImagePicker, area: Size| {
                    if calls.fetch_add(1, Ordering::AcqRel) == 0 {
                        panic!("test encoder panic");
                    }
                    encode_protocol(image, picker, area)
                },
            )
        };
        let pool = ImageProtocolPool::new(1, encoder).unwrap();
        let (completion_tx, completion_rx) = channel();
        let desired = |revision| DesiredImageProtocol {
            key: key(revision, 0),
            image: Arc::new(DynamicImage::new_rgba8(1, 1)),
            picker: ImagePicker::test_halfblocks(),
        };

        pool.submit(1, desired(1), completion_tx.clone());
        let failed = completion_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let Err(error) = failed.result else {
            panic!("panicking encoder unexpectedly succeeded");
        };
        assert!(error.contains("panicked"));

        pool.submit(1, desired(2), completion_tx);
        let completed = completion_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(completed.result.is_ok());
    }
}
