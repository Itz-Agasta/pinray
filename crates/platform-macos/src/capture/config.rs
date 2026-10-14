//! SCContentFilter and SCStreamConfiguration builders: target lookup
//! (display/window), output sizing, pixel format, cursor, frame rate,
//! audio, and crop rect.

use objc2::rc::Retained;
use objc2::{AllocAnyThread, Message, runtime::NSObjectProtocol, sel};
use objc2_core_foundation::{CGRect, CGSize};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID};
use objc2_core_media::CMTimeFlags;
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use pinray_core::{CursorMode, PinrayError, Result, SessionConfig, VideoCaptureTarget};

pub(super) fn build_content_filter(
    content: &SCShareableContent,
    config: &SessionConfig,
) -> Result<(Retained<SCContentFilter>, (u32, u32))> {
    match &config.video_target {
        None | Some(VideoCaptureTarget::Display(_)) => {
            let display = find_display(content, config)?;
            let excluded = objc2_foundation::NSArray::<SCWindow>::new();
            let filter = unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            };
            let output_size = filter_dimensions(&filter).unwrap_or_else(|| {
                let size = CGSize {
                    width: unsafe { display.width() } as f64,
                    height: unsafe { display.height() } as f64,
                };
                pixel_dimensions(size, display_scale(&display))
            });
            Ok((filter, output_size))
        }
        Some(VideoCaptureTarget::Window(source_id)) => {
            let win = find_window(content, &source_id.0)?;
            let filter = unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &win)
            };
            let output_size = window_dimensions(content, &filter, &win);
            Ok((filter, output_size))
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
    output_size: (u32, u32),
    capture_audio: bool,
) -> Retained<SCStreamConfiguration> {
    let cfg = unsafe { SCStreamConfiguration::new() };

    // Choose the output size once. Resizing or moving a window later does not
    // change the frame dimensions during this capture session.
    let (out_w, out_h) = output_size;

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

    cfg
}

fn filter_dimensions(filter: &SCContentFilter) -> Option<(u32, u32)> {
    // These selectors were added in macOS 14. Guard both before sending either
    // message so the backend remains usable on macOS 13.
    if filter.respondsToSelector(sel!(contentRect))
        && filter.respondsToSelector(sel!(pointPixelScale))
    {
        let size = unsafe { filter.contentRect() }.size;
        let scale = unsafe { filter.pointPixelScale() } as f64;
        Some(pixel_dimensions(size, scale))
    } else {
        None
    }
}

fn window_dimensions(
    content: &SCShareableContent,
    filter: &SCContentFilter,
    window: &SCWindow,
) -> (u32, u32) {
    if let Some(size) = filter_dimensions(filter) {
        return size;
    }

    let frame = unsafe { window.frame() };
    let displays = unsafe { content.displays() };
    let display = displays
        .iter()
        .find(|display| contains_window_center(unsafe { display.frame() }, frame))
        .or_else(|| {
            let main_id = CGMainDisplayID();
            displays
                .iter()
                .find(|display| unsafe { display.displayID() } == main_id)
        });

    let scale = display
        .map(|display| display_scale(&display))
        .unwrap_or(1.0);

    pixel_dimensions(frame.size, scale)
}

fn display_scale(display: &SCDisplay) -> f64 {
    let pixel_width = CGDisplayCopyDisplayMode(unsafe { display.displayID() })
        .map(|mode| CGDisplayMode::pixel_width(Some(&mode)));
    pixel_scale(pixel_width, unsafe { display.width() })
}

fn pixel_scale(pixel_width: Option<usize>, point_width: isize) -> f64 {
    pixel_width
        .map(|pixels| pixels as f64 / point_width as f64)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0)
}

fn contains_window_center(display: CGRect, window: CGRect) -> bool {
    let x = window.origin.x + window.size.width / 2.0;
    let y = window.origin.y + window.size.height / 2.0;
    x >= display.origin.x
        && x < display.origin.x + display.size.width
        && y >= display.origin.y
        && y < display.origin.y + display.size.height
}

fn pixel_dimensions(size: CGSize, scale: f64) -> (u32, u32) {
    // Match the existing minimum/even output sizing used for display capture.
    (
        ((size.width * scale) as u32).max(2) & !1,
        ((size.height * scale) as u32).max(2) & !1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::CGPoint;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    #[test]
    fn display_fallback_size_uses_current_mode_scale() {
        let size = rect(0.0, 0.0, 1920.0, 1080.0).size;
        assert_eq!(
            pixel_dimensions(size, pixel_scale(Some(1920), 1920)),
            (1920, 1080)
        );
        assert_eq!(
            pixel_dimensions(size, pixel_scale(Some(3840), 1920)),
            (3840, 2160)
        );
        assert_eq!(
            pixel_dimensions(size, pixel_scale(Some(2880), 1920)),
            (2880, 1620)
        );
        // A scaled "looks like" mode can have a non-integer pixel-to-point ratio.
        assert_eq!(
            pixel_dimensions(
                rect(0.0, 0.0, 1800.0, 1169.0).size,
                pixel_scale(Some(3024), 1800),
            ),
            (3024, 1962)
        );
    }

    #[test]
    fn display_scale_falls_back_when_mode_is_missing_or_invalid() {
        assert_eq!(pixel_scale(None, 1920), 1.0);
        assert_eq!(pixel_scale(Some(0), 1920), 1.0);
        assert_eq!(pixel_scale(Some(1920), 0), 1.0);
        assert_eq!(pixel_scale(Some(0), 0), 1.0);
        assert_eq!(pixel_scale(Some(1920), -1920), 1.0);
    }

    #[test]
    fn window_size_uses_its_own_bounds_at_the_display_scale() {
        let window = rect(200.0, 100.0, 800.0, 600.0);
        assert_eq!(pixel_dimensions(window.size, 1.0), (800, 600));
        assert_eq!(pixel_dimensions(window.size, 2.0), (1600, 1200));
        assert_eq!(pixel_dimensions(window.size, 1.5), (1200, 900));
    }

    #[test]
    fn output_size_preserves_minimum_and_even_dimensions() {
        assert_eq!(
            pixel_dimensions(rect(0.0, 0.0, 801.0, 603.0).size, 1.0),
            (800, 602)
        );
        assert_eq!(pixel_dimensions(rect(0.0, 0.0, 0.5, 1.0).size, 1.0), (2, 2));
        assert_eq!(
            pixel_dimensions(rect(0.0, 0.0, 400.5, 301.5).size, 2.0),
            (800, 602)
        );
        assert_eq!(
            pixel_dimensions(rect(0.0, 0.0, 801.0, 603.0).size, 1.5),
            (1200, 904)
        );
    }

    #[test]
    fn display_selection_uses_window_center_not_origin() {
        let left = rect(-1920.0, 0.0, 1920.0, 1080.0);
        let right = rect(0.0, 0.0, 2560.0, 1440.0);
        let spanning = rect(-200.0, 100.0, 800.0, 600.0);
        assert!(!contains_window_center(left, spanning));
        assert!(contains_window_center(right, spanning));

        let on_left = rect(-1000.0, 100.0, 800.0, 600.0);
        assert!(contains_window_center(left, on_left));
        assert!(!contains_window_center(right, on_left));
    }

    #[test]
    fn display_selection_handles_boundaries_and_offscreen_windows() {
        let left = rect(0.0, 0.0, 1920.0, 1080.0);
        let right = rect(1920.0, 0.0, 1920.0, 1080.0);
        let at_boundary = rect(1520.0, 100.0, 800.0, 600.0);
        assert!(!contains_window_center(left, at_boundary));
        assert!(contains_window_center(right, at_boundary));

        let offscreen = rect(100.0, -700.0, 800.0, 600.0);
        assert!(!contains_window_center(left, offscreen));
        assert!(!contains_window_center(right, offscreen));
    }
}
