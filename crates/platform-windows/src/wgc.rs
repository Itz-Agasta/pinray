//! Windows Graphics Capture (WGC) video backend.
//!
//! Push-model: a free-threaded `Direct3D11CaptureFramePool` fires
//! `FrameArrived` on a thread-pool thread, where the frame is copied to host
//! memory and pushed into a bounded channel. `next_event` drains the channel.
//! Frames that arrive while the channel is full are dropped and surfaced as
//! a `Gap` event on the next call.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pinray_core::{
    BackendInfo, BackendKind, CaptureEvent, CursorMode, FrameData, GapEvent, GapReason,
    PinrayError, PixelFormat, Rect, Result, SessionConfig, VideoBackend, VideoFrame,
};
use tracing::{debug, warn};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use crate::d3d::{create_d3d_device, texture_to_host, win_err};

/// Ensures the process has an MTA so WinRT activation works regardless of
/// which thread the library is called from. The usage cookie is intentionally
/// leaked: capture can be (re)started at any point in the process lifetime.
fn ensure_mta() -> Result<()> {
    static MTA: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    MTA.get_or_init(|| {
        unsafe { CoIncrementMTAUsage() }
            .map(|_cookie| ())
            .map_err(|e| e.to_string())
    })
    .clone()
    .map_err(|e| PinrayError::Platform(format!("CoIncrementMTAUsage: {e}")))
}

/// Early-arrival allowance. Compositor timestamps wobble around the nominal
/// vblank by an absolute amount, not a share of the requested interval.
const VBLANK_JITTER_NS: i64 = 4_000_000;

/// Rate limiter for `FrameArrived`; `true` keeps the frame. `time_ns` is the
/// frame's own `SystemRelativeTime`, so pacing follows the compositor clock,
/// not callback scheduling.
///
/// Three things are load-bearing, each guarded by a test below:
///
/// - The deadline advances by one interval per kept frame rather than
///   measuring back to the last kept frame. Gap-measuring under-delivers on
///   non-divisor refresh rates (60 fps becomes 45 on a 90 Hz panel), since
///   only whole vblanks can be selected.
/// - The tolerance is absolute. A proportional one is too tight where the
///   request equals the refresh rate and an early frame is lost outright
///   rather than deferred.
/// - A stall is a gap in the *arrival* stream, not a frame late against the
///   grid. Kept frames are legitimately late on non-divisor rates, so
///   resyncing on lateness reintroduces the 45 fps case.
fn accept_frame(
    next_deadline_ns: &AtomicI64,
    last_arrival_ns: &AtomicI64,
    time_ns: i64,
    interval_ns: i64,
) -> bool {
    let tolerance = (interval_ns / 2).min(VBLANK_JITTER_NS);
    // Every arrival counts here, kept or not; the gap being measured is the
    // compositor's output, not this session's.
    let previous_arrival = last_arrival_ns.swap(time_ns, Ordering::AcqRel);
    let stalled =
        previous_arrival != i64::MIN && time_ns.saturating_sub(previous_arrival) > interval_ns;

    let mut deadline = next_deadline_ns.load(Ordering::Relaxed);
    loop {
        let next = if deadline == i64::MIN {
            // First frame of the session establishes the schedule origin.
            time_ns.saturating_add(interval_ns)
        } else {
            if time_ns.saturating_add(tolerance) < deadline {
                return false;
            }
            let advanced = deadline.saturating_add(interval_ns);
            if stalled || advanced <= time_ns {
                time_ns.saturating_add(interval_ns)
            } else {
                advanced
            }
        };
        match next_deadline_ns.compare_exchange_weak(
            deadline,
            next,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(current) => deadline = current,
        }
    }
}

pub(crate) enum WgcTarget {
    Monitor(HMONITOR),
    Window(HWND),
}

/// `IDirect3DDevice` wraps the free-threaded D3D11 device, but windows-rs
/// only auto-implements `Send` for WinRT runtime classes, not interfaces.
struct SendDevice(IDirect3DDevice);
unsafe impl Send for SendDevice {}

pub(crate) struct WgcVideoBackend {
    item: GraphicsCaptureItem,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: SendDevice,
    pixel_format: PixelFormat,
    crop: Option<Rect>,
    cursor_embedded: bool,
    queue_depth: usize,
    /// `None` delivers every compositor frame; otherwise the minimum spacing
    /// between delivered frames, derived from `SessionConfig::frame_rate`.
    min_frame_interval_ns: Option<i64>,
    runtime: Option<(Direct3D11CaptureFramePool, GraphicsCaptureSession)>,
    rx: Option<Receiver<VideoFrame>>,
    sequence: Arc<AtomicU64>,
    dropped: Arc<AtomicU32>,
    next_deadline_ns: Arc<AtomicI64>,
    last_arrival_ns: Arc<AtomicI64>,
}

impl WgcVideoBackend {
    pub(crate) fn new(target: WgcTarget, config: &SessionConfig) -> Result<Self> {
        ensure_mta()?;

        if !GraphicsCaptureSession::IsSupported()
            .map_err(|e| win_err("GraphicsCaptureSession::IsSupported", e))?
        {
            return Err(PinrayError::BackendUnavailable(
                "Windows Graphics Capture is not supported on this system (needs Windows 10 1903+)"
                    .into(),
            ));
        }

        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| win_err("IGraphicsCaptureItemInterop factory", e))?;
        let item: GraphicsCaptureItem = match target {
            WgcTarget::Monitor(hmonitor) => unsafe { interop.CreateForMonitor(hmonitor) }
                .map_err(|e| win_err("CreateForMonitor", e))?,
            WgcTarget::Window(hwnd) => unsafe { interop.CreateForWindow(hwnd) }
                .map_err(|e| win_err("CreateForWindow", e))?,
        };

        let (device, context) = create_d3d_device(None)?;
        let dxgi_device: IDXGIDevice = device.cast().map_err(|e| win_err("IDXGIDevice cast", e))?;
        let winrt_device: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
                .map_err(|e| win_err("CreateDirect3D11DeviceFromDXGIDevice", e))?
                .cast()
                .map_err(|e| win_err("IDirect3DDevice cast", e))?;

        Ok(Self {
            item,
            device,
            context,
            winrt_device: SendDevice(winrt_device),
            pixel_format: config.pixel_format,
            crop: config.crop_rect,
            cursor_embedded: config.cursor_mode == CursorMode::Embedded,
            queue_depth: config.queue_depth as usize,
            min_frame_interval_ns: config
                .frame_rate
                .map(|fps| 1_000_000_000 / i64::from(fps.max(1))),
            runtime: None,
            rx: None,
            sequence: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU32::new(0)),
            next_deadline_ns: Arc::new(AtomicI64::new(i64::MIN)),
            last_arrival_ns: Arc::new(AtomicI64::new(i64::MIN)),
        })
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::WindowsWgc,
            supports_audio: false,
            zero_copy: false,
            notes: "Windows Graphics Capture (Windows 10 1903+): display and window capture, cursor toggle, frame_rate-paced",
        }
    }
}

impl VideoBackend for WgcVideoBackend {
    fn info(&self) -> BackendInfo {
        Self::backend_info()
    }

    fn start(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }

        let size = self.item.Size().map_err(|e| win_err("item.Size", e))?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &self.winrt_device.0,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            self.queue_depth as i32,
            size,
        )
        .map_err(|e| win_err("CreateFreeThreaded", e))?;

        let (tx, rx) = sync_channel::<VideoFrame>(self.queue_depth);
        let device = self.device.clone();
        let context = self.context.clone();
        let pixel_format = self.pixel_format;
        let crop = self.crop;
        let sequence = Arc::clone(&self.sequence);
        let dropped = Arc::clone(&self.dropped);
        let next_deadline_ns = Arc::clone(&self.next_deadline_ns);
        let last_arrival_ns = Arc::clone(&self.last_arrival_ns);
        let min_frame_interval_ns = self.min_frame_interval_ns;
        next_deadline_ns.store(i64::MIN, Ordering::Relaxed);
        last_arrival_ns.store(i64::MIN, Ordering::Relaxed);

        frame_pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
                    move |pool, _| {
                        let Some(pool) = pool.as_ref() else {
                            return Ok(());
                        };
                        let Ok(frame) = pool.TryGetNextFrame() else {
                            return Ok(());
                        };

                        let time_ns = match frame.SystemRelativeTime() {
                            Ok(time) => time.Duration * 100,
                            Err(error) => {
                                warn!("wgc SystemRelativeTime failed: {error}");
                                let _ = frame.Close();
                                return Ok(());
                            }
                        };

                        // Before `texture_to_host`: the staging copy is what
                        // costs, so a frame this session will not emit must go
                        // now. Paced-out frames are not counted as drops.
                        if let Some(interval_ns) = min_frame_interval_ns
                            && !accept_frame(
                                &next_deadline_ns,
                                &last_arrival_ns,
                                time_ns,
                                interval_ns,
                            )
                        {
                            let _ = frame.Close();
                            return Ok(());
                        }

                        let result = (|| -> Result<VideoFrame> {
                            let surface =
                                frame.Surface().map_err(|e| win_err("frame.Surface", e))?;
                            let access: IDirect3DDxgiInterfaceAccess = surface
                                .cast()
                                .map_err(|e| win_err("IDirect3DDxgiInterfaceAccess cast", e))?;
                            let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
                                .map_err(|e| win_err("GetInterface", e))?;

                            let copy =
                                texture_to_host(&device, &context, &texture, crop, pixel_format)?;
                            Ok(VideoFrame {
                                stream_time_ns: time_ns,
                                sequence: sequence.fetch_add(1, Ordering::Relaxed),
                                width: copy.width,
                                height: copy.height,
                                stride: copy.stride,
                                pixel_format,
                                color_space: None,
                                data: FrameData::Host(copy.data),
                                damage: None,
                            })
                        })();
                        let _ = frame.Close();

                        match result {
                            Ok(video_frame) => {
                                if tx.try_send(video_frame).is_err() {
                                    dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            Err(error) => warn!("wgc frame extraction failed: {error}"),
                        }
                        Ok(())
                    },
                ),
            )
            .map_err(|e| win_err("FrameArrived", e))?;

        let session = frame_pool
            .CreateCaptureSession(&self.item)
            .map_err(|e| win_err("CreateCaptureSession", e))?;

        // Best-effort: not available on all Windows builds.
        if let Err(error) = session.SetIsCursorCaptureEnabled(self.cursor_embedded) {
            debug!("SetIsCursorCaptureEnabled failed (non-fatal): {error}");
        }
        if let Err(error) = session.SetIsBorderRequired(false) {
            debug!("SetIsBorderRequired failed (non-fatal): {error}");
        }
        // Throttle at the source where the OS can. IGraphicsCaptureSession5 is
        // Windows 11 only; elsewhere `accept_frame` carries the rate limit.
        if let Some(interval_ns) = self.min_frame_interval_ns
            && let Err(error) = session.SetMinUpdateInterval(TimeSpan {
                Duration: interval_ns / 100,
            })
        {
            debug!("SetMinUpdateInterval unavailable (non-fatal): {error}");
        }

        session
            .StartCapture()
            .map_err(|e| win_err("StartCapture", e))?;

        self.runtime = Some((frame_pool, session));
        self.rx = Some(rx);
        debug!("wgc capture started");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some((frame_pool, session)) = self.runtime.take() {
            let _ = session.Close();
            let _ = frame_pool.Close();
        }
        self.rx = None;
        debug!("wgc capture stopped");
        Ok(())
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            return Ok(CaptureEvent::Gap(GapEvent {
                stream_time_ns: 0,
                reason: GapReason::Dropped,
                dropped_frames: Some(dropped),
            }));
        }

        let rx = self
            .rx
            .as_ref()
            .ok_or_else(|| PinrayError::Platform("wgc backend not started".into()))?;

        let frame = match timeout {
            Some(timeout) => rx.recv_timeout(timeout).map_err(|error| match error {
                RecvTimeoutError::Timeout => PinrayError::Timeout(timeout),
                RecvTimeoutError::Disconnected => {
                    PinrayError::Platform("wgc frame channel disconnected".into())
                }
            })?,
            None => rx
                .recv()
                .map_err(|_| PinrayError::Platform("wgc frame channel disconnected".into()))?,
        };
        Ok(CaptureEvent::Video(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::{AtomicI64, accept_frame};

    fn hz(rate: i64) -> i64 {
        1_000_000_000 / rate
    }

    /// Runs `arrivals` through a fresh pacer and returns the kept timestamps.
    fn emitted(arrivals: impl IntoIterator<Item = i64>, interval: i64) -> Vec<i64> {
        let (deadline, arrival) = (AtomicI64::new(i64::MIN), AtomicI64::new(i64::MIN));
        arrivals
            .into_iter()
            .filter(|t| accept_frame(&deadline, &arrival, *t, interval))
            .collect()
    }

    fn vblanks(refresh: i64, secs: i64) -> impl Iterator<Item = i64> {
        (0..refresh * secs).map(move |i| i * hz(refresh))
    }

    /// The same stream with a deterministic `+-spread` wobble, standing in for
    /// the jitter real compositor timestamps carry.
    fn jittered(refresh: i64, secs: i64, spread: i64) -> impl Iterator<Item = i64> {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        vblanks(refresh, secs).map(move |t| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            t + (seed >> 33) as i64 % (2 * spread + 1) - spread
        })
    }

    #[track_caller]
    fn assert_rate(refresh: i64, requested: i64, want: f64) {
        // 30 s amortizes the one extra frame that starts the schedule.
        let got = emitted(vblanks(refresh, 30), hz(requested)).len() as f64 / 30.0;
        assert!(
            (got - want).abs() / want < 0.02,
            "{refresh} Hz requesting {requested} fps: got {got:.2}, want {want:.2}"
        );
    }

    #[test]
    fn first_frame_is_always_kept() {
        assert_eq!(emitted([0], hz(30)), vec![0]);
    }

    #[test]
    fn frames_inside_the_interval_are_rejected() {
        let kept = emitted([0, hz(60), 2 * hz(60)], hz(30));
        assert_eq!(kept, vec![0, 2 * hz(60)]);
    }

    #[test]
    fn divisor_refresh_rates_hit_the_request_exactly() {
        assert_rate(60, 30, 30.0);
        assert_rate(60, 5, 5.0);
        assert_rate(144, 30, 30.0);
        assert_rate(240, 60, 60.0);
    }

    #[test]
    fn non_divisor_refresh_rates_still_average_the_request() {
        // Resynchronizing the grid on every late frame under-delivers here:
        // 45, 72, 37.5 and 28.6 fps respectively.
        assert_rate(90, 60, 60.0);
        assert_rate(144, 120, 120.0);
        assert_rate(75, 60, 60.0);
        assert_rate(100, 30, 30.0);
    }

    #[test]
    fn requesting_the_display_refresh_rate_survives_vblank_jitter() {
        // A tolerance narrower than the wobble under-delivers here: 55 and
        // 108 fps respectively with `interval / 8`.
        for refresh in [60, 144] {
            let kept = emitted(jittered(refresh, 30, 1_500_000), hz(refresh)).len() as f64 / 30.0;
            let want = refresh as f64;
            assert!(
                (kept - want).abs() / want < 0.02,
                "{refresh} Hz requesting {refresh} fps with jitter: got {kept:.2}, want {want:.2}"
            );
        }
    }

    #[test]
    fn requesting_more_than_the_display_refresh_throttles_nothing() {
        assert_eq!(emitted(vblanks(60, 2), hz(120)).len(), 120);
    }

    #[test]
    fn a_long_stall_does_not_release_a_catch_up_burst() {
        let resume = 10 * 1_000_000_000;
        let arrivals = std::iter::once(0).chain((0..6).map(|i| resume + i * hz(60)));
        assert_eq!(
            emitted(arrivals, hz(30)).len(),
            4,
            "the frame at t=0 plus three of the six resumed 60 Hz frames"
        );
    }

    #[test]
    fn a_stall_shorter_than_two_intervals_does_not_emit_a_close_pair() {
        // A resumed frame less than one interval past its deadline leaves the
        // grid stale; holding it emitted the next frame one vblank later.
        let interval = hz(5);
        let gap_ends = 383 * 1_000_000;
        let arrivals = std::iter::once(0).chain(vblanks(60, 2).filter(move |t| *t >= gap_ends));

        let kept = emitted(arrivals, interval);
        let closest = kept
            .windows(2)
            .map(|w| w[1] - w[0])
            .min()
            .expect("more than one frame kept");
        assert!(
            closest >= interval - interval / 8,
            "emitted two frames {closest} ns apart for a {interval} ns interval"
        );
    }

    #[test]
    fn high_refresh_pacing_never_exceeds_the_request() {
        // An absolute tolerance keeps frames several vblanks early on a fast
        // panel, which cannot raise the rate: the deadline advances from the
        // previous deadline, never from the kept frame.
        for (refresh, request) in [
            (144, 120),
            (165, 60),
            (240, 60),
            (240, 120),
            (300, 120),
            (500, 480),
        ] {
            for jitter in [0, 1_000_000, 2_000_000] {
                let secs = 10;
                let kept = if jitter == 0 {
                    emitted(vblanks(refresh, secs), hz(request)).len()
                } else {
                    emitted(jittered(refresh, secs, jitter), hz(request)).len()
                } as f64
                    / secs as f64;
                assert!(
                    kept <= request as f64 * 1.02,
                    "{refresh} Hz at {request} fps (jitter {jitter} ns): got {kept:.2}, must not exceed the request"
                );
            }
        }
    }
}
