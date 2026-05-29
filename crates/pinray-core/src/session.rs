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
            (Some(_), Some(_)) => Err(PinrayError::Unsupported(
                "phase 0 session scaffolding does not multiplex simultaneous video and audio events yet"
                    .into(),
            )),
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
    use super::SessionBuilder;

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
}
