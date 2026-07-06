<div align="center">



# pinray

**Cross-platform screen and audio capture for Rust — raw frames, real metadata, native backends.**

[![Crates.io](https://img.shields.io/crates/v/pinray.svg)](https://crates.io/crates/pinray)
[![Documentation](https://docs.rs/pinray/badge.svg)](https://docs.rs/pinray)
[![CI](https://github.com/Itz-Agasta/pinray/actions/workflows/ci.yml/badge.svg)](https://github.com/Itz-Agasta/pinray/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange.svg)](#minimum-supported-rust-version)

[Getting Started](docs/getting-started.md) •
[Platform Support](docs/platforms.md) •
[Troubleshooting](docs/troubleshooting.md) •
[API Docs](https://docs.rs/pinray)


<p align="center">
  <img src="https://github.com/user-attachments/assets/d4d1a711-5e24-4ae1-81cb-47c64231caf5" alt="pinray banner" width="100%">
</p>

</div>

---

pinray is a capture **infrastructure** crate: it talks to each OS's native capture API and hands you raw video and audio frames with their metadata intact - stride, pixel format, timestamps, sequence numbers, drop signals. Encoding is deliberately out of scope; pipe frames into ffmpeg, WebRTC, wgpu, image crates, or your own pipeline.

## Why pinray

- **Native backends, no wrapper crates** - XDG Desktop Portal + PipeWire and X11 on Linux, ScreenCaptureKit (via objc2) on macOS, DXGI Desktop Duplication + Windows Graphics Capture + WASAPI (via the `windows` crate) on Windows.
- **Audio is first-class** - system-mix capture on all four backends, timestamped on the same clock as video, no `cpal` detour and no shelling out to `pactl`.
- **A frame model that doesn't lie** - every frame carries width/height/stride, pixel format, color-space hint, a monotonic timestamp, and a per-stream sequence number. Dropped frames surface as explicit `Gap` events.
- **Inspectable backend selection** - `Auto` picks the right backend per platform (with fallbacks like DXGI→WGC), and `session.backend_info()` tells you exactly what you got and its caveats.
- **Honest about platform differences** - X11 is documented as polling, DXGI as change-driven, Wayland as portal-mediated. No pretending all platforms behave the same.

## Platform support

|                   | Video                          | System audio        | Notes                          |
| ----------------- | ------------------------------ | ------------------- | ------------------------------ |
| **Linux** Wayland | ✅ portal + PipeWire streaming | ✅ PipeWire         | restore tokens skip the dialog |
| **Linux** X11     | ✅ paced polling (GetImage)    | ✅ PipeWire         | XFixes cursor blend            |
| **macOS** 12.3+   | ✅ ScreenCaptureKit            | ✅ ScreenCaptureKit | display + window capture       |
| **Windows** 10+   | ✅ DXGI + WGC                  | ✅ WASAPI loopback  | DXGI→WGC auto-fallback         |

Full matrix with per-backend limitations: [docs/platforms.md](docs/platforms.md).

## Quick start

```console
cargo add pinray
```

```rust,no_run
use std::time::Duration;
use pinray::{AudioCapture, CaptureEvent, CaptureSession, SourceId, VideoCaptureTarget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Capture the primary display plus everything the system plays.
    let mut session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .audio(AudioCapture::SystemMix)
        .build()?;

    println!("backend: {:?}", session.backend_info().kind);
    session.start()?;

    loop {
        match session.next_event(Some(Duration::from_secs(5)))? {
            CaptureEvent::Video(f) => println!("video {}x{} t={}ns", f.width, f.height, f.stream_time_ns),
            CaptureEvent::Audio(f) => println!("audio {} Hz {}ch", f.sample_rate, f.channels),
            CaptureEvent::Gap(g) => println!("gap: {:?}", g.reason),
            CaptureEvent::End => break,
        }
    }

    session.stop()?;
    Ok(())
}
```

Pick specific displays/windows with `pinray::enumerate_sources()`, capture audio-only without any permission dialogs, force a backend with `BackendPreference` - see [Getting started](docs/getting-started.md).

## Requirements

| Platform | Build                           | Runtime                                  |
| -------- | ------------------------------- | ---------------------------------------- |
| Linux    | `libpipewire-0.3-dev` + `clang` | PipeWire; XDG portal for Wayland video   |
| macOS    | -                               | macOS 12.3+, Screen Recording permission |
| Windows  | -                               | Windows 10+ (WGC needs 1903+)            |

## Examples

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```

## Architecture

Cargo workspace: [`pinray-core`](crates/core) defines the frame model and backend traits, one crate per platform owns its native/unsafe code ([`platform-linux`](crates/platform-linux), [`platform-macos`](crates/platform-macos), [`platform-windows`](crates/platform-windows)), and [`pinray`](crates/pinray) is the public facade. A future encoder crate can slot in without touching capture.

## Stability

pinray is `0.2` and moving fast: minor versions may break the API while the frame model and builder settle against real-world use. Pin a minor version if you need stability.

## Minimum supported Rust version

Rust **1.88** (let-chains). MSRV bumps are minor-version changes during 0.2.

## Contributing

Issues and PRs welcome. Please include `session.backend_info()` output and `RUST_LOG=debug` logs in bug reports - backend selection differs per machine. CI runs clippy (warnings deny), tests on Linux/macOS/Windows, cross-target checks, and a real Xvfb capture smoke test.

## License

MIT - see [LICENSE](LICENSE). Contact: rupamgolui@proton.me
