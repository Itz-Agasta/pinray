//! Captured video frames, pixel layouts, and the events a session yields.

use crate::audio::AudioFrame;

/// An axis-aligned rectangle in pixels, used for crop regions and damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
///
/// Coordinates are relative to the top-left corner of the captured source.
pub struct Rect {
    /// Left edge, in pixels from the source's left edge.
    pub x: i32,
    /// Top edge, in pixels from the source's top edge.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
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

impl PixelFormat {
    /// Bytes per pixel for packed RGB/RGBA formats.
    ///
    /// `None` for the planar/biplanar YUV formats, where a single
    /// `width * bytes_per_pixel` row size doesn't apply (each plane has its
    /// own stride and subsampling).
    ///
    /// # Examples
    ///
    /// ```
    /// use pinray_core::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Bgra8888.bytes_per_pixel(), Some(4));
    /// assert_eq!(PixelFormat::Nv12.bytes_per_pixel(), None);
    /// ```
    pub fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            PixelFormat::Bgra8888 | PixelFormat::Rgba8888 => Some(4),
            PixelFormat::Rgb888 => Some(3),
            PixelFormat::Nv12 | PixelFormat::I420 => None,
        }
    }
}

/// Color space hint for interpreting pixel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// sRGB, the standard desktop color space.
    Srgb,
    /// Display P3, the wide-gamut space used by recent Apple displays.
    DisplayP3,
    /// ITU-R BT.709 (HDTV).
    Bt709,
    /// ITU-R BT.2020 (UHD / HDR).
    Bt2020,
}

/// A Linux DMA-BUF plane handle (zero-copy path, not yet produced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmabufFrame {
    /// DMA-BUF file descriptor.
    pub fd: i32,
    /// Byte offset of the plane within the buffer.
    pub offset: u32,
    /// Size of the plane in bytes.
    pub size: u32,
    /// Bytes from the start of one row to the next.
    pub stride: u32,
    /// DRM format modifier describing the memory layout (tiling).
    pub modifier: u64,
}

/// A macOS `CVPixelBuffer` pointer (zero-copy path, not yet produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvPixelBufferHandle {
    /// Raw `CVPixelBufferRef` address.
    pub ptr: usize,
}

/// A Windows `ID3D11Texture2D` pointer (zero-copy path, not yet produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D3D11TextureHandle {
    /// Raw `ID3D11Texture2D*` address.
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
    /// A Linux DMA-BUF plane. Not produced yet.
    Dmabuf(DmabufFrame),
    /// A macOS `CVPixelBuffer`. Not produced yet.
    CvPixelBuffer(CvPixelBufferHandle),
    /// A Windows D3D11 texture. Not produced yet.
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
    /// Per-stream counter advanced once per captured frame, including frames
    /// dropped because the consumer's queue was full; a jump means frames
    /// were dropped. Frames skipped by `frame_rate` pacing are not counted.
    pub sequence: u64,
    /// Frame width in pixels (after cropping).
    pub width: u32,
    /// Frame height in pixels (after cropping).
    pub height: u32,
    /// Bytes from the start of one row to the next in [`FrameData::Host`].
    /// Can be larger than `width * bytes_per_pixel`; see
    /// [`to_tight_bytes`](Self::to_tight_bytes).
    pub stride: u32,
    /// Pixel layout of [`data`](Self::data).
    pub pixel_format: PixelFormat,
    /// Color space of the pixels, when the backend knows it.
    pub color_space: Option<ColorSpace>,
    /// The pixels. Always [`FrameData::Host`] today.
    pub data: FrameData,
    /// Changed regions since the previous frame, when the backend knows.
    pub damage: Option<Vec<Rect>>,
}

impl VideoFrame {
    /// Returns [`FrameData::Host`] bytes with row padding stripped, i.e.
    /// `width * bytes_per_pixel` bytes per row instead of `stride`.
    ///
    /// `stride` can be wider than the tightly-packed row size (platform row
    /// alignment); copying `data` directly into something that assumes tight
    /// packing (a `rawvideo` pipe, an image encoder) silently skews every row
    /// after the first. Use this instead of touching `data` directly.
    ///
    /// Returns `None` for non-[`FrameData::Host`] variants, or for pixel
    /// formats without a well-defined bytes-per-pixel (see
    /// [`PixelFormat::bytes_per_pixel`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use pinray_core::{FrameData, PixelFormat, VideoFrame};
    ///
    /// let (width, height, stride) = (2u32, 2u32, 12u32); // 4 padding bytes/row
    /// let mut data = Vec::new();
    /// for row in 0..height {
    ///     data.extend(std::iter::repeat_n(row as u8, width as usize * 4));
    ///     data.extend(std::iter::repeat_n(0xAA, (stride - width * 4) as usize));
    /// }
    /// let frame = VideoFrame {
    ///     stream_time_ns: 0,
    ///     sequence: 0,
    ///     width,
    ///     height,
    ///     stride,
    ///     pixel_format: PixelFormat::Bgra8888,
    ///     color_space: None,
    ///     data: FrameData::Host(data),
    ///     damage: None,
    /// };
    ///
    /// let tight = frame.to_tight_bytes().unwrap();
    /// assert_eq!(tight.len(), (width * height * 4) as usize);
    /// assert!(tight.iter().all(|&b| b != 0xAA)); // padding is gone
    /// ```
    pub fn to_tight_bytes(&self) -> Option<Vec<u8>> {
        let FrameData::Host(buf) = &self.data else {
            return None;
        };
        let bpp = self.pixel_format.bytes_per_pixel()?;
        let row_bytes = self.width as usize * bpp;
        if self.stride as usize == row_bytes {
            return Some(buf.clone());
        }
        let mut out = Vec::with_capacity(row_bytes * self.height as usize);
        for row in 0..self.height as usize {
            let start = row * self.stride as usize;
            out.extend_from_slice(&buf[start..start + row_bytes]);
        }
        Some(out)
    }
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
    /// The backend could not determine the cause.
    Unknown,
}

/// A discontinuity notice: frames were lost or the stream restarted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapEvent {
    /// When the gap was detected, on the same clock as
    /// [`VideoFrame::stream_time_ns`].
    pub stream_time_ns: i64,
    /// Why the gap happened.
    pub reason: GapReason,
    /// How many frames were lost, when the backend can count them.
    pub dropped_frames: Option<u32>,
}

/// What [`CaptureSession::next_event`](crate::CaptureSession::next_event)
/// yields.
///
/// # Examples
///
/// ```
/// use pinray_core::CaptureEvent;
///
/// fn describe(event: &CaptureEvent) -> String {
///     match event {
///         CaptureEvent::Video(frame) => format!("{}x{} frame", frame.width, frame.height),
///         CaptureEvent::Audio(frame) => format!("{} Hz audio", frame.sample_rate),
///         CaptureEvent::Gap(gap) => format!("gap: {:?}", gap.reason),
///         CaptureEvent::End => "end of stream".into(),
///     }
/// }
/// # assert_eq!(describe(&CaptureEvent::End), "end of stream");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// A captured video frame.
    Video(VideoFrame),
    /// A captured audio packet.
    Audio(AudioFrame),
    /// Frames were lost or the stream restarted; see [`GapReason`].
    Gap(GapEvent),
    /// The stream ended and no further events will arrive.
    End,
}
