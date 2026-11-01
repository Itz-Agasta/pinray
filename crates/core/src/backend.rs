//! Backend selection and the traits platform capture backends implement.
//!
//! Applications usually only touch [`BackendPreference`] (to force a
//! backend) and [`BackendInfo`] (to see which one was picked). The traits
//! are for backend authors; see the [crate-level example](crate) for a
//! minimal implementation.

use std::time::Duration;

use crate::{audio::AudioFrame, config::SessionConfig, error::Result, frame::CaptureEvent};

/// Which backend the session builder should pick.
///
/// `Auto` chooses the platform default (Wayland portal / DXGI /
/// ScreenCaptureKit) with sensible fallbacks; the other variants force a
/// specific video backend or fail. Forcing a backend from another OS fails
/// the build with [`PinrayError::BackendUnavailable`].
///
/// [`PinrayError::BackendUnavailable`]: crate::PinrayError::BackendUnavailable
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendPreference {
    /// Let pinray choose. On Linux: the Wayland portal in a Wayland session,
    /// otherwise X11. On Windows: DXGI for displays (falling back to WGC),
    /// WGC for windows. On macOS: ScreenCaptureKit.
    Auto,
    /// XDG Desktop Portal ScreenCast + PipeWire. Shows a permission dialog
    /// unless a valid restore token is supplied.
    LinuxWaylandPortal,
    /// X11 `GetImage` polling. Works under Xwayland only for X11 windows;
    /// the Xwayland root window is not readable.
    LinuxX11,
    /// ScreenCaptureKit (macOS 13.0+).
    MacScreenCaptureKit,
    /// DXGI desktop duplication. Displays only; the cursor is not embedded.
    WindowsDxgi,
    /// Windows Graphics Capture (Windows 10 1903+). Displays and windows.
    WindowsWgc,
}

/// Identifies a concrete backend implementation (video or audio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// Wayland XDG portal + PipeWire video.
    LinuxWaylandPortal,
    /// X11 `GetImage` polling video.
    LinuxX11,
    /// PipeWire sink-monitor system audio.
    LinuxPipeWireAudio,
    /// macOS ScreenCaptureKit video and audio.
    MacScreenCaptureKit,
    /// Windows DXGI desktop duplication video.
    WindowsDxgi,
    /// Windows Graphics Capture video.
    WindowsWgc,
    /// Windows WASAPI loopback audio.
    WindowsWasapi,
}

/// What a backend can do — surfaced through
/// [`CaptureSession::backend_info`](crate::CaptureSession::backend_info) so
/// applications (and bug reports) can tell which path was actually taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    /// Which implementation this is. For a session with both video and
    /// audio, this is the video backend.
    pub kind: BackendKind,
    /// Whether this backend (or session) delivers audio.
    pub supports_audio: bool,
    /// Whether frames stay in GPU/native memory instead of [`FrameData::Host`].
    /// `false` for every backend today.
    ///
    /// [`FrameData::Host`]: crate::FrameData::Host
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
    /// Describes this backend.
    fn info(&self) -> BackendInfo;

    /// Begins producing frames.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS capture stream cannot be started.
    fn start(&mut self) -> Result<()>;

    /// Stops producing frames and releases the OS capture stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS reports a failure while stopping.
    fn stop(&mut self) -> Result<()>;

    /// Waits up to `timeout` for the next video frame or stream event.
    ///
    /// # Errors
    ///
    /// Returns [`PinrayError::Timeout`] when nothing arrived in time, and
    /// other variants for backend failures.
    ///
    /// [`PinrayError::Timeout`]: crate::PinrayError::Timeout
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent>;

    /// Returns the portal restore token negotiated for this session, if any.
    ///
    /// Only meaningful for backends that go through a permission portal
    /// (Wayland's XDG Desktop Portal); other backends keep the default `None`.
    fn restore_token(&self) -> Option<String> {
        None
    }
}

/// A platform audio capture implementation; same timeout contract as
/// [`VideoBackend`].
pub trait AudioBackend: Send {
    /// Describes this backend.
    fn info(&self) -> BackendInfo;

    /// Begins producing audio packets.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS audio stream cannot be started.
    fn start(&mut self) -> Result<()>;

    /// Stops producing audio and releases the OS audio stream.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS reports a failure while stopping.
    fn stop(&mut self) -> Result<()>;

    /// Waits up to `timeout` for the next audio packet.
    ///
    /// # Errors
    ///
    /// Returns [`PinrayError::Timeout`] when nothing arrived in time, and
    /// other variants for backend failures.
    ///
    /// [`PinrayError::Timeout`]: crate::PinrayError::Timeout
    fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame>;
}

/// The backends a [`BackendResolver`] picked for one session.
///
/// At least one of `video` and `audio` should be `Some`; a session with
/// neither fails every read with
/// [`PinrayError::BackendNotSelected`](crate::PinrayError::BackendNotSelected).
pub struct BackendBundle {
    /// Reported by [`CaptureSession::backend_info`](crate::CaptureSession::backend_info).
    pub info: BackendInfo,
    /// The video backend, if the session captures video.
    pub video: Option<Box<dyn VideoBackend>>,
    /// The audio backend, if the session captures audio.
    pub audio: Option<Box<dyn AudioBackend>>,
}

/// Turns a validated [`SessionConfig`] into concrete backends.
///
/// Pass an implementation to
/// [`SessionBuilder::build_with_resolver`](crate::SessionBuilder::build_with_resolver).
/// The `pinray` crate supplies one for the current OS; implement your own for
/// custom or mock backends.
pub trait BackendResolver {
    /// Picks and constructs backends for `config`.
    ///
    /// `config` has already passed [`SessionConfig::validate`]. This is where
    /// permission prompts happen (e.g. the Wayland portal dialog).
    ///
    /// # Errors
    ///
    /// Returns an error if no backend can satisfy the configuration, the
    /// requested source does not exist, or the user denies permission.
    fn resolve(&self, config: &SessionConfig) -> Result<BackendBundle>;
}
