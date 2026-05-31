use pinray_core::{
    BackendBundle, BackendInfo, BackendKind, BackendPreference, CaptureSource, PinrayError, Result,
    SessionConfig,
};

pub fn available_backends() -> Vec<BackendInfo> {
    if cfg!(target_os = "macos") {
        vec![BackendInfo {
            kind: BackendKind::MacScreenCaptureKit,
            supports_audio: true,
            zero_copy: true,
            notes: "ScreenCaptureKit backend not implemented yet",
        }]
    } else {
        Vec::new()
    }
}

pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    Ok(Vec::new())
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

    Err(PinrayError::BackendUnavailable(
        "macOS backend scaffolding exists, but ScreenCaptureKit will work on implementation later"
            .into(),
    ))
}
