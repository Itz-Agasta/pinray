#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    I16,
    I32,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioData {
    Interleaved(Vec<u8>),
    Planar(Vec<Vec<u8>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub stream_time_ns: i64,
    pub sequence: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub data: AudioData,
}
