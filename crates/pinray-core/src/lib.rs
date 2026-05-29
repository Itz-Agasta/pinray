pub mod audio;
pub mod backend;
pub mod config;
pub mod error;
pub mod frame;
pub mod session;
pub mod source;

pub use audio::{AudioData, AudioFrame, SampleFormat};
pub use backend::{
    AudioBackend, BackendBundle, BackendInfo, BackendKind, BackendPreference, BackendResolver,
    VideoBackend,
};
pub use config::{AudioCapture, CursorMode, SessionConfig, VideoCaptureTarget};
pub use error::{PinrayError, Result};
pub use frame::{
    CaptureEvent, ColorSpace, CvPixelBufferHandle, D3D11TextureHandle, DmabufFrame, FrameData,
    GapEvent, GapReason, PixelFormat, Rect, VideoFrame,
};
pub use session::{CaptureSession, SessionBuilder};
pub use source::{AudioDeviceSource, CaptureSource, DisplaySource, SourceId, WindowSource};
