//! Session configuration: what to capture and how.

use crate::{
    error::{PinrayError, Result},
    frame::{ColorSpace, PixelFormat, Rect},
    source::SourceId,
};

/// Whether the pointer is drawn into captured frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    /// Draw the cursor into each frame (the default). DXGI cannot do this;
    /// check [`BackendInfo::notes`](crate::BackendInfo::notes).
    Embedded,
    /// Leave the cursor out of frames.
    Hidden,
}

/// What to capture video from; ids come from source enumeration or
/// `SourceId::new("auto")` for the primary display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCaptureTarget {
    /// A whole monitor. On Wayland the id is not enforced: the user picks
    /// the output in the portal dialog.
    Display(SourceId),
    /// A single application window.
    Window(SourceId),
}

/// What to capture audio from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCapture {
    /// Everything the system plays (loopback / sink monitor).
    SystemMix,
    /// A microphone device. No backend implements this yet; building a
    /// session with it fails with
    /// [`PinrayError::Unsupported`].
    Microphone(SourceId),
}

/// Validated session configuration; construct through the session builder.
///
/// Each field has a builder method of the same name on
/// [`SessionBuilder`](crate::SessionBuilder), which documents it in detail.
/// The exception is `audio_capture`, set with
/// [`SessionBuilder::audio`](crate::SessionBuilder::audio).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// Which backend to use. Default: [`BackendPreference::Auto`](crate::BackendPreference::Auto).
    pub backend_preference: crate::backend::BackendPreference,
    /// Video source, or `None` for an audio-only session. Default: `None`.
    pub video_target: Option<VideoCaptureTarget>,
    /// Audio source, or `None` for a video-only session. Default: `None`.
    pub audio_capture: Option<AudioCapture>,
    /// Wayland portal restore token from a previous session. Default: `None`.
    pub restore_token: Option<String>,
    /// Requested pixel layout of video frames. Default: [`PixelFormat::Bgra8888`].
    pub pixel_format: PixelFormat,
    /// Requested color space hint. Default: `Some(ColorSpace::Srgb)`.
    pub color_space: Option<ColorSpace>,
    /// Whether to draw the cursor. Default: [`CursorMode::Embedded`].
    pub cursor_mode: CursorMode,
    /// Capture only this region of the source. Default: `None` (whole source).
    pub crop_rect: Option<Rect>,
    /// Maximum frames per second, or `None` for the backend default.
    /// Default: `Some(60)`.
    pub frame_rate: Option<u32>,
    /// Frames buffered before new frames are dropped. Default: `2`.
    pub queue_depth: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            backend_preference: crate::backend::BackendPreference::Auto,
            video_target: None,
            audio_capture: None,
            restore_token: None,
            pixel_format: PixelFormat::Bgra8888,
            color_space: Some(ColorSpace::Srgb),
            cursor_mode: CursorMode::Embedded,
            crop_rect: None,
            frame_rate: Some(60),
            queue_depth: 2,
        }
    }
}

impl SessionConfig {
    /// Checks the configuration for mistakes that no backend could accept.
    ///
    /// Called automatically when a session is built. Backend-specific limits
    /// (e.g. a crop rectangle larger than the display) are checked later, by
    /// the backend.
    ///
    /// # Errors
    ///
    /// Returns [`PinrayError::InvalidConfig`] if:
    ///
    /// - neither `video_target` nor `audio_capture` is set,
    /// - `frame_rate` is `Some(0)`,
    /// - `queue_depth` is `0`, or
    /// - `crop_rect` has zero width or height.
    ///
    /// # Examples
    ///
    /// ```
    /// use pinray_core::{AudioCapture, SessionConfig};
    ///
    /// let mut config = SessionConfig::default();
    /// assert!(config.validate().is_err()); // nothing to capture
    ///
    /// config.audio_capture = Some(AudioCapture::SystemMix);
    /// assert!(config.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<()> {
        if self.video_target.is_none() && self.audio_capture.is_none() {
            return Err(PinrayError::InvalidConfig(
                "at least one of video_target or audio_capture must be set".into(),
            ));
        }

        if let Some(frame_rate) = self.frame_rate
            && frame_rate == 0
        {
            return Err(PinrayError::InvalidConfig(
                "frame_rate must be greater than zero".into(),
            ));
        }

        if self.queue_depth == 0 {
            return Err(PinrayError::InvalidConfig(
                "queue_depth must be greater than zero".into(),
            ));
        }

        if let Some(rect) = self.crop_rect
            && (rect.width == 0 || rect.height == 0)
        {
            return Err(PinrayError::InvalidConfig(
                "crop_rect width and height must be greater than zero".into(),
            ));
        }

        Ok(())
    }
}
