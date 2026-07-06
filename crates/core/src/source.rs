/// Opaque identifier for a capturable source.
///
/// Formats are platform-specific (e.g. `display:\\.\DISPLAY1`,
/// `window:1234`, `audio:system-mix`); treat them as opaque strings from
/// [`CaptureSource`] enumeration. The special value `"auto"` selects the
/// primary display without enumerating. Window ids are only valid while the
/// window exists — re-enumerate before building a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

impl SourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// A capturable display/monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySource {
    pub id: SourceId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Display scale × 1000 (e.g. 2000 = 2× HiDPI). Best-effort; 1000 where
    /// the platform has no reliable per-monitor scale.
    pub scale_factor_milli: u32,
    pub is_primary: bool,
}

/// A capturable application window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSource {
    pub id: SourceId,
    pub title: String,
    /// Owning application, when the platform exposes it.
    pub app_name: Option<String>,
}

/// A capturable audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceSource {
    pub id: SourceId,
    pub name: String,
    pub is_default: bool,
}

/// Anything [`enumerate_sources`](crate::CaptureSession) can return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    Display(DisplaySource),
    Window(WindowSource),
    SystemAudio(AudioDeviceSource),
    Microphone(AudioDeviceSource),
}
