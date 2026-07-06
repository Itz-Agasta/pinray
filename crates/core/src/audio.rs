/// Sample encoding of [`AudioFrame::data`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    I16,
    I32,
    F32,
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
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    /// Capture timestamp in nanoseconds; same clock as
    /// [`VideoFrame::stream_time_ns`](crate::VideoFrame::stream_time_ns)
    /// within a session.
    pub stream_time_ns: i64,
    /// Per-stream packet counter; a jump means packets were dropped.
    pub sequence: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub data: AudioData,
}
