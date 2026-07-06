//! Sample-buffer → frame extraction: CVPixelBuffer host copies with stride
//! stripping and pixel-format normalization, CMBlockBuffer audio copies, and
//! CMTime → nanosecond conversion.

use std::{ptr::NonNull, slice};

use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA, kCVPixelFormatType_32RGBA,
};

use pinray_core::{
    AudioData, AudioFrame, ColorSpace, FrameData, PixelFormat, SampleFormat, VideoFrame,
};

pub(super) fn pts_to_ns(pts: objc2_core_media::CMTime) -> i64 {
    // Widen to i128: SCKit uses host-clock timescales (1e9), so
    // value * 1e9 overflows i64 within seconds of uptime.
    if pts.timescale != 0 {
        (pts.value as i128 * 1_000_000_000 / pts.timescale as i128) as i64
    } else {
        0
    }
}

pub(super) fn extract_video_frame(
    sample_buffer: &CMSampleBuffer,
    sequence: u64,
    desired: PixelFormat,
) -> Option<VideoFrame> {
    let stream_time_ns = pts_to_ns(unsafe { sample_buffer.presentation_time_stamp() });

    let pixel_buffer = unsafe { sample_buffer.image_buffer()? };
    unsafe { CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };

    let result = (|| {
        let w = CVPixelBufferGetWidth(&pixel_buffer) as u32;
        let h = CVPixelBufferGetHeight(&pixel_buffer) as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let bpr = CVPixelBufferGetBytesPerRow(&pixel_buffer) as u32;
        let base = CVPixelBufferGetBaseAddress(&pixel_buffer);
        let size = CVPixelBufferGetDataSize(&pixel_buffer);
        let native = CVPixelBufferGetPixelFormatType(&pixel_buffer);
        if base.is_null() || size == 0 {
            return None;
        }
        let raw = unsafe { slice::from_raw_parts(base.cast::<u8>(), size) };
        let (pixel_format, data) = normalize_pixels(native, raw, w, h, bpr, desired);

        Some(VideoFrame {
            stream_time_ns,
            sequence,
            width: w,
            height: h,
            // copy_rows strips CVPixelBuffer row padding, so rows are packed.
            stride: w * 4,
            pixel_format,
            color_space: Some(ColorSpace::Srgb),
            data: FrameData::Host(data),
            damage: None,
        })
    })();

    unsafe { CVPixelBufferUnlockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly) };
    result
}

fn normalize_pixels(
    native: u32,
    raw: &[u8],
    w: u32,
    h: u32,
    bpr: u32,
    desired: PixelFormat,
) -> (PixelFormat, Vec<u8>) {
    let row = bpr as usize;
    let pw = w as usize * 4;
    let ph = h as usize;

    match (native, desired) {
        (f, PixelFormat::Bgra8888) if f == kCVPixelFormatType_32BGRA => {
            (PixelFormat::Bgra8888, copy_rows(raw, row, pw, ph))
        }
        (f, PixelFormat::Rgba8888) if f == kCVPixelFormatType_32RGBA => {
            (PixelFormat::Rgba8888, copy_rows(raw, row, pw, ph))
        }
        (f, _) if f == kCVPixelFormatType_32BGRA => {
            let mut dst = copy_rows(raw, row, pw, ph);
            for px in dst.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            (PixelFormat::Rgba8888, dst)
        }
        (f, _) if f == kCVPixelFormatType_32RGBA => {
            let mut dst = copy_rows(raw, row, pw, ph);
            for px in dst.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            (PixelFormat::Bgra8888, dst)
        }
        _ => (PixelFormat::Bgra8888, copy_rows(raw, row, pw, ph)),
    }
}

/// Copies `h` rows of `pixel_row_bytes` out of a buffer whose rows are
/// `bpr` bytes apart, dropping any per-row padding.
fn copy_rows(raw: &[u8], bpr: usize, pixel_row_bytes: usize, h: usize) -> Vec<u8> {
    if bpr == pixel_row_bytes {
        return raw[..pixel_row_bytes * h].to_vec();
    }
    let mut out = Vec::with_capacity(pixel_row_bytes * h);
    for row in 0..h {
        let s = row * bpr;
        let e = s + pixel_row_bytes;
        if e <= raw.len() {
            out.extend_from_slice(&raw[s..e]);
        }
    }
    out
}

pub(super) fn extract_audio_frame(
    sample_buffer: &CMSampleBuffer,
    sequence: u64,
) -> Option<AudioFrame> {
    let stream_time_ns = pts_to_ns(unsafe { sample_buffer.presentation_time_stamp() });

    if unsafe { sample_buffer.num_samples() } == 0 {
        return None;
    }

    let block_buf = unsafe { sample_buffer.data_buffer()? };
    let data_len = unsafe { block_buf.data_length() };
    if data_len == 0 {
        return None;
    }

    let mut bytes = vec![0u8; data_len];
    let dst = NonNull::new(bytes.as_mut_ptr().cast()).expect("vec ptr is non-null");
    let status = unsafe { block_buf.copy_data_bytes(0, data_len, dst) };
    if status != 0 {
        tracing::warn!(status, "CMBlockBufferCopyDataBytes failed");
        return None;
    }

    Some(AudioFrame {
        stream_time_ns,
        sequence,
        // NOTE: format assumed from SCStreamConfiguration (48 kHz stereo F32).
        // SCKit LPCM may actually be planar (non-interleaved) — see
        // docs/macos.md "Known Issues"; needs a runtime check on real macOS.
        sample_rate: 48_000,
        channels: 2,
        sample_format: SampleFormat::F32,
        data: AudioData::Interleaved(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::{copy_rows, pts_to_ns};

    #[test]
    fn pts_to_ns_survives_large_host_clock_values() {
        // Host-clock CMTime: timescale 1e9, value = ns since boot. A machine
        // up for 30 days must not overflow.
        let pts = objc2_core_media::CMTime {
            value: 30 * 24 * 3600 * 1_000_000_000i64,
            timescale: 1_000_000_000,
            flags: objc2_core_media::CMTimeFlags(1),
            epoch: 0,
        };
        assert_eq!(pts_to_ns(pts), 30 * 24 * 3600 * 1_000_000_000i64);
    }

    #[test]
    fn pts_to_ns_zero_timescale_is_zero() {
        let pts = objc2_core_media::CMTime {
            value: 42,
            timescale: 0,
            flags: objc2_core_media::CMTimeFlags(1),
            epoch: 0,
        };
        assert_eq!(pts_to_ns(pts), 0);
    }

    #[test]
    fn copy_rows_strips_padding() {
        // 2 rows, 4 pixel bytes per row, 6 bytes per row in source (2 pad).
        let raw = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0];
        assert_eq!(copy_rows(&raw, 6, 4, 2), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn copy_rows_packed_passthrough() {
        let raw = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(copy_rows(&raw, 4, 4, 2), raw.to_vec());
    }
}
