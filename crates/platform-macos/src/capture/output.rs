//! ObjC classes bridging SCStream callbacks into Rust channels.
//!
//! Two classes are defined with `define_class!`:
//! - `SckOutput` implements `SCStreamOutput` — receives every sample buffer
//!   on a serial GCD queue and forwards extracted frames into the event
//!   channel.
//! - `SckDelegate` implements `SCStreamDelegate` — receives
//!   `stream:didStopWithError:` when the stream dies on its own (user hits
//!   the stop button in the menu bar, display disconnects, XPC failure).

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};

use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, rc::Retained};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSError, NSObjectProtocol};
use objc2_screen_capture_kit::{SCStream, SCStreamDelegate, SCStreamOutput, SCStreamOutputType};

use pinray_core::{AudioFrame, PixelFormat, VideoFrame};

use super::extract::{extract_audio_frame, extract_video_frame};

/// Internal event type flowing from the GCD callback to `next_event`.
pub(super) enum RawEvent {
    Video(VideoFrame),
    Audio(AudioFrame),
    /// Stream stopped on its own (error or system action); carries the message.
    Ended(String),
}

pub(super) struct OutputIvars {
    event_tx: mpsc::SyncSender<RawEvent>,
    active: Arc<AtomicBool>,
    // Separate per-stream counters, advanced only when a frame is actually
    // delivered, so consumers can detect drops per stream.
    video_sequence: AtomicU64,
    audio_sequence: AtomicU64,
    desired_pixel_format: PixelFormat,
    capture_audio: bool,
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[name = "PinraySCStreamOutput"]
    #[ivars = OutputIvars]
    pub(super) struct SckOutput;

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

            // This callback runs on one serial GCD queue, so the
            // load-then-store on the sequence counters is race-free.
            match output_type {
                SCStreamOutputType::Screen => {
                    let seq = ivars.video_sequence.load(Ordering::Relaxed);
                    if let Some(frame) =
                        extract_video_frame(sample_buffer, seq, ivars.desired_pixel_format)
                        && ivars.event_tx.try_send(RawEvent::Video(frame)).is_ok()
                    {
                        ivars.video_sequence.store(seq + 1, Ordering::Relaxed);
                    }
                }
                SCStreamOutputType::Audio if ivars.capture_audio => {
                    let seq = ivars.audio_sequence.load(Ordering::Relaxed);
                    if let Some(frame) = extract_audio_frame(sample_buffer, seq)
                        && ivars.event_tx.try_send(RawEvent::Audio(frame)).is_ok()
                    {
                        ivars.audio_sequence.store(seq + 1, Ordering::Relaxed);
                    }
                }
                _ => {}
            }
        }
    }
);

impl SckOutput {
    pub(super) fn new(
        event_tx: mpsc::SyncSender<RawEvent>,
        active: Arc<AtomicBool>,
        desired_pixel_format: PixelFormat,
        capture_audio: bool,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            event_tx,
            active,
            video_sequence: AtomicU64::new(0),
            audio_sequence: AtomicU64::new(0),
            desired_pixel_format,
            capture_audio,
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub(super) struct DelegateIvars {
    errored: Arc<AtomicBool>,
    event_tx: mpsc::SyncSender<RawEvent>,
}

define_class!(
    #[unsafe(super(objc2_foundation::NSObject))]
    #[name = "PinraySCStreamDelegate"]
    #[ivars = DelegateIvars]
    pub(super) struct SckDelegate;

    unsafe impl NSObjectProtocol for SckDelegate {}

    unsafe impl SCStreamDelegate for SckDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let desc = error.localizedDescription();
            tracing::error!(%desc, "SCStream stopped unexpectedly");
            self.ivars().errored.store(true, Ordering::Release);
            // Wake a caller blocked in next_event(None). If the channel is
            // full the receiver is not blocked and the errored flag catches
            // it on the next call instead.
            let _ = self
                .ivars()
                .event_tx
                .try_send(RawEvent::Ended(desc.to_string()));
        }
    }
);

impl SckDelegate {
    pub(super) fn new(
        errored: Arc<AtomicBool>,
        event_tx: mpsc::SyncSender<RawEvent>,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { errored, event_tx });
        unsafe { msg_send![super(this), init] }
    }
}
