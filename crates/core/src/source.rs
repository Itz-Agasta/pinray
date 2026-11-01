//! Capturable sources (displays, windows, audio endpoints) and their ids.

/// Opaque identifier for a capturable source.
///
/// Formats are platform-specific (e.g. `display:\\.\DISPLAY1`,
/// `window:1234`, `audio:system-mix`); treat them as opaque strings from
/// [`CaptureSource`] enumeration. The special value `"auto"` selects the
/// primary display without enumerating. Window ids are only valid while the
/// window exists — re-enumerate before building a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(
    /// The raw platform-specific id string.
    pub String,
);

impl SourceId {
    /// Wraps a string as a source id.
    ///
    /// Use ids returned by source enumeration, or `"auto"` for the primary
    /// display. pinray does not validate the string here; an unknown id
    /// surfaces as an error when the session is built.
    ///
    /// # Examples
    ///
    /// ```
    /// use pinray_core::{SourceId, VideoCaptureTarget};
    ///
    /// let target = VideoCaptureTarget::Display(SourceId::new("auto"));
    /// assert_eq!(SourceId::new("auto").0, "auto");
    /// # let _ = target;
    /// ```
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// A capturable display/monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySource {
    /// Pass to [`VideoCaptureTarget::Display`](crate::VideoCaptureTarget::Display).
    pub id: SourceId,
    /// Human-readable name, e.g. a connector name like `DP-1` or the
    /// monitor model. Not guaranteed unique.
    pub name: String,
    /// Width as reported by the platform: pixels on Linux and Windows,
    /// points on macOS (multiply by the display's backing scale for pixels).
    pub width: u32,
    /// Height, in the same units as [`width`](Self::width).
    pub height: u32,
    /// Display scale × 1000 (e.g. 2000 = 2× HiDPI). Best-effort; 1000 where
    /// the platform has no reliable per-monitor scale.
    pub scale_factor_milli: u32,
    /// Whether this is the primary display (the one `"auto"` selects).
    /// Not reported on macOS yet, where it is always `false`.
    pub is_primary: bool,
}

/// A capturable application window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSource {
    /// Pass to [`VideoCaptureTarget::Window`](crate::VideoCaptureTarget::Window).
    /// Valid only while the window exists.
    pub id: SourceId,
    /// Window title at enumeration time; may be empty.
    pub title: String,
    /// Owning application, when the platform exposes it.
    pub app_name: Option<String>,
}

/// A capturable audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceSource {
    /// Platform id of the endpoint.
    pub id: SourceId,
    /// Human-readable device name.
    pub name: String,
    /// Whether this is the system's default device.
    pub is_default: bool,
}

/// One entry returned by source enumeration (`pinray::enumerate_sources`).
///
/// # Examples
///
/// Pick the primary display out of an enumeration result:
///
/// ```
/// use pinray_core::{CaptureSource, SourceId};
///
/// fn primary_display(sources: &[CaptureSource]) -> Option<SourceId> {
///     sources.iter().find_map(|source| match source {
///         CaptureSource::Display(display) if display.is_primary => Some(display.id.clone()),
///         _ => None,
///     })
/// }
/// # assert_eq!(primary_display(&[]), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    /// A monitor that can be captured as a whole.
    Display(DisplaySource),
    /// A single application window.
    Window(WindowSource),
    /// A system playback mix (loopback), used by
    /// [`AudioCapture::SystemMix`](crate::AudioCapture::SystemMix).
    SystemAudio(AudioDeviceSource),
    /// A microphone or other input device. No backend returns this from
    /// enumeration yet, and microphone capture is not implemented.
    Microphone(AudioDeviceSource),
}
