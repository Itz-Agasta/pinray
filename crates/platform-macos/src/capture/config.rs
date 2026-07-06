//! SCContentFilter and SCStreamConfiguration builders: target lookup
//! (display/window), output sizing, pixel format, cursor, frame rate,
//! audio, and crop rect.

use objc2::rc::Retained;
use objc2::{AllocAnyThread, Message};
use objc2_core_media::CMTimeFlags;
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use pinray_core::{CursorMode, PinrayError, Result, SessionConfig, VideoCaptureTarget};

pub(super) fn build_content_filter(
    content: &SCShareableContent,
    config: &SessionConfig,
) -> Result<Retained<SCContentFilter>> {
    match &config.video_target {
        None | Some(VideoCaptureTarget::Display(_)) => {
            let display = find_display(content, config)?;
            let excluded = objc2_foundation::NSArray::<SCWindow>::new();
            Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            })
        }
        Some(VideoCaptureTarget::Window(source_id)) => {
            let win = find_window(content, &source_id.0)?;
            Ok(unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &win)
            })
        }
    }
}

fn find_display(
    content: &SCShareableContent,
    config: &SessionConfig,
) -> Result<Retained<SCDisplay>> {
    let displays = unsafe { content.displays() };

    if let Some(VideoCaptureTarget::Display(source_id)) = &config.video_target
        && source_id.0 != "auto"
    {
        let target_id: u32 = source_id.0.parse().map_err(|_| {
            PinrayError::InvalidConfig(format!("invalid display id '{}'", source_id.0))
        })?;
        for display in displays.iter() {
            if unsafe { display.displayID() } == target_id {
                return Ok(display.retain());
            }
        }
        return Err(PinrayError::Platform(format!(
            "display {target_id} not found"
        )));
    }

    displays
        .firstObject()
        .ok_or_else(|| PinrayError::Platform("no displays via SCShareableContent".into()))
}

fn find_window(content: &SCShareableContent, id_str: &str) -> Result<Retained<SCWindow>> {
    let target_id: u32 = id_str
        .parse()
        .map_err(|_| PinrayError::InvalidConfig(format!("invalid window id '{id_str}'")))?;
    let windows = unsafe { content.windows() };
    for window in windows.iter() {
        if unsafe { window.windowID() } == target_id {
            return Ok(window.retain());
        }
    }
    Err(PinrayError::Platform(format!(
        "window {target_id} not found"
    )))
}

pub(super) fn build_stream_configuration(
    config: &SessionConfig,
    content: &SCShareableContent,
    capture_audio: bool,
) -> Result<Retained<SCStreamConfiguration>> {
    let cfg = unsafe { SCStreamConfiguration::new() };

    let (display_w, display_h, scale) = display_dimensions(content, config);
    let out_w = (display_w * scale).max(2) & !1;
    let out_h = (display_h * scale).max(2) & !1;

    unsafe {
        cfg.setWidth(out_w as usize);
        cfg.setHeight(out_h as usize);
    }

    // SCKit only accepts BGRA (plus l10r/420v/420f/...); RGBA is not a valid
    // stream format. Always capture BGRA — normalize_pixels swizzles to RGBA
    // when the caller asked for it.
    unsafe { cfg.setPixelFormat(kCVPixelFormatType_32BGRA) };

    unsafe { cfg.setShowsCursor(matches!(config.cursor_mode, CursorMode::Embedded)) };

    let fps = config.frame_rate.unwrap_or(60).max(1) as i32;
    // kCMTimeFlags_Valid = 1; value/timescale = seconds → 1/fps = frame interval
    let interval = objc2_core_media::CMTime {
        value: 1,
        timescale: fps,
        flags: CMTimeFlags(1),
        epoch: 0,
    };
    unsafe { cfg.setMinimumFrameInterval(interval) };

    unsafe { cfg.setCapturesAudio(capture_audio) };

    if capture_audio {
        unsafe {
            cfg.setSampleRate(48_000); // NSInteger
            cfg.setChannelCount(2); // NSInteger
        }
    }

    if let Some(crop) = config.crop_rect {
        let cg_rect = objc2_core_foundation::CGRect {
            origin: objc2_core_foundation::CGPoint {
                x: crop.x as f64,
                y: crop.y as f64,
            },
            size: objc2_core_foundation::CGSize {
                width: crop.width as f64,
                height: crop.height as f64,
            },
        };
        unsafe { cfg.setSourceRect(cg_rect) };
    }

    Ok(cfg)
}

fn display_dimensions(content: &SCShareableContent, config: &SessionConfig) -> (u32, u32, u32) {
    let displays = unsafe { content.displays() };

    let display = if let Some(VideoCaptureTarget::Display(source_id)) = &config.video_target {
        if source_id.0 != "auto" {
            if let Ok(id) = source_id.0.parse::<u32>() {
                displays.iter().find(|d| unsafe { d.displayID() } == id)
            } else {
                displays.firstObject()
            }
        } else {
            displays.firstObject()
        }
    } else {
        displays.firstObject()
    };

    match display {
        Some(d) => {
            let w = unsafe { d.width() } as u32;
            let h = unsafe { d.height() } as u32;
            (w, h, 2) // 2× retina default; exact scale needs CGDisplayMode
        }
        None => (1920, 1080, 1),
    }
}
