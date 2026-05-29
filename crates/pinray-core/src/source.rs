#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

impl SourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySource {
    pub id: SourceId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor_milli: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSource {
    pub id: SourceId,
    pub title: String,
    pub app_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceSource {
    pub id: SourceId,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    Display(DisplaySource),
    Window(WindowSource),
    SystemAudio(AudioDeviceSource),
    Microphone(AudioDeviceSource),
}
