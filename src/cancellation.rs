use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

pub(crate) const FINAL_OUTPUT_FLAG: i32 = 1 << 30;
pub(crate) const SIGNAL_MASK: i32 = FINAL_OUTPUT_FLAG - 1;

#[derive(Debug, Clone)]
pub(crate) struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    signal: Option<Arc<AtomicI32>>,
}

impl CancellationToken {
    pub(crate) fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            signal: None,
        }
    }

    pub(crate) fn with_signal(signal: Arc<AtomicI32>) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            signal: Some(signal),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .signal
                .as_ref()
                .is_some_and(|signal| signal.load(Ordering::Acquire) & SIGNAL_MASK != 0)
    }
}
