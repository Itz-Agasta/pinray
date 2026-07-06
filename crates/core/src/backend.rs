use std::time::Duration;

use crate::{audio::AudioFrame, config::SessionConfig, error::Result, frame::CaptureEvent};

/// Which backend the session builder should pick.
///
/// `Auto` chooses the platform default (Wayland portal / DXGI /
/// ScreenCaptureKit) with sensible fallbacks; the other variants force a
/// specific video backend or fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendPreference {
    Auto,
    LinuxWaylandPortal,
    LinuxX11,
    MacScreenCaptureKit,
    WindowsDxgi,
    WindowsWgc,
}

/// Identifies a concrete backend implementation (video or audio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    LinuxWaylandPortal,
    LinuxX11,
    LinuxPipeWireAudio,
    MacScreenCaptureKit,
    WindowsDxgi,
    WindowsWgc,
    WindowsWasapi,
}

/// What a backend can do — surfaced through
/// [`CaptureSession::backend_info`](crate::CaptureSession::backend_info) so
/// applications (and bug reports) can tell which path was actually taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    pub kind: BackendKind,
    pub supports_audio: bool,
    pub zero_copy: bool,
    /// Human-readable caveats (e.g. "cursor not embedded", "polling-based").
    pub notes: &'static str,
}

/// A platform video capture implementation.
///
/// Contract: `next_event` returns [`PinrayError::Timeout`] when no event
/// arrives within `timeout` (`None` = block forever), and must treat
/// `Some(Duration::ZERO)` as a non-blocking poll. `start`/`stop` must be
/// safe to call repeatedly and in either state.
///
/// [`PinrayError::Timeout`]: crate::PinrayError::Timeout
pub trait VideoBackend: Send {
    fn info(&self) -> BackendInfo;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent>;
}

/// A platform audio capture implementation; same timeout contract as
/// [`VideoBackend`].
pub trait AudioBackend: Send {
    fn info(&self) -> BackendInfo;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame>;
}

pub struct BackendBundle {
    pub info: BackendInfo,
    pub video: Option<Box<dyn VideoBackend>>,
    pub audio: Option<Box<dyn AudioBackend>>,
}

pub trait BackendResolver {
    fn resolve(&self, config: &SessionConfig) -> Result<BackendBundle>;
}
