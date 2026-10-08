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
