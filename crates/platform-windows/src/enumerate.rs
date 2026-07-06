//! Display and window enumeration.
//!
//! Displays are enumerated through DXGI (adapter → output) so that a display
//! `SourceId` can later be mapped back to the exact adapter/output pair the
//! DXGI duplication backend needs. Windows are enumerated with `EnumWindows`.

use pinray_core::{
    AudioDeviceSource, CaptureSource, DisplaySource, PinrayError, Result, SourceId, WindowSource,
};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIOutput,
};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

use crate::d3d::win_err;

pub(crate) const SYSTEM_AUDIO_ID: &str = "audio:system-mix";

/// A display plus the DXGI adapter/output coordinates needed to duplicate it.
pub(crate) struct DisplayEntry {
    pub source: DisplaySource,
    pub adapter: IDXGIAdapter,
    pub output: IDXGIOutput,
    pub hmonitor: HMONITOR,
}

fn utf16_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

pub(crate) fn enumerate_displays() -> Result<Vec<DisplayEntry>> {
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| win_err("CreateDXGIFactory1", e))?;

    let mut entries = Vec::new();
    let mut adapter_index = 0u32;
    loop {
        let adapter: IDXGIAdapter = match unsafe { factory.EnumAdapters(adapter_index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        adapter_index += 1;

        let mut output_index = 0u32;
        loop {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(_) => break,
            };
            output_index += 1;

            let desc = match unsafe { output.GetDesc() } {
                Ok(desc) => desc,
                Err(_) => continue,
            };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }

            let name = utf16_to_string(&desc.DeviceName);
            let coords = desc.DesktopCoordinates;
            let width = (coords.right - coords.left).max(0) as u32;
            let height = (coords.bottom - coords.top).max(0) as u32;

            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = unsafe {
                GetDpiForMonitor(desc.Monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
            };

            entries.push(DisplayEntry {
                source: DisplaySource {
                    id: SourceId::new(format!("display:{name}")),
                    name,
                    width,
                    height,
                    scale_factor_milli: dpi_x * 1000 / 96,
                    is_primary: coords.left == 0 && coords.top == 0,
                },
                adapter: adapter.clone(),
                output,
                hmonitor: desc.Monitor,
            });
        }
    }

    Ok(entries)
}

/// Resolves a display `SourceId` (or `"auto"` for the primary display).
pub(crate) fn find_display(id: &SourceId) -> Result<DisplayEntry> {
    let mut entries = enumerate_displays()?;
    if entries.is_empty() {
        return Err(PinrayError::BackendUnavailable("no displays found".into()));
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

/// Parses a window `SourceId` of the form `window:<hwnd>` back into an HWND.
pub(crate) fn find_window(id: &SourceId) -> Result<HWND> {
    let raw = id
        .0
        .strip_prefix("window:")
        .and_then(|v| v.parse::<isize>().ok())
        .ok_or_else(|| PinrayError::InvalidConfig(format!("invalid window source id: {}", id.0)))?;
    Ok(HWND(raw as *mut core::ffi::c_void))
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    }
    .map(|_| cloaked != 0)
    .unwrap_or(false)
}

fn process_image_name(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(process) };
    result.ok()?;

    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit('\\').next().map(|s| s.to_string())
}

pub(crate) fn enumerate_windows() -> Result<Vec<WindowSource>> {
    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let windows = unsafe { &mut *(lparam.0 as *mut Vec<WindowSource>) };

        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || is_cloaked(hwnd) {
            return BOOL(1);
        }

        let mut title_buf = [0u16; 512];
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
        if title_len <= 0 {
            return BOOL(1);
        }

        windows.push(WindowSource {
            id: SourceId::new(format!("window:{}", hwnd.0 as isize)),
            title: String::from_utf16_lossy(&title_buf[..title_len as usize]),
            app_name: process_image_name(hwnd),
        });
        BOOL(1)
    }

    let mut windows: Vec<WindowSource> = Vec::new();
    unsafe { EnumWindows(Some(callback), LPARAM(&mut windows as *mut _ as isize)) }
        .map_err(|e| win_err("EnumWindows", e))?;
    Ok(windows)
}

pub(crate) fn enumerate_all() -> Result<Vec<CaptureSource>> {
    let mut sources = Vec::new();
    for entry in enumerate_displays()? {
        sources.push(CaptureSource::Display(entry.source));
    }
    for window in enumerate_windows()? {
        sources.push(CaptureSource::Window(window));
    }
    sources.push(CaptureSource::SystemAudio(AudioDeviceSource {
        id: SourceId::new(SYSTEM_AUDIO_ID),
        name: "System audio (WASAPI loopback)".into(),
        is_default: true,
    }));
    Ok(sources)
}
