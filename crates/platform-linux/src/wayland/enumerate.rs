use pinray_core::{DisplaySource, PinrayError, Result, SourceId, WindowSource};

pub(super) struct MonitorEntry {
    pub source: DisplaySource,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

pub(super) fn enumerate_monitors(/* todo */) -> Result<Vec<MonitorEntry>> {


    todo!()
}