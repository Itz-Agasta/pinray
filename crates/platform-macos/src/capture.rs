use std::{
    ptr::NonNull,
    slice,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use block2::RcBlock;
use dispatch2::{DispatchQueueAttr, DispatchRetained};
use objc2::{
    AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_core_media::{CMSampleBuffer, CMTimeFlags};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_32RGBA,
};
use objc2_foundation::{NSError, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamOutput, SCStreamOutputType, SCWindow,
};

use pinray_core::{
    AudioData, AudioFrame, BackendBundle, BackendInfo, BackendKind, CaptureEvent, ColorSpace,
    CursorMode, FrameData, PinrayError, PixelFormat, Result, SampleFormat, SessionConfig,
    VideoBackend, VideoCaptureTarget, VideoFrame,
};

use crate::content::get_shareable_content;

//internal event type
enum RawEvent {
    Video(VideoFrame),
    Audio(AudioFrame),
}

//  SCStreamOutput delegate
struct OutputIvars {
    event_tx: mpsc::SyncSender<RawEvent>,
    active: Arc<AtomicBool>,
    sequence: Arc<AtomicU64>,
    desired_pixel_format: PixelFormat,
    capture_audio: bool,
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[name = "PinraySCStreamOutput"]
    #[ivars = OutputIvars]
    struct SckOutput;

    unsafe impl NSObjectProtocol for SckOutput {}

    unsafe impl SCStreamOutput for SckOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            let ivars = self.ivars();
            if !ivars.active.load(Ordering::Acquire) {
                return;
            }
            let seq = ivars.sequence.fetch_add(1, Ordering::Relaxed);

            match output_type {
                SCStreamOutputType::Screen => {
                    if let Some(frame) =
                        extract_video_frame(sample_buffer, seq, ivars.desired_pixel_format)
                    {
                        let _ = ivars.event_tx.try_send(RawEvent::Video(frame));
                    }
                }
                SCStreamOutputType::Audio if ivars.capture_audio => {
                    if let Some(frame) = extract_audio_frame(sample_buffer, seq) {
                        let _ = ivars.event_tx.try_send(RawEvent::Audio(frame));
                    }
                }
                _ => {}
            }
        }
    }
);

impl SckOutput {
    fn new(
        event_tx: mpsc::SyncSender<RawEvent>,
        active: Arc<AtomicBool>,
        sequence: Arc<AtomicU64>,
        desired_pixel_format: PixelFormat,
        capture_audio: bool,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            event_tx,
            active,
            sequence,
            desired_pixel_format,
            capture_audio,
        });
        unsafe { msg_send![super(this), init] }
    }
}

//  SCStreamDelegate
struct DelegateIvars {
    errored: Arc<AtomicBool>,
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[name = "PinraySCStreamDelegate"]
    #[ivars = DelegateIvars]
    struct SckDelegate;

    unsafe impl NSObjectProtocol for SckDelegate {}

    unsafe impl SCStreamDelegate for SckDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let desc = unsafe { error.localizedDescription() };
            tracing::error!(%desc, "SCStream stopped unexpectedly");
            self.ivars().errored.store(true, Ordering::Release);
        }
    }
);

impl SckDelegate {
    fn new(errored: Arc<AtomicBool>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { errored });
        unsafe { msg_send![super(this), init] }
    }
}

// ─── backend ──────────────────────────────────────────────────────────────────

pub struct MacVideoBackend {
    info: BackendInfo,
    stream: Retained<SCStream>,
    _output: Retained<SckOutput>,
    _delegate: Retained<SckDelegate>,
    _queue: DispatchRetained<dispatch2::DispatchQueue>,
    event_rx: mpsc::Receiver<RawEvent>,
    active: Arc<AtomicBool>,
    errored: Arc<AtomicBool>,
}

// SAFETY: SCStream is ObjC refcounted. start/stop are caller-serialized.
unsafe impl Send for MacVideoBackend {}

impl VideoBackend for MacVideoBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    fn start(&mut self) -> Result<()> {
        let result: Arc<Mutex<Option<Result<()>>>> = Arc::new(Mutex::new(None));
        let cv = Arc::new(Condvar::new());
        let r2 = Arc::clone(&result);
        let cv2 = Arc::clone(&cv);

        let block = RcBlock::new(move |error_ptr: *mut NSError| {
            let outcome = if error_ptr.is_null() {
                Ok(())
            } else {
                let msg = unsafe { &*error_ptr }.localizedDescription().to_string();
                Err(PinrayError::Platform(format!("startCapture: {msg}")))
            };
            *r2.lock().unwrap() = Some(outcome);
            cv2.notify_one();
        });

        unsafe { self.stream.startCaptureWithCompletionHandler(Some(&block)) };

        let mut guard = cv
            .wait_while(result.lock().unwrap(), |v| v.is_none())
            .unwrap();
        guard.take().unwrap()?;

        self.active.store(true, Ordering::Release);
        tracing::debug!("SCStream started");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.active.store(false, Ordering::Release);

        let result: Arc<Mutex<Option<Result<()>>>> = Arc::new(Mutex::new(None));
        let cv = Arc::new(Condvar::new());
        let r2 = Arc::clone(&result);
        let cv2 = Arc::clone(&cv);

        let block = RcBlock::new(move |error_ptr: *mut NSError| {
            let outcome = if error_ptr.is_null() {
                Ok(())
            } else {
                let msg = unsafe { &*error_ptr }.localizedDescription().to_string();
                Err(PinrayError::Platform(format!("stopCapture: {msg}")))
            };
            *r2.lock().unwrap() = Some(outcome);
            cv2.notify_one();
        });

        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };

        let mut guard = cv
            .wait_while(result.lock().unwrap(), |v| v.is_none())
            .unwrap();
        tracing::debug!("SCStream stopped");
        guard.take().unwrap()
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        if self.errored.load(Ordering::Acquire) {
            return Err(PinrayError::Platform(
                "SCStream stopped with error; check tracing logs".into(),
            ));
        }
        let raw = match timeout {
            Some(t) => self.event_rx.recv_timeout(t).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => PinrayError::Timeout(t),
                mpsc::RecvTimeoutError::Disconnected => {
                    PinrayError::Platform("SCStream event channel disconnected".into())
                }
            })?,
            None => self
                .event_rx
                .recv()
                .map_err(|_| PinrayError::Platform("SCStream event channel disconnected".into()))?,
        };
        Ok(match raw {
            RawEvent::Video(f) => CaptureEvent::Video(f),
            RawEvent::Audio(f) => CaptureEvent::Audio(f),
        })
    }
}

// ─── public constructor ───────────────────────────────────────────────────────

pub fn build_backend(config: &SessionConfig) -> Result<BackendBundle> {
    let supports_audio = config.audio_capture.is_some();
    let content = get_shareable_content()?;

    let filter = build_content_filter(&content, config)?;
    let stream_cfg = build_stream_configuration(config, &content, supports_audio)?;

    let (event_tx, event_rx) = mpsc::sync_channel::<RawEvent>(config.queue_depth as usize);

    let active = Arc::new(AtomicBool::new(false));
    let errored = Arc::new(AtomicBool::new(false));
    let sequence = Arc::new(AtomicU64::new(0));

    let output = SckOutput::new(
        event_tx,
        Arc::clone(&active),
        Arc::clone(&sequence),
        config.pixel_format,
        supports_audio,
    );
    let delegate = SckDelegate::new(Arc::clone(&errored));

    let delegate_obj = ProtocolObject::<dyn SCStreamDelegate>::from_ref(delegate.as_ref());

    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &stream_cfg,
            Some(delegate_obj),
        )
    };

    let queue = dispatch2::DispatchQueue::new("dev.pinray.sck_output", DispatchQueueAttr::SERIAL);

    let output_obj = ProtocolObject::<dyn SCStreamOutput>::from_ref(output.as_ref());

    // addStreamOutput returns Result<(), Retained<NSError>>
    unsafe {
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                output_obj,
                SCStreamOutputType::Screen,
                Some(&*queue),
            )
            .map_err(|e| {
                PinrayError::Platform(format!(
                    "addStreamOutput video: {}",
                    e.localizedDescription()
                ))
            })?;
    }

    if supports_audio {
        unsafe {
            if let Err(e) = stream.addStreamOutput_type_sampleHandlerQueue_error(
                output_obj,
                SCStreamOutputType::Audio,
                Some(&*queue),
            ) {
                tracing::warn!(
                    "addStreamOutput audio: {}; continuing video-only",
                    e.localizedDescription()
                );
            }
        }
    }

    let info = BackendInfo {
        kind: BackendKind::MacScreenCaptureKit,
        supports_audio,
        zero_copy: false,
        notes: "ScreenCaptureKit via XPC + CVPixelBuffer host-copy",
    };

    Ok(BackendBundle {
        info: info.clone(),
        video: Some(Box::new(MacVideoBackend {
            info,
            stream,
            _output: output,
            _delegate: delegate,
            _queue: queue,
            event_rx,
            active,
            errored,
        })),
        audio: None,
    })
}

// ─── SCContentFilter builder ──────────────────────────────────────────────────

fn build_content_filter(
    content: &SCShareableContent,
    config: &SessionConfig,
) -> Result<Retained<SCContentFilter>> {
    match &config.video_target {
        None | Some(VideoCaptureTarget::Display(_)) => {
            let display = find_display(content, config)?;
            let excluded = unsafe { objc2_foundation::NSArray::<SCWindow>::new() };
            Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            })
        }
        Some(VideoCaptureTarget::Window(source_id)) => {
            let win = find_window(content, &source_id.0)?;
            Ok(unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &win)
            })
        }
    }
}

fn find_display(
    content: &SCShareableContent,
    config: &SessionConfig,
) -> Result<Retained<SCDisplay>> {
    let displays = unsafe { content.displays() };

    if let Some(VideoCaptureTarget::Display(source_id)) = &config.video_target {
        if source_id.0 != "auto" {
            let target_id: u32 = source_id.0.parse().map_err(|_| {
                PinrayError::InvalidConfig(format!("invalid display id '{}'", source_id.0))
            })?;
            for display in displays.iter() {
                if unsafe { display.displayID() } == target_id {
                    return Ok(display.retain());
                }
            }
            return Err(PinrayError::Platform(format!(
                "display {target_id} not found"
            )));
        }
    }

    displays
        .firstObject()
        .ok_or_else(|| PinrayError::Platform("no displays via SCShareableContent".into()))
}

fn find_window(content: &SCShareableContent, id_str: &str) -> Result<Retained<SCWindow>> {
    let target_id: u32 = id_str
        .parse()
        .map_err(|_| PinrayError::InvalidConfig(format!("invalid window id '{id_str}'")))?;
    let windows = unsafe { content.windows() };
    for window in windows.iter() {
        if unsafe { window.windowID() } == target_id {
            return Ok(window.retain());
        }
    }
    Err(PinrayError::Platform(format!(
        "window {target_id} not found"
    )))
}

// ─── SCStreamConfiguration builder ───────────────────────────────────────────

fn build_stream_configuration(
    config: &SessionConfig,
    content: &SCShareableContent,
    capture_audio: bool,
) -> Result<Retained<SCStreamConfiguration>> {
    let cfg = unsafe { SCStreamConfiguration::new() };

    let (display_w, display_h, scale) = display_dimensions(content, config);
    let out_w = (display_w * scale).max(2) & !1;
    let out_h = (display_h * scale).max(2) & !1;

    unsafe {
        cfg.setWidth(out_w as usize);
        cfg.setHeight(out_h as usize);
    }

    let cv_fmt = match config.pixel_format {
        PixelFormat::Rgba8888 => kCVPixelFormatType_32RGBA,
        _ => kCVPixelFormatType_32BGRA,
    };
    unsafe { cfg.setPixelFormat(cv_fmt) };

    unsafe { cfg.setShowsCursor(matches!(config.cursor_mode, CursorMode::Embedded)) };

    let fps = config.frame_rate.unwrap_or(60).max(1) as i32;
    // kCMTimeFlags_Valid = 1; value/timescale = seconds → 1/fps = frame interval
    let interval = objc2_core_media::CMTime {
        value: 1,
        timescale: fps,
        flags: CMTimeFlags(1),
        epoch: 0,
    };
    unsafe { cfg.setMinimumFrameInterval(interval) };

    unsafe { cfg.setCapturesAudio(capture_audio) };

    if capture_audio {
        unsafe {
            cfg.setSampleRate(48_000); // NSInteger
            cfg.setChannelCount(2); // NSInteger
        }
    }

    if let Some(crop) = config.crop_rect {
        let cg_rect = objc2_core_graphics::CGRect {
            origin: objc2_core_graphics::CGPoint {
                x: crop.x as f64,
                y: crop.y as f64,
            },
            size: objc2_core_graphics::CGSize {
                width: crop.width as f64,
                height: crop.height as f64,
            },
        };
        unsafe { cfg.setSourceRect(cg_rect) };
    }

    Ok(cfg)
}

fn display_dimensions(content: &SCShareableContent, config: &SessionConfig) -> (u32, u32, u32) {
    let displays = unsafe { content.displays() };

    let display = if let Some(VideoCaptureTarget::Display(source_id)) = &config.video_target {
        if source_id.0 != "auto" {
            if let Ok(id) = source_id.0.parse::<u32>() {
                displays.iter().find(|d| unsafe { d.displayID() } == id)
            } else {
                displays.firstObject()
            }
        } else {
            displays.firstObject()
        }
    } else {
        displays.firstObject()
    };

    match display {
        Some(d) => {
            let w = unsafe { d.width() } as u32;
            let h = unsafe { d.height() } as u32;
            (w, h, 2) // 2× retina default; exact scale needs CGDisplayMode
        }
        None => (1920, 1080, 1),
    }
}

// ─── frame extraction ─────────────────────────────────────────────────────────

fn pts_to_ns(pts: objc2_core_media::CMTime) -> i64 {
    if pts.timescale != 0 {
        pts.value
            .saturating_mul(1_000_000_000)
            .checked_div(pts.timescale as i64)
            .unwrap_or(0)
    } else {
        0
    }
}

fn extract_video_frame(
    sample_buffer: &CMSampleBuffer,
    sequence: u64,
    desired: PixelFormat,
) -> Option<VideoFrame> {
    let stream_time_ns = pts_to_ns(unsafe { sample_buffer.presentation_time_stamp() });

    let pixel_buffer = unsafe { sample_buffer.image_buffer()? };
    unsafe { CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };

    let result = (|| {
        let w = unsafe { CVPixelBufferGetWidth(&pixel_buffer) } as u32;
        let h = unsafe { CVPixelBufferGetHeight(&pixel_buffer) } as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let bpr = unsafe { CVPixelBufferGetBytesPerRow(&pixel_buffer) } as u32;
        let base = unsafe { CVPixelBufferGetBaseAddress(&pixel_buffer) };
        let size = unsafe { CVPixelBufferGetDataSize(&pixel_buffer) };
        let native = unsafe { CVPixelBufferGetPixelFormatType(&pixel_buffer) };
        if base.is_null() || size == 0 {
            return None;
        }
        let raw = unsafe { slice::from_raw_parts(base.cast::<u8>(), size) };
        let (pixel_format, data) = normalize_pixels(native, raw, w, h, bpr, desired);

        Some(VideoFrame {
            stream_time_ns,
            sequence,
            width: w,
            height: h,
            stride: bpr,
            pixel_format,
            color_space: Some(ColorSpace::Srgb),
            data: FrameData::Host(data),
            damage: None,
        })
    })();

    unsafe { CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    result
}

fn normalize_pixels(
    native: u32,
    raw: &[u8],
    w: u32,
    h: u32,
    bpr: u32,
    desired: PixelFormat,
) -> (PixelFormat, Vec<u8>) {
    let row = bpr as usize;
    let pw = w as usize * 4;
    let ph = h as usize;

    match (native, desired) {
        (f, PixelFormat::Bgra8888) if f == kCVPixelFormatType_32BGRA => {
            (PixelFormat::Bgra8888, copy_rows(raw, row, pw, ph))
        }
        (f, PixelFormat::Rgba8888) if f == kCVPixelFormatType_32RGBA => {
            (PixelFormat::Rgba8888, copy_rows(raw, row, pw, ph))
        }
        (f, _) if f == kCVPixelFormatType_32BGRA => {
            let mut dst = copy_rows(raw, row, pw, ph);
            for px in dst.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            (PixelFormat::Rgba8888, dst)
        }
        (f, _) if f == kCVPixelFormatType_32RGBA => {
            let mut dst = copy_rows(raw, row, pw, ph);
            for px in dst.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            (PixelFormat::Bgra8888, dst)
        }
        _ => (PixelFormat::Bgra8888, copy_rows(raw, row, pw, ph)),
    }
}

fn copy_rows(raw: &[u8], bpr: usize, pixel_row_bytes: usize, h: usize) -> Vec<u8> {
    if bpr == pixel_row_bytes {
        return raw[..pixel_row_bytes * h].to_vec();
    }
    let mut out = Vec::with_capacity(pixel_row_bytes * h);
    for row in 0..h {
        let s = row * bpr;
        let e = s + pixel_row_bytes;
        if e <= raw.len() {
            out.extend_from_slice(&raw[s..e]);
        }
    }
    out
}

fn extract_audio_frame(sample_buffer: &CMSampleBuffer, sequence: u64) -> Option<AudioFrame> {
    let stream_time_ns = pts_to_ns(unsafe { sample_buffer.presentation_time_stamp() });

    if unsafe { sample_buffer.num_samples() } == 0 {
        return None;
    }

    let block_buf = unsafe { sample_buffer.data_buffer()? };
    let data_len = unsafe { block_buf.data_length() };
    if data_len == 0 {
        return None;
    }

    let mut bytes = vec![0u8; data_len];
    let dst = NonNull::new(bytes.as_mut_ptr().cast()).expect("vec ptr is non-null");
    let status = unsafe { block_buf.copy_data_bytes(0, data_len, dst) };
    if status != 0 {
        tracing::warn!(status, "CMBlockBufferCopyDataBytes failed");
        return None;
    }

    Some(AudioFrame {
        stream_time_ns,
        sequence,
        sample_rate: 48_000,
        channels: 2,
        sample_format: SampleFormat::F32,
        data: AudioData::Interleaved(bytes),
    })
}
