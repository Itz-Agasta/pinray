use std::time::Duration;

use tracing::{debug, instrument};

use crate::{
    audio::AudioFrame,
    backend::{AudioBackend, BackendBundle, BackendInfo, BackendResolver, VideoBackend},
    config::{AudioCapture, CursorMode, SessionConfig, VideoCaptureTarget},
    error::{PinrayError, Result},
    frame::{CaptureEvent, ColorSpace, PixelFormat, Rect},
};

pub struct CaptureSession {
    config: SessionConfig,
    backend_info: BackendInfo,
    video_backend: Option<Box<dyn VideoBackend>>,
    audio_backend: Option<Box<dyn AudioBackend>>,
    running: bool,
}

impl CaptureSession {
    pub fn new(config: SessionConfig, bundle: BackendBundle) -> Self {
        Self {
            config,
            backend_info: bundle.info,
            video_backend: bundle.video,
            audio_backend: bundle.audio,
            running: false,
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn backend_info(&self) -> &BackendInfo {
        &self.backend_info
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

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

    pub fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame> {
        let audio = self
            .audio_backend
            .as_mut()
            .ok_or(PinrayError::BackendNotSelected)?;
        audio.next_audio(timeout)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SessionBuilder {
    config: SessionConfig,
}

impl SessionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn backend_preference(
        mut self,
        backend_preference: crate::backend::BackendPreference,
    ) -> Self {
        self.config.backend_preference = backend_preference;
        self
    }

    pub fn video_target(mut self, target: VideoCaptureTarget) -> Self {
        self.config.video_target = Some(target);
        self
    }

    pub fn audio(mut self, audio_capture: AudioCapture) -> Self {
        self.config.audio_capture = Some(audio_capture);
        self
    }

    pub fn pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.config.pixel_format = pixel_format;
        self
    }

    pub fn restore_token(mut self, restore_token: impl Into<String>) -> Self {
        self.config.restore_token = Some(restore_token.into());
        self
    }

    pub fn color_space(mut self, color_space: Option<ColorSpace>) -> Self {
        self.config.color_space = color_space;
        self
    }

    pub fn cursor_mode(mut self, cursor_mode: CursorMode) -> Self {
        self.config.cursor_mode = cursor_mode;
        self
    }

    pub fn crop_rect(mut self, crop_rect: Option<Rect>) -> Self {
        self.config.crop_rect = crop_rect;
        self
    }

    pub fn frame_rate(mut self, frame_rate: Option<u32>) -> Self {
        self.config.frame_rate = frame_rate;
        self
    }

    pub fn queue_depth(mut self, queue_depth: u32) -> Self {
        self.config.queue_depth = queue_depth;
        self
    }

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
