//! X11 video backend and enumeration.
//!
//! Module layout:
//! - `enumerate.rs` — RandR monitors + EWMH window list
//! - `capture.rs`   — polling `GetImage` backend with XFixes cursor overlay
//!
//! System audio pairs through the session-agnostic PipeWire backend, same
//! as on Wayland.

mod capture;
mod enumerate;

use pinray_core::{
    AudioBackend, BackendBundle, BackendInfo, CaptureSource, PinrayError, Result, SessionConfig,
    VideoBackend,
};
use x11rb::connection::Connection;

use crate::audio::resolve_system_audio;

pub(crate) fn backend_info() -> BackendInfo {
    capture::X11VideoBackend::backend_info()
}

/// X11 is reachable when a `DISPLAY` is set — this includes Xwayland, which
/// makes the backend usable (against the Xwayland root) inside Wayland
/// sessions when explicitly requested.
pub fn is_x11_available() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

pub fn resolve_x11_backend(config: &SessionConfig) -> Result<BackendBundle> {
    let audio: Option<Box<dyn AudioBackend>> = resolve_system_audio(&config.audio_capture)?;

    let video: Option<Box<dyn VideoBackend>> = if config.video_target.is_some() {
        Some(Box::new(capture::X11VideoBackend::new(config)?))
    } else {
        None
    };

    let info = match (&video, &audio) {
        (Some(video), _) => {
            let mut info = video.info();
            info.supports_audio = audio.is_some();
            info
        }
        (None, Some(audio)) => audio.info(),
        (None, None) => {
            return Err(PinrayError::InvalidConfig(
                "at least one of video_target or audio_capture must be set".into(),
            ));
        }
    };

    Ok(BackendBundle { info, video, audio })
}

/// Enumerates monitors and windows over a short-lived X11 connection.
pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(x11_error("x11 connect (is DISPLAY set?)"))?;
    let root = conn.setup().roots[screen_num].root;

    let mut sources = Vec::new();
    for entry in enumerate::enumerate_monitors(&conn, root)? {
        sources.push(CaptureSource::Display(entry.source));
    }
    for window in enumerate::enumerate_windows(&conn, root)? {
        sources.push(CaptureSource::Window(window));
    }
    Ok(sources)
}

pub(super) fn x11_error<E: std::fmt::Display>(
    context: &'static str,
) -> impl FnOnce(E) -> PinrayError {
    move |error| PinrayError::Platform(format!("{context}: {error}"))
}
