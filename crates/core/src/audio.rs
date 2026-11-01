//! Captured audio packets and their sample layout.

/// Sample encoding of [`AudioFrame::data`].
///
/// Multi-byte samples are little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Signed 16-bit integer samples.
    I16,
    /// Signed 32-bit integer samples.
    I32,
    /// 32-bit float samples in `-1.0..=1.0`. The most common format.
    F32,
    /// 64-bit float samples in `-1.0..=1.0`.
    F64,
}

/// Channel layout of the raw sample bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioData {
    /// Samples alternate across channels: `L R L R …`.
    Interleaved(Vec<u8>),
    /// One byte buffer per channel.
    Planar(Vec<Vec<u8>>),
}

/// One captured audio packet (typically ~10 ms of samples).
///
/// Decode [`data`](Self::data) with [`sample_format`](Self::sample_format)
/// and [`channels`](Self::channels).
///
/// # Examples
///
/// Convert an interleaved `F32` packet to `f32` samples:
///
/// ```
/// use pinray_core::{AudioData, AudioFrame, SampleFormat};
///
/// # let frame = AudioFrame {
/// #     stream_time_ns: 0, sequence: 0, sample_rate: 48_000, channels: 2,
/// #     sample_format: SampleFormat::F32,
/// #     data: AudioData::Interleaved([0.5f32, -0.5].iter().flat_map(|s| s.to_le_bytes()).collect()),
/// # };
/// if let (SampleFormat::F32, AudioData::Interleaved(bytes)) = (frame.sample_format, &frame.data) {
///     let samples: Vec<f32> = bytes
///         .chunks_exact(4)
///         .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
///         .collect();
///     assert_eq!(samples, [0.5, -0.5]);
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    /// Capture timestamp in nanoseconds; same clock as
    /// [`VideoFrame::stream_time_ns`](crate::VideoFrame::stream_time_ns)
    /// within a session.
    pub stream_time_ns: i64,
    /// Per-stream packet counter; a jump means packets were dropped.
    pub sequence: u64,
    /// Samples per second per channel, e.g. `48_000`.
    pub sample_rate: u32,
    /// Number of channels, e.g. `2` for stereo.
    pub channels: u16,
    /// Encoding of each sample in [`data`](Self::data).
    pub sample_format: SampleFormat,
    /// The raw sample bytes.
    pub data: AudioData,
}
