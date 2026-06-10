mod portal;
mod wayland;

use pinray_core::{
    BackendBundle, BackendInfo, BackendKind, BackendPreference, CaptureSource, PinrayError, Result,
    SessionConfig,
};

pub fn available_backends() -> Vec<BackendInfo> {
    if cfg!(target_os = "linux") {
        vec![
            BackendInfo {
                kind: BackendKind::LinuxWaylandPortal,
                supports_audio: false,
                zero_copy: false,
                notes: "Wayland portal + PipeWire video backend",
            },
            BackendInfo {
                kind: BackendKind::LinuxX11,
                supports_audio: false,
                zero_copy: false,
                notes: "X11 backend planned in phase 4",
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
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }

    let applies = matches!(
        config.backend_preference,
        BackendPreference::Auto
            | BackendPreference::LinuxWaylandPortal
            | BackendPreference::LinuxX11
    );

    if !applies {
        return Ok(None);
    }

    if wayland::is_wayland_session() {
        return wayland::resolve_wayland_backend(config).map(Some);
    }

    Err(PinrayError::BackendUnavailable(
        "linux x11 backend is not implemented yet; current session is not wayland".into(),
    ))
}
