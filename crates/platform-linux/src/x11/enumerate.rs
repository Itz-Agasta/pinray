//! X11 display and window enumeration.
//!
//! Displays come from RandR `GetMonitors` (per-monitor regions of the root
//! window, primary flag included). Windows come from the window manager's
//! EWMH `_NET_CLIENT_LIST`, which lists managed application windows and
//! avoids walking the tree of override-redirect/decoration windows.

use pinray_core::{DisplaySource, PinrayError, Result, SourceId, WindowSource};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

use super::x11_error;

/// A monitor plus the root-window region it occupies, needed by the capture
/// path to position `GetImage` and translate cursor coordinates.
pub(super) struct MonitorEntry {
    pub source: DisplaySource,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

pub(super) fn enumerate_monitors(
    conn: &RustConnection,
    root: Window,
) -> Result<Vec<MonitorEntry>> {
    let monitors = conn
        .randr_get_monitors(root, true)
        .map_err(x11_error("randr_get_monitors"))?
        .reply()
        .map_err(x11_error("randr_get_monitors reply"))?;

    let mut entries = Vec::new();
    for monitor in monitors.monitors {
        let name = conn
            .get_atom_name(monitor.name)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
            .unwrap_or_else(|| format!("monitor-{}", monitor.name));

        entries.push(MonitorEntry {
            source: DisplaySource {
                id: SourceId::new(format!("display:{name}")),
                name,
                width: monitor.width as u32,
                height: monitor.height as u32,
                // X11 has no reliable per-monitor scale; report 1×.
                scale_factor_milli: 1000,
                is_primary: monitor.primary,
            },
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
        });
    }
    Ok(entries)
}

/// Resolves a display `SourceId` (or `"auto"` for the primary monitor).
pub(super) fn find_monitor(
    conn: &RustConnection,
    root: Window,
    id: &SourceId,
) -> Result<MonitorEntry> {
    let mut entries = enumerate_monitors(conn, root)?;
    if entries.is_empty() {
        // Minimal servers (Xvfb without configured outputs) may report no
        // RandR monitors; fall back to the whole root screen.
        let geo = conn
            .get_geometry(root)
            .map_err(x11_error("get_geometry(root)"))?
            .reply()
            .map_err(x11_error("get_geometry(root) reply"))?;
        entries.push(MonitorEntry {
            source: DisplaySource {
                id: SourceId::new("display:root"),
                name: "root".into(),
                width: geo.width as u32,
                height: geo.height as u32,
                scale_factor_milli: 1000,
                is_primary: true,
            },
            x: 0,
            y: 0,
            width: geo.width,
            height: geo.height,
        });
    }

    if id.0 == "auto" {
        let primary = entries
            .iter()
            .position(|e| e.source.is_primary)
            .unwrap_or(0);
        return Ok(entries.swap_remove(primary));
    }

    entries
        .into_iter()
        .find(|e| e.source.id == *id)
        .ok_or_else(|| PinrayError::InvalidConfig(format!("display source not found: {}", id.0)))
}

/// Parses a window `SourceId` of the form `window:<xid>`.
pub(super) fn parse_window_id(id: &SourceId) -> Result<Window> {
    id.0.strip_prefix("window:")
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| PinrayError::InvalidConfig(format!("invalid window source id: {}", id.0)))
}

pub(super) fn enumerate_windows(
    conn: &RustConnection,
    root: Window,
) -> Result<Vec<WindowSource>> {
    let client_list = intern(conn, "_NET_CLIENT_LIST")?;
    let net_wm_name = intern(conn, "_NET_WM_NAME")?;
    let utf8_string = intern(conn, "UTF8_STRING")?;

    let reply = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .map_err(x11_error("get_property(_NET_CLIENT_LIST)"))?
        .reply()
        .map_err(x11_error("_NET_CLIENT_LIST reply"))?;

    let mut windows = Vec::new();
    for window in reply.value32().into_iter().flatten() {
        // Prefer the UTF-8 EWMH title, fall back to legacy WM_NAME.
        let title = read_string_property(conn, window, net_wm_name, utf8_string)
            .or_else(|| read_string_property(conn, window, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        // WM_CLASS is "instance\0class\0"; the class half names the app.
        let app_name = read_string_property(
            conn,
            window,
            AtomEnum::WM_CLASS.into(),
            AtomEnum::STRING.into(),
        )
        .and_then(|raw| {
            raw.split('\0')
                .filter(|s| !s.is_empty())
                .nth(1)
                .map(str::to_owned)
        });

        windows.push(WindowSource {
            id: SourceId::new(format!("window:{window}")),
            title,
            app_name,
        });
    }
    Ok(windows)
}

fn intern(conn: &RustConnection, name: &str) -> Result<u32> {
    Ok(conn
        .intern_atom(false, name.as_bytes())
        .map_err(x11_error("intern_atom"))?
        .reply()
        .map_err(x11_error("intern_atom reply"))?
        .atom)
}

fn read_string_property(
    conn: &RustConnection,
    window: Window,
    property: u32,
    ty: u32,
) -> Option<String> {
    let reply = conn
        .get_property(false, window, property, ty, 0, 1024)
        .ok()?
        .reply()
        .ok()?;
    if reply.value.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&reply.value).into_owned())
}
