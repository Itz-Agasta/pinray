//! DXGI Desktop Duplication video backend.
//!
//! Pull-model: `next_event` maps directly onto `AcquireNextFrame`, so no
//! capture thread is needed. Frames are only delivered when the desktop
//! changes; an unchanged desktop surfaces as `PinrayError::Timeout`.
//!
//! `frame_rate` is honored by delaying the acquire, not by discarding frames
//! afterwards: duplication only copies once a frame has been acquired, so a
//! frame that pacing skips is never produced in the first place.
//!
//! Known limitation: duplication does not composite the mouse cursor into
//! the frame (cursor arrives as separate metadata, which we do not yet
//! draw). Use the WGC backend when an embedded cursor is required.

use std::time::{Duration, Instant};

use pinray_core::{
    BackendInfo, BackendKind, CaptureEvent, FrameData, GapEvent, GapReason, PinrayError,
    PixelFormat, Rect, Result, SessionConfig, VideoBackend, VideoFrame,
};
use tracing::{debug, warn};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::d3d::{create_d3d_device, qpc_frequency, qpc_to_ns, texture_to_host, win_err};
use crate::enumerate::DisplayEntry;

/// What the pacing gate decided before anything is acquired.
#[derive(Debug, PartialEq, Eq)]
enum Pace {
    /// The caller's timeout runs out before the next frame is due.
    Expired,
    /// Sleep this long, then acquire.
    Wait(Duration),
}

/// Decides whether the caller can be made to wait for the next scheduled
/// frame. The caller's timeout wins: a frame due after it is a `Timeout`, the
/// same answer an idle desktop gives.
fn paced_wait(next_frame_at: Option<Instant>, deadline: Option<Instant>, now: Instant) -> Pace {
    match next_frame_at {
        Some(due) if deadline.is_some_and(|deadline| due > deadline) => Pace::Expired,
        Some(due) => Pace::Wait(due.saturating_duration_since(now)),
        None => Pace::Wait(Duration::ZERO),
    }
}

/// When the next frame may be acquired. Advances one interval per delivered
/// frame so the average rate holds even when a frame lands late, and
/// resynchronizes once the desktop has been quiet for longer than an interval
/// instead of releasing a catch-up burst.
fn advance_schedule(previous: Option<Instant>, now: Instant, interval: Duration) -> Instant {
    match previous {
        Some(previous) if previous + interval > now => previous + interval,
        _ => now + interval,
    }
}

pub(crate) struct DxgiVideoBackend {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    pixel_format: PixelFormat,
    crop: Option<Rect>,
    sequence: u64,
    qpc_freq: i64,
    /// `None` delivers frames as fast as the desktop changes.
    min_frame_interval: Option<Duration>,
    /// Earliest instant the next frame may be acquired.
    next_frame_at: Option<Instant>,
}

impl DxgiVideoBackend {
    pub(crate) fn new(display: &DisplayEntry, config: &SessionConfig) -> Result<Self> {
        let (device, context) = create_d3d_device(Some(&display.adapter))?;
        let output: IDXGIOutput1 = display
            .output
            .cast()
            .map_err(|e| win_err("IDXGIOutput1 cast", e))?;

        // Probe duplication now so that Auto backend selection can fall back
        // to WGC when duplication is denied (e.g. by session policy).
        let probe = unsafe { output.DuplicateOutput(&device) }
            .map_err(|e| PinrayError::BackendUnavailable(format!("DuplicateOutput: {e}")))?;
        drop(probe);

        Ok(Self {
            device,
            context,
            output,
            duplication: None,
            pixel_format: config.pixel_format,
            crop: config.crop_rect,
            sequence: 0,
            qpc_freq: qpc_frequency()?,
            min_frame_interval: config
                .frame_rate
                .map(|fps| Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))),
            next_frame_at: None,
        })
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::WindowsDxgi,
            supports_audio: false,
            zero_copy: false,
            notes: "DXGI desktop duplication: display capture only, frames on desktop change, frame_rate-paced, cursor not embedded",
        }
    }

    fn recreate_duplication(&mut self) -> Result<()> {
        self.duplication = None;
        let duplication = unsafe { self.output.DuplicateOutput(&self.device) }
            .map_err(|e| win_err("DuplicateOutput", e))?;
        self.duplication = Some(duplication);
        Ok(())
    }
}

impl VideoBackend for DxgiVideoBackend {
    fn info(&self) -> BackendInfo {
        Self::backend_info()
    }

    fn start(&mut self) -> Result<()> {
        if self.duplication.is_none() {
            self.recreate_duplication()?;
            self.next_frame_at = None;
            debug!("dxgi duplication started");
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.duplication = None;
        self.next_frame_at = None;
        debug!("dxgi duplication stopped");
        Ok(())
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        let deadline = timeout.map(|t| Instant::now() + t);

        // Hold off the acquire until the frame is due. Waiting costs nothing
        // here, where acquiring early would copy a frame only to throw it away.
        match paced_wait(self.next_frame_at, deadline, Instant::now()) {
            Pace::Expired => return Err(PinrayError::Timeout(timeout.unwrap_or_default())),
            Pace::Wait(wait) if !wait.is_zero() => std::thread::sleep(wait),
            Pace::Wait(_) => {}
        }

        loop {
            let wait_ms = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(PinrayError::Timeout(timeout.unwrap_or_default()));
                    }
                    remaining.as_millis().min(u32::MAX as u128) as u32
                }
                None => u32::MAX,
            };

            let duplication = self
                .duplication
                .as_ref()
                .ok_or_else(|| PinrayError::Platform("dxgi backend not started".into()))?
                .clone();

            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            let acquired =
                unsafe { duplication.AcquireNextFrame(wait_ms, &mut frame_info, &mut resource) };

            if let Err(error) = acquired {
                if error.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return Err(PinrayError::Timeout(timeout.unwrap_or_default()));
                }
                if error.code() == DXGI_ERROR_ACCESS_LOST {
                    // Mode switch, secure desktop, etc. Recreate and report a gap.
                    warn!("dxgi duplication access lost; recreating");
                    self.recreate_duplication()?;
                    return Ok(CaptureEvent::Gap(GapEvent {
                        stream_time_ns: qpc_to_ns(frame_info.LastPresentTime, self.qpc_freq),
                        reason: GapReason::BackendRestarted,
                        dropped_frames: None,
                    }));
                }
                return Err(win_err("AcquireNextFrame", error));
            }

            // LastPresentTime == 0 means no new image (e.g. only mouse
            // movement); release and keep waiting.
            if frame_info.LastPresentTime == 0 {
                let _ = unsafe { duplication.ReleaseFrame() };
                continue;
            }

            // Anchor the schedule here, before the staging copy. Using the
            // post-copy instant adds the copy on top of the interval and
            // stretches every period by it once a copy approaches one frame.
            let acquired_at = Instant::now();

            let resource = resource.ok_or_else(|| {
                PinrayError::Platform("AcquireNextFrame returned no resource".into())
            })?;
            let texture: ID3D11Texture2D = resource
                .cast()
                .map_err(|e| win_err("ID3D11Texture2D cast", e))?;

            let copy = texture_to_host(
                &self.device,
                &self.context,
                &texture,
                self.crop,
                self.pixel_format,
            );
            let _ = unsafe { duplication.ReleaseFrame() };
            let copy = copy?;

            if let Some(interval) = self.min_frame_interval {
                self.next_frame_at =
                    Some(advance_schedule(self.next_frame_at, acquired_at, interval));
            }

            let sequence = self.sequence;
            self.sequence += 1;

            return Ok(CaptureEvent::Video(VideoFrame {
                stream_time_ns: qpc_to_ns(frame_info.LastPresentTime, self.qpc_freq),
                sequence,
                width: copy.width,
                height: copy.height,
                stride: copy.stride,
                pixel_format: self.pixel_format,
                color_space: None,
                data: FrameData::Host(copy.data),
                damage: None,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Pace, advance_schedule, paced_wait};
    use std::time::{Duration, Instant};

    const INTERVAL: Duration = Duration::from_millis(100);

    #[test]
    fn a_frame_arriving_late_keeps_the_grid() {
        // Acquiring waits for the next desktop change, so frames land a little
        // after their deadline. Resetting the schedule to that arrival would
        // stretch every period by the wait and under-deliver.
        let base = Instant::now();
        let next = advance_schedule(Some(base), base + Duration::from_millis(5), INTERVAL);
        assert_eq!(next, base + INTERVAL);
    }

    #[test]
    fn a_quiet_desktop_resynchronizes_instead_of_bursting() {
        let base = Instant::now();
        let woke = base + Duration::from_millis(350);
        assert_eq!(
            advance_schedule(Some(base), woke, INTERVAL),
            woke + INTERVAL
        );
    }

    #[test]
    fn the_first_frame_starts_the_schedule() {
        let base = Instant::now();
        assert_eq!(advance_schedule(None, base, INTERVAL), base + INTERVAL);
    }

    /// Drives the scheduler against a desktop changing every `change_ms` and
    /// returns how many frames come out.
    fn delivered(change_ms: u64, requested: u64, secs: u64) -> u64 {
        delivered_with_copy(change_ms, requested, secs, 0)
    }

    /// As above, but each frame also costs `copy_ms` to stage into host memory.
    fn delivered_with_copy(change_ms: u64, requested: u64, secs: u64, copy_ms: u64) -> u64 {
        let interval = Duration::from_nanos(1_000_000_000 / requested);
        let change = Duration::from_millis(change_ms);
        let base = Instant::now();
        let end = base + Duration::from_secs(secs);

        let (mut now, mut deadline, mut frames) = (base, None::<Instant>, 0u64);
        while now < end {
            // Sleep until due, then wait for the next change at or after that.
            let ready = deadline.map_or(now, |d| d.max(now));
            let waited = (ready - base).as_nanos() / change.as_nanos();
            let mut arrival = base + change * (waited as u32);
            if arrival < ready {
                arrival += change;
            }
            // The schedule is anchored on the acquire; the copy happens after.
            deadline = Some(advance_schedule(deadline, arrival, interval));
            now = arrival + Duration::from_millis(copy_ms);
            frames += 1;
        }
        frames
    }

    fn delivered_fps(change_ms: u64, requested: u64, secs: u64) -> f64 {
        delivered(change_ms, requested, secs) as f64 / secs as f64
    }

    #[test]
    fn pacing_never_delivers_faster_than_requested() {
        // A desktop changing at 60 Hz, throttled to a range of rates. The one
        // extra frame is the one that starts the schedule.
        let secs = 20;
        for requested in [1, 2, 5, 10, 15, 24, 30, 50] {
            let frames = delivered(16, requested, secs);
            assert!(
                frames <= requested * secs + 1,
                "requested {requested} fps over {secs}s: {frames} frames, cap {}",
                requested * secs + 1
            );
        }
    }

    #[test]
    fn pacing_holds_the_requested_rate_when_the_desktop_is_fast_enough() {
        for requested in [1, 2, 5, 10, 15, 30] {
            let got = delivered_fps(16, requested, 20);
            assert!(
                (got - requested as f64).abs() / (requested as f64) < 0.1,
                "requested {requested} fps, delivered {got:.2}"
            );
        }
    }

    #[test]
    fn a_slow_desktop_is_not_inflated() {
        // Changes every 200 ms cannot become 30 fps just because it was asked for.
        let got = delivered_fps(200, 30, 20);
        assert!(got <= 5.2, "delivered {got:.2} from a 5 fps desktop");
    }

    #[test]
    fn the_gate_waits_until_the_frame_is_due() {
        let base = Instant::now();
        let due = base + Duration::from_millis(30);
        assert_eq!(
            paced_wait(Some(due), Some(base + Duration::from_millis(500)), base),
            Pace::Wait(Duration::from_millis(30))
        );
    }

    #[test]
    fn the_gate_does_not_wait_without_a_schedule() {
        let base = Instant::now();
        assert_eq!(
            paced_wait(None, Some(base), base),
            Pace::Wait(Duration::ZERO)
        );
    }

    #[test]
    fn an_overdue_frame_is_not_slept_on() {
        let base = Instant::now();
        let due = base - Duration::from_millis(40);
        assert_eq!(
            paced_wait(Some(due), None, base),
            Pace::Wait(Duration::ZERO)
        );
    }

    #[test]
    fn the_callers_timeout_wins_over_the_schedule() {
        // 5 fps requested but only 10 ms of patience: the caller gets Timeout,
        // the same answer an idle desktop gives, instead of being slept past it.
        let base = Instant::now();
        let due = base + Duration::from_millis(200);
        let deadline = base + Duration::from_millis(10);
        assert_eq!(paced_wait(Some(due), Some(deadline), base), Pace::Expired);
    }

    #[test]
    fn a_frame_due_exactly_at_the_deadline_is_still_waited_for() {
        let base = Instant::now();
        let due = base + Duration::from_millis(100);
        assert_eq!(
            paced_wait(Some(due), Some(due), base),
            Pace::Wait(Duration::from_millis(100))
        );
    }

    #[test]
    fn without_a_timeout_the_gate_always_waits() {
        let base = Instant::now();
        let due = base + Duration::from_secs(10);
        assert_eq!(
            paced_wait(Some(due), None, base),
            Pace::Wait(Duration::from_secs(10))
        );
    }

    #[test]
    fn a_slow_copy_does_not_stretch_the_period() {
        // A 20 ms copy against a 60 fps request (16.6 ms interval). Anchoring
        // the schedule after the copy adds it on top of the interval and
        // settles near 27 fps, where the copy alone allows about 50.
        let fps = delivered_with_copy(4, 60, 20, 20) as f64 / 20.0;
        assert!(fps > 40.0, "copy-bound delivery collapsed to {fps:.1} fps");
    }
}
