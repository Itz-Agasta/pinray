//! Cross-platform screen and audio capture with raw frames and real metadata.
//!
//! pinray captures displays, windows, and system audio through each OS's
//! native API — Wayland portals + PipeWire and X11 on Linux,
//! ScreenCaptureKit on macOS, DXGI/Windows Graphics Capture + WASAPI on
//! Windows — and hands you raw [`VideoFrame`]s and [`AudioFrame`]s with
//! stride, pixel format, timestamps, and sequence numbers intact. Encoding
//! is deliberately out of scope: feed frames to ffmpeg, WebRTC, wgpu, or
//! your own pipeline.
//!
//! - [`CaptureSession`] — builder-configured lifecycle (`start` / `next_event`
//!   / `stop`); [`enumerate_sources`] lists displays, windows, and audio
//!   devices up front.
//! - [`CaptureEvent`] — one of [`VideoFrame`], [`AudioFrame`], a [`GapEvent`]
//!   (dropped frames / backend restart), or `End`.
//! - [`BackendPreference`] — `Auto` picks the right backend per platform;
//!   [`CaptureSession::backend_info`] reports what was actually selected.
#![doc = include_str!("../docs/getting-started.md")]
#![warn(missing_docs)]

use pinray_core::{BackendBundle, BackendResolver, Result, SessionConfig};

pub use pinray_core::{
    AudioCapture, AudioData, AudioDeviceSource, AudioFrame, BackendInfo, BackendKind,
    BackendPreference, CaptureEvent, CaptureSource, ColorSpace, CursorMode, CvPixelBufferHandle,
    D3D11TextureHandle, DisplaySource, DmabufFrame, FrameData, GapEvent, GapReason, PinrayError,
    PixelFormat, Rect, SampleFormat, SessionConfig as CoreSessionConfig, SourceId,
    VideoCaptureTarget, VideoFrame, WindowSource,
};

/// A running or configured capture session bound to the platform backend
/// selected at build time.
///
/// Create one with [`CaptureSession::builder`]. `start`/`stop` are
/// idempotent and a session survives repeated restarts. Dropping the session
/// releases the OS capture resources.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// use pinray::{CaptureEvent, CaptureSession, SourceId, VideoCaptureTarget};
///
/// let mut session = CaptureSession::builder()
///     .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
///     .build()?;
///
/// session.start()?;
/// if let CaptureEvent::Video(frame) = session.next_event(Some(Duration::from_secs(5)))? {
///     println!("{}x{} {:?}", frame.width, frame.height, frame.pixel_format);
/// }
/// session.stop()?;
/// # Ok::<(), pinray::PinrayError>(())
/// ```
#[doc(alias("screenshot", "screencast", "recorder", "capture"))]
pub struct CaptureSession {
    inner: pinray_core::CaptureSession,
}

/// Builder for [`CaptureSession`]; validates the configuration and resolves
/// the platform backend on [`build`](SessionBuilder::build).
///
/// Setters take and return the builder by value so calls chain; nothing is
/// checked until `build`. You must set a [`video_target`](Self::video_target),
/// an [`audio`](Self::audio) source, or both.
///
/// # Examples
///
/// ```no_run
/// use pinray::{AudioCapture, CaptureSession, CursorMode, SourceId, VideoCaptureTarget};
///
/// let session = CaptureSession::builder()
///     .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
///     .audio(AudioCapture::SystemMix)
///     .cursor_mode(CursorMode::Hidden)
///     .frame_rate(Some(30))
///     .build()?;
/// # Ok::<(), pinray::PinrayError>(())
/// ```
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
    /// Starts configuring a session. Equivalent to `SessionBuilder::default()`.
    pub fn builder() -> SessionBuilder {
        SessionBuilder::default()
    }

    /// The configuration this session was built with.
    pub fn config(&self) -> &CoreSessionConfig {
        self.inner.config()
    }

    /// Which backend was actually selected and what it supports.
    ///
    /// Backend choice depends on the machine (Wayland vs. X11, DXGI vs. WGC),
    /// so log this and include it in bug reports.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # let session = pinray::CaptureSession::builder()
    /// #     .audio(pinray::AudioCapture::SystemMix).build()?;
    /// let info = session.backend_info();
    /// println!("{:?}: audio={} notes={}", info.kind, info.supports_audio, info.notes);
    /// # Ok::<(), pinray::PinrayError>(())
    /// ```
    pub fn backend_info(&self) -> &BackendInfo {
        self.inner.backend_info()
    }

    /// Returns the portal restore token negotiated for this session, if any.
    ///
    /// Available once the session is built on backends that use a permission
    /// portal (Wayland); other backends return `None`. Persist it and pass it
    /// to [`SessionBuilder::restore_token`] next time to skip the permission
    /// dialog.
    pub fn restore_token(&self) -> Option<String> {
        self.inner.restore_token()
    }

    /// Whether the session is between [`start`](Self::start) and
    /// [`stop`](Self::stop).
    pub fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    /// Starts capturing. Does nothing if already running.
    ///
    /// # Errors
    ///
    /// Returns [`PinrayError::Platform`] (or another variant) if the OS
    /// capture stream fails to start.
    pub fn start(&mut self) -> Result<()> {
        self.inner.start()
    }

    /// Stops capturing. Does nothing if not running. The session can be
    /// started again.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS reports a failure while stopping.
    pub fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }

    /// Waits up to `timeout` for the next video frame, audio packet, or gap.
    ///
    /// `None` blocks until something arrives; `Some(Duration::ZERO)` polls
    /// without blocking. With both video and audio, pending audio is returned
    /// first, and the call can wait up to twice `timeout`.
    ///
    /// # Errors
    ///
    /// - [`PinrayError::Timeout`] if nothing arrived in time. Not fatal; call
    ///   again.
    /// - [`PinrayError::Platform`] if the backend failed, e.g. the captured
    ///   window closed.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use pinray::{CaptureEvent, PinrayError};
    ///
    /// # let mut session = pinray::CaptureSession::builder()
    /// #     .audio(pinray::AudioCapture::SystemMix).build()?;
    /// session.start()?;
    /// loop {
    ///     match session.next_event(Some(Duration::from_millis(500))) {
    ///         Ok(CaptureEvent::Video(frame)) => println!("frame #{}", frame.sequence),
    ///         Ok(CaptureEvent::Audio(audio)) => println!("{} Hz audio", audio.sample_rate),
    ///         Ok(CaptureEvent::Gap(gap)) => eprintln!("gap: {:?}", gap.reason),
    ///         Ok(CaptureEvent::End) => break,
    ///         Err(PinrayError::Timeout(_)) => continue,
    ///         Err(error) => return Err(error),
    ///     }
    /// }
    /// # Ok::<(), pinray::PinrayError>(())
    /// ```
    pub fn next_event(&mut self, timeout: Option<std::time::Duration>) -> Result<CaptureEvent> {
        self.inner.next_event(timeout)
    }

    /// Waits up to `timeout` for the next audio packet, ignoring video.
    ///
    /// The simplest way to read an audio-only session.
    ///
    /// # Errors
    ///
    /// - [`PinrayError::BackendNotSelected`] if the session has no audio.
    /// - [`PinrayError::Timeout`] if nothing arrived in time.
    /// - Any backend error.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use pinray::{AudioCapture, CaptureSession};
    ///
    /// let mut session = CaptureSession::builder().audio(AudioCapture::SystemMix).build()?;
    /// session.start()?;
    /// let frame = session.next_audio(Some(Duration::from_secs(3)))?;
    /// println!("{} Hz, {} channels, {:?}", frame.sample_rate, frame.channels, frame.sample_format);
    /// # Ok::<(), pinray::PinrayError>(())
    /// ```
    pub fn next_audio(&mut self, timeout: Option<std::time::Duration>) -> Result<AudioFrame> {
        self.inner.next_audio(timeout)
    }
}

impl SessionBuilder {
    /// Forces a backend, or `Auto` (the default) to let pinray choose.
    ///
    /// A backend from another OS fails at [`build`](Self::build) with
    /// [`PinrayError::BackendUnavailable`]. See [`BackendPreference`].
    pub fn backend_preference(mut self, backend_preference: BackendPreference) -> Self {
        self.inner = self.inner.backend_preference(backend_preference);
        self
    }

    /// Captures a display or window. Omit for an audio-only session.
    ///
    /// Use ids from [`enumerate_sources`], or `SourceId::new("auto")` for
    /// the primary display.
    pub fn video_target(mut self, target: VideoCaptureTarget) -> Self {
        self.inner = self.inner.video_target(target);
        self
    }

    /// Also captures audio. Only [`AudioCapture::SystemMix`] is implemented;
    /// [`AudioCapture::Microphone`] fails at [`build`](Self::build) with
    /// [`PinrayError::Unsupported`].
    pub fn audio(mut self, audio_capture: AudioCapture) -> Self {
        self.inner = self.inner.audio(audio_capture);
        self
    }

    /// Requests a pixel layout. Default: [`PixelFormat::Bgra8888`], native
    /// everywhere. [`PixelFormat::Rgba8888`] costs a CPU swizzle; other formats
    /// are not reliably supported. Check [`VideoFrame::pixel_format`].
    pub fn pixel_format(mut self, pixel_format: PixelFormat) -> Self {
        self.inner = self.inner.pixel_format(pixel_format);
        self
    }

    /// Supplies a Wayland portal restore token from
    /// [`CaptureSession::restore_token`] to skip the permission dialog.
    /// Ignored on other backends.
    pub fn restore_token(mut self, restore_token: impl Into<String>) -> Self {
        self.inner = self.inner.restore_token(restore_token);
        self
    }

    /// Records a color space preference. Default: `Some(ColorSpace::Srgb)`.
    /// Current backends ignore it; check [`VideoFrame::color_space`].
    pub fn color_space(mut self, color_space: Option<ColorSpace>) -> Self {
        self.inner = self.inner.color_space(color_space);
        self
    }

    /// Shows or hides the cursor. Default: [`CursorMode::Embedded`]. DXGI
    /// never embeds the cursor.
    pub fn cursor_mode(mut self, cursor_mode: CursorMode) -> Self {
        self.inner = self.inner.cursor_mode(cursor_mode);
        self
    }

    /// Captures only this region of the source (`None`, the default, captures
    /// all of it). Ignored by the Wayland portal backend.
    pub fn crop_rect(mut self, crop_rect: Option<Rect>) -> Self {
        self.inner = self.inner.crop_rect(crop_rect);
        self
    }

    /// Caps frames per second. Default: `Some(60)`. A maximum, not a
    /// guarantee: damage-driven backends send frames only when the screen
    /// changes. Use [`VideoFrame::stream_time_ns`] for timing.
    pub fn frame_rate(mut self, frame_rate: Option<u32>) -> Self {
        self.inner = self.inner.frame_rate(frame_rate);
        self
    }

    /// Frames buffered before new ones are dropped. Default: `2`. Raise it if
    /// your consumer is bursty; drops show up as [`VideoFrame::sequence`] jumps.
    pub fn queue_depth(mut self, queue_depth: u32) -> Self {
        self.inner = self.inner.queue_depth(queue_depth);
        self
    }

    /// The configuration built so far.
    pub fn config(&self) -> &CoreSessionConfig {
        self.inner.config()
    }

    /// Validates the configuration and opens the platform backend.
    ///
    /// The returned session is stopped; call [`CaptureSession::start`]. This
    /// is where permission prompts appear: the Wayland portal dialog, or the
    /// macOS screen-recording check.
    ///
    /// # Errors
    ///
    /// - [`PinrayError::InvalidConfig`] if no video target or audio is set,
    ///   `frame_rate` or `queue_depth` is zero, or the crop rectangle is
    ///   empty or outside the source.
    /// - [`PinrayError::BackendUnavailable`] if the requested backend can't
    ///   run here (e.g. no Wayland session and no `DISPLAY`).
    /// - [`PinrayError::Unsupported`] for unimplemented options, such as
    ///   microphone capture, or if the user cancels the Wayland portal dialog.
    /// - [`PinrayError::Platform`] if the OS refuses, e.g. macOS
    ///   screen-recording permission is denied or the window id no longer
    ///   exists.
    pub fn build(self) -> Result<CaptureSession> {
        let inner = self.inner.build_with_resolver(&PlatformResolver)?;
        Ok(CaptureSession { inner })
    }
}

/// Lists the capture backends compiled in for the current platform, with
/// their capabilities and caveats in [`BackendInfo::notes`].
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

/// Enumerates capturable displays, windows, and audio sources.
///
/// Call it right before building a session: window ids become invalid when
/// the window closes.
///
/// Source ids feed [`VideoCaptureTarget`]; `SourceId::new("auto")` selects
/// the primary display without enumerating. On Wayland, video sources are
/// listed without a portal dialog, but windows are not. IDs are informational:
/// the ScreenCast portal cannot be told which output to capture, that is instead
/// done in a portal dialog. `VideoCaptureTarget` id is not enforced.
/// On macOS this triggers the screen-recording permission check.
///
/// # Errors
///
/// Returns [`PinrayError::Platform`] if the OS enumeration API fails or, on
/// macOS, screen-recording permission is denied.
///
/// # Examples
///
/// ```no_run
/// use pinray::{CaptureSession, CaptureSource, VideoCaptureTarget};
///
/// let window = pinray::enumerate_sources()?.into_iter().find_map(|source| match source {
///     CaptureSource::Window(window) if window.title.contains("Firefox") => Some(window.id),
///     _ => None,
/// });
/// if let Some(id) = window {
///     let session = CaptureSession::builder()
///         .video_target(VideoCaptureTarget::Window(id))
///         .build()?;
/// #   let _ = session;
/// }
/// # Ok::<(), pinray::PinrayError>(())
/// ```
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
