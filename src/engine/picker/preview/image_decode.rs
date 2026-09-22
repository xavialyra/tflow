use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

pub(crate) const MAX_IMAGE_WIDTH: u32 = 8192;
pub(crate) const MAX_IMAGE_HEIGHT: u32 = 8192;
pub(crate) const MAX_IMAGE_DECODE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_IMAGE_ENCODED_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_CONCURRENT_IMAGE_DECODES: usize = 2;

type DecodeResult = std::result::Result<DynamicImage, String>;
type Decoder = dyn Fn(&Path) -> DecodeResult + Send + Sync + 'static;

pub(crate) struct DecodedImage {
    pub(crate) block: usize,
    pub(crate) path: PathBuf,
    pub(crate) result: DecodeResult,
}

pub(crate) struct ImageDecodeBatch {
    pub(crate) revision: u64,
    pub(crate) images: Vec<DecodedImage>,
}

pub(crate) struct ImageDecodeHandle {
    cancellation: Arc<AtomicBool>,
    completion: Receiver<ImageDecodeBatch>,
}

impl ImageDecodeHandle {
    pub(crate) fn try_recv(&self) -> std::result::Result<ImageDecodeBatch, TryRecvError> {
        self.completion.try_recv()
    }
}

impl Drop for ImageDecodeHandle {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::Release);
    }
}

struct ImageDecodeJob {
    revision: u64,
    images: Vec<(usize, PathBuf)>,
    cancellation: Arc<AtomicBool>,
    completion: std::sync::mpsc::SyncSender<ImageDecodeBatch>,
}

struct PoolState {
    job: Option<ImageDecodeJob>,
    closed: bool,
}

struct SharedPool {
    state: Mutex<PoolState>,
    ready: Condvar,
    decoder: Arc<Decoder>,
}

pub(super) struct ImageDecodePool {
    shared: Arc<SharedPool>,
    workers: Vec<JoinHandle<()>>,
}

impl ImageDecodePool {
    fn new(worker_count: usize, decoder: Arc<Decoder>) -> std::io::Result<Self> {
        assert!(worker_count > 0, "image decode pool needs a worker");
        let shared = Arc::new(SharedPool {
            state: Mutex::new(PoolState {
                job: None,
                closed: false,
            }),
            ready: Condvar::new(),
            decoder,
        });
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let worker = Arc::clone(&shared);
            match thread::Builder::new()
                .name(format!("tui-image-decode-{index}"))
                .spawn(move || image_worker(worker))
            {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    let mut state = shared
                        .state
                        .lock()
                        .expect("image decode queue was poisoned");
                    state.closed = true;
                    drop(state);
                    shared.ready.notify_all();
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self { shared, workers })
    }

    pub(super) fn submit(&self, revision: u64, images: Vec<(usize, PathBuf)>) -> ImageDecodeHandle {
        let cancellation = Arc::new(AtomicBool::new(false));
        let (completion, receiver) = sync_channel(1);
        let job = ImageDecodeJob {
            revision,
            images,
            cancellation: Arc::clone(&cancellation),
            completion,
        };
        let mut state = self
            .shared
            .state
            .lock()
            .expect("image decode queue was poisoned");
        if let Some(previous) = state.job.replace(job) {
            previous.cancellation.store(true, Ordering::Release);
        }
        drop(state);
        self.shared.ready.notify_one();
        ImageDecodeHandle {
            cancellation,
            completion: receiver,
        }
    }
}

impl Drop for ImageDecodePool {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("image decode queue was poisoned");
        state.closed = true;
        if let Some(job) = state.job.take() {
            job.cancellation.store(true, Ordering::Release);
        }
        drop(state);
        self.shared.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn image_worker(shared: Arc<SharedPool>) {
    loop {
        let job = {
            let mut state = shared
                .state
                .lock()
                .expect("image decode queue was poisoned");
            while state.job.is_none() && !state.closed {
                state = shared
                    .ready
                    .wait(state)
                    .expect("image decode queue was poisoned");
            }
            if state.closed {
                return;
            }
            state.job.take().expect("image job disappeared")
        };

        if job.cancellation.load(Ordering::Acquire) {
            continue;
        }
        let mut images = Vec::with_capacity(job.images.len());
        for (block, path) in job.images {
            if job.cancellation.load(Ordering::Acquire) {
                break;
            }
            let result = (shared.decoder)(&path);
            if job.cancellation.load(Ordering::Acquire) {
                break;
            }
            images.push(DecodedImage {
                block,
                path,
                result,
            });
        }
        if !job.cancellation.load(Ordering::Acquire) {
            let _ = job.completion.send(ImageDecodeBatch {
                revision: job.revision,
                images,
            });
        }
    }
}

pub(super) fn new_default_pool() -> std::result::Result<Arc<ImageDecodePool>, String> {
    ImageDecodePool::new(MAX_CONCURRENT_IMAGE_DECODES, Arc::new(decode_image))
        .map(Arc::new)
        .map_err(|error| format!("could not start bounded image decode workers: {error}"))
}

fn decode_image(path: &Path) -> DecodeResult {
    let file = open_image_file(path)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_WIDTH);
    limits.max_image_height = Some(MAX_IMAGE_HEIGHT);
    limits.max_alloc = Some(MAX_IMAGE_DECODE_BYTES);
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|error| {
        format!("image exceeds configured dimensions or allocation limits: {error}")
    })?;
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    validate_decoded_bytes(decoder.total_bytes())?;
    DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())
}

fn open_image_file(path: &Path) -> std::result::Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "image path {} is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_IMAGE_ENCODED_BYTES {
        return Err(format!(
            "image source {} is {} bytes, exceeding the {}-byte encoded limit",
            path.display(),
            metadata.len(),
            MAX_IMAGE_ENCODED_BYTES
        ));
    }
    Ok(file)
}

fn validate_dimensions(width: u32, height: u32) -> std::result::Result<(), String> {
    if width > MAX_IMAGE_WIDTH || height > MAX_IMAGE_HEIGHT {
        return Err(format!(
            "image dimensions {width}x{height} exceed the {MAX_IMAGE_WIDTH}x{MAX_IMAGE_HEIGHT} limit"
        ));
    }
    Ok(())
}

fn validate_decoded_bytes(decoded_bytes: u64) -> std::result::Result<(), String> {
    if decoded_bytes > MAX_IMAGE_DECODE_BYTES {
        return Err(format!(
            "image requires {decoded_bytes} decoded bytes, exceeding the {MAX_IMAGE_DECODE_BYTES}-byte limit"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use std::fs::{self, File};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Barrier, Mutex};
    use std::time::Duration;

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tflow-image-{}-{name}", std::process::id()))
    }

    #[test]
    fn pool_enforces_its_worker_limit() {
        let running = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(MAX_CONCURRENT_IMAGE_DECODES + 1));
        let (started_tx, started_rx) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let decoder = {
            let running = Arc::clone(&running);
            let maximum = Arc::clone(&maximum);
            let barrier = Arc::clone(&barrier);
            let calls = Arc::clone(&calls);
            Arc::new(move |_path: &Path| {
                let current = running.fetch_add(1, Ordering::AcqRel) + 1;
                maximum.fetch_max(current, Ordering::AcqRel);
                let call = calls.fetch_add(1, Ordering::AcqRel);
                if call == 0 {
                    started_tx.send(()).unwrap();
                }
                if call < MAX_CONCURRENT_IMAGE_DECODES {
                    barrier.wait();
                }
                running.fetch_sub(1, Ordering::AcqRel);
                Ok(DynamicImage::new_rgba8(1, 1))
            })
        };
        let pool = ImageDecodePool::new(MAX_CONCURRENT_IMAGE_DECODES, decoder).unwrap();
        let first = pool.submit(1, vec![(0, PathBuf::from("first"))]);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let second = pool.submit(2, vec![(0, PathBuf::from("second"))]);
        barrier.wait();
        let handles = vec![first, second];
        for handle in handles {
            handle
                .completion
                .recv_timeout(Duration::from_secs(1))
                .expect("image batch did not finish");
        }
        assert_eq!(
            maximum.load(Ordering::Acquire),
            MAX_CONCURRENT_IMAGE_DECODES
        );
    }

    #[test]
    fn latest_pending_selection_replaces_the_old_one() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let decoder = {
            let executed = Arc::clone(&executed);
            let release_rx = Arc::clone(&release_rx);
            Arc::new(move |path: &Path| {
                let name = path.to_string_lossy().to_string();
                executed.lock().unwrap().push(name.clone());
                if name == "running" {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }
                Ok(DynamicImage::new_rgba8(1, 1))
            })
        };
        let pool = ImageDecodePool::new(1, decoder).unwrap();
        let running = pool.submit(1, vec![(0, PathBuf::from("running"))]);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let old = pool.submit(2, vec![(0, PathBuf::from("old"))]);
        let latest = pool.submit(3, vec![(0, PathBuf::from("latest"))]);
        drop(old);
        release_tx.send(()).unwrap();

        running
            .completion
            .recv_timeout(Duration::from_secs(1))
            .expect("running batch did not finish");
        let result = latest
            .completion
            .recv_timeout(Duration::from_secs(1))
            .expect("latest batch did not finish");
        assert_eq!(result.revision, 3);
        assert_eq!(*executed.lock().unwrap(), ["running", "latest"]);
    }

    #[test]
    fn separate_pools_do_not_cancel_each_other() {
        let decoder: Arc<Decoder> = Arc::new(|_path: &Path| Ok(DynamicImage::new_rgba8(1, 1)));
        let first_pool = ImageDecodePool::new(1, Arc::clone(&decoder)).unwrap();
        let second_pool = ImageDecodePool::new(1, Arc::clone(&decoder)).unwrap();
        let first = first_pool.submit(1, vec![(0, PathBuf::from("first"))]);
        let second = second_pool.submit(2, vec![(0, PathBuf::from("second"))]);

        assert_eq!(
            first
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            1
        );
        assert_eq!(
            second
                .completion
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn batch_cancellation_stops_before_the_next_image() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let executed = Arc::new(Mutex::new(Vec::new()));
        let decoder = {
            let executed = Arc::clone(&executed);
            let release_rx = Arc::clone(&release_rx);
            Arc::new(move |path: &Path| {
                executed
                    .lock()
                    .unwrap()
                    .push(path.to_string_lossy().to_string());
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                Ok(DynamicImage::new_rgba8(1, 1))
            })
        };
        let pool = ImageDecodePool::new(1, decoder).unwrap();
        let handle = pool.submit(
            1,
            vec![(0, PathBuf::from("first")), (1, PathBuf::from("second"))],
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(handle);
        release_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(*executed.lock().unwrap(), ["first"]);
    }

    #[test]
    fn decodes_a_valid_image_through_the_real_decoder() {
        let path = temporary_path("valid.png");
        DynamicImage::new_rgba8(2, 3).save(&path).unwrap();
        let decoded = decode_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (2, 3));
        fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_nonregular_and_oversized_encoded_sources_before_decoding() {
        use std::ffi::CString;

        let oversized = temporary_path("oversized-source.bin");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_IMAGE_ENCODED_BYTES + 1).unwrap();
        let error = decode_image(&oversized).unwrap_err();
        assert!(error.contains("encoded limit"), "error: {error}");
        fs::remove_file(&oversized).unwrap();

        let fifo = temporary_path("source.fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let error = decode_image(&fifo).unwrap_err();
        assert!(error.contains("not a regular file"), "error: {error}");
        fs::remove_file(fifo).unwrap();
    }

    #[test]
    fn rejects_oversized_and_corrupt_images_with_readable_errors() {
        let oversized = temporary_path("oversized.bmp");
        let corrupt = temporary_path("corrupt.png");
        let write_bmp_header = |path: &Path, width: u32, height: u32| {
            let mut header = vec![0_u8; 54];
            header[..2].copy_from_slice(b"BM");
            header[2..6].copy_from_slice(&54_u32.to_le_bytes());
            header[10..14].copy_from_slice(&54_u32.to_le_bytes());
            header[14..18].copy_from_slice(&40_u32.to_le_bytes());
            header[18..22].copy_from_slice(&width.to_le_bytes());
            header[22..26].copy_from_slice(&height.to_le_bytes());
            header[26..28].copy_from_slice(&1_u16.to_le_bytes());
            header[28..30].copy_from_slice(&24_u16.to_le_bytes());
            fs::write(path, header).unwrap();
        };
        write_bmp_header(&oversized, MAX_IMAGE_WIDTH + 1, 1);
        fs::write(&corrupt, b"not a png").unwrap();

        let oversized_error = decode_image(&oversized).unwrap_err();
        assert!(oversized_error.contains("limit"), "{oversized_error}");
        let allocation_error = validate_decoded_bytes(MAX_IMAGE_DECODE_BYTES + 1).unwrap_err();
        assert!(
            allocation_error.contains("decoded bytes"),
            "{allocation_error}"
        );
        let corrupt_error = decode_image(&corrupt).unwrap_err();
        assert!(!corrupt_error.is_empty());

        fs::remove_file(oversized).unwrap();
        fs::remove_file(corrupt).unwrap();
    }
}
