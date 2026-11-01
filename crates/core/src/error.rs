//! The error type returned by every fallible pinray operation.

use std::time::Duration;

use thiserror::Error;

/// Everything that can go wrong while configuring or running a capture.
///
/// Match on the variant to decide how to react. Most variants carry a
/// human-readable message; log it for diagnostics.
///
/// # Examples
///
/// Treat a timeout as "nothing yet" and everything else as fatal:
///
/// ```
/// use std::time::Duration;
/// use pinray_core::{CaptureEvent, PinrayError};
///
/// fn handle(result: Result<CaptureEvent, PinrayError>) -> Result<(), PinrayError> {
///     match result {
///         Ok(event) => println!("got {event:?}"),
///         Err(PinrayError::Timeout(after)) => println!("no event within {after:?}"),
///         Err(other) => return Err(other),
///     }
///     Ok(())
/// }
///
/// handle(Err(PinrayError::Timeout(Duration::from_millis(5)))).unwrap();
/// assert!(handle(Err(PinrayError::Platform("boom".into()))).is_err());
/// ```
#[derive(Debug, Error)]
pub enum PinrayError {
    /// The session configuration is invalid, e.g. no video target and no
    /// audio, a zero `frame_rate`/`queue_depth`, or a crop rectangle outside
    /// the source. Fix the builder call; retrying will not help.
    #[error("invalid session configuration: {0}")]
    InvalidConfig(String),
    /// The requested backend cannot run on this machine, e.g. no Wayland
    /// session and no `DISPLAY` on Linux, or macOS older than 13.0.
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    /// The session has no backend for the requested stream, e.g. calling
    /// `next_audio` on a video-only session.
    #[error("capture backend not selected")]
    BackendNotSelected,
    /// No event arrived within the given timeout. The session is still
    /// healthy; call again to keep waiting.
    #[error("capture timed out after {0:?}")]
    Timeout(Duration),
    /// The configuration is valid but this backend does not implement it,
    /// e.g. microphone capture, or a pixel format the backend can't produce.
    /// Also returned when the user cancels the Wayland portal dialog.
    #[error("operation not supported: {0}")]
    Unsupported(String),
    /// The OS capture API reported an error, or macOS screen-recording
    /// permission was refused.
    #[error("platform error: {0}")]
    Platform(String),
}

/// Shorthand for `std::result::Result<T, PinrayError>`.
pub type Result<T> = std::result::Result<T, PinrayError>;
