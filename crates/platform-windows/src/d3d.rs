//! Shared Direct3D 11 helpers: device creation, staging copy to host memory,
//! and QPC timestamp normalization. Used by both the DXGI and WGC backends.

use pinray_core::{PinrayError, PixelFormat, Rect, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::System::Performance::QueryPerformanceFrequency;
use windows::core::Interface;

pub(crate) fn win_err(context: &str, error: windows::core::Error) -> PinrayError {
    PinrayError::Platform(format!("{context}: {error}"))
}

/// Creates a D3D11 device, either on a specific DXGI adapter (required by
/// DXGI desktop duplication, which fails cross-adapter) or on the default
/// hardware adapter.
pub(crate) fn create_d3d_device(
    adapter: Option<&IDXGIAdapter>,
) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let driver_type: D3D_DRIVER_TYPE = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };

    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver_type,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(|e| win_err("D3D11CreateDevice", e))?;
    }

    let device = device.ok_or_else(|| PinrayError::Platform("D3D11CreateDevice returned no device".into()))?;
    let context = unsafe { device.GetImmediateContext() }
        .map_err(|e| win_err("GetImmediateContext", e))?;
    Ok((device, context))
}

pub(crate) fn qpc_frequency() -> Result<i64> {
    let mut freq = 0i64;
    unsafe { QueryPerformanceFrequency(&mut freq) }
        .map_err(|e| win_err("QueryPerformanceFrequency", e))?;
    Ok(freq)
}

/// Converts a raw QPC counter value to nanoseconds since boot.
pub(crate) fn qpc_to_ns(qpc: i64, freq: i64) -> i64 {
    (qpc as i128 * 1_000_000_000 / freq as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::qpc_to_ns;

    #[test]
    fn qpc_to_ns_survives_large_uptimes() {
        // 10 MHz QPC frequency (common on modern Windows), 30 days uptime.
        let freq = 10_000_000i64;
        let qpc = 30 * 24 * 3600 * freq;
        assert_eq!(qpc_to_ns(qpc, freq), 30 * 24 * 3600 * 1_000_000_000i64);
    }

    #[test]
    fn qpc_to_ns_sub_second_precision() {
        // 1 tick at 10 MHz = 100 ns.
        assert_eq!(qpc_to_ns(1, 10_000_000), 100);
    }
}

pub(crate) struct HostCopy {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// Copies a GPU texture (assumed BGRA8) into host memory via a staging
/// texture, applying an optional crop and an optional BGRA→RGBA swizzle.
pub(crate) fn texture_to_host(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    source: &ID3D11Texture2D,
    crop: Option<Rect>,
    pixel_format: PixelFormat,
) -> Result<HostCopy> {
    unsafe {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut src_desc);

        let (x, y, width, height) = match crop {
            Some(rect) => (rect.x.max(0) as u32, rect.y.max(0) as u32, rect.width, rect.height),
            None => (0, 0, src_desc.Width, src_desc.Height),
        };
        if x + width > src_desc.Width || y + height > src_desc.Height {
            return Err(PinrayError::InvalidConfig(format!(
                "crop_rect {x},{y} {width}x{height} exceeds source {}x{}",
                src_desc.Width, src_desc.Height
            )));
        }

        let staging = {
            let mut desc = src_desc;
            desc.Width = width;
            desc.Height = height;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.SampleDesc.Count = 1;
            desc.SampleDesc.Quality = 0;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;

            let mut staging = None;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .map_err(|e| win_err("CreateTexture2D(staging)", e))?;
            staging.ok_or_else(|| PinrayError::Platform("CreateTexture2D returned no texture".into()))?
        };

        let region = D3D11_BOX {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
            front: 0,
            back: 1,
        };
        let staging_resource: ID3D11Resource =
            staging.cast().map_err(|e| win_err("staging cast", e))?;
        let source_resource: ID3D11Resource =
            source.cast().map_err(|e| win_err("source cast", e))?;
        context.CopySubresourceRegion(
            Some(&staging_resource),
            0,
            0,
            0,
            0,
            Some(&source_resource),
            0,
            Some(&region),
        );

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(Some(&staging_resource), 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| win_err("Map(staging)", e))?;

        let row_bytes = (width * 4) as usize;
        let mut data = vec![0u8; row_bytes * height as usize];
        let src_ptr = mapped.pData as *const u8;
        for row in 0..height as usize {
            let src = std::slice::from_raw_parts(
                src_ptr.add(row * mapped.RowPitch as usize),
                row_bytes,
            );
            data[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(src);
        }
        context.Unmap(Some(&staging_resource), 0);

        if pixel_format == PixelFormat::Rgba8888 {
            for px in data.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }

        Ok(HostCopy {
            data,
            width,
            height,
            stride: width * 4,
        })
    }
}
