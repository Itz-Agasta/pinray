use pinray_core::{PinrayError, Result};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Check whether the process already holds screen-recording permission.
///
/// Returns `Ok(())` if access is granted, or `Err(PinrayError::Platform)` if
/// access is denied or the check fails.
pub fn preflight_screen_capture() -> Result<()> {
    let granted = unsafe { CGPreflightScreenCaptureAccess() };
    if granted {
        Ok(())
    } else {
        Err(PinrayError::Platform(
            "screen recording permission not granted; call request_screen_capture() and prompt the user".into(),
        ))
    }
}

/// Trigger the system permission dialog and wait for the user's response.
///
/// On macOS 10.15+, this may open the Privacy & Security System Preferences
/// pane. The function returns `Ok(())` only if the user grants access.
pub fn request_screen_capture() -> Result<()> {
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    if granted {
        Ok(())
    } else {
        Err(PinrayError::Platform(
            "user denied screen recording permission".into(),
        ))
    }
}

/// Ensure screen-recording permission is held, requesting it if needed.
///
/// First preflights without a dialog. Falls back to requesting if not already
/// granted. Returns `Ok(())` only when access is confirmed.
pub fn ensure_screen_capture_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    request_screen_capture()
}
