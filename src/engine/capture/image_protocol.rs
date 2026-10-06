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
use std::time::{Duration, Instant};

// Decoding a source can temporarily allocate tens of MiB. Serialize it across views.
const MAX_CONCURRENT_PROTOCOL_ENCODES: usize = 1;
static IMAGE_PROTOCOL_POOL: OnceLock<std::result::Result<ImageProtocolPool, String>> =
    OnceLock::new();
static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);

type EncodeResult = std::result::Result<StatefulProtocol, String>;
type Encoder = dyn Fn(Arc<ImageSource>, ImagePicker, Size) -> EncodeResult + Send + Sync + 'static;

pub(crate) struct ImageSource {
    id: usize,
    path: std::path::PathBuf,
    #[cfg(test)]
    pixels: Option<DynamicImage>,
}

impl ImageSource {
    pub(crate) fn file(path: std::path::PathBuf) -> Self {
        static NEXT_SOURCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        Self {
            id: NEXT_SOURCE.fetch_add(1, Ordering::Relaxed),
            path,
            #[cfg(test)]
            pixels: None,
        }
    }

    fn decode(&self) -> Result<DynamicImage, String> {
        #[cfg(test)]
        if let Some(image) = &self.pixels {
            return Ok(image.clone());
        }
        super::image_decode::decode_image(&self.path)
    }

    #[cfg(test)]
    pub(crate) fn new_rgba8(width: u32, height: u32) -> Self {
        Self {
            pixels: Some(DynamicImage::new_rgba8(width, height)),
            ..Self::file(Default::default())
        }
    }

    #[cfg(test)]
    fn width(&self) -> u32 {
        self.pixels.as_ref().unwrap().width()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ImageProtocolKey {
    block: usize,
    image: usize,
    area: Size,
    picker: ImagePickerFingerprint,
}

impl ImageProtocolKey {
    pub(crate) fn new(
        block: usize,
        image: &Arc<ImageSource>,
        area: Size,
        picker: ImagePicker,
    ) -> Self {
        Self {
            block,
            image: image.id,
            area,
            picker: picker.fingerprint(),
        }
    }
}

pub(crate) struct DesiredImageProtocol {
    pub(crate) key: ImageProtocolKey,
    pub(crate) image: Arc<ImageSource>,
    pub(crate) picker: ImagePicker,
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
    image: Arc<ImageSource>,
    picker: ImagePicker,
    cancellation: Arc<AtomicBool>,
    completion: Sender<ProtocolCompletion>,
    not_before: Instant,
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
    debounce: Duration,
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
        Ok(Self {
            shared,
            workers,
            debounce: Duration::ZERO,
        })
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
            not_before: Instant::now() + self.debounce,
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
            loop {
                if state.closed {
                    return;
                }
                state
                    .jobs
                    .retain(|job| !job.cancellation.load(Ordering::Acquire));
                if let Some(job) = state.jobs.front() {
                    let remaining = job.not_before.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    state = shared
                        .ready
                        .wait_timeout(state, remaining)
                        .expect("image protocol queue was poisoned")
                        .0;
                } else {
                    state = shared
                        .ready
                        .wait(state)
                        .expect("image protocol queue was poisoned");
                }
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
            .map(|mut pool| {
                // Coalesce rapid selection/resize events before opening files.
                pool.debounce = Duration::from_millis(100);
                pool
            })
            .map_err(|error| format!("could not start image protocol workers: {error}"))
    }) {
        Ok(pool) => Ok(pool),
        Err(error) => Err(error.clone()),
    }
}

fn encode_protocol(image: Arc<ImageSource>, picker: ImagePicker, area: Size) -> EncodeResult {
    let image = display_image(&image, picker, area)?;
    let mut protocol = picker.new_resize_protocol(image);
    let resize = Resize::Fit(None);
    let encoded_size = protocol.size_for(resize.clone(), area);
    protocol.resize_encode(&resize, encoded_size);
    match protocol.last_encoding_result() {
        Some(Ok(())) => Ok(protocol),
        Some(Err(error)) => Err(error.to_string()),
        None => Err("image protocol did not produce an encoding result".to_string()),
    }
}

fn display_image(
    source: &ImageSource,
    picker: ImagePicker,
    area: Size,
) -> Result<DynamicImage, String> {
    if area.width == 0 || area.height == 0 {
        return Err("image display area is empty".into());
    }
    let image = source.decode()?;
    let (width, height) = picker.image_pixel_bounds(area);
    // thumbnail never enlarges the source. The full decoded image is dropped
    // before constructing a protocol, which only owns display-sized pixels.
    if image.width() <= width && image.height() <= height {
        Ok(image)
    } else {
        Ok(image.thumbnail(width, height))
    }
}

const PROTOCOL_CACHE_CAPACITY: usize = 32;

pub(crate) struct ImageProtocolCache {
    id: u64,
    entries: HashMap<ImageProtocolKey, CachedProtocol>,
    order: VecDeque<ImageProtocolKey>,
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
            order: VecDeque::new(),
            completion_tx,
            completion_rx,
            #[cfg(test)]
            submissions: 0,
        }
    }

    pub(crate) fn update(&mut self, desired: Vec<DesiredImageProtocol>) {
        // Encoded protocols can own large, terminal-specific image buffers.
        // Keep only protocols needed by the current frame: retaining every
        // previously visible image made scrolling through a few large images
        // grow the process substantially (up to the entry-count limit).
        self.entries.retain(|key, state| {
            if !desired.iter().any(|image| image.key == *key) {
                if let CachedProtocol::Pending(cancellation) = state {
                    cancellation.store(true, Ordering::Release);
                }
                return false;
            }
            true
        });
        self.order.retain(|key| self.entries.contains_key(key));
        self.collect();

        for desired in desired {
            if self.entries.contains_key(&desired.key) {
                if let Some(pos) = self.order.iter().position(|k| *k == desired.key) {
                    self.order.remove(pos);
                }
                self.order.push_back(desired.key);
                continue;
            }

            while self.entries.len() >= PROTOCOL_CACHE_CAPACITY {
                if let Some(oldest) = self.order.pop_front() {
                    if let Some(CachedProtocol::Pending(cancellation)) =
                        self.entries.remove(&oldest)
                    {
                        cancellation.store(true, Ordering::Release);
                    }
                } else {
                    break;
                }
            }

            let key = desired.key;
            let cancellation = match default_pool() {
                Ok(pool) => pool.submit(self.id, desired, self.completion_tx.clone()),
                Err(error) => {
                    self.entries.insert(key, CachedProtocol::Failed(error));
                    self.order.push_back(key);
                    continue;
                }
            };
            #[cfg(test)]
            {
                self.submissions += 1;
            }
            self.entries
                .insert(key, CachedProtocol::Pending(cancellation));
            self.order.push_back(key);
        }
    }

    pub(crate) fn clear(&mut self) {
        for (_, state) in self.entries.drain() {
            if let CachedProtocol::Pending(cancellation) = state {
                cancellation.store(true, Ordering::Release);
            }
        }
        self.order.clear();
    }

    pub(crate) fn collect(&mut self) -> bool {
        let mut changed = false;
        while let Ok(completed) = self.completion_rx.try_recv() {
            let Some(state) = self.entries.get_mut(&completed.key) else {
                continue;
            };
            changed = true;
            *state = match completed.result {
                Ok(protocol) => CachedProtocol::Ready(Box::new(protocol)),
                Err(error) => CachedProtocol::Failed(error),
            };
        }
        changed
    }

    pub(crate) fn protocol(&mut self, key: ImageProtocolKey) -> Option<&mut StatefulProtocol> {
        // Collect once before traversing the document, not between image leaves.
        // Otherwise a later leaf can consume an earlier leaf's completion
        // without painting it or leaving an invalidation for the next tick.
        match self.entries.get_mut(&key) {
            Some(CachedProtocol::Ready(protocol)) => {
                if let Some(pos) = self.order.iter().position(|k| *k == key) {
                    self.order.remove(pos);
                }
                self.order.push_back(key);
                Some(protocol.as_mut())
            }
            _ => None,
        }
    }

    pub(crate) fn error(&self, key: ImageProtocolKey) -> Option<&str> {
        match self.entries.get(&key) {
            Some(CachedProtocol::Failed(error)) => Some(error.as_str()),
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

    #[test]
    fn debounced_requests_encode_only_the_latest_size() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = count.clone();
        let mut pool = ImageProtocolPool::new(
            1,
            Arc::new(move |image, picker, area| {
                calls.fetch_add(1, Ordering::Relaxed);
                encode_protocol(image, picker, area)
            }),
        )
        .unwrap();
        pool.debounce = Duration::from_millis(100);
        let (tx, rx) = channel();
        let image = Arc::new(ImageSource::new_rgba8(2, 2));
        let picker = ImagePicker::test_halfblocks();
        for width in 20..40 {
            let key = ImageProtocolKey::new(0, &image, Size::new(width, 10), picker);
            pool.submit(
                1,
                DesiredImageProtocol {
                    key,
                    image: image.clone(),
                    picker,
                },
                tx.clone(),
            );
        }
        let completion = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(completion.key.area.width, 39);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn display_pixels_follow_area_without_upscaling() {
        let source = ImageSource::new_rgba8(1000, 500);
        let picker = ImagePicker::test_halfblocks();
        let small = display_image(&source, picker, Size::new(20, 10)).unwrap();
        assert_eq!((small.width(), small.height()), (200, 100));
        let large = display_image(&source, picker, Size::new(40, 20)).unwrap();
        assert_eq!((large.width(), large.height()), (400, 200));
        let original = display_image(&source, picker, Size::new(200, 100)).unwrap();
        assert_eq!((original.width(), original.height()), (1000, 500));
    }

    #[test]
    fn resizing_reloads_file_and_failed_requests_are_not_retried_each_frame() {
        let path = std::env::temp_dir().join(format!("tflow-resize-{}.png", std::process::id()));
        DynamicImage::new_rgba8(1000, 500).save(&path).unwrap();
        let source = Arc::new(ImageSource::file(path.clone()));
        let picker = ImagePicker::test_halfblocks();
        let mut cache = ImageProtocolCache::new();
        let request = |area| DesiredImageProtocol {
            key: ImageProtocolKey::new(0, &source, area, picker),
            image: source.clone(),
            picker,
        };
        let small = Size::new(20, 10);
        let small_key = request(small).key;
        cache.update(vec![request(small)]);
        for _ in 0..1000 {
            cache.collect();
            if cache.protocol(small_key).is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(cache.protocol(small_key).is_some());
        std::fs::remove_file(path).unwrap();
        cache.update(vec![request(small)]);
        assert_eq!(cache.submissions, 1);
        let large = Size::new(40, 20);
        let large_key = request(large).key;
        cache.update(vec![request(large)]);
        for _ in 0..1000 {
            cache.collect();
            if cache.error(large_key).is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            cache.error(large_key).is_some(),
            "resize must reopen the file"
        );
        assert!(!cache.entries.contains_key(&small_key));
        cache.update(vec![request(large)]);
        assert_eq!(cache.submissions, 2);
    }

    // Run in separate processes for comparable RSS/HWM. Baseline reproduces
    // the former source + full-resolution protocol ownership, not the whole UI.
    #[test]
    #[ignore = "manual memory probe: TFLOW_IMAGE_DIR and optional TFLOW_IMAGE_BASELINE"]
    fn wallpaper_memory_probe() {
        let directory = std::env::var("TFLOW_IMAGE_DIR").expect("set TFLOW_IMAGE_DIR");
        let baseline = std::env::var_os("TFLOW_IMAGE_BASELINE").is_some();
        let picker = ImagePicker::test_halfblocks();
        let area = Size::new(80, 24);
        let files = [
            "70022444_p0.jpg",
            "FtWuV8uWIAg9tYH",
            "FjWyBEdWYAAu07Q.jpg",
            "E7okZhGWUAIKu5H.jpg",
        ];
        let mut held = Vec::new();
        for name in files {
            let source = ImageSource::file(std::path::Path::new(&directory).join(name));
            if baseline {
                let original = source.decode().unwrap();
                let mut protocol = picker.new_resize_protocol(original.clone());
                let resize = Resize::Fit(None);
                let size = protocol.size_for(resize.clone(), area);
                protocol.resize_encode(&resize, size);
                held.push((Some(original), protocol));
            } else {
                let thumbnail = display_image(&source, picker, area).unwrap();
                eprintln!(
                    "{name}: display={}x{}, bytes={}",
                    thumbnail.width(),
                    thumbnail.height(),
                    thumbnail.as_bytes().len()
                );
                drop(thumbnail);
                held.push((
                    None,
                    encode_protocol(Arc::new(source), picker, area).unwrap(),
                ));
            }
            let status = std::fs::read_to_string("/proc/self/status").unwrap();
            eprintln!(
                "{name}: {}",
                status
                    .lines()
                    .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        std::hint::black_box(&held);
    }

    fn key(block: usize, image_id: usize) -> ImageProtocolKey {
        ImageProtocolKey {
            block,
            image: image_id,
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
                move |image: Arc<ImageSource>, picker: ImagePicker, area: Size| {
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
            key: key(0, revision),
            image: Arc::new(ImageSource::new_rgba8(revision as u32, 1)),
            picker: ImagePicker::test_halfblocks(),
        };
        let first = pool.submit(1, submit(1), completion_tx.clone());
        while !started.load(Ordering::Acquire) {
            thread::yield_now();
        }
        let _old = pool.submit(1, submit(2), completion_tx.clone());
        let _latest = pool.submit(1, submit(3), completion_tx);
        barrier.wait();
        let mut completions = Vec::new();
        while completions.len() < 2 {
            if let Ok(c) = completion.recv_timeout(Duration::from_millis(500)) {
                completions.push(c);
            } else {
                break;
            }
        }
        first.store(true, Ordering::Release);
        assert_eq!(*executed.lock().unwrap(), [1, 3]);
        assert_eq!(completions.len(), 2);
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
                move |image: Arc<ImageSource>, picker: ImagePicker, area: Size| {
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
            key: key(0, revision),
            image: Arc::new(ImageSource::new_rgba8(revision as u32, 1)),
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
    fn completion_between_image_lookups_remains_available_to_the_next_tick() {
        let mut cache = ImageProtocolCache::new();
        let picker = ImagePicker::test_halfblocks();
        let image = Arc::new(ImageSource::new_rgba8(2, 2));
        let first = key(0, 1);
        let second = key(1, 2);
        cache.entries.insert(
            first,
            CachedProtocol::Pending(Arc::new(AtomicBool::new(false))),
        );
        cache.entries.insert(
            second,
            CachedProtocol::Ready(Box::new(
                encode_protocol(Arc::clone(&image), picker, second.area).unwrap(),
            )),
        );
        cache.order.extend([first, second]);

        assert!(cache.protocol(first).is_none());
        // First image finishes after its leaf was skipped in this frame.
        cache
            .completion_tx
            .send(ProtocolCompletion {
                key: first,
                result: Ok(encode_protocol(image, picker, first.area).unwrap()),
            })
            .unwrap();
        assert!(cache.protocol(second).is_some());
        assert!(
            cache.collect(),
            "next tick must request a redraw for the first image"
        );
        assert!(cache.protocol(first).is_some());
        assert!(!cache.collect(), "completion must invalidate only once");
    }

    #[test]
    fn stable_key_is_submitted_only_once() {
        let mut cache = ImageProtocolCache::new();
        let image = Arc::new(ImageSource::new_rgba8(2, 2));
        let picker = ImagePicker::test_halfblocks();
        let key = ImageProtocolKey::new(0, &image, Size::new(20, 10), picker);
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
        let old_image = Arc::new(ImageSource::new_rgba8(1, 1));
        let new_image = Arc::new(ImageSource::new_rgba8(2, 2));
        let old_key = ImageProtocolKey::new(0, &old_image, Size::new(20, 10), picker);
        let new_key = ImageProtocolKey::new(0, &new_image, Size::new(20, 10), picker);
        cache.entries.insert(
            new_key,
            CachedProtocol::Pending(Arc::new(AtomicBool::new(false))),
        );
        cache.order.push_back(new_key);
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
            cache.entries.get(&new_key),
            Some(CachedProtocol::Pending(_))
        ));
    }

    #[test]
    fn updating_visible_images_cancels_only_obsolete_pending_work() {
        let mut cache = ImageProtocolCache::new();
        let picker = ImagePicker::test_halfblocks();
        let image = Arc::new(ImageSource::new_rgba8(2, 2));
        let current = ImageProtocolKey::new(0, &image, Size::new(20, 10), picker);
        let obsolete = key(0, 1);
        let ready = key(1, 2);
        let cancelled = Arc::new(AtomicBool::new(false));
        let retained = Arc::new(AtomicBool::new(false));
        cache
            .entries
            .insert(obsolete, CachedProtocol::Pending(Arc::clone(&cancelled)));
        cache
            .entries
            .insert(current, CachedProtocol::Pending(Arc::clone(&retained)));
        cache.entries.insert(
            ready,
            CachedProtocol::Ready(Box::new(
                encode_protocol(Arc::clone(&image), picker, ready.area).unwrap(),
            )),
        );
        cache.order.extend([obsolete, current, ready]);
        cache
            .completion_tx
            .send(ProtocolCompletion {
                key: obsolete,
                result: Err("late completion".to_string()),
            })
            .unwrap();

        cache.update(vec![DesiredImageProtocol {
            key: current,
            image,
            picker,
        }]);

        assert!(cancelled.load(Ordering::Acquire));
        assert!(!retained.load(Ordering::Acquire));
        assert!(!cache.entries.contains_key(&obsolete));
        assert!(!cache.order.contains(&obsolete));
        assert!(!cache.entries.contains_key(&ready));
        assert_eq!(cache.submissions, 0);

        cache.update(Vec::new());

        assert!(retained.load(Ordering::Acquire));
        assert!(!cache.entries.contains_key(&current));
        assert!(!cache.order.contains(&current));
        assert!(!cache.entries.contains_key(&ready));
    }

    #[test]
    fn clearing_cache_cancels_pending_work() {
        let mut cache = ImageProtocolCache::new();
        let cancellation = Arc::new(AtomicBool::new(false));
        let k = key(0, 1);
        cache
            .entries
            .insert(k, CachedProtocol::Pending(Arc::clone(&cancellation)));
        cache.order.push_back(k);

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
                move |image: Arc<ImageSource>, picker: ImagePicker, area: Size| {
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
            key: key(0, revision),
            image: Arc::new(ImageSource::new_rgba8(1, 1)),
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
