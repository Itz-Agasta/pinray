# Getting Started

pinray captures screens, windows, and system audio through each OS's native API and hands you raw frames. This guide walks from install to your first frames. Full API reference: [docs.rs/pinray](https://docs.rs/pinray).

## Install

```console
cargo add pinray
```

Linux needs build-time system libraries (see [platforms.md](platforms.md#linux)); macOS and Windows need nothing extra.

## Capture your first frames

```rust,no_run
use std::time::Duration;
use pinray::{CaptureEvent, CaptureSession, SourceId, VideoCaptureTarget};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
        .build()?;

    session.start()?;
    for _ in 0..60 {
        match session.next_event(Some(Duration::from_secs(5)))? {
            CaptureEvent::Video(frame) => {
                // frame.data is FrameData::Host(Vec<u8>) - packed rows of
                // frame.stride bytes, frame.pixel_format (BGRA by default)
                println!("{}x{} at t={}ns", frame.width, frame.height, frame.stream_time_ns);
            }
            CaptureEvent::Gap(gap) => eprintln!("dropped frames: {:?}", gap),
            _ => {}
        }
    }
    session.stop()?;
    Ok(())
}
```

`SourceId::new("auto")` targets the primary display. On Wayland the compositor shows a permission dialog instead - whatever the user picks there is what you capture.

## Pick a specific display or window

```rust,no_run
use pinray::{CaptureSource, CaptureSession, VideoCaptureTarget};

let sources = pinray::enumerate_sources().unwrap();
let window = sources.iter().find_map(|s| match s {
    CaptureSource::Window(w) if w.title.contains("Firefox") => Some(w.id.clone()),
    _ => None,
});
if let Some(id) = window {
    let session = CaptureSession::builder()
        .video_target(VideoCaptureTarget::Window(id))
        .build();
}
```

Window ids die with the window - enumerate right before building the session.

## Add system audio

```rust,no_run
use std::time::Duration;
use pinray::{AudioCapture, CaptureSession, SourceId, VideoCaptureTarget};

let mut session = CaptureSession::builder()
    .video_target(VideoCaptureTarget::Display(SourceId::new("auto")))
    .audio(AudioCapture::SystemMix)
    .build()
    .unwrap();
session.start().unwrap();
```

Audio arrives as `CaptureEvent::Audio` interleaved with video from `next_event`, or drain it directly with `session.next_audio(timeout)`. Audio-only sessions (no `video_target`) work too and never show permission dialogs on Linux.

## Tuning

```rust,no_run
use pinray::{BackendPreference, CaptureSession, CursorMode, PixelFormat, Rect};

let builder = CaptureSession::builder()
    .backend_preference(BackendPreference::Auto) // or force WindowsWgc, LinuxX11, ...
    .pixel_format(PixelFormat::Rgba8888)         // BGRA is native everywhere; RGBA costs a swizzle
    .cursor_mode(CursorMode::Hidden)
    .crop_rect(Some(Rect { x: 0, y: 0, width: 1280, height: 720 }))
    .frame_rate(Some(30))
    .queue_depth(4);                             // frames buffered before drops
```

Check what you actually got:

```rust,no_run
# let session = pinray::CaptureSession::builder().audio(pinray::AudioCapture::SystemMix).build().unwrap();
let info = session.backend_info();
println!("{:?}: audio={} notes={}", info.kind, info.supports_audio, info.notes);
```

Include that output in bug reports - backend selection differs per machine.

## Timestamps, sequences, gaps

- `stream_time_ns` is monotonic and comparable between a session's audio and video streams. The epoch differs per platform (boot time on macOS/Windows, process-relative on Linux) - compute deltas, don't compare across machines.
- `sequence` increments once per delivered frame per stream; a jump means the consumer fell behind and frames were dropped.
- `CaptureEvent::Gap` reports drops and backend restarts explicitly.

## Runnable examples

```console
cargo run --example wayland_smoke    # Linux Wayland: video + audio
cargo run --example x11_smoke        # Linux X11: polling video + audio
cargo run --example audio_smoke      # Linux/Windows: audio only, no dialogs
cargo run --example macos_smoke      # macOS
cargo run --example windows_smoke    # Windows (PINRAY_BACKEND=wgc|dxgi to force)
```
