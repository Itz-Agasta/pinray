//! The capture session lifecycle and its builder.

use std::time::Duration;

use tracing::{debug, instrument};

use crate::{
    audio::AudioFrame,
    backend::{AudioBackend, BackendBundle, BackendInfo, BackendResolver, VideoBackend},
    config::{AudioCapture, CursorMode, SessionConfig, VideoCaptureTarget},
    error::{PinrayError, Result},
    frame::{CaptureEvent, ColorSpace, PixelFormat, Rect},
};

/// A capture session bound to the backends a [`BackendResolver`] picked.
///
/// Lifecycle: build with [`SessionBuilder::build_with_resolver`], call
/// [`start`](Self::start), read with [`next_event`](Self::next_event) or
/// [`next_audio`](Self::next_audio), then [`stop`](Self::stop). `start` and
/// `stop` are idempotent and a session can be restarted any number of
/// times. Dropping the session releases the backends.
///
/// Applications using the `pinray` crate get the same API through
/// `pinray::CaptureSession`, which picks the platform backend for you.
pub struct CaptureSession {
    config: SessionConfig,
    backend_info: BackendInfo,
    video_backend: Option<Box<dyn VideoBackend>>,
    audio_backend: Option<Box<dyn AudioBackend>>,
    running: bool,
}

impl CaptureSession {
    /// Wraps already-resolved backends in a session.
    ///
    /// Prefer [`SessionBuilder::build_with_resolver`], which validates
    /// `config` first. The session starts stopped.
    pub fn new(config: SessionConfig, bundle: BackendBundle) -> Self {
        Self {
            config,
            backend_info: bundle.info,
            video_backend: bundle.video,
            audio_backend: bundle.audio,
            running: false,
        }
    }

    /// The configuration this session was built with.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Which backend was selected and what it supports. Include its
    /// `Debug` output in bug reports.
    pub fn backend_info(&self) -> &BackendInfo {
        &self.backend_info
    }

    /// Returns the portal restore token negotiated for this session, if any.
    ///
    /// Available once the session is built on backends that use a permission
    /// portal (Wayland); other backends return `None`.
    pub fn restore_token(&self) -> Option<String> {
        self.video_backend
            .as_ref()
            .and_then(|video| video.restore_token())
    }

    /// Whether [`start`](Self::start) has been called without a matching
    /// [`stop`](Self::stop).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Starts capturing. Does nothing if already running.
    ///
    /// Starts the video backend first, then audio.
    ///
    /// # Errors
    ///
    /// Returns the backend's error if either stream fails to start. If audio
    /// fails after video started, video is left running; call
    /// [`stop`](Self::stop) or drop the session to release it.
    #[instrument(skip(self), fields(backend = ?self.backend_info.kind))]
    pub fn start(&mut self) -> Result<()> {
        if self.running {
            return Ok(());
        }

        if let Some(video) = self.video_backend.as_mut() {
            video.start()?;
        }

        if let Some(audio) = self.audio_backend.as_mut() {
            audio.start()?;
        }

        self.running = true;
        debug!("capture session started");
        Ok(())
    }

    /// Stops capturing. Does nothing if not running.
    ///
    /// # Errors
    ///
    /// Returns the backend's error if a stream fails to stop; the session
    /// then still reports [`is_running`](Self::is_running) as `true`.
    #[instrument(skip(self), fields(backend = ?self.backend_info.kind))]
    pub fn stop(&mut self) -> Result<()> {
        if !self.running {
            return Ok(());
        }

        if let Some(audio) = self.audio_backend.as_mut() {
            audio.stop()?;
        }

        if let Some(video) = self.video_backend.as_mut() {
            video.stop()?;
        }

        self.running = false;
        debug!("capture session stopped");
        Ok(())
    }

    /// Waits up to `timeout` for the next video frame, audio packet, or gap.
    ///
    /// `None` blocks until an event arrives; `Some(Duration::ZERO)` polls
    /// without blocking. In a session with both streams, pending audio is
    /// returned before video so a busy video stream cannot starve it; in
    /// that case the wait can last up to twice `timeout` (once for video,
    /// once for audio).
    ///
    /// # Errors
    ///
    /// - [`PinrayError::Timeout`] if nothing arrived in time. The session is
    ///   still healthy; call again.
    /// - [`PinrayError::BackendNotSelected`] if the session has no backends.
    /// - Any backend error, e.g. [`PinrayError::Platform`] if the capture
    ///   source disappeared.
    pub fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        match (self.video_backend.as_mut(), self.audio_backend.as_mut()) {
            (Some(video), None) => video.next_event(timeout),
            (None, Some(audio)) => audio.next_audio(timeout).map(CaptureEvent::Audio),
            (Some(video), Some(audio)) => {
                // Drain pending audio first (non-blocking); otherwise a busy
                // video stream would starve audio, since audio is only polled
                // after a video timeout.
                match audio.next_audio(Some(Duration::ZERO)) {
                    Ok(frame) => return Ok(CaptureEvent::Audio(frame)),
                    Err(PinrayError::Timeout(_)) => {}
                    Err(error) => return Err(error),
                }
                match video.next_event(timeout) {
                    Ok(event) => Ok(event),
                    Err(PinrayError::Timeout(_)) => {
                        audio.next_audio(timeout).map(CaptureEvent::Audio)
                    }
                    Err(error) => Err(error),
                }
            }
            (None, None) => Err(PinrayError::BackendNotSelected),
        }
    }

    /// Waits up to `timeout` for the next audio packet, skipping video.
    ///
    /// Useful for audio-only sessions, or for draining audio on its own
    /// thread of control. Same timeout semantics as
    /// [`next_event`](Self::next_event).
    ///
    /// # Errors
    ///
    /// - [`PinrayError::BackendNotSelected`] if the session has no audio.
    /// - [`PinrayError::Timeout`] if nothing arrived in time.
    /// - Any backend error.
    pub fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame> {
        let audio = self
            .audio_backend
            .as_mut()
            .ok_or(PinrayError::BackendNotSelected)?;
        audio.next_audio(timeout)
    }
}

/// Builds a [`SessionConfig`] and resolves it into a [`CaptureSession`].
///
/// Every setter takes and returns the builder by value, so calls chain.
/// Nothing is validated until
/// [`build_with_resolver`](Self::build_with_resolver).
///
/// # Examples
///
/// ```
/// use pinray_core::{AudioCapture, CursorMode, SessionBuilder};
///
/// let builder = SessionBuilder::new()
///     .audio(AudioCapture::SystemMix)
///     .cursor_mode(CursorMode::Hidden)
///     .queue_depth(4);
/// assert!(builder.config().validate().is_ok());
/// ```
#[derive(Debug, Clone, Default)]
pub struct SessionBuilder {
    config: SessionConfig,
}

impl SessionBuilder {
    /// Creates a builder with [`SessionConfig::default`] settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// The configuration built so far.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Chooses the backend. Default: `Auto`. See
    /// [`BackendPreference`](crate::BackendPreference) for what each one does.
    pub fn backend_preference(
        mut self,
        backend_preference: crate::backend::BackendPreference,
    ) -> Self {
        self.config.backend_preference = backend_preference;
        self
    }

    /// Captures video from a display or window. Omit for audio-only.
    pub fn video_target(mut self, target: VideoCaptureTarget) -> Self {
        self.config.video_target = Some(target);
        self
    }

    /// Also captures audio. Only [`AudioCapture::SystemMix`] is implemented.
    pub fn audio(mut self, audio_capture: AudioCapture) -> Self {
        self.config.audio_capture = Some(audio_capture);
        self
    }

    /// Requests a pixel layout. Default: [`PixelFormat::Bgra8888`].
    ///
    /// BGRA is native on every backend; [`PixelFormat::Rgba8888`] costs a CPU
    /// swizzle. Other formats are not reliably supported: Windows rejects
    /// them with [`PinrayError::Unsupported`], and other backends may fall
    /// back to BGRA. Always read the delivered
    /// [`VideoFrame::pixel_format`](crate::VideoFrame::pixel_format).
    pub fn pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.config.pixel_format = pixel_format;
        self
    }

    /// Reuses a Wayland portal permission from a previous session, skipping
    /// the dialog if the compositor still honors it.
    ///
    /// Get the token from `CaptureSession::restore_token` after building a
    /// session, and persist it. Ignored by non-portal backends.
    pub fn restore_token(mut self, restore_token: impl Into<String>) -> Self {
        self.config.restore_token = Some(restore_token.into());
        self
    }

    /// Records a color space preference. Default: `Some(ColorSpace::Srgb)`.
    ///
    /// Current backends do not act on this; read the delivered
    /// [`VideoFrame::color_space`](crate::VideoFrame::color_space) instead.
    pub fn color_space(mut self, color_space: Option<ColorSpace>) -> Self {
        self.config.color_space = color_space;
        self
    }

    /// Shows or hides the cursor in frames. Default: [`CursorMode::Embedded`].
    pub fn cursor_mode(mut self, cursor_mode: CursorMode) -> Self {
        self.config.cursor_mode = cursor_mode;
        self
    }

    /// Captures only `crop_rect` of the source, or the whole source with
    /// `None` (the default).
    ///
    /// Must have non-zero size, and backends reject rectangles outside the
    /// source. The Wayland portal backend ignores it.
    pub fn crop_rect(mut self, crop_rect: Option<Rect>) -> Self {
        self.config.crop_rect = crop_rect;
        self
    }

    /// Caps the frame rate. Default: `Some(60)`.
    ///
    /// This is a maximum, not a guarantee: damage-driven backends (Wayland,
    /// DXGI, WGC) deliver frames only when the screen changes, so use
    /// [`VideoFrame::stream_time_ns`](crate::VideoFrame::stream_time_ns) for
    /// timing rather than counting frames. `None` means uncapped on Wayland, DXGI, and WGC; 30 fps on
    /// X11 (polling); 60 fps on macOS. `Some(0)` fails validation.
    pub fn frame_rate(mut self, frame_rate: Option<u32>) -> Self {
        self.config.frame_rate = frame_rate;
        self
    }

    /// Sets how many frames are buffered between the backend and your reads.
    /// Default: `2`.
    ///
    /// When the queue is full, newly captured frames are dropped (visible as
    /// [`VideoFrame::sequence`](crate::VideoFrame::sequence) jumps). Larger
    /// values absorb hiccups at the cost of latency and memory. `0` fails
    /// validation.
    pub fn queue_depth(mut self, queue_depth: u32) -> Self {
        self.config.queue_depth = queue_depth;
        self
    }

    /// Validates the configuration and asks `resolver` for backends.
    ///
    /// The returned session is stopped; call [`CaptureSession::start`].
    ///
    /// # Errors
    ///
    /// - [`PinrayError::InvalidConfig`] if [`SessionConfig::validate`] fails.
    /// - Whatever `resolver` returns, e.g. [`PinrayError::BackendUnavailable`]
    ///   or [`PinrayError::Unsupported`].
    #[instrument(skip(self, resolver))]
    pub fn build_with_resolver<R>(self, resolver: &R) -> Result<CaptureSession>
    where
        R: BackendResolver,
    {
        self.config.validate()?;
        let bundle = resolver.resolve(&self.config)?;
        Ok(CaptureSession::new(self.config, bundle))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::{CaptureSession, SessionBuilder};
    use crate::{
        audio::{AudioData, AudioFrame, SampleFormat},
        backend::{AudioBackend, BackendBundle, BackendInfo, BackendKind, VideoBackend},
        config::SessionConfig,
        error::{PinrayError, Result},
        frame::{CaptureEvent, FrameData, PixelFormat, VideoFrame},
    };

    #[test]
    fn builder_requires_video_or_audio() {
        let config = SessionBuilder::new().config().clone();
        assert!(config.validate().is_err());
    }

    #[test]
    fn queue_depth_must_be_positive() {
        let config = SessionBuilder::new().queue_depth(0).config().clone();
        assert!(config.validate().is_err());
    }

    fn mock_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::LinuxWaylandPortal,
            supports_audio: true,
            zero_copy: false,
            notes: "mock",
        }
    }

    /// Video backend that always has a frame ready.
    struct BusyVideo;

    impl VideoBackend for BusyVideo {
        fn info(&self) -> BackendInfo {
            mock_info()
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
        fn next_event(&mut self, _timeout: Option<Duration>) -> Result<CaptureEvent> {
            Ok(CaptureEvent::Video(VideoFrame {
                stream_time_ns: 0,
                sequence: 0,
                width: 1,
                height: 1,
                stride: 4,
                pixel_format: PixelFormat::Bgra8888,
                color_space: None,
                data: FrameData::Host(vec![0; 4]),
                damage: None,
            }))
        }
    }

    struct QueuedAudio(VecDeque<AudioFrame>);

    impl AudioBackend for QueuedAudio {
        fn info(&self) -> BackendInfo {
            mock_info()
        }
        fn start(&mut self) -> Result<()> {
            Ok(())
        }
        fn stop(&mut self) -> Result<()> {
            Ok(())
        }
        fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame> {
            self.0
                .pop_front()
                .ok_or(PinrayError::Timeout(timeout.unwrap_or_default()))
        }
    }

    #[test]
    fn repeated_start_stop_is_stable() {
        let bundle = BackendBundle {
            info: mock_info(),
            video: Some(Box::new(BusyVideo)),
            audio: None,
        };
        let mut session = CaptureSession::new(SessionConfig::default(), bundle);

        for _ in 0..100 {
            session.start().unwrap();
            assert!(session.is_running());
            assert!(matches!(
                session.next_event(Some(Duration::from_millis(1))),
                Ok(CaptureEvent::Video(_))
            ));
            session.stop().unwrap();
            assert!(!session.is_running());
        }
    }

    #[test]
    fn busy_video_does_not_starve_audio() {
        let audio_frame = AudioFrame {
            stream_time_ns: 0,
            sequence: 0,
            sample_rate: 48_000,
            channels: 2,
            sample_format: SampleFormat::F32,
            data: AudioData::Interleaved(vec![0; 8]),
        };
        let bundle = BackendBundle {
            info: mock_info(),
            video: Some(Box::new(BusyVideo)),
            audio: Some(Box::new(QueuedAudio(VecDeque::from([
                audio_frame.clone(),
                audio_frame,
            ])))),
        };
        let mut session = CaptureSession::new(SessionConfig::default(), bundle);

        let mut audio_events = 0;
        let mut video_events = 0;
        for _ in 0..6 {
            match session.next_event(Some(Duration::from_millis(1))).unwrap() {
                CaptureEvent::Audio(_) => audio_events += 1,
                CaptureEvent::Video(_) => video_events += 1,
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(audio_events, 2, "queued audio must be drained");
        assert_eq!(video_events, 4);
    }
}
