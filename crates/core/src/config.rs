use crate::{
    error::{PinrayError, Result},
    frame::{ColorSpace, PixelFormat, Rect},
    source::SourceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Embedded,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCaptureTarget {
    Display(SourceId),
    Window(SourceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCapture {
    SystemMix,
    Microphone(SourceId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub backend_preference: crate::backend::BackendPreference,
    pub video_target: Option<VideoCaptureTarget>,
    pub audio_capture: Option<AudioCapture>,
    pub restore_token: Option<String>,
    pub pixel_format: PixelFormat,
    pub color_space: Option<ColorSpace>,
    pub cursor_mode: CursorMode,
    pub crop_rect: Option<Rect>,
    pub frame_rate: Option<u32>,
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
