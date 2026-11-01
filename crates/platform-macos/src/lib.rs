//! macOS capture backend: ScreenCaptureKit (macOS 13.0+) for display,
//! window, and system-audio capture.
//!
//! This is an implementation crate for [pinray](https://docs.rs/pinray);
//! depend on `pinray` instead.
#![warn(missing_docs)]

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod content;
#[cfg(target_os = "macos")]
mod permissions;

use pinray_core::{
    BackendBundle, BackendInfo, BackendKind, BackendPreference, CaptureSource, PinrayError, Result,
    SessionConfig,
};

/// ScreenCaptureKit itself is 12.3+, but the stream configuration sends audio
/// setters that only exist on 13.0+ and abort with an unrecognized selector below it.
#[cfg(target_os = "macos")]
fn os_supported() -> bool {
    objc2_foundation::NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(
        objc2_foundation::NSOperatingSystemVersion {
            majorVersion: 13,
            minorVersion: 0,
            patchVersion: 0,
        },
    )
}

#[cfg(not(target_os = "macos"))]
fn os_supported() -> bool {
    false
}

/// Lists the ScreenCaptureKit backends with their capabilities; empty on other OSes and before macOS 13.0.
pub fn available_backends() -> Vec<BackendInfo> {
    if os_supported() {
        vec![BackendInfo {
            kind: BackendKind::MacScreenCaptureKit,
            supports_audio: true,
            zero_copy: false,
            notes: "ScreenCaptureKit (macOS 13.0+): display/window capture with optional system audio",
        }]
    } else {
        Vec::new()
    }
}

/// Enumerates ScreenCaptureKit capture sources; empty on other OSes.
///
/// # Errors
///
/// Returns [`pinray_core::PinrayError::Platform`] if the OS enumeration API fails or
/// screen-recording permission is denied. Triggers the permission check.
pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    if !cfg!(target_os = "macos") {
        return Ok(Vec::new());
    }

    #[cfg(target_os = "macos")]
    {
        permissions::ensure_screen_capture_permission()?;
        content::enumerate_sources()
    }

    #[cfg(not(target_os = "macos"))]
    unreachable!()
}

/// Picks ScreenCaptureKit backends for `config`, or `Ok(None)` if this crate does not
/// handle the requested [`pinray_core::BackendPreference`] (or this isn't macOS).
///
/// Called by `pinray`'s platform resolver; applications should use
/// `pinray::CaptureSession::builder` instead.
///
/// # Errors
///
/// - [`pinray_core::PinrayError::BackendUnavailable`] on macOS older than 13.0.
/// - [`pinray_core::PinrayError::Platform`] if screen-recording permission
///   is denied, the requested source doesn't exist, or ScreenCaptureKit
///   fails.
/// - [`pinray_core::PinrayError::Unsupported`] for microphone capture.
/// - [`pinray_core::PinrayError::InvalidConfig`] if the source id is
///   malformed.
pub fn try_resolve(config: &SessionConfig) -> Result<Option<BackendBundle>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }

    let applies = matches!(
        config.backend_preference,
        BackendPreference::Auto | BackendPreference::MacScreenCaptureKit
    );
    if !applies {
        return Ok(None);
    }
    if !os_supported() {
        return Err(PinrayError::BackendUnavailable(
            "ScreenCaptureKit backend requires macOS 13.0+".into(),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        permissions::ensure_screen_capture_permission()?;
        capture::build_backend(config).map(Some)
    }

    #[cfg(not(target_os = "macos"))]
    unreachable!()
}
