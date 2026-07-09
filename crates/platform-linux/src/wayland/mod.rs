//! Wayland video backend: XDG Desktop Portal session + PipeWire stream.
//!
//! The portal negotiates permission and hands us a PipeWire fd + node id;
//! a dedicated worker thread (see `worker.rs`) runs the PipeWire main loop
//! and pushes frames through an mpsc channel. Restore tokens let repeat
//! sessions skip the permission dialog.
//!
//! Module layout:
//! - `worker.rs` — PipeWire main-loop thread
//! - `format.rs` — EnumFormat params + pixel-format normalization

mod format;
mod worker;

use std::{sync::mpsc, thread, time::Duration};

use pinray_core::{
    AudioBackend, BackendBundle, BackendInfo, BackendKind, CaptureEvent, CursorMode, PinrayError,
    Result, SessionConfig, VideoBackend,
};

use crate::{audio::resolve_system_audio, portal::PortalClient};

/// Returns `true` if the current session is a Wayland session.
///
/// Checks `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY` environment variables.
pub fn is_wayland_session() -> bool {
    std::env::var_os("XDG_SESSION_TYPE")
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Resolves the Wayland video and/or PipeWire audio backends for the given
/// session configuration.
///
/// System audio is captured natively from the default sink monitor and does
/// not need a portal round-trip, so audio-only sessions skip the portal
/// dialog entirely.
pub fn resolve_wayland_backend(config: &SessionConfig) -> Result<BackendBundle> {
    let audio: Option<Box<dyn AudioBackend>> = resolve_system_audio(&config.audio_capture)?;

    let video: Option<Box<dyn VideoBackend>> = if config.video_target.is_some() {
        Some(Box::new(WaylandVideoBackend::new(config.clone())?))
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

#[derive(Debug)]
pub(super) enum ControlMessage {
    Start,
    Stop,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VideoSize {
    pub width: u32,
    pub height: u32,
}

struct WaylandVideoBackend {
    info: BackendInfo,
    restore_token: Option<String>,
    control_tx: mpsc::Sender<ControlMessage>,
    event_rx: mpsc::Receiver<CaptureEvent>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl WaylandVideoBackend {
    /// Creates a new Wayland capture backend.
    ///
    /// Opens a portal session, obtains the PipeWire fd, and spawns a worker
    /// thread that runs the PipeWire main loop and frame capture.
    fn new(config: SessionConfig) -> Result<Self> {
        let portal = PortalClient::new()?;
        let cast = portal.start_screen_cast(
            matches!(config.cursor_mode, CursorMode::Embedded),
            config.restore_token.as_deref(),
        )?;
        let (fd, streams, restore_token) = cast.into_parts();
        let stream = streams
            .into_iter()
            .next()
            .ok_or_else(|| PinrayError::Platform("portal returned no screencast stream".into()))?;

        if let Some(token) = &restore_token {
            tracing::debug!(restore_token = %token, "portal returned restore token");
        }

        let (control_tx, control_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let desired_format = config.pixel_format;
        let frame_rate = config.frame_rate.unwrap_or(60);
        let stream_size = stream
            .width
            .zip(stream.height)
            .map(|(width, height)| VideoSize { width, height });

        // CRITICAL: The portal (D-Bus connection) must stay alive while PipeWire
        // uses the fd obtained from it. Dropping the PortalClient closes the D-Bus
        // socket, which invalidates the fd and causes "no more input formats" errors.
        let worker = thread::spawn(move || {
            let _portal_keepalive = portal;
            worker::run_video_loop(
                fd,
                stream.node_id,
                stream_size,
                desired_format,
                frame_rate,
                control_rx,
                event_tx,
            )
        });

        Ok(Self {
            info: BackendInfo {
                kind: BackendKind::LinuxWaylandPortal,
                supports_audio: false,
                zero_copy: false,
                notes: "Wayland video via XDG Desktop Portal + PipeWire",
            },
            restore_token,
            control_tx,
            event_rx,
            worker: Some(worker),
        })
    }
}

impl VideoBackend for WaylandVideoBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    fn restore_token(&self) -> Option<String> {
        self.restore_token.clone()
    }

    fn start(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Start)
            .map_err(|_| PinrayError::Platform("wayland capture worker is not available".into()))
    }

    fn stop(&mut self) -> Result<()> {
        self.control_tx
            .send(ControlMessage::Stop)
            .map_err(|_| PinrayError::Platform("wayland capture worker is not available".into()))
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        match timeout {
            Some(timeout) => self
                .event_rx
                .recv_timeout(timeout)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => PinrayError::Timeout(timeout),
                    mpsc::RecvTimeoutError::Disconnected => {
                        PinrayError::Platform("wayland event channel disconnected".into())
                    }
                }),
            None => self
                .event_rx
                .recv()
                .map_err(|_| PinrayError::Platform("wayland event channel disconnected".into())),
        }
    }
}

impl Drop for WaylandVideoBackend {
    fn drop(&mut self) {
        let _ = self.control_tx.send(ControlMessage::Terminate);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn platform_error(error: impl std::fmt::Display) -> PinrayError {
    PinrayError::Platform(error.to_string())
}
