//! Bounded retention of the previous base View frame across navigation.
//!
//! A navigation or replace mounts a fresh base View. While that View is still
//! loading, the host paints the frame captured from the View it replaced inside
//! the region the new View declares as safe, so the transition does not flash an
//! empty surface. Retention is owned here rather than in `render` so the state
//! machine can be exercised without rendering and so a View that never finishes
//! loading cannot re-arm stale pixels every frame.

use crate::protocol::contracts::ViewInstanceId;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the replaced frame may be retained after the target becomes active.
const RETENTION_DURATION: Duration = Duration::from_millis(150);

/// A base View frame captured while that View was settled, meaning it had fully
/// published content and was not covered by a popup or modal overlay.
///
/// Only pixels are retained. The render result of a retained frame is never
/// reused: the frame on screen always reports the current View's metadata and
/// cursor.
#[derive(Clone)]
pub(super) struct SettledFrame {
    pub(super) instance: ViewInstanceId,
    pub(super) buffer: Arc<Buffer>,
    pub(super) area: Rect,
}

/// The frame a render should retain, together with the deadline for retaining it.
#[derive(Clone)]
pub(super) struct RetainedFrame {
    pub(super) instance: ViewInstanceId,
    pub(super) expires_at: Instant,
    settled: SettledFrame,
}

#[derive(Default)]
pub(super) struct NavigationHandoff {
    settled: Option<SettledFrame>,
    active: Option<RetainedFrame>,
    /// Instance whose retention already ran to completion. Remembering it stops a
    /// View that never finishes loading from re-arming retention every frame,
    /// which would otherwise make stale pixels unbounded in time.
    consumed: Option<ViewInstanceId>,
}

impl NavigationHandoff {
    /// Decide what the upcoming render should retain.
    ///
    /// Retention applies only to a freshly mounted base View that is still
    /// loading, and only from a frame captured before that View was mounted.
    /// Instance ids follow mount order, so a frame from a newer instance means
    /// this render is returning to an older View that already owns its own
    /// rendered state and must not be covered.
    pub(super) fn begin(
        &mut self,
        instance: Option<ViewInstanceId>,
        loading: bool,
        now: Instant,
    ) -> Option<RetainedFrame> {
        let Some(instance) = instance else {
            self.active = None;
            return None;
        };
        if !loading {
            self.active = None;
            return None;
        }
        if let Some(active) = &self.active {
            if active.instance == instance {
                if now < active.expires_at {
                    return Some(active.clone());
                }
                self.active = None;
                self.consumed = Some(instance);
                return None;
            }
            self.active = None;
        }
        if self.consumed == Some(instance) {
            return None;
        }
        let settled = self.settled.as_ref().filter(|s| s.instance < instance)?;
        let retained = RetainedFrame {
            instance,
            expires_at: now + RETENTION_DURATION,
            settled: settled.clone(),
        };
        self.consumed = Some(instance);
        self.active = Some(retained.clone());
        Some(retained)
    }

    /// Record the base View's frame once it is settled and may be retained later.
    pub(super) fn settle(&mut self, frame: SettledFrame) {
        self.settled = Some(frame);
    }

    #[cfg(test)]
    pub(super) fn is_retaining(&self) -> bool {
        self.active.is_some()
    }

    #[cfg(test)]
    pub(super) fn settled(&self) -> Option<&SettledFrame> {
        self.settled.as_ref()
    }
}

/// Paint the retained region onto the frame.
///
/// The returned area is what was actually covered, so the caller can suppress a
/// cursor that would otherwise point at stale pixels.
pub(super) fn paint_retained(
    frame: &mut Frame,
    retained: &RetainedFrame,
    content_area: Rect,
    retain_area: Option<Rect>,
) -> Option<Rect> {
    let retain_area = retain_area?;
    let settled = &retained.settled;
    let copy_area = settled
        .area
        .intersection(content_area)
        .intersection(retain_area);
    if copy_area.is_empty() {
        return None;
    }
    let source = settled.buffer.as_ref();
    let destination = frame.buffer_mut();
    let width = copy_area.width as usize;
    let source_stride = source.area.width as usize;
    let destination_stride = destination.area.width as usize;
    for y in copy_area.y..copy_area.bottom() {
        let source_start =
            (y - source.area.y) as usize * source_stride + (copy_area.x - source.area.x) as usize;
        let destination_start = (y - destination.area.y) as usize * destination_stride
            + (copy_area.x - destination.area.x) as usize;
        let (Some(source_row), Some(destination_row)) = (
            source.content.get(source_start..source_start + width),
            destination
                .content
                .get_mut(destination_start..destination_start + width),
        ) else {
            continue;
        };
        destination_row.clone_from_slice(source_row);
    }
    Some(copy_area)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(instance: u64) -> SettledFrame {
        SettledFrame {
            instance: ViewInstanceId(instance),
            buffer: Arc::new(Buffer::empty(Rect::new(0, 0, 4, 2))),
            area: Rect::new(0, 0, 4, 2),
        }
    }

    fn id(value: u64) -> Option<ViewInstanceId> {
        Some(ViewInstanceId(value))
    }

    #[test]
    fn retains_the_settled_frame_until_the_deadline() {
        let mut handoff = NavigationHandoff::default();
        handoff.settle(frame(1));
        let now = Instant::now();
        assert!(handoff.begin(id(2), true, now).is_some());
        assert!(
            handoff
                .begin(id(2), true, now + Duration::from_millis(149))
                .is_some()
        );
    }

    #[test]
    fn expired_retention_never_rearms_for_the_same_instance() {
        let mut handoff = NavigationHandoff::default();
        handoff.settle(frame(1));
        let now = Instant::now();
        assert!(handoff.begin(id(2), true, now).is_some());
        let after = now + Duration::from_millis(151);
        assert!(
            handoff.begin(id(2), true, after).is_none(),
            "an expired retention must stop painting stale pixels"
        );
        assert!(
            handoff
                .begin(id(2), true, after + Duration::from_millis(1))
                .is_none(),
            "a still-loading instance must not re-arm retention every frame"
        );
        assert!(!handoff.is_retaining());
    }

    #[test]
    fn a_settled_target_clears_retention_and_does_not_rearm() {
        let mut handoff = NavigationHandoff::default();
        handoff.settle(frame(1));
        let now = Instant::now();
        assert!(handoff.begin(id(2), true, now).is_some());
        assert!(handoff.begin(id(2), false, now).is_none());
        assert!(
            handoff.begin(id(2), true, now).is_none(),
            "retention is one-shot per mounted instance"
        );
    }

    #[test]
    fn returning_to_an_older_instance_retains_nothing() {
        let mut handoff = NavigationHandoff::default();
        handoff.settle(frame(3));
        let now = Instant::now();
        assert!(handoff.begin(id(2), true, now).is_none());
        assert!(!handoff.is_retaining());
    }

    #[test]
    fn each_newly_mounted_instance_arms_its_own_retention() {
        let mut handoff = NavigationHandoff::default();
        handoff.settle(frame(1));
        let now = Instant::now();
        assert!(handoff.begin(id(2), true, now).is_some());
        handoff.settle(frame(2));
        assert!(handoff.begin(id(3), true, now).is_some());
        handoff.settle(frame(3));
        assert!(handoff.begin(id(4), true, now).is_some());
    }

    #[test]
    fn missing_instance_or_missing_settled_frame_retains_nothing() {
        let mut handoff = NavigationHandoff::default();
        assert!(handoff.begin(id(1), true, Instant::now()).is_none());
        handoff.settle(frame(1));
        assert!(handoff.begin(None, true, Instant::now()).is_none());
    }
}
