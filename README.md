<div align="center">

# pinray

**Cross-platform screen and audio capture for Rust: raw frames, real metadata, native backends.**

[![Crates.io](https://img.shields.io/crates/v/pinray.svg)](https://crates.io/crates/pinray)
[![Documentation](https://docs.rs/pinray/badge.svg)](https://docs.rs/pinray)
[![CI](https://github.com/Itz-Agasta/pinray/actions/workflows/ci.yml/badge.svg)](https://github.com/Itz-Agasta/pinray/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/Itz-Agasta/pinray/blob/main/LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange.svg)](#minimum-supported-rust-version)

[Getting started](https://github.com/Itz-Agasta/pinray/blob/main/crates/pinray/docs/getting-started.md) •
[Platform support](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md) •
[Troubleshooting](https://github.com/Itz-Agasta/pinray/blob/main/docs/troubleshooting.md) •
[API docs](https://docs.rs/pinray) •
[Type reference (any platform)](https://docs.rs/pinray-core)

<img
  alt="banner"
  src="https://github.com/user-attachments/assets/f8744ece-3aff-4c72-8fa8-add76d571280"
/>

</div>

pinray talks to each operating system's native capture API and gives you raw video and audio frames with their metadata intact: stride, pixel format, timestamps, sequence numbers, and drop signals. It doesn't encode. You pipe frames into ffmpeg, WebRTC, wgpu, an image crate, or your own pipeline.

## Why pinray

- **Native backends, no wrapper crates**: XDG Desktop Portal + PipeWire and X11 on Linux, ScreenCaptureKit (via objc2) on macOS, and DXGI Desktop Duplication, Windows Graphics Capture, and WASAPI (via the `windows` crate) on Windows
- **Audio built in**: system-mix capture on every platform, timestamped on the same clock as video, with no `cpal` detour or shelling out to `pactl`
- **Complete frame metadata**: every frame carries width, height, stride, pixel format, a monotonic timestamp, and a per-stream sequence number, and dropped frames show up as explicit `Gap` events
- **Backend selection you can inspect**: `Auto` picks the right backend per platform, with fallbacks such as DXGI to WGC, and `session.backend_info()` tells you which one you got and its caveats
- **Documented platform differences**: X11 polls, DXGI delivers on change, and Wayland goes through a permission portal. [Platform support](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md) lists every gap

## Platform support

|                   | Video                          | System audio        | Notes                          |
| ----------------- | ------------------------------ | ------------------- | ------------------------------ |
| **Linux** Wayland | ✅ portal + PipeWire streaming | ✅ PipeWire         | restore tokens skip the dialog |
| **Linux** X11     | ✅ paced polling (GetImage)    | ✅ PipeWire         | XFixes cursor blend            |
| **macOS** 13.0+   | ✅ ScreenCaptureKit            | ✅ ScreenCaptureKit | display + window capture       |
| **Windows** 10+   | ✅ DXGI + WGC                  | ✅ WASAPI loopback  | DXGI→WGC auto-fallback         |

Per-backend limits, such as window capture and crop support, are in [docs/platforms.md](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md).

## Quick start

```console
cargo add pinray
```

This captures the primary display plus everything the system plays, and prints each event:

```rust,no_run
use std::time::Duration;
use pinray::{
    AudioCapture, CaptureEvent, CaptureSession, SourceId, VideoCaptureTarget,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .audio(AudioCapture::SystemMix)
        .build()?;

    println!("backend: {:?}", session.backend_info().kind);
    session.start()?;

    loop {
        match session.next_event(Some(Duration::from_secs(5)))? {
            CaptureEvent::Video(f) => {
                println!("video {}x{} t={}ns", f.width, f.height, f.stream_time_ns)
            }
            CaptureEvent::Audio(f) => {
                println!("audio {} Hz {}ch", f.sample_rate, f.channels)
            }
            CaptureEvent::Gap(g) => println!("gap: {:?}", g.reason),
            CaptureEvent::End => break,
        }
    }

    session.stop()?;
    Ok(())
}
```

[Getting started](https://github.com/Itz-Agasta/pinray/blob/main/crates/pinray/docs/getting-started.md) covers picking a display or window with `pinray::enumerate_sources()`, audio-only capture (no permission dialog on Linux and Windows), forcing a backend with `BackendPreference`, and writing frames to a video file.

## Requirements

| Platform | Build                           | Runtime                                  |
| -------- | ------------------------------- | ---------------------------------------- |
| Linux    | `libpipewire-0.3-dev` + `clang` | PipeWire; XDG portal for Wayland video   |
| macOS    | none                            | macOS 13.0+, Screen Recording permission |
| Windows  | none                            | Windows 10+ (WGC needs 1903+)            |

## Documentation

Where to look for what:

- **[docs.rs/pinray](https://docs.rs/pinray)**: API reference for every public item, with examples for the main entry points, plus the getting-started guide
- **[docs.rs/pinray-core](https://docs.rs/pinray-core)**: the same types, built for the default target, and the backend traits if you write your own backend
- **[Getting started](https://github.com/Itz-Agasta/pinray/blob/main/crates/pinray/docs/getting-started.md)**: install, first frames, sources, audio, tuning, and muxing
- **[Platform support](https://github.com/Itz-Agasta/pinray/blob/main/docs/platforms.md)**: the feature matrix and per-backend behavior
- **[Troubleshooting](https://github.com/Itz-Agasta/pinray/blob/main/docs/troubleshooting.md)**: build errors and capture problems by platform
- **[llms.txt](https://github.com/Itz-Agasta/pinray/blob/main/llms.txt)**: the whole API surface on one page, for coding agents
- **[CHANGELOG](https://github.com/Itz-Agasta/pinray/blob/main/CHANGELOG.md)**: what changed in each release

## Examples

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```

## Architecture

pinray is a Cargo workspace. [`pinray-core`](https://github.com/Itz-Agasta/pinray/tree/main/crates/core) defines the frame model and backend traits. Each platform crate owns its native and `unsafe` code: [`platform-linux`](https://github.com/Itz-Agasta/pinray/tree/main/crates/platform-linux), [`platform-macos`](https://github.com/Itz-Agasta/pinray/tree/main/crates/platform-macos), and [`platform-windows`](https://github.com/Itz-Agasta/pinray/tree/main/crates/platform-windows). [`pinray`](https://github.com/Itz-Agasta/pinray/tree/main/crates/pinray) is the public facade that picks the platform backend. A future encoder crate can be added without touching capture.

## Stability

pinray is at `0.2` and changing fast. Minor versions may break the API while the frame model and builder settle, so pin a minor version if you need stability.

## Minimum supported Rust version

pinray needs Rust 1.88 or later, for let-chains. Raising the minimum counts as a minor-version change while pinray is at 0.x.

## Contributing

Issues and pull requests are welcome. [CONTRIBUTING.md](https://github.com/Itz-Agasta/pinray/blob/main/CONTRIBUTING.md) covers setup, the checks CI runs, and how to test on each platform. When you report a bug, include the `session.backend_info()` output and a `RUST_LOG=debug` log, because backend selection differs per machine.

To report a security problem, see [SECURITY.md](https://github.com/Itz-Agasta/pinray/blob/main/SECURITY.md) instead of opening an issue.

## License

MIT. See [LICENSE](https://github.com/Itz-Agasta/pinray/blob/main/LICENSE). Contact: rupamgolui@proton.me
