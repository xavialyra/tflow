use crate::cancellation::{CancellationToken, FINAL_OUTPUT_FLAG, SIGNAL_MASK};
use signal_hook::SigId;
use signal_hook::low_level;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

const SIGNALS: [libc::c_int; 4] = [libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT, libc::SIGINT];

#[derive(Debug)]
pub(crate) struct SignalGuard {
    state: Arc<AtomicI32>,
    handlers: Vec<(libc::c_int, SigId)>,
}

impl Default for SignalGuard {
    fn default() -> Self {
        Self {
            state: Arc::new(AtomicI32::new(0)),
            handlers: Vec::new(),
        }
    }
}

impl SignalGuard {
    pub(crate) fn install() -> io::Result<Self> {
        let mut guard = Self::default();
        for signal in SIGNALS {
            let state = Arc::clone(&guard.state);
            let id = unsafe {
                low_level::register(signal, move || {
                    if state
                        .compare_exchange(0, signal, Ordering::Release, Ordering::Relaxed)
                        .is_err()
                    {
                        libc::_exit(128 + signal);
                    }
                })
            }?;
            guard.handlers.push((signal, id));
        }
        Ok(guard)
    }

    pub(crate) fn received(&self) -> Option<i32> {
        let signal = self.state.load(Ordering::Acquire) & SIGNAL_MASK;
        (signal != 0).then_some(signal)
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        CancellationToken::with_signal(Arc::clone(&self.state))
    }

    pub(crate) fn enter_final_output(&self) {
        self.state.fetch_or(FINAL_OUTPUT_FLAG, Ordering::AcqRel);
    }
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (_, id) in self.handlers.drain(..) {
            low_level::unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_output_flag_shares_state_without_becoming_a_signal() {
        let guard = SignalGuard::default();
        assert_eq!(guard.received(), None);
        assert!(!guard.cancellation_token().is_cancelled());

        guard.enter_final_output();
        assert_eq!(guard.received(), None);
        assert!(!guard.cancellation_token().is_cancelled());

        guard.state.store(libc::SIGTERM, Ordering::Release);
        guard.enter_final_output();
        assert_eq!(guard.received(), Some(libc::SIGTERM));
        assert!(guard.cancellation_token().is_cancelled());
    }
}
