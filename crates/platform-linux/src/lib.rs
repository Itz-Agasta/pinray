//! Linux capture backends: Wayland (XDG portal + PipeWire), X11 (polling
//! GetImage), and PipeWire system audio.
//!
//! Backend selection policy:
//! - `Auto` in a Wayland session → portal backend.
//! - `Auto` elsewhere with `DISPLAY` set → X11 backend.
//! - Explicit `LinuxWaylandPortal` / `LinuxX11` preferences are honored
//!   as-is; X11 works inside Wayland sessions too (via Xwayland) when asked
//!   for explicitly.
//! - `AudioCapture::SystemMix` uses PipeWire regardless of session type, so
//!   audio-only sessions never need a portal dialog or an X server.

#[cfg(target_os = "linux")]
mod audio;
#[cfg(target_os = "linux")]
mod clock;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
use pinray_core::{AudioDeviceSource, BackendKind, BackendPreference, PinrayError, SourceId};
use pinray_core::{BackendBundle, BackendInfo, CaptureSource, Result, SessionConfig};

pub const SYSTEM_AUDIO_ID: &str = "audio:system-mix";

pub fn available_backends() -> Vec<BackendInfo> {
    #[cfg(target_os = "linux")]
    {
        vec![
            BackendInfo {
                kind: BackendKind::LinuxWaylandPortal,
                supports_audio: false,
                zero_copy: false,
                notes: "Wayland portal + PipeWire video backend",
            },
            BackendInfo {
                kind: BackendKind::LinuxPipeWireAudio,
                supports_audio: true,
                zero_copy: false,
                notes: "PipeWire system-mix capture via default sink monitor",
            },
            x11::backend_info(),
        ]
    }

    #[cfg(not(target_os = "linux"))]
    Vec::new()
}

/// Enumerates displays and windows via X11 when a `DISPLAY` is reachable
/// (including Xwayland). Pure Wayland cannot enumerate without opening a
/// portal dialog, so there the list only carries the system-audio source and
/// display selection happens inside the portal dialog at session build.
pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    #[cfg(target_os = "linux")]
    {
        let mut sources = Vec::new();

        if x11::is_x11_available() {
            match x11::enumerate_sources() {
                Ok(x11_sources) => sources.extend(x11_sources),
                Err(error) => tracing::warn!(%error, "x11 source enumeration failed"),
            }
        }

        sources.push(CaptureSource::SystemAudio(AudioDeviceSource {
            id: SourceId::new(SYSTEM_AUDIO_ID),
            name: "System audio (PipeWire sink monitor)".into(),
            is_default: true,
        }));
        Ok(sources)
    }

    #[cfg(not(target_os = "linux"))]
    Ok(Vec::new())
}

pub fn try_resolve(config: &SessionConfig) -> Result<Option<BackendBundle>> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Ok(None)
    }

    #[cfg(target_os = "linux")]
    resolve(config)
}

#[cfg(target_os = "linux")]
fn resolve(config: &SessionConfig) -> Result<Option<BackendBundle>> {
    match config.backend_preference {
        BackendPreference::LinuxWaylandPortal => {
            return wayland::resolve_wayland_backend(config).map(Some);
        }
        BackendPreference::LinuxX11 => {
            if config.video_target.is_some() && !x11::is_x11_available() {
                return Err(PinrayError::BackendUnavailable(
                    "x11 backend requested but DISPLAY is not set".into(),
                ));
            }
            return x11::resolve_x11_backend(config).map(Some);
        }
        BackendPreference::Auto => {}
        _ => return Ok(None),
    }

    // Audio-only sessions don't care about the session type; PipeWire works
    // everywhere. resolve_x11_backend handles the video-less case without
    // touching the X server.
    if config.video_target.is_none() && config.audio_capture.is_some() {
        return x11::resolve_x11_backend(config).map(Some);
    }

    if wayland::is_wayland_session() {
        return wayland::resolve_wayland_backend(config).map(Some);
    }

    if x11::is_x11_available() {
        return x11::resolve_x11_backend(config).map(Some);
    }

    Err(PinrayError::BackendUnavailable(
        "no wayland session and no DISPLAY; cannot capture video on this host".into(),
    ))
}
