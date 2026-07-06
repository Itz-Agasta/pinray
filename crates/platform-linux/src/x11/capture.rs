//! Polling X11 video capture via `GetImage`.
//!
//! Pull-model like the Windows DXGI backend: `next_event` paces itself to
//! the configured frame rate, then issues a synchronous `GetImage` for the
//! target region. This is fundamentally a polling screenshot loop — X11 has
//! no damage-driven streaming path comparable to portals/DXGI/SCKit — and is
//! documented as such. Shared-memory (XShm) transfer is a later optimization.
//!
//! `GetImage` never includes the pointer, so for display capture with
//! `CursorMode::Embedded` the cursor is fetched via XFixes and alpha-blended
//! into the frame (XFixes delivers premultiplied ARGB).

use std::time::{Duration, Instant};

use pinray_core::{
    BackendInfo, BackendKind, CaptureEvent, CursorMode, FrameData, PinrayError, PixelFormat,
    Result, SessionConfig, VideoBackend, VideoCaptureTarget, VideoFrame,
};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::ConnectionExt as XfixesConnectionExt;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use super::enumerate::{find_monitor, parse_window_id};
use super::x11_error;
use crate::clock::monotonic_time_ns;

enum Target {
    /// Region of the root window (monitor capture). Cursor coordinates are
    /// root-relative, so the region origin doubles as the cursor offset.
    Root {
        root: Window,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    /// A specific application window; size re-read every frame since windows
    /// resize. Cursor embedding is not supported here (best-effort backend).
    Window(Window),
}

pub(super) struct X11VideoBackend {
    conn: RustConnection,
    target: Target,
    crop: Option<pinray_core::Rect>,
    pixel_format: PixelFormat,
    embed_cursor: bool,
    frame_interval: Duration,
    next_due: Option<Instant>,
    sequence: u64,
    running: bool,
}

impl X11VideoBackend {
    pub(super) fn new(config: &SessionConfig) -> Result<Self> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(x11_error("x11 connect (is DISPLAY set?)"))?;
        let root = conn.setup().roots[screen_num].root;

        let target = match &config.video_target {
            Some(VideoCaptureTarget::Window(id)) => Target::Window(parse_window_id(id)?),
            display => {
                let auto = pinray_core::SourceId::new("auto");
                let id = match display {
                    Some(VideoCaptureTarget::Display(id)) => id,
                    _ => &auto,
                };
                let monitor = find_monitor(&conn, root, id)?;
                Target::Root {
                    root,
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                }
            }
        };

        let embed_cursor = matches!(config.cursor_mode, CursorMode::Embedded)
            && matches!(target, Target::Root { .. });
        if embed_cursor {
            // XFixes requires a version handshake before other requests.
            conn.xfixes_query_version(5, 0)
                .map_err(x11_error("xfixes_query_version"))?
                .reply()
                .map_err(x11_error("xfixes_query_version reply"))?;
        }

        let fps = config.frame_rate.unwrap_or(30).max(1);

        Ok(Self {
            conn,
            target,
            crop: config.crop_rect,
            pixel_format: config.pixel_format,
            embed_cursor,
            frame_interval: Duration::from_nanos(1_000_000_000 / fps as u64),
            next_due: None,
            sequence: 0,
            running: false,
        })
    }

    pub(super) fn backend_info() -> BackendInfo {
        BackendInfo {
            kind: BackendKind::LinuxX11,
            supports_audio: false,
            zero_copy: false,
            notes: "X11 polling capture via GetImage (frame_rate-paced, not damage-driven)",
        }
    }

    /// Resolves the capture rectangle for this frame: monitor region or
    /// current window geometry, with the config crop applied on top.
    fn frame_region(&self) -> Result<(Window, i16, i16, u16, u16)> {
        let (drawable, base_x, base_y, base_w, base_h) = match &self.target {
            Target::Root {
                root,
                x,
                y,
                width,
                height,
            } => (*root, *x, *y, *width, *height),
            Target::Window(window) => {
                let geo = self
                    .conn
                    .get_geometry(*window)
                    .map_err(x11_error("get_geometry"))?
                    .reply()
                    .map_err(|_| {
                        PinrayError::Platform("window disappeared during capture".into())
                    })?;
                (*window, 0, 0, geo.width, geo.height)
            }
        };

        match self.crop {
            None => Ok((drawable, base_x, base_y, base_w, base_h)),
            Some(rect) => {
                let cx = rect.x.max(0) as u16;
                let cy = rect.y.max(0) as u16;
                if cx as u32 + rect.width > base_w as u32
                    || cy as u32 + rect.height > base_h as u32
                {
                    return Err(PinrayError::InvalidConfig(format!(
                        "crop_rect exceeds capture region {base_w}x{base_h}"
                    )));
                }
                Ok((
                    drawable,
                    base_x + cx as i16,
                    base_y + cy as i16,
                    rect.width as u16,
                    rect.height as u16,
                ))
            }
        }
    }

    fn blend_cursor(&self, frame: &mut [u8], region_x: i16, region_y: i16, w: u16, h: u16) {
        let Ok(cookie) = self.conn.xfixes_get_cursor_image() else {
            return;
        };
        let Ok(cursor) = cookie.reply() else {
            return;
        };

        // Cursor x/y are the pointer position in root coordinates; the image
        // is anchored at (x - xhot, y - yhot). XFixes pixels are
        // premultiplied ARGB, so: out = src + dst * (255 - alpha) / 255.
        let origin_x = cursor.x as i32 - cursor.xhot as i32 - region_x as i32;
        let origin_y = cursor.y as i32 - cursor.yhot as i32 - region_y as i32;

        for row in 0..cursor.height as i32 {
            let fy = origin_y + row;
            if fy < 0 || fy >= h as i32 {
                continue;
            }
            for col in 0..cursor.width as i32 {
                let fx = origin_x + col;
                if fx < 0 || fx >= w as i32 {
                    continue;
                }
                let px = cursor.cursor_image[(row * cursor.width as i32 + col) as usize];
                let a = (px >> 24) & 0xff;
                if a == 0 {
                    continue;
                }
                let (sr, sg, sb) = ((px >> 16) & 0xff, (px >> 8) & 0xff, px & 0xff);
                let idx = (fy as usize * w as usize + fx as usize) * 4;
                // Frame is BGRA at this point (swizzle happens afterwards).
                frame[idx] = (sb + frame[idx] as u32 * (255 - a) / 255) as u8;
                frame[idx + 1] = (sg + frame[idx + 1] as u32 * (255 - a) / 255) as u8;
                frame[idx + 2] = (sr + frame[idx + 2] as u32 * (255 - a) / 255) as u8;
                frame[idx + 3] = 255;
            }
        }
    }
}

impl VideoBackend for X11VideoBackend {
    fn info(&self) -> BackendInfo {
        Self::backend_info()
    }

    fn start(&mut self) -> Result<()> {
        self.running = true;
        self.next_due = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.running = false;
        Ok(())
    }

    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent> {
        if !self.running {
            return Err(PinrayError::Platform("x11 backend not started".into()));
        }

        // Pace to the configured frame rate; a shorter caller timeout wins.
        let now = Instant::now();
        let due = self.next_due.unwrap_or(now);
        let wait = due.saturating_duration_since(now);
        if let Some(t) = timeout
            && wait > t
        {
            std::thread::sleep(t);
            return Err(PinrayError::Timeout(t));
        }
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        self.next_due = Some(due.max(now) + self.frame_interval);

        let (drawable, x, y, w, h) = self.frame_region()?;

        let image = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, u32::MAX)
            .map_err(x11_error("get_image"))?
            .reply()
            .map_err(|error| {
                // BadMatch on the root window is the rootless-Xwayland
                // signature: the root exists but has no readable backing.
                if matches!(self.target, Target::Root { .. }) {
                    PinrayError::BackendUnavailable(format!(
                        "GetImage on the root window failed ({error}); if this is a Wayland \
                         session, the Xwayland root is not readable — use the Wayland backend"
                    ))
                } else {
                    x11_error("get_image reply")(error)
                }
            })?;

        if image.depth != 24 && image.depth != 32 {
            return Err(PinrayError::Unsupported(format!(
                "x11 visual depth {} not supported (need 24/32-bit truecolor)",
                image.depth
            )));
        }
        let expected = w as usize * h as usize * 4;
        let mut data = image.data;
        if data.len() < expected {
            return Err(PinrayError::Platform(format!(
                "get_image returned {} bytes, expected {expected}",
                data.len()
            )));
        }
        data.truncate(expected);

        // ZPixmap on a little-endian truecolor visual is effectively BGRx;
        // depth-24 alpha bytes are undefined, so force them opaque.
        if image.depth == 24 {
            for px in data.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }

        if self.embed_cursor {
            self.blend_cursor(&mut data, x, y, w, h);
        }

        if self.pixel_format == PixelFormat::Rgba8888 {
            for px in data.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }

        let sequence = self.sequence;
        self.sequence += 1;

        Ok(CaptureEvent::Video(VideoFrame {
            stream_time_ns: monotonic_time_ns(),
            sequence,
            width: w as u32,
            height: h as u32,
            stride: w as u32 * 4,
            pixel_format: self.pixel_format,
            color_space: None,
            data: FrameData::Host(data),
            damage: None,
        }))
    }
}
