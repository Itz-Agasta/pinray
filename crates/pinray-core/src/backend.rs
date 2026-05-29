use std::time::Duration;

use crate::{audio::AudioFrame, config::SessionConfig, error::Result, frame::CaptureEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendPreference {
    Auto,
    LinuxWaylandPortal,
    LinuxX11,
    MacScreenCaptureKit,
    WindowsDxgi,
    WindowsWgc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    LinuxWaylandPortal,
    LinuxX11,
    MacScreenCaptureKit,
    WindowsDxgi,
    WindowsWgc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    pub kind: BackendKind,
    pub supports_audio: bool,
    pub zero_copy: bool,
    pub notes: &'static str,
}

pub trait VideoBackend: Send {
    fn info(&self) -> BackendInfo;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next_event(&mut self, timeout: Option<Duration>) -> Result<CaptureEvent>;
}

pub trait AudioBackend: Send {
    fn info(&self) -> BackendInfo;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next_audio(&mut self, timeout: Option<Duration>) -> Result<AudioFrame>;
}

pub struct BackendBundle {
    pub info: BackendInfo,
    pub video: Option<Box<dyn VideoBackend>>,
    pub audio: Option<Box<dyn AudioBackend>>,
}

pub trait BackendResolver {
    fn resolve(&self, config: &SessionConfig) -> Result<BackendBundle>;
}
