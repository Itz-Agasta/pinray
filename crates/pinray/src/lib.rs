use pinray_core::{BackendBundle, BackendResolver, PinrayError, Result, SessionConfig};

pub use pinray_core::{
    AudioCapture, AudioData, AudioDeviceSource, AudioFrame, BackendInfo, BackendKind,
    BackendPreference, CaptureEvent, CaptureSource, ColorSpace, CursorMode, CvPixelBufferHandle,
    D3D11TextureHandle, DisplaySource, DmabufFrame, FrameData, GapEvent, GapReason, PixelFormat,
    Rect, SampleFormat, SessionConfig as CoreSessionConfig, SourceId, VideoCaptureTarget,
    VideoFrame, WindowSource,
};

pub struct CaptureSession {
    inner: pinray_core::CaptureSession,
}

#[derive(Debug, Clone, Default)]
pub struct SessionBuilder {
    inner: pinray_core::SessionBuilder,
}

struct PlatformResolver;

impl BackendResolver for PlatformResolver {
    fn resolve(&self, config: &SessionConfig) -> Result<BackendBundle> {
        #[cfg(target_os = "linux")]
        if let Some(bundle) = pinray_platform_linux::try_resolve(config)? {
            return Ok(bundle);
        }

        #[cfg(target_os = "macos")]
        if let Some(bundle) = pinray_platform_macos::try_resolve(config)? {
            return Ok(bundle);
        }

        #[cfg(target_os = "windows")]
        if let Some(bundle) = pinray_platform_windows::try_resolve(config)? {
            return Ok(bundle);
        }

        Err(PinrayError::BackendUnavailable(
            "no platform backend resolver matched the current target".into(),
        ))
    }
}

impl CaptureSession {
    pub fn builder() -> SessionBuilder {
        SessionBuilder::default()
    }

    pub fn config(&self) -> &CoreSessionConfig {
        self.inner.config()
    }

    pub fn backend_info(&self) -> &BackendInfo {
        self.inner.backend_info()
    }

    pub fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    pub fn start(&mut self) -> Result<()> {
        self.inner.start()
    }

    pub fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }

    pub fn next_event(&mut self, timeout: Option<std::time::Duration>) -> Result<CaptureEvent> {
        self.inner.next_event(timeout)
    }

    pub fn next_audio(&mut self, timeout: Option<std::time::Duration>) -> Result<AudioFrame> {
        self.inner.next_audio(timeout)
    }
}

impl SessionBuilder {
    pub fn backend_preference(mut self, backend_preference: BackendPreference) -> Self {
        self.inner = self.inner.backend_preference(backend_preference);
        self
    }

    pub fn video_target(mut self, target: VideoCaptureTarget) -> Self {
        self.inner = self.inner.video_target(target);
        self
    }

    pub fn audio(mut self, audio_capture: AudioCapture) -> Self {
        self.inner = self.inner.audio(audio_capture);
        self
    }

    pub fn pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.inner = self.inner.pixel_format(pixel_format);
        self
    }

    pub fn restore_token(mut self, restore_token: impl Into<String>) -> Self {
        self.inner = self.inner.restore_token(restore_token);
        self
    }

    pub fn color_space(mut self, color_space: Option<ColorSpace>) -> Self {
        self.inner = self.inner.color_space(color_space);
        self
    }

    pub fn cursor_mode(mut self, cursor_mode: CursorMode) -> Self {
        self.inner = self.inner.cursor_mode(cursor_mode);
        self
    }

    pub fn crop_rect(mut self, crop_rect: Option<Rect>) -> Self {
        self.inner = self.inner.crop_rect(crop_rect);
        self
    }

    pub fn frame_rate(mut self, frame_rate: Option<u32>) -> Self {
        self.inner = self.inner.frame_rate(frame_rate);
        self
    }

    pub fn queue_depth(mut self, queue_depth: u32) -> Self {
        self.inner = self.inner.queue_depth(queue_depth);
        self
    }

    pub fn config(&self) -> &CoreSessionConfig {
        self.inner.config()
    }

    pub fn build(self) -> Result<CaptureSession> {
        let inner = self.inner.build_with_resolver(&PlatformResolver)?;
        Ok(CaptureSession { inner })
    }
}

pub fn available_backends() -> Vec<BackendInfo> {
    let mut backends = Vec::new();
    #[cfg(target_os = "linux")]
    backends.extend(pinray_platform_linux::available_backends());
    #[cfg(target_os = "macos")]
    backends.extend(pinray_platform_macos::available_backends());
    #[cfg(target_os = "windows")]
    backends.extend(pinray_platform_windows::available_backends());
    backends
}

pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    let mut sources = Vec::new();
    #[cfg(target_os = "linux")]
    sources.extend(pinray_platform_linux::enumerate_sources()?);
    #[cfg(target_os = "macos")]
    sources.extend(pinray_platform_macos::enumerate_sources()?);
    #[cfg(target_os = "windows")]
    sources.extend(pinray_platform_windows::enumerate_sources()?);
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::{CaptureSession, available_backends};

    #[test]
    fn facade_exposes_current_platform_backends() {
        let backends = available_backends();
        if cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        )) {
            assert!(!backends.is_empty());
        }
    }

    #[test]
    fn build_requires_valid_config() {
        let result = CaptureSession::builder().build();
        assert!(result.is_err());
    }
}
