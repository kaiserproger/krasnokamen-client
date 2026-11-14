//! Presentation time is wall-clock in live play and explicitly stepped in replay.
//! The scope is owner-thread local: IO/mesh profiling never inherits replay time.
use std::{cell::Cell, marker::PhantomData, rc::Rc, time::{Duration, Instant}};

thread_local! {
    static VIRTUAL: Cell<Option<(Instant, Duration)>> = const { Cell::new(None) };
}

pub(crate) fn now() -> Instant {
    VIRTUAL.with(|clock| clock.get().map_or_else(Instant::now, |(origin, time)| origin + time))
}

pub(crate) fn elapsed(origin: Instant) -> Duration {
    now().saturating_duration_since(origin)
}

/// One replay per owner thread. Dropping restores the enclosing live/replay clock.
pub(crate) struct VirtualClock {
    previous: Option<(Instant, Duration)>,
    origin: Instant,
    _owner: PhantomData<Rc<()>>,
}
impl VirtualClock {
    pub(crate) fn new() -> Self {
        let origin = Instant::now();
        let previous = VIRTUAL.with(|clock| clock.replace(Some((origin, Duration::ZERO))));
        Self { previous, origin, _owner: PhantomData }
    }
    pub(crate) fn set(&self, time: Duration) {
        VIRTUAL.with(|clock| clock.set(Some((self.origin, time))));
    }
}
impl Drop for VirtualClock {
    fn drop(&mut self) { VIRTUAL.with(|clock| clock.set(self.previous)); }
}

/// Rational frame time, with no accumulated floating-point or nanosecond drift.
pub(crate) fn frame_time(frame: u64, rate: u32) -> Duration {
    Duration::from_secs(frame / u64::from(rate))
        + Duration::from_nanos((frame % u64::from(rate)) * 1_000_000_000 / u64::from(rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rational_sixty_hz_and_scoped_rewind() {
        let clock = VirtualClock::new();
        let start = now();
        for frame in 0..=600 { clock.set(frame_time(frame, 60)); assert_eq!(elapsed(start), frame_time(frame, 60)); }
        assert_eq!(elapsed(start), Duration::from_secs(10));
        clock.set(Duration::ZERO);
        assert_eq!(now(), start);
        { let inner = VirtualClock::new(); inner.set(Duration::from_secs(7)); }
        assert_eq!(now(), start);
    }
}
