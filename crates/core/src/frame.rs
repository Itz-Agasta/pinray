use crate::audio::AudioFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8888,
    Rgba8888,
    Rgb888,
    Nv12,
    I420,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
    Bt709,
    Bt2020,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmabufFrame {
    pub fd: i32,
    pub offset: u32,
    pub size: u32,
    pub stride: u32,
    pub modifier: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvPixelBufferHandle {
    pub ptr: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D3D11TextureHandle {
    pub ptr: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameData {
    Host(Vec<u8>),
    Dmabuf(DmabufFrame),
    CvPixelBuffer(CvPixelBufferHandle),
    D3D11Texture(D3D11TextureHandle),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoFrame {
    pub stream_time_ns: i64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub color_space: Option<ColorSpace>,
    pub data: FrameData,
    pub damage: Option<Vec<Rect>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    Dropped,
    FormatChanged,
    BackendRestarted,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapEvent {
    pub stream_time_ns: i64,
    pub reason: GapReason,
    pub dropped_frames: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    Video(VideoFrame),
    Audio(AudioFrame),
    Gap(GapEvent),
    End,
}
