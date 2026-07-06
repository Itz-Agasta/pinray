//! Process-relative monotonic timestamps.
//!
//! PipeWire buffer metadata is not exposed by the `pipewire 0.10` bindings we
//! use, so video and audio frames are stamped at dequeue time from one shared
//! monotonic anchor. Timestamps are comparable across the audio and video
//! streams of a session (same anchor, same clock) but are *not* boot-relative
//! like the macOS/Windows backends. Replace with SPA_META_Header /
//! GetBuffer-style native timing once the bindings expose it.

use std::sync::OnceLock;
use std::time::Instant;

static ANCHOR: OnceLock<Instant> = OnceLock::new();

pub(crate) fn monotonic_time_ns() -> i64 {
    let anchor = *ANCHOR.get_or_init(Instant::now);
    Instant::now().duration_since(anchor).as_nanos() as i64
}

#[cfg(test)]
mod tests {
    use super::monotonic_time_ns;

    #[test]
    fn timestamps_never_decrease() {
        let mut last = monotonic_time_ns();
        for _ in 0..1000 {
            let now = monotonic_time_ns();
            assert!(now >= last);
            last = now;
        }
    }
}
