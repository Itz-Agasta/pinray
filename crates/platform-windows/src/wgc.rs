//! Windows Graphics Capture (WGC) video backend.
//!
//! Push-model: a free-threaded `Direct3D11CaptureFramePool` fires
//! `FrameArrived` on a thread-pool thread, where the frame is copied to host
//! memory and pushed into a bounded channel. `next_event` drains the channel.
//! Frames that arrive while the channel is full are dropped and surfaced as
//! a `Gap` event on the next call.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use pinray_core::{
    BackendInfo, BackendKind, CaptureEvent, CursorMode, FrameData, GapEvent, GapReason,
    PinrayError, PixelFormat, Rect, Result, SessionConfig, VideoBackend, VideoFrame,
};
use tracing::{debug, warn};
use windows::Foundation::TypedEventHandler;
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
    runtime: Option<(Direct3D11CaptureFramePool, GraphicsCaptureSession)>,
    rx: Option<Receiver<VideoFrame>>,
    sequence: Arc<AtomicU64>,
    dropped: Arc<AtomicU32>,
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
            runtime: None,
            rx: None,
            sequence: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU32::new(0)),
        })
    }

    pub(crate) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::WindowsWgc,
            supports_audio: false,
            zero_copy: false,
            notes: "Windows Graphics Capture (Windows 10 1903+): display and window capture, cursor toggle",
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

                        let result = (|| -> Result<VideoFrame> {
                            let time_ns = frame
                                .SystemRelativeTime()
                                .map_err(|e| win_err("SystemRelativeTime", e))?
                                .Duration
                                * 100;
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
