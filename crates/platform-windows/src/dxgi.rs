//! DXGI Desktop Duplication video backend.
//!
//! Pull-model: `next_event` maps directly onto `AcquireNextFrame`, so no
//! capture thread is needed. Frames are only delivered when the desktop
//! changes; an unchanged desktop surfaces as `PinrayError::Timeout`.
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

pub(crate) struct DxgiVideoBackend {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    pixel_format: PixelFormat,
    crop: Option<Rect>,
    sequence: u64,
    qpc_freq: i64,
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
        })
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::WindowsDxgi,
            supports_audio: false,
            zero_copy: false,
            notes: "DXGI desktop duplication: display capture only, frames on desktop change, cursor not embedded",
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
            debug!("dxgi duplication started");
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.duplication = None;
        debug!("dxgi duplication stopped");
        Ok(())
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        let deadline = timeout.map(|t| Instant::now() + t);

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
