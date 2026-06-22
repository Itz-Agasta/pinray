use pinray_core::{
    BackendBundle, BackendInfo, BackendKind, BackendPreference, CaptureSource, PinrayError, Result,
    SessionConfig,
};

pub fn available_backends() -> Vec<BackendInfo> {
    if cfg!(target_os = "windows") {
        vec![
            BackendInfo {
                kind: BackendKind::WindowsDxgi,
                supports_audio: true,
                zero_copy: true,
                notes: "DXGI backend not implemented yet",
            },
            BackendInfo {
                kind: BackendKind::WindowsWgc,
                supports_audio: true,
                zero_copy: true,
                notes: "WGC backend not implemented yet",
            },
        ]
    } else {
        Vec::new()
    }
}

pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    Ok(Vec::new())
}

pub fn try_resolve(config: &SessionConfig) -> Result<Option<BackendBundle>> {
    if !cfg!(target_os = "windows") {
        return Ok(None);
    }

    let applies = matches!(
        config.backend_preference,
        BackendPreference::Auto | BackendPreference::WindowsDxgi | BackendPreference::WindowsWgc
    );

    if !applies {
        return Ok(None);
    }

    Err(PinrayError::BackendUnavailable(
        "windows backend scaffolding exists, but will work on DXGI/WGC implementation later".into(),
    ))
}
