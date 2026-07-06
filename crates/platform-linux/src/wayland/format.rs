//! PipeWire stream format parameters and pixel-format normalization.

use pipewire::{
    self as pw,
    spa::{
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
            video::VideoFormat,
        },
        utils::SpaTypes,
    },
};

use pinray_core::{PixelFormat, Result};

use super::{VideoSize, platform_error};

/// Build PipeWire stream format parameters for the video capture stream.
///
/// We offer a single `EnumFormat` param with BGRA/BGRx/RGBA/RGBx as a Choice.
/// The compositor's screencast node intersects this with its own advertised
/// formats and picks one. No modifier property -- the compositor handles format
/// conversion internally.
pub(super) fn build_stream_params(
    frame_rate: u32,
    source_size: Option<VideoSize>,
) -> Result<Vec<Vec<u8>>> {
    let default_size = source_size.unwrap_or(VideoSize {
        width: 1920,
        height: 1080,
    });

    let format = pw::spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRx,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: default_size.width,
                height: default_size.height
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 7680,
                height: 4320
            }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction {
                num: frame_rate,
                denom: 1
            },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction {
                num: frame_rate,
                denom: 1
            }
        ),
    );

    let format_bytes = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format),
    )
    .map_err(platform_error)?
    .0
    .into_inner();

    Ok(vec![format_bytes])
}

/// Converts a negotiated PipeWire pixel format into the caller's desired
/// `PixelFormat`, swizzling channels where needed. Unknown combinations fall
/// through as BGRA, which is what compositors overwhelmingly deliver.
pub(super) fn normalize_frame(
    source_format: VideoFormat,
    raw: &[u8],
    desired_format: PixelFormat,
) -> (PixelFormat, Vec<u8>) {
    match (source_format, desired_format) {
        (VideoFormat::BGRA | VideoFormat::BGRx, PixelFormat::Bgra8888) => {
            (PixelFormat::Bgra8888, raw.to_vec())
        }
        (VideoFormat::BGRA, PixelFormat::Rgba8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Rgba8888, data)
        }
        (VideoFormat::RGBx, PixelFormat::Rgba8888) => (PixelFormat::Rgba8888, raw.to_vec()),
        (VideoFormat::RGBA, PixelFormat::Rgba8888) => (PixelFormat::Rgba8888, raw.to_vec()),
        (VideoFormat::RGB, PixelFormat::Rgb888) => (PixelFormat::Rgb888, raw.to_vec()),
        (VideoFormat::BGRx, PixelFormat::Rgba8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Rgba8888, data)
        }
        (VideoFormat::RGBx, PixelFormat::Bgra8888) => {
            let mut data = raw.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            (PixelFormat::Bgra8888, data)
        }
        (VideoFormat::RGB, _) => (PixelFormat::Rgb888, raw.to_vec()),
        _ => (PixelFormat::Bgra8888, raw.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_passthrough() {
        let (fmt, data) = normalize_frame(VideoFormat::BGRA, &[1, 2, 3, 4], PixelFormat::Bgra8888);
        assert_eq!(fmt, PixelFormat::Bgra8888);
        assert_eq!(data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn bgra_to_rgba_swizzles() {
        let (fmt, data) = normalize_frame(VideoFormat::BGRA, &[1, 2, 3, 4], PixelFormat::Rgba8888);
        assert_eq!(fmt, PixelFormat::Rgba8888);
        assert_eq!(data, vec![3, 2, 1, 4]);
    }

    #[test]
    fn rgbx_to_bgra_swizzles() {
        let (fmt, data) = normalize_frame(VideoFormat::RGBx, &[1, 2, 3, 4], PixelFormat::Bgra8888);
        assert_eq!(fmt, PixelFormat::Bgra8888);
        assert_eq!(data, vec![3, 2, 1, 4]);
    }

    #[test]
    fn unknown_combination_falls_back_to_bgra() {
        let (fmt, data) = normalize_frame(VideoFormat::NV12, &[9, 9], PixelFormat::Bgra8888);
        assert_eq!(fmt, PixelFormat::Bgra8888);
        assert_eq!(data, vec![9, 9]);
    }
}
