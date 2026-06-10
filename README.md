# pinray

> Cross-platform screen and audio capture for Rust

[![Crates.io](https://img.shields.io/crates/v/pinray.svg)](https://crates.io/crates/pinray)
[![Documentation](https://docs.rs/pinray/badge.svg)](https://docs.rs/pinray)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/yourusername/pinray/actions/workflows/ci.yml/badge.svg)](https://github.com/yourusername/pinray/actions)

Raw screen and audio capture with a clean backend trait boundary. Capture first, encode separately.

- Native backends: Wayland (XDG Portal + PipeWire), macOS (ScreenCaptureKit), Windows (DXGI + WGC)
- Rich frame model with stride, pixel format, color space, timestamps
- Source enumeration before capture
- Restore tokens to skip permission dialogs

## Install

```console
cargo add pinray
```

## Example

```rust
use pinray::{SessionBuilder, PixelFormat, CursorMode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sources = pinray::enumerate_sources()?;

    let mut session = SessionBuilder::new()
        .video_target(sources.first().expect("no sources"))
        .pixel_format(PixelFormat::Bgra8888)
        .cursor_mode(CursorMode::Embedded)
        .build()?;

    session.start()?;

    loop {
        match session.next_event(None)? {
            pinray::CaptureEvent::Video(frame) => {
                println!("{}×{} stride={}", frame.width, frame.height, frame.stride);
            }
            pinray::CaptureEvent::End => break,
            _ => {}
        }
    }

    session.stop()?;
    Ok(())
}
```

## Platform Support

| Platform | Video | Audio |
|----------|-------|-------|
| Linux Wayland | Done | — |
| Linux X11 | planned | — |
| macOS | planned | planned |
| Windows | planned | planned |

## Requirements

**Linux:** Wayland session with XDG Desktop Portal, PipeWire, D-Bus

**macOS:** ScreenCaptureKit (macOS 12.3+)

**Windows:** Windows 10 1903+ for WGC

## License

Licensed under either of [MIT](LICENSE) or Apache License, Version 2.0 at your option.
