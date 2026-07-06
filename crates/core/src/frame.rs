use crate::audio::AudioFrame;

/// An axis-aligned rectangle in pixels, used for crop regions and damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Pixel layout of [`VideoFrame::data`].
///
/// All backends deliver `Bgra8888` natively and can swizzle to `Rgba8888`;
/// the remaining formats exist for future zero-copy paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit B, G, R, A byte order — the native format everywhere.
    Bgra8888,
    /// 8-bit R, G, B, A byte order (CPU swizzle from BGRA).
    Rgba8888,
    /// 24-bit packed RGB, no alpha.
    Rgb888,
    /// Biplanar 4:2:0 YCbCr (Y plane + interleaved CbCr).
    Nv12,
    /// Planar 4:2:0 YCbCr.
    I420,
}

/// Color space hint for interpreting pixel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
    Bt709,
    Bt2020,
}

/// A Linux DMA-BUF plane handle (zero-copy path, not yet produced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmabufFrame {
    pub fd: i32,
    pub offset: u32,
    pub size: u32,
    pub stride: u32,
    pub modifier: u64,
}

/// A macOS `CVPixelBuffer` pointer (zero-copy path, not yet produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvPixelBufferHandle {
    pub ptr: usize,
}

/// A Windows `ID3D11Texture2D` pointer (zero-copy path, not yet produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D3D11TextureHandle {
    pub ptr: usize,
}

/// Where a frame's pixels live.
///
/// Every backend currently produces `Host` (a plain CPU byte buffer); the
/// native-handle variants are reserved for opt-in zero-copy support.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameData {
    /// CPU memory, rows packed at [`VideoFrame::stride`] bytes.
    Host(Vec<u8>),
    Dmabuf(DmabufFrame),
    CvPixelBuffer(CvPixelBufferHandle),
    D3D11Texture(D3D11TextureHandle),
}

/// One captured video frame with complete layout and timing metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFrame {
    /// Capture timestamp in nanoseconds, monotonic within a session and
    /// comparable with [`AudioFrame::stream_time_ns`] of the same session.
    /// The epoch is platform-dependent (boot time on macOS/Windows,
    /// process-relative on Linux).
    pub stream_time_ns: i64,
    /// Per-stream counter advanced once per delivered frame; a jump means
    /// frames were dropped.
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the next in [`FrameData::Host`].
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub color_space: Option<ColorSpace>,
    pub data: FrameData,
    /// Changed regions since the previous frame, when the backend knows.
    pub damage: Option<Vec<Rect>>,
}

/// Why a [`GapEvent`] was emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    /// Frames were dropped because the consumer fell behind.
    Dropped,
    /// The stream format changed mid-session.
    FormatChanged,
    /// The backend recovered from losing its capture source (e.g. a display
    /// mode switch invalidating DXGI duplication).
    BackendRestarted,
    Unknown,
}

/// A discontinuity notice: frames were lost or the stream restarted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapEvent {
    pub stream_time_ns: i64,
    pub reason: GapReason,
    pub dropped_frames: Option<u32>,
}

/// What [`CaptureSession::next_event`](crate::CaptureSession::next_event)
/// yields.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    Video(VideoFrame),
    Audio(AudioFrame),
    Gap(GapEvent),
    /// The stream ended and no further events will arrive.
    End,
}
