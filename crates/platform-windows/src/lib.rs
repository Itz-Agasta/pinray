//! Windows capture backends: DXGI desktop duplication, Windows Graphics
//! Capture (WGC), and WASAPI loopback audio.
//!
//! Backend selection policy (see `docs/windows.md`):
//! - Display target + `Auto`: DXGI first, WGC as fallback.
//! - Window target + `Auto`: WGC (DXGI cannot capture single windows).
//! - Explicit `WindowsDxgi` / `WindowsWgc` preferences are honored as-is.
//! - `AudioCapture::SystemMix` pairs a WASAPI loopback backend with either
//!   video backend, or runs standalone for audio-only sessions.

#[cfg(target_os = "windows")]
mod d3d;
#[cfg(target_os = "windows")]
mod dxgi;
#[cfg(target_os = "windows")]
mod enumerate;
#[cfg(target_os = "windows")]
mod wasapi;
#[cfg(target_os = "windows")]
mod wgc;

use pinray_core::{
    BackendBundle, BackendInfo, BackendPreference, CaptureSource, Result, SessionConfig,
};

pub fn available_backends() -> Vec<BackendInfo> {
    #[cfg(target_os = "windows")]
    {
        vec![
            dxgi::DxgiVideoBackend::backend_info(),
            wgc::WgcVideoBackend::backend_info(),
            wasapi::WasapiAudioBackend::backend_info(),
        ]
    }

    #[cfg(not(target_os = "windows"))]
    Vec::new()
}

pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    #[cfg(target_os = "windows")]
    {
        enumerate::enumerate_all()
    }

    #[cfg(not(target_os = "windows"))]
    Ok(Vec::new())
}

pub fn try_resolve(config: &SessionConfig) -> Result<Option<BackendBundle>> {
    let applies = matches!(
        config.backend_preference,
        BackendPreference::Auto | BackendPreference::WindowsDxgi | BackendPreference::WindowsWgc
    );
    if !cfg!(target_os = "windows") || !applies {
        return Ok(None);
    }

    #[cfg(target_os = "windows")]
    {
        resolve(config).map(Some)
    }

    #[cfg(not(target_os = "windows"))]
    unreachable!()
}

#[cfg(target_os = "windows")]
fn resolve(config: &SessionConfig) -> Result<BackendBundle> {
    use pinray_core::{
        AudioBackend, AudioCapture, PinrayError, PixelFormat, VideoBackend, VideoCaptureTarget,
    };
    use tracing::warn;

    if !matches!(
        config.pixel_format,
        PixelFormat::Bgra8888 | PixelFormat::Rgba8888
    ) {
        return Err(PinrayError::Unsupported(format!(
            "windows backends support Bgra8888 and Rgba8888 only, got {:?}",
            config.pixel_format
        )));
    }
    if let Some(rect) = config.crop_rect
        && (rect.x < 0 || rect.y < 0)
    {
        return Err(PinrayError::InvalidConfig(
            "crop_rect x and y must be non-negative on windows backends".into(),
        ));
    }

    let video: Option<Box<dyn VideoBackend>> = match &config.video_target {
        None => None,
        Some(VideoCaptureTarget::Display(id)) => {
            let display = enumerate::find_display(id)?;
            match config.backend_preference {
                BackendPreference::WindowsWgc => Some(Box::new(wgc::WgcVideoBackend::new(
                    wgc::WgcTarget::Monitor(display.hmonitor),
                    config,
                )?)),
                BackendPreference::WindowsDxgi => {
                    Some(Box::new(dxgi::DxgiVideoBackend::new(&display, config)?))
                }
                _ => match dxgi::DxgiVideoBackend::new(&display, config) {
                    Ok(backend) => Some(Box::new(backend)),
                    Err(error) => {
                        warn!("dxgi unavailable ({error}); falling back to wgc");
                        Some(Box::new(wgc::WgcVideoBackend::new(
                            wgc::WgcTarget::Monitor(display.hmonitor),
                            config,
                        )?))
                    }
                },
            }
        }
        Some(VideoCaptureTarget::Window(id)) => {
            if config.backend_preference == BackendPreference::WindowsDxgi {
                return Err(PinrayError::Unsupported(
                    "DXGI desktop duplication cannot capture a single window; use WindowsWgc or Auto"
                        .into(),
                ));
            }
            let hwnd = enumerate::find_window(id)?;
            Some(Box::new(wgc::WgcVideoBackend::new(
                wgc::WgcTarget::Window(hwnd),
                config,
            )?))
        }
    };

    let audio: Option<Box<dyn AudioBackend>> = match &config.audio_capture {
        None => None,
        Some(AudioCapture::SystemMix) => Some(Box::new(wasapi::WasapiAudioBackend::new())),
        Some(AudioCapture::Microphone(_)) => {
            return Err(PinrayError::Unsupported(
                "microphone capture is not implemented on windows yet".into(),
            ));
        }
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
