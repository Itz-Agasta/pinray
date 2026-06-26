#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod content;
#[cfg(target_os = "macos")]
mod permissions;

use pinray_core::{
    BackendBundle, BackendInfo, BackendKind, BackendPreference, CaptureSource, Result,
    SessionConfig,
};

pub fn available_backends() -> Vec<BackendInfo> {
    if cfg!(target_os = "macos") {
        vec![BackendInfo {
            kind: BackendKind::MacScreenCaptureKit,
            supports_audio: true,
            zero_copy: false,
            notes: "ScreenCaptureKit (macOS 12.3+): display/window capture with optional system audio",
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

    #[cfg(target_os = "macos")]
    {
        permissions::ensure_screen_capture_permission()?;
        capture::build_backend(config).map(Some)
    }

    #[cfg(not(target_os = "macos"))]
    unreachable!()
}
