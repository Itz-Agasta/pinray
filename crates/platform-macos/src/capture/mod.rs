//! ScreenCaptureKit video backend (macOS 12.3+).
//!
//! Push-model: SCKit delivers `CMSampleBuffer`s over XPC onto a serial GCD
//! queue, where `SckOutput` (see `output.rs`) extracts host-memory frames
//! and pushes them into a bounded channel. Audio and video share one
//! `SCStream`, so both surface through this `VideoBackend` as
//! `CaptureEvent::Video` / `CaptureEvent::Audio` — `BackendBundle.audio` is
//! always `None` on macOS.
//!
//! Module layout:
//! - `output.rs`  — ObjC `SCStreamOutput`/`SCStreamDelegate` classes
//! - `config.rs`  — content filter + `SCStreamConfiguration` builders
//! - `extract.rs` — sample buffer → `VideoFrame`/`AudioFrame` conversion

mod config;
mod extract;
mod output;

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueueAttr, DispatchRetained};
use objc2::{AllocAnyThread, rc::Retained, runtime::ProtocolObject};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{SCStream, SCStreamDelegate, SCStreamOutput, SCStreamOutputType};

use pinray_core::{
    AudioCapture, BackendBundle, BackendInfo, BackendKind, CaptureEvent, PinrayError, Result,
    SessionConfig, VideoBackend,
};

use crate::content::get_shareable_content;
use config::{build_content_filter, build_stream_configuration};
use output::{RawEvent, SckDelegate, SckOutput};

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

/// Blocks the calling thread until an SCKit completion handler fires.
///
/// SCKit completion handlers are async (called on GCD); the returned closure
/// is handed to `startCapture`/`stopCapture`, and `wait()` parks on a
/// Condvar until it runs.
struct CompletionGate {
    result: Arc<Mutex<Option<Result<()>>>>,
    cv: Arc<Condvar>,
}

impl CompletionGate {
    fn new(context: &'static str) -> (Self, RcBlock<dyn Fn(*mut NSError)>) {
        let result: Arc<Mutex<Option<Result<()>>>> = Arc::new(Mutex::new(None));
        let cv = Arc::new(Condvar::new());
        let (r2, cv2) = (Arc::clone(&result), Arc::clone(&cv));

        let block = RcBlock::new(move |error_ptr: *mut NSError| {
            let outcome = if error_ptr.is_null() {
                Ok(())
            } else {
                let msg = unsafe { &*error_ptr }.localizedDescription().to_string();
                Err(PinrayError::Platform(format!("{context}: {msg}")))
            };
            *r2.lock().unwrap() = Some(outcome);
            cv2.notify_one();
        });

        (Self { result, cv }, block)
    }

    fn wait(self) -> Result<()> {
        let mut guard = self
            .cv
            .wait_while(self.result.lock().unwrap(), |v| v.is_none())
            .unwrap();
        guard.take().unwrap()
    }
}

impl VideoBackend for MacVideoBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    fn start(&mut self) -> Result<()> {
        let (gate, block) = CompletionGate::new("startCapture");

        // Set active before starting so frames delivered between the
        // completion handler firing and our return are not discarded.
        self.active.store(true, Ordering::Release);
        unsafe { self.stream.startCaptureWithCompletionHandler(Some(&block)) };

        if let Err(error) = gate.wait() {
            self.active.store(false, Ordering::Release);
            return Err(error);
        }

        tracing::debug!("SCStream started");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.active.store(false, Ordering::Release);

        let (gate, block) = CompletionGate::new("stopCapture");
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };

        let result = gate.wait();
        tracing::debug!("SCStream stopped");
        result
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
        match raw {
            RawEvent::Video(f) => Ok(CaptureEvent::Video(f)),
            RawEvent::Audio(f) => Ok(CaptureEvent::Audio(f)),
            RawEvent::Ended(msg) => Err(PinrayError::Platform(format!(
                "SCStream stopped with error: {msg}"
            ))),
        }
    }
}

pub fn build_backend(config: &SessionConfig) -> Result<BackendBundle> {
    let supports_audio = match &config.audio_capture {
        None => false,
        Some(AudioCapture::SystemMix) => true,
        Some(AudioCapture::Microphone(_)) => {
            return Err(PinrayError::Unsupported(
                "macos microphone capture is not implemented yet".into(),
            ));
        }
    };
    let content = get_shareable_content()?;

    let filter = build_content_filter(&content, config)?;
    let stream_cfg = build_stream_configuration(config, &content, supports_audio)?;

    let (event_tx, event_rx) = mpsc::sync_channel::<RawEvent>(config.queue_depth as usize);

    let active = Arc::new(AtomicBool::new(false));
    let errored = Arc::new(AtomicBool::new(false));

    let output = SckOutput::new(
        event_tx.clone(),
        Arc::clone(&active),
        config.pixel_format,
        supports_audio,
    );
    let delegate = SckDelegate::new(Arc::clone(&errored), event_tx);

    let delegate_obj = ProtocolObject::<dyn SCStreamDelegate>::from_ref::<SckDelegate>(&*delegate);

    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &stream_cfg,
            Some(delegate_obj),
        )
    };

    let queue = dispatch2::DispatchQueue::new("dev.pinray.sck_output", DispatchQueueAttr::SERIAL);

    let output_obj = ProtocolObject::<dyn SCStreamOutput>::from_ref::<SckOutput>(&*output);

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
